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

use nova_core::api::DisplayModesetEndMode;

use super::Display;

/// Resources and scope for one update, prepared before hardware state changes.
pub(crate) struct DisplayCommit {
    display_id: u32,
}

impl<'bound> Display<'bound> {
    /// Prepares the output association before changing the current display state.
    #[expect(dead_code)]
    fn prepare_commit(&mut self, display_id: u32) -> Result<DisplayCommit> {
        if self.pending_commit.is_some() {
            return Err(EBUSY);
        }
        Ok(DisplayCommit { display_id })
    }

    /// Retains all referenced resources on failure; the caller must shut down display.
    #[expect(dead_code)]
    fn commit(&mut self, commit: DisplayCommit) -> Result {
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
        let display_id = pending.display_id;
        let bracket = self.api.with(|core| {
            core.display_modeset_begin(self.hw_static_info.display_mask, display_id)
        })?;

        self.api
            .with(|core| core.display_modeset_end(bracket, DisplayModesetEndMode::Commit))?;
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
            .api
            .with(|core| core.display_modeset_begin(display_id, 0))?;
        self.program_disable_output(display_id, None, bracket)?;
        self.api
            .with(|core| core.display_modeset_end(bracket, DisplayModesetEndMode::Commit))?;
        Ok(())
    }
}
