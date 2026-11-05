// SPDX-License-Identifier: GPL-2.0

//! Pixel formats supported by the display implementation.

/// Framebuffer pixel format.
///
/// The current implementation supports XRGB8888 framebuffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayPixelFormat {
    /// Eight bits each of red, green and blue, with eight unused bits.
    Xrgb8888,
}

impl DisplayPixelFormat {
    /// Returns the number of bytes occupied by one pixel.
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Xrgb8888 => 4,
        }
    }
}
