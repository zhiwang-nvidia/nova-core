// SPDX-License-Identifier: GPL-2.0

//! Display modeset preparation and submission.
//!
//! A modeset configures the resolution, pixel clock and synchronization
//! timings of a display.
//!
//! # Commit sequence
//!
//! 1. Validate the configuration and choose a head and window. Identify the old
//!    resources to stop and bind the GEM framebuffer before changing any
//!    active display state.
//! 2. Exchange DRM software state. When timings, layout and HDMI configuration are
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
//! containing all affected CRTCs and planes. Preparation and submission take
//! exclusive access to this display.

use kernel::prelude::*;

use nova_core::api::{
    DisplayConfiguration as FirmwareConfiguration,
    DisplayModesetEndMode,
    DisplayPixelFormat as FirmwarePixelFormat,
    VramRegion, //
};

use super::{
    channel::Update,
    format::DisplayPixelFormat,
    framebuffer::Framebuffer,
    stream::Stream,
    Display,
    DisplayConfiguration,
    DisplayMode, //
};

/// Resources and scope for one update, prepared before hardware state changes.
pub(crate) struct DisplayCommit {
    id: u64,
    configuration: DisplayConfiguration,
    head_index: usize,
    window_index: usize,
    /// Display ID of the old stream to release.
    stream_to_release: Option<u32>,
    /// Index of the old head to disable, if no other stream uses it.
    head_to_disable: Option<usize>,
    /// Window indices whose old framebuffers must finish before being unbound.
    window_framebuffers_to_retire: KVec<usize>,
    framebuffer: Framebuffer,
    stream: Option<Stream>,
}

/// Resources selected before disabling an output.
pub(crate) struct DisplayDisableCommit {
    id: u64,
    display_id: u32,
    head_to_disable: Option<usize>,
    window_framebuffers_to_retire: KVec<usize>,
}

impl DisplayCommit {
    pub(super) fn retain_backing(&self) {
        self.framebuffer.retain_backing();
    }
}

impl<'bound> Display<'bound> {
    fn next_prepared_id(&self) -> Result<u64> {
        if self.pending_commit.is_some() || self.prepared_commit_id.is_some() {
            return Err(EBUSY);
        }
        self.next_commit_id.checked_add(1).ok_or(EOVERFLOW)
    }

    /// Reports retained resources from a failed submission or cleanup.
    #[expect(dead_code)]
    pub(crate) fn needs_shutdown(&self) -> bool {
        self.pending_commit.is_some()
    }

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

    /// Checks the display generation's timing and viewport limits.
    pub(crate) fn validate_mode(&self, mode: &DisplayMode) -> Result {
        self.hal.validate_mode(mode)
    }

    /// Checks the configuration and returns the selected head and window indices.
    pub(crate) fn check_configuration(
        &self,
        configuration: &DisplayConfiguration,
    ) -> Result<(usize, usize)> {
        const FEATURE_IMP: u64 = 1 << 4;

        let mode = &configuration.mode;
        let framebuffer = &configuration.framebuffer;
        configuration.validate_framebuffer()?;
        self.validate_mode(mode)?;
        // This is the current driver's conservative RGB8 TMDS limit, not a
        // fixed hardware maximum. Raising it requires a limit derived from
        // both SOR and sink capabilities and the corresponding link setup,
        // including SCDC and scrambling for TMDS rates above 340 MHz.
        if mode.clock_khz > 165_000
            || !framebuffer.pitch.is_multiple_of(256)
            || framebuffer.pitch >> 6 > 0x1fff
            || configuration.hdmi_sink_caps & !7 != 0
            || self.caps.features & FEATURE_IMP == 0
        {
            return Err(EOPNOTSUPP);
        }
        let (head_index, window_index) = self.select_head_and_window(configuration.display_id)?;
        let hblank_end = mode.htotal - mode.hsync_start - 1;
        let vblank_end = mode.vtotal - mode.vsync_start - 1;
        let query = FirmwareConfiguration {
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
            // GMC checks the fetched image dimensions, independently of surface size.
            width: mode.hdisplay,
            height: mode.vdisplay,
            pitch: framebuffer.pitch,
            format: match framebuffer.format {
                DisplayPixelFormat::Xrgb8888 => FirmwarePixelFormat::Xrgb8888,
            },
        };
        if !self
            .api
            .with(|core| core.display_check_configuration(&query))?
        {
            return Err(EINVAL);
        }
        Ok((head_index, window_index))
    }

    /// Prepares and binds GEM backing before DRM exchanges its software state.
    #[expect(dead_code)]
    fn prepare_commit(
        &mut self,
        configuration: &DisplayConfiguration,
        region: VramRegion,
    ) -> Result<DisplayCommit> {
        let id = self.next_prepared_id()?;
        let (head_index, window_index) = self.check_configuration(configuration)?;
        let head = &self.heads[head_index];
        let window = &self.windows[window_index];
        // Check all participating channels before binding the GEM backing.
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
        let framebuffer = Framebuffer::new(configuration, region)?;
        self.streams.reserve(1, GFP_KERNEL)?;
        let mut commit = DisplayCommit {
            id,
            configuration: *configuration,
            head_index,
            window_index,
            stream_to_release,
            head_to_disable,
            window_framebuffers_to_retire,
            framebuffer,
            stream: Some(Stream::new(configuration, head.id)),
        };
        let channel = self.windows[window_index].channel.as_mut().ok_or(ENODEV)?;
        if let Err(error) = commit.framebuffer.bind(channel, &mut *self.hal) {
            if let Err(cleanup_error) = commit.framebuffer.unbind(channel, &mut *self.hal) {
                dev_err!(
                    self.dev,
                    "Failed to release prepared framebuffer: {:?}\n",
                    cleanup_error
                );
                self.pending_commit = Some(commit);
            }
            return Err(error);
        }
        self.next_commit_id = id;
        self.prepared_commit_id = Some(id);
        Ok(commit)
    }

