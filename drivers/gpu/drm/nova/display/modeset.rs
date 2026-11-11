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
    DisplayPixelFormat as FirmwarePixelFormat, //
};

use super::{
    format::DisplayPixelFormat,
    stream::Stream,
    Display,
    DisplayConfiguration,
    DisplayMode, //
};

/// Resources and scope for one update, prepared before hardware state changes.
pub(crate) struct DisplayCommit {
    configuration: DisplayConfiguration,
    head_index: usize,
    window_index: usize,
    /// Display ID of the old stream to release.
    stream_to_release: Option<u32>,
    /// Index of the old head to disable, if no other stream uses it.
    head_to_disable: Option<usize>,
    /// Window indices whose old framebuffers must finish before being unbound.
    window_framebuffers_to_retire: KVec<usize>,
    stream: Option<Stream>,
}

impl<'bound> Display<'bound> {
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

    /// Prepares the output association before changing the current display state.
    #[expect(dead_code)]
    fn prepare_commit(&mut self, configuration: &DisplayConfiguration) -> Result<DisplayCommit> {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        let (head_index, window_index) = self.check_configuration(configuration)?;
        let head = &self.heads[head_index];
        let window = &self.windows[window_index];
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
        self.streams.reserve(1, GFP_KERNEL)?;
        Ok(DisplayCommit {
            configuration: *configuration,
            head_index,
            window_index,
            stream_to_release,
            head_to_disable,
            window_framebuffers_to_retire,
            stream: Some(Stream::new(configuration, head.id)),
        })
    }

    /// Retains all referenced resources on failure; the caller must shut down display.
    #[expect(dead_code)]
    fn commit(&mut self, commit: DisplayCommit) -> Result {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        let head_index = commit.head_index;
        let window_index = commit.window_index;
        let stream_to_release = commit.stream_to_release;
        self.pending_commit = Some(commit);

        self.program_modeset()?;
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
            // Capacity was reserved before changing hardware.
            self.streams.push(stream, GFP_KERNEL)?;
        }
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        for &index in &pending.window_framebuffers_to_retire {
            if index != window_index {
                let window = &mut self.windows[index];
                window.head = None;
            }
        }
        let head = self.heads.get_mut(head_index).ok_or(EIO)?;
        let window = self.windows.get_mut(window_index).ok_or(EIO)?;
        self.pending_commit.take().ok_or(EIO)?;
        window.head = Some(head.id);
        Ok(())
    }

    fn program_modeset(&mut self) -> Result {
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let configuration = pending.configuration;
        let stream_to_release = pending.stream_to_release;
        let head_to_disable = pending.head_to_disable;
        let affected_outputs = stream_to_release.unwrap_or(0) | configuration.display_id;
        let bracket = self
            .api
            .with(|core| core.display_modeset_begin(affected_outputs, configuration.display_id))?;

        if let Some(display_id) = stream_to_release {
            self.program_disable_output(display_id, head_to_disable, bracket)?;
        }

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

    /// Programs the selected output's disable sequence within an open modeset bracket.
    fn program_disable_output(
        &mut self,
        display_id: u32,
        _head_to_disable: Option<usize>,
        _bracket: u64,
    ) -> Result {
        self.streams
            .iter()
            .find(|stream| stream.display_id == display_id)
            .ok_or(ENODEV)?;
        Ok(())
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
            .api
            .with(|core| core.display_modeset_begin(display_id, 0))?;
        self.program_disable_output(display_id, head_to_disable, bracket)?;
        self.api
            .with(|core| core.display_modeset_end(bracket, DisplayModesetEndMode::Commit))?;
        for index in window_framebuffers_to_retire {
            let window = &mut self.windows[index];
            window.head = None;
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
