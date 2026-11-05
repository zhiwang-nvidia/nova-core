// SPDX-License-Identifier: GPL-2.0

//! Display state preparation and submission.
//!
//! # Commit sequence
//!
//! 1. Validate the configuration and choose a head and window. Identify the old
//!    resources to stop and prepare the new framebuffer before changing any
//!    active display state.
//! 2. Bind the pending framebuffer. When timings, layout and HDMI configuration are
//!    unchanged, retire the old window framebuffer and submit the replacement while
//!    keeping the head and output route running.
//! 3. For a full modeset, open a MODESET_BEGIN/END bracket. On the first modeset,
//!    cover all static outputs and clear the initial head/SOR state. Later brackets
//!    cover the old and new outputs, disabling only the previous output.
//! 4. Assign the new stream route and configure HDMI sink capabilities and TMDS.
//!    Establish window ownership, program head/window state, then submit the
//!    interlocked UPDATEs and wait for core FINISHED and window BEGUN.
//! 5. Enable monitor power, HDMI packets and source activity, clear AVMUTE and
//!    end the bracket. Release retired resources and publish the committed state.
//!
//! # Output disable
//!
//! Stop a head's window fetches only when its last output is being removed. Wait
//! for framebuffer retirement before disabling that head. Recompute the affected
//! SOR's owner mask from the streams that remain. Release the selected stream and
//! its unused framebuffers after the bracket completes. Other paths retain their
//! state.
//!
//! This first implementation still admits one active HDMI output. Multiple active
//! outputs require a complete-target GMC bandwidth check and a DRM atomic request
//! containing all affected CRTCs and planes. The shared core channel is serialized
//! by the existing display mutex.

use kernel::prelude::*;

use crate::gsp::commands::{
    gmc::{
        DisplayCheckConfiguration,
        DisplayModesetBegin,
        DisplayModesetEnd, //
    },
    DisplayConfiguration as FirmwareConfiguration,
    DisplayModesetEndMode, //
};

use super::{
    channel::Update,
    framebuffer::Framebuffer,
    stream::Stream,
    Display,
    DisplayConfiguration, //
};

/// Resources and scope for one update, prepared before hardware state changes.
pub(crate) struct DisplayCommit<'a, 'gpu> {
    configuration: DisplayConfiguration,
    head_index: usize,
    window_index: usize,
    /// Display ID of the old stream to release.
    stream_to_release: Option<u32>,
    /// Index of the old head to disable, if no other stream uses it.
    head_to_disable: Option<usize>,
    /// Window indices whose old framebuffers must finish before being unbound.
    window_framebuffers_to_retire: KVec<usize>,
    framebuffer: Framebuffer<'a, 'gpu>,
    stream: Option<Stream>,
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    fn select_head_and_window(&self, display_id: u32) -> Result<(usize, usize)> {
        // The current GMC check describes one head/window, not aggregate bandwidth.
        if self.streams.len() > 1 {
            return Err(EOPNOTSUPP);
        }
        let output = self
            .outputs()?
            .into_iter()
            .find(|output| output.display_id == display_id && output.connected)
            .ok_or(ENODEV)?;
        let previous_head = self.streams.first().map(|stream| stream.head);
        let head_index = self
            .heads
            .iter()
            .position(|head| {
                Some(head.id) == previous_head && output.head_mask & (1 << head.id) != 0
            })
            .or_else(|| {
                self.heads
                    .iter()
                    .position(|head| output.head_mask & (1 << head.id) != 0)
            })
            .ok_or(ENODEV)?;
        let window_index = self
            .windows
            .iter()
            .position(|window| previous_head.is_some() && window.head == previous_head)
            .or_else(|| self.windows.iter().position(|window| window.head.is_none()))
            .ok_or(EBUSY)?;
        Ok((head_index, window_index))
    }

