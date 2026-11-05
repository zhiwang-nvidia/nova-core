// SPDX-License-Identifier: GPL-2.0

//! Progressive display timings supplied by the DRM driver.
//!
//! Mode validation applies the current driver's supported range. Unpacked timing
//! calculations are independent of the GPU class; the HAL applies each
//! generation's method field limits and encoding.

use kernel::prelude::*;

use super::DisplayPixelFormat;

/// Active-first raster coordinates, with the pixel clock in kHz.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    /// Pixel clock in kHz.
    pub clock_khz: u32,
    /// Visible horizontal pixels.
    pub hdisplay: u32,
    /// Horizontal sync start in pixels.
    pub hsync_start: u32,
    /// Horizontal sync end in pixels.
    pub hsync_end: u32,
    /// Total horizontal pixels, including blanking.
    pub htotal: u32,
    /// Visible vertical lines.
    pub vdisplay: u32,
    /// Vertical sync start in lines.
    pub vsync_start: u32,
    /// Vertical sync end in lines.
    pub vsync_end: u32,
    /// Total vertical lines, including blanking.
    pub vtotal: u32,
    /// Whether horizontal sync is active low.
    pub negative_hsync: bool,
    /// Whether vertical sync is active low.
    pub negative_vsync: bool,
}

impl DisplayMode {
    /// Checks the driver mode limits and the framebuffer's minimum row stride.
    ///
    /// These checks are independent of the GPU class. Hardware pitch alignment
    /// and method field limits are checked by the HAL.
    pub(crate) fn validate_framebuffer(&self, format: DisplayPixelFormat, pitch: u32) -> Result {
        self.validate()?;
        let minimum_pitch = self
            .hdisplay
            .checked_mul(format.bytes_per_pixel())
            .ok_or(EOVERFLOW)?;
        if pitch < minimum_pitch {
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
}
