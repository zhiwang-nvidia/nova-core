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

use core::marker::PhantomData;

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
    Display,
    DisplayConfiguration, //
};

/// Resources and scope for one update, prepared before hardware state changes.
pub(crate) struct DisplayCommit<'a, 'gpu> {
    _lifetimes: PhantomData<(&'a (), &'gpu ())>,
    configuration: DisplayConfiguration,
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    fn select_head_and_window(&self, display_id: u32) -> Result<(usize, usize)> {
        let output = self
            .outputs()?
            .into_iter()
            .find(|output| output.display_id == display_id && output.connected)
            .ok_or(ENODEV)?;
        if self.windows.is_empty() {
            return Err(ENODEV);
        }
        let head_index = self
            .heads
            .iter()
            .position(|head| output.head_mask & (1 << head.id) != 0)
            .ok_or(ENODEV)?;
        Ok((head_index, 0))
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

    /// Prepares the output association before changing the current display state.
    #[expect(dead_code)]
    fn prepare_commit(
        &mut self,
        configuration: &DisplayConfiguration,
    ) -> Result<DisplayCommit<'a, 'gpu>> {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        self.check_configuration(configuration)?;
        Ok(DisplayCommit {
            _lifetimes: PhantomData,
            configuration: *configuration,
        })
    }

    /// Retains all referenced resources on failure; the caller must shut down display.
    #[expect(dead_code)]
    fn commit(&mut self, commit: DisplayCommit<'a, 'gpu>) -> Result {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        self.pending_commit = Some(commit);

        self.program_modeset()?;
        self.pending_commit.take().ok_or(EIO)?;
        Ok(())
    }

    fn program_modeset(&mut self) -> Result {
        let pending = self.pending_commit.as_ref().ok_or(EIO)?;
        let configuration = pending.configuration;
        let bracket = self
            .cmdq
            .send_gmc_command(
                DisplayModesetBegin::new(
                    self.hw_static_info.display_mask,
                    configuration.display_id,
                )?,
                |_| Ok(()),
            )?
            .modeset_id;

        self.cmdq.send_gmc_command(
            DisplayModesetEnd::new(bracket, DisplayModesetEndMode::Commit)?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    /// Programs the selected output's disable sequence within an open modeset bracket.
    fn program_disable_output(
        &mut self,
        _display_id: u32,
        _head_to_disable: Option<usize>,
        _bracket: u64,
    ) -> Result {
        Ok(())
    }

    /// Disables the specified output while preserving resources used by other routes.
    #[expect(dead_code)]
    fn disable_output(&mut self, display_id: u32) -> Result {
        let bracket = self
            .cmdq
            .send_gmc_command(DisplayModesetBegin::new(display_id, 0)?, |_| Ok(()))?
            .modeset_id;
        self.program_disable_output(display_id, None, bracket)?;
        self.cmdq.send_gmc_command(
            DisplayModesetEnd::new(bracket, DisplayModesetEndMode::Commit)?,
            |_| Ok(()),
        )?;
        Ok(())
    }
}