    /// Checks the configuration and returns the selected head and window indices.
    pub(crate) fn check_configuration(
        &self,
        configuration: &DisplayConfiguration,
    ) -> Result<(usize, usize)> {
        const FEATURE_IMP: u64 = 1 << 4;
        const CHECKED_DOMAIN_IMP: u32 = 1;

        let mode = &configuration.mode;
        self.hal.validate_mode(mode)?;
        // This is the current driver's conservative RGB8 TMDS limit, not a
        // fixed hardware maximum. Raising it requires a limit derived from
        // both SOR and sink capabilities and the corresponding link setup,
        // including SCDC and scrambling for TMDS rates above 340 MHz.
        if mode.clock_khz > 165_000
            || configuration.pitch < mode.hdisplay * configuration.format.bytes_per_pixel()
            || !configuration.pitch.is_multiple_of(256)
            || configuration.pitch >> 6 > 0x1fff
            || configuration.hdmi_sink_caps & !7 != 0
            || self.caps.features & FEATURE_IMP == 0
        {
            return Err(EOPNOTSUPP);
        }
        let (head_index, window_index) = self.select_head_and_window(configuration.display_id)?;
        let hblank_end = mode.htotal - mode.hsync_start - 1;
        let vblank_end = mode.vtotal - mode.vsync_start - 1;
        let command = DisplayCheckConfiguration::new(&FirmwareConfiguration {
            head: self.heads[head_index].id,
            window: self.windows[window_index].id,
            display_id: configuration.display_id,
            clock_khz: mode.clock_khz,
            htotal: mode.htotal,
            vtotal: mode.vtotal,
            hblank_start: mode.hdisplay + hblank_end,
            hblank_end,
            vblank_start: mode.vdisplay + vblank_end,
            vblank_end,
            width: mode.hdisplay,
            height: mode.vdisplay,
            pitch: configuration.pitch,
            format: configuration.format,
        })?;
        let reply = self.cmdq.send_gmc_command(command, |_| Ok(()))?;
        if reply.possible != 1 || reply.checked_domains != CHECKED_DOMAIN_IMP {
            return Err(EINVAL);
        }
        Ok((head_index, window_index))
    }

    /// Prepares an unbound framebuffer; failure leaves the current display untouched.
    #[expect(dead_code)]
    fn prepare_commit(
        &mut self,
        configuration: &DisplayConfiguration,
        pixels: &[u8],
    ) -> Result<DisplayCommit<'a, 'gpu>> {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        let (head_index, window_index) = self.check_configuration(configuration)?;
        let head = &self.heads[head_index];
        let window = &self.windows[window_index];
        // Check all participating channels before allocating the new framebuffer.
        window.channel.as_ref().ok_or(ENODEV)?;
        window.immediate_channel.as_ref().ok_or(ENODEV)?;
        self.core_channel.as_ref().ok_or(ENODEV)?;

        let (stream_to_release, head_to_disable, window_framebuffers_to_retire) =
            if let Some(previous) = self.streams.first() {
                let (head, windows) = self.prepare_output_disable(previous.display_id)?;
                (Some(previous.display_id), head, windows)
            } else {
                (None, None, KVec::new())
            };
        let framebuffer = Framebuffer::new(self.mm, self.bar_user, configuration, pixels)?;
        self.streams.reserve(1, GFP_KERNEL)?;
        Ok(DisplayCommit {
            configuration: *configuration,
            head_index,
            window_index,
            stream_to_release,
            head_to_disable,
            window_framebuffers_to_retire,
            framebuffer,
            stream: Some(Stream::new(configuration, head.id)),
        })
    }