    /// Releases a prepared binding when DRM has not exchanged its state.
    #[expect(dead_code)]
    pub(crate) fn abort_commit(&mut self, mut commit: DisplayCommit) -> Result {
        if self.prepared_commit_id != Some(commit.id) {
            return Err(EINVAL);
        }
        let result = (|| {
            let window = self.windows.get_mut(commit.window_index).ok_or(ENODEV)?;
            let channel = window.channel.as_mut().ok_or(ENODEV)?;
            commit.framebuffer.unbind(channel, &mut *self.hal)
        })();
        self.prepared_commit_id = None;
        if let Err(error) = result {
            self.pending_commit = Some(commit);
            return Err(error);
        }
        Ok(())
    }

    /// Retains all referenced resources on failure; the caller must shut down display.
    #[expect(dead_code)]
    fn commit(&mut self, commit: DisplayCommit) -> Result {
        if self.pending_commit.is_some() || self.prepared_commit_id != Some(commit.id) {
            return Err(EINVAL);
        }
        self.prepared_commit_id = None;
        let head_index = commit.head_index;
        let window_index = commit.window_index;
        let stream_to_release = commit.stream_to_release;
        let head_to_disable = commit.head_to_disable;
        self.pending_commit = Some(commit);

        self.program_modeset()?;
        // Hardware has accepted the change; release the retired resources.
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        for &index in &pending.window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            let channel = window.channel.as_mut().ok_or(ENODEV)?;
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
        self.hal
            .emit_disable_heads_and_detach_sors(core, self.hw_static_info.head_mask)?;
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
        if window.read_armed_owner(&self.api)? != Some(head) {
            window.emit_owner_methods(&*self.hal, core, Some(head))?;
            window.emit_clear_format_usage_bounds_methods(&*self.hal, core)?;
            core.update(&*self.hal)?;
        }
        window.emit_usage_methods(
            &*self.hal,
            core,
            &configuration.mode,
            configuration.framebuffer.format,
        )?;
        let channel = window.channel.as_mut().ok_or(ENODEV)?;
        window.immediate_channel.as_ref().ok_or(ENODEV)?;
        self.hal.emit_head_configuration(
            core,
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
            &configuration.mode,
            &configuration.framebuffer,
            pending.framebuffer.descriptor()?,
        )?;
        window.emit_set_window_position(&*self.hal, 0, 0)?;
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
            .api
            .with(|core| core.display_modeset_begin(affected_outputs, configuration.display_id))?;

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
        stream.assign(&self.api, bracket)?;
        stream.set_sink_capabilities(&self.api, bracket)?;
        stream.set_tmds_protocol(&self.api, bracket)?;

        self.apply_head_and_window_state()?;

        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let stream = pending.stream.as_ref().ok_or(EIO)?;
        stream.set_monitor_power(&self.api, bracket, true)?;
        self.api
            .with(|core| core.display_modeset_end(bracket, DisplayModesetEndMode::Commit))?;
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
        self.hal.emit_disable_window(channel)?;
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
        stream.set_monitor_power(&self.api, bracket, false)?;
        if let Some(head) = head {
            self.hal.emit_disable_head(core, head)?;
        }
        self.hal
            .emit_sor_control(core, stream.sor, stream.protocol, remaining_sor_heads)?;
        core.update(&*self.hal)?;
        core.wait_idle()?;
        stream.release(&self.api, bracket)
    }

    /// Selects the resources that an output disable will retire before the state exchange.
    pub(crate) fn prepare_disable_output(
        &mut self,
        display_id: u32,
    ) -> Result<Option<DisplayDisableCommit>> {
        let id = self.next_prepared_id()?;
        if !self
            .streams
            .iter()
            .any(|stream| stream.display_id == display_id)
        {
            return Ok(None);
        }
        let (head_to_disable, window_framebuffers_to_retire) =
            self.prepare_output_disable(display_id)?;
        self.next_commit_id = id;
        self.prepared_commit_id = Some(id);
        Ok(Some(DisplayDisableCommit {
            id,
            display_id,
            head_to_disable,
            window_framebuffers_to_retire,
        }))
    }

    #[expect(dead_code)]
    pub(crate) fn abort_disable_output(&mut self, commit: DisplayDisableCommit) -> Result {
        if self.prepared_commit_id != Some(commit.id) {
            return Err(EINVAL);
        }
        self.prepared_commit_id = None;
        Ok(())
    }

    pub(crate) fn commit_disable_output(&mut self, commit: DisplayDisableCommit) -> Result {
        if self.pending_commit.is_some() || self.prepared_commit_id != Some(commit.id) {
            return Err(EINVAL);
        }
        let DisplayDisableCommit {
            display_id,
            head_to_disable,
            window_framebuffers_to_retire,
            ..
        } = commit;
        let bracket = self
            .api
            .with(|core| core.display_modeset_begin(display_id, 0))?;
        for &index in &window_framebuffers_to_retire {
            self.program_disable_window(index)?;
        }
        self.program_disable_output(display_id, head_to_disable, bracket)?;
        self.api
            .with(|core| core.display_modeset_end(bracket, DisplayModesetEndMode::Commit))?;
        for &index in &window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            let channel = window.channel.as_mut().ok_or(ENODEV)?;
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
        self.prepared_commit_id = None;
        Ok(())
    }

    #[expect(dead_code)]
    fn disable_output(&mut self, display_id: u32) -> Result {
        let Some(commit) = self.prepare_disable_output(display_id)? else {
            return Ok(());
        };
        self.commit_disable_output(commit)
    }
}
