// SPDX-License-Identifier: GPL-2.0

//! Framebuffer layout.

use kernel::prelude::*;

use super::format::DisplayPixelFormat;

/// The dimensions and storage layout of a complete framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FramebufferLayout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: DisplayPixelFormat,
    pub(crate) pitch: u32,
}

impl FramebufferLayout {
    // DISP 4.x and 5.x window surface dimensions have 16-bit fields.
    pub(crate) const MAX_DIMENSION: u32 = 0xffff;

    pub(crate) fn validate(&self) -> Result {
        if self.width == 0
            || self.height == 0
            || self.width > Self::MAX_DIMENSION
            || self.height > Self::MAX_DIMENSION
        {
            return Err(EINVAL);
        }
        let minimum_pitch = self
            .width
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or(EOVERFLOW)?;
        if self.pitch < minimum_pitch {
            return Err(EINVAL);
        }
        Ok(())
    }
}