    /// Retains all referenced resources on failure; the caller must shut down display.
    #[expect(dead_code)]
    fn commit(&mut self, commit: DisplayCommit<'a, 'gpu>) -> Result {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        let head_index = commit.head_index;
        let window_index = commit.window_index;
        let stream_to_release = commit.stream_to_release;
        let head_to_disable = commit.head_to_disable;
        self.pending_commit = Some(commit);
        let channel = self.windows[window_index].channel.as_ref().ok_or(ENODEV)?;
        self.pending_commit
            .as_mut()
            .ok_or(EIO)?
            .framebuffer
            .bind(channel, &mut *self.hal)?;

        self.program_modeset()?;
        // Hardware has accepted the change; release the retired resources.
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        for &index in &pending.window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            let channel = window.channel.as_ref().ok_or(ENODEV)?;
            window
                .framebuffer
                .as_mut()
                .ok_or(EIO)?
                .unbind(channel, &mut *self.hal)?;
        }
        // Record the new Window -> Head -> Output association after success.
        if let Some(display_id) = stream_to_release {
            let index = self
                .streams
                .iter()
                .position(|stream| stream.display_id == display_id)
                .ok_or(EIO)?;
            self.streams.remove(index).map_err(|_| EIO)?;
        }
        if let Some(stream) = self.pending_commit.as_mut().ok_or(EIO)?.stream.take() {
            // Capacity was reserved before changing hardware. The pending framebuffer
            // remains owned even if publishing the stream unexpectedly fails.
            self.streams.push(stream, GFP_KERNEL)?;
        }
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        for &index in &pending.window_framebuffers_to_retire {
            if index != window_index {
                let window = &mut self.windows[index];
                window.framebuffer = None;
                window.head = None;
            }
        }
        if let Some(index) = head_to_disable.filter(|&index| index != head_index) {
            let head = &mut self.heads[index];
            head.mode = None;
            head.limited_range = false;
        }
        let head = self.heads.get_mut(head_index).ok_or(EIO)?;
        let window = self.windows.get_mut(window_index).ok_or(EIO)?;
        let pending = self.pending_commit.take().ok_or(EIO)?;
        head.mode = Some(pending.configuration.mode);
        head.limited_range = pending.configuration.limited_range;
        window.head = Some(head.id);
        window.framebuffer = Some(pending.framebuffer);
        Ok(())
    }

    /// Disables initial head auxiliary accesses and detaches heads from SORs.
    ///
    /// Called inside the first modeset bracket, before assigning a stream route.
    fn detach_initial_heads_from_sors(&mut self) -> Result {
        let core = self.core_channel.as_mut().ok_or(ENODEV)?;
        core.wait_idle()?;
        self.hal.emit_disable_heads_and_detach_sors(
            core,
            self.bar,
            self.hw_static_info.head_mask,
        )?;
        core.update(&*self.hal)?;
        core.wait_idle()?;
        self.initial_detach_pending = false;
        Ok(())
    }

    /// Applies the head and window configuration and waits for the framebuffer to become active.
    fn apply_head_and_window_state(&mut self) -> Result {
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let configuration = pending.configuration;
        let head = self.heads[pending.head_index].id;
        let window_index = pending.window_index;
        let stream = pending.stream.as_ref().ok_or(EIO)?;
        let core = self.core_channel.as_mut().ok_or(ENODEV)?;
        core.wait_idle()?;
        let window = &mut self.windows[window_index];
        if window.read_armed_owner(self.bar)? != Some(head) {
            window.emit_owner_methods(&*self.hal, core, Some(head))?;
            window.emit_clear_format_usage_bounds_methods(&*self.hal, core)?;
            core.update(&*self.hal)?;
        }
        window.emit_usage_methods(&*self.hal, core, &configuration.mode, configuration.format)?;
        let channel = window.channel.as_mut().ok_or(ENODEV)?;
        window.immediate_channel.as_ref().ok_or(ENODEV)?;
        self.hal.emit_head_configuration(
            core,
            self.bar,
            head,
            &configuration.mode,
            configuration.limited_range,
        )?;
        self.hal
            .emit_sor_control(core, stream.sor, stream.protocol, 1 << head)?;
        self.hal
            .emit_head_display_id(core, head, configuration.display_id)?;
        self.hal.emit_window_framebuffer_configuration(
            channel,
            self.bar,
            &configuration.mode,
            configuration.format,
            configuration.pitch,
            pending.framebuffer.descriptor()?,
        )?;
        window.emit_set_window_position(&*self.hal)?;
        let channel = window.channel.as_mut().ok_or(ENODEV)?;
        let immediate = window.immediate_channel.as_mut().ok_or(ENODEV)?;
        channel.update(&*self.hal, core, Some(immediate), Update::WaitForActive)
    }

    fn program_modeset(&mut self) -> Result {
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let configuration = pending.configuration;
        let stream_to_release = pending.stream_to_release;
        let head_to_disable = pending.head_to_disable;
        let window_count = pending.window_framebuffers_to_retire.len();
        let affected_outputs = if self.initial_detach_pending {
            self.hw_static_info.display_mask
        } else {
            stream_to_release.unwrap_or(0) | configuration.display_id
        };
        let bracket = self
            .cmdq
            .send_gmc_command(
                DisplayModesetBegin::new(affected_outputs, configuration.display_id)?,
                |_| Ok(()),
            )?
            .modeset_id;

        if self.initial_detach_pending {
            self.detach_initial_heads_from_sors()?;
        } else if let Some(display_id) = stream_to_release {
            for index in 0..window_count {
                let window_index = self
                    .pending_commit
                    .as_ref()
                    .ok_or(EIO)?
                    .window_framebuffers_to_retire[index];
                self.program_disable_window(window_index)?;
            }
            self.program_disable_output(display_id, head_to_disable, bracket)?;
        }

        let pending = self.pending_commit.as_mut().ok_or(EIO)?;
        let stream = pending.stream.as_mut().ok_or(EIO)?;
        stream.assign(self.cmdq, bracket)?;
        stream.set_sink_capabilities(self.cmdq, bracket)?;
        stream.set_tmds_protocol(self.cmdq, bracket)?;

        self.apply_head_and_window_state()?;

        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let stream = pending.stream.as_ref().ok_or(EIO)?;
        let head = self.heads[pending.head_index].id;
        stream.set_monitor_power(self.cmdq, bracket, true)?;
        self.hal
            .configure_hdmi_packets(self.bar, head, &configuration.avi_infoframe)?;
        stream.set_source_active(self.cmdq, bracket, true)?;
        self.hal.set_hdmi_avmute(self.bar, head, false)?;
        self.cmdq.send_gmc_command(
            DisplayModesetEnd::new(bracket, DisplayModesetEndMode::Commit)?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    /// Finds the head and windows that become unused when the stream is removed.
    fn prepare_output_disable(&self, display_id: u32) -> Result<(Option<usize>, KVec<usize>)> {
        let stream = self
            .streams
            .iter()
            .find(|stream| stream.display_id == display_id)
            .ok_or(ENODEV)?;
        if self
            .streams
            .iter()
            .any(|other| other.display_id != display_id && other.head == stream.head)
        {
            return Ok((None, KVec::new()));
        }
        let head_index = self
            .heads
            .iter()
            .position(|head| head.id == stream.head)
            .ok_or(EIO)?;
        let mut windows = KVec::new();
        for (index, window) in self.windows.iter().enumerate() {
            if window.head == Some(stream.head) {
                windows.push(index, GFP_KERNEL)?;
            }
        }
        Ok((Some(head_index), windows))
    }

    /// Stops a window's framebuffer fetches and queues its owner removal.
    fn program_disable_window(&mut self, window_index: usize) -> Result {
        let core = self.core_channel.as_mut().ok_or(ENODEV)?;
        let window = &mut self.windows[window_index];
        let channel = window.channel.as_mut().ok_or(ENODEV)?;
        self.hal.emit_disable_window(channel, self.bar)?;
        channel.update(&*self.hal, core, None, Update::WaitForRetire)?;
        window.emit_owner_methods(&*self.hal, core, None)?;
        window.emit_clear_format_usage_bounds_methods(&*self.hal, core)
    }

    /// Programs the selected output's disable sequence within an open modeset bracket.
    fn program_disable_output(
        &mut self,
        display_id: u32,
        head_to_disable: Option<usize>,
        bracket: u64,
    ) -> Result {
        let stream = self
            .streams
            .iter()
            .find(|stream| stream.display_id == display_id)
            .ok_or(ENODEV)?;
        let head = head_to_disable.map(|index| self.heads[index].id);
        let remaining_sor_heads = self
            .streams
            .iter()
            .filter(|other| other.display_id != display_id && other.sor == stream.sor)
            .fold(0, |mask, other| mask | (1 << other.head));
        let core = self.core_channel.as_mut().ok_or(ENODEV)?;
        if let Some(head) = head {
            self.hal.set_hdmi_avmute(self.bar, head, true)?;
        }
        stream.set_source_active(self.cmdq, bracket, false)?;
        if let Some(head) = head {
            self.hal.disable_hdmi_packets(self.bar, head)?;
        }
        stream.set_monitor_power(self.cmdq, bracket, false)?;
        if let Some(head) = head {
            self.hal.emit_disable_head(core, self.bar, head)?;
        }
        self.hal
            .emit_sor_control(core, stream.sor, stream.protocol, remaining_sor_heads)?;
        core.update(&*self.hal)?;
        core.wait_idle()?;
        stream.release(self.cmdq, bracket)
    }

    /// Disables the specified output while preserving resources used by other routes.
    #[expect(dead_code)]
    fn disable_output(&mut self, display_id: u32) -> Result {
        if !self
            .streams
            .iter()
            .any(|stream| stream.display_id == display_id)
        {
            return Ok(());
        }
        let (head_to_disable, window_framebuffers_to_retire) =
            self.prepare_output_disable(display_id)?;
        let bracket = self
            .cmdq
            .send_gmc_command(DisplayModesetBegin::new(display_id, 0)?, |_| Ok(()))?
            .modeset_id;
        for &index in &window_framebuffers_to_retire {
            self.program_disable_window(index)?;
        }
        self.program_disable_output(display_id, head_to_disable, bracket)?;
        self.cmdq.send_gmc_command(
            DisplayModesetEnd::new(bracket, DisplayModesetEndMode::Commit)?,
            |_| Ok(()),
        )?;
        for &index in &window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            let channel = window.channel.as_ref().ok_or(ENODEV)?;
            window
                .framebuffer
                .as_mut()
                .ok_or(EIO)?
                .unbind(channel, &mut *self.hal)?;
        }
        for index in window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            window.framebuffer = None;
            window.head = None;
        }
        if let Some(index) = head_to_disable {
            let head = &mut self.heads[index];
            head.mode = None;
            head.limited_range = false;
        }
        let index = self
            .streams
            .iter()
            .position(|stream| stream.display_id == display_id)
            .ok_or(EIO)?;
        self.streams.remove(index).map_err(|_| EIO)?;
        Ok(())
    }
}
