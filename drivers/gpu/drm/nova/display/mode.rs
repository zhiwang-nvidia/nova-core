// SPDX-License-Identifier: GPL-2.0

//! Progressive display timings.
//!
//! Mode validation applies the current driver's supported range. Unpacked timing
//! calculations are independent of the GPU class; the HAL applies each
//! generation's method field limits and encoding.

use kernel::prelude::*;

use super::framebuffer::FramebufferLayout;

/// Active-first raster coordinates, with the pixel clock in kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayMode {
    /// Pixel clock in kHz.
    pub(crate) clock_khz: u32,
    /// Visible horizontal pixels.
    pub(crate) hdisplay: u32,
    /// Horizontal sync start in pixels.
    pub(crate) hsync_start: u32,
    /// Horizontal sync end in pixels.
    pub(crate) hsync_end: u32,
    /// Total horizontal pixels, including blanking.
    pub(crate) htotal: u32,
    /// Visible vertical lines.
    pub(crate) vdisplay: u32,
    /// Vertical sync start in lines.
    pub(crate) vsync_start: u32,
    /// Vertical sync end in lines.
    pub(crate) vsync_end: u32,
    /// Total vertical lines, including blanking.
    pub(crate) vtotal: u32,
    /// Whether horizontal sync is active low.
    pub(crate) negative_hsync: bool,
    /// Whether vertical sync is active low.
    pub(crate) negative_vsync: bool,
}

/// Display output configuration and its framebuffer.
///
/// The current modeset implementation supports one active HDMI output with
/// progressive RGB8 timings and an unscaled full-screen image taken from the
/// framebuffer's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DisplayConfiguration {
    /// Single-bit GSP-RM display identifier.
    pub(crate) display_id: u32,
    /// Progressive mode selected from the sink's advertised modes.
    pub(crate) mode: DisplayMode,
    /// Complete framebuffer layout, independently of the visible mode.
    pub(crate) framebuffer: FramebufferLayout,
    /// GMC HDMI sink bits: above-340-MHz support, low-rate scrambling and SCDC.
    pub(crate) hdmi_sink_caps: u32,
    /// Packed AVI infoframe, including its header and checksum.
    pub(crate) avi_infoframe: [u8; 17],
    /// Whether the HDMI output converts framebuffer RGB to limited range.
    pub(crate) limited_range: bool,
}

impl DisplayConfiguration {
    /// Checks that the framebuffer can supply the visible mode.
    ///
    /// Generation-specific timing and pitch limits are checked by the HAL.
    pub(crate) fn validate_framebuffer(&self) -> Result {
        self.mode.validate_framebuffer(&self.framebuffer)
    }
}

/// Sync-relative raster coordinates shared by DISP 4.x and 5.x.
///
/// These values are unpacked pixel and line counts. Their method field widths
/// and addresses depend on the display generation.
pub(crate) struct RasterTimings {
    pub(crate) hsync_end: u32,
    pub(crate) vsync_end: u32,
    pub(crate) hblank_end: u32,
    pub(crate) vblank_end: u32,
    pub(crate) hblank_start: u32,
    pub(crate) vblank_start: u32,
    pub(crate) leading_idle: u32,
    pub(crate) trailing_idle: u32,
}

impl DisplayMode {
    /// Checks that the framebuffer can supply the visible mode.
    ///
    /// Generation-specific timing and pitch limits are checked by the HAL.
    pub(crate) fn validate_framebuffer(&self, framebuffer: &FramebufferLayout) -> Result {
        self.validate()?;
        framebuffer.validate()?;
        if framebuffer.width < self.hdisplay || framebuffer.height < self.vdisplay {
            return Err(EINVAL);
        }
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result {
        // The current clock path converts kHz to the 31-bit HERTZ field of
        // HEAD_SET_PIXEL_CLOCK_FREQUENCY. Bit 31 controls ADJ1000DIV1001, so
        // the nonzero clock must not exceed floor(0x7fff_ffff / 1000) kHz.
        if self.clock_khz == 0 || self.clock_khz > 2_147_483 {
            return Err(EINVAL);
        }

        // Full-screen framebuffers require nonzero, 15-bit head viewport dimensions.
        // At 1:1, our window usage bound reserves 22 extra pixels per line:
        // width + 22 must fit in 15 bits, leaving at most 32_745 visible pixels.
        if self.hdisplay == 0
            || self.hdisplay > 32_745
            || self.vdisplay == 0
            || self.vdisplay > 32_767
        {
            return Err(EINVAL);
        }

        // The current timing path requires positive front/back porches and
        // sync pulse widths: active end < sync start < sync end < total.
        // This also keeps the sync-relative subtractions below nonnegative.
        if self.hdisplay >= self.hsync_start
            || self.hsync_start >= self.hsync_end
            || self.hsync_end >= self.htotal
            || self.vdisplay >= self.vsync_start
            || self.vsync_start >= self.vsync_end
            || self.vsync_end >= self.vtotal
        {
            return Err(EINVAL);
        }

        // Keep totals within the DRM timing and current GMC service range.
        // The HAL additionally enforces the generation's raster field widths;
        // DISP 4.x uses 15-bit fields, while DISP 5.x uses 16-bit fields.
        if self.htotal > u32::from(u16::MAX) || self.vtotal > u32::from(u16::MAX) {
            return Err(EINVAL);
        }
        Ok(())
    }

    /// Converts active-first timings to coordinates measured from sync start.
    pub(crate) fn raster_timings(&self) -> Result<RasterTimings> {
        self.validate()?;

        let hblank_end = self.htotal - self.hsync_start - 1;
        let vblank_end = self.vtotal - self.vsync_start - 1;
        let leading_idle = vblank_end + 1;
        Ok(RasterTimings {
            hsync_end: self.hsync_end - self.hsync_start - 1,
            vsync_end: self.vsync_end - self.vsync_start - 1,
            hblank_end,
            vblank_end,
            hblank_start: self.hdisplay + hblank_end,
            vblank_start: self.vdisplay + vblank_end,
            leading_idle,
            trailing_idle: self.vtotal - leading_idle - self.vdisplay,
        })
    }
}
