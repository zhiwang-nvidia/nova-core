// SPDX-License-Identifier: GPL-2.0

//! Display capabilities shared by DISP 4.x and 5.x discrete GPUs.
//!
//! Indices identify hardware capability slots. Post-composition slots correspond
//! to heads on DISP 4.x and to tiles on tiled DISP 5.x hardware.

use kernel::prelude::*;

use super::NovaCoreApi;

/// Capabilities of one display pre-composition slot.
pub struct DisplayPrecompCaps {
    /// Whether an input scaler is present.
    pub scaler: bool,
    /// Whether the first color space conversion lookup table is present.
    pub csc0_lut: bool,
    /// Whether the second color space conversion lookup table is present.
    pub csc1_lut: bool,
    /// Whether a tone-mapping lookup table is present.
    pub tmo_lut: bool,
}

/// Capabilities of one display post-composition slot.
pub struct DisplayPostcompCaps {
    /// Whether an output scaler is present.
    pub scaler: bool,
}

impl NovaCoreApi<'_> {
    /// Reads one pre-composition slot after display initialization.
    ///
    /// Returns `EINVAL` for an invalid slot and `EIO` for an all-ones response.
    pub fn display_precomp_caps(&self, index: usize) -> Result<DisplayPrecompCaps> {
        let caps = self.gpu.display_precomp_caps(index)?;
        Ok(DisplayPrecompCaps {
            scaler: caps.sclr_present(),
            csc0_lut: caps.csc0lut_present(),
            csc1_lut: caps.csc1lut_present(),
            tmo_lut: caps.tmo_present(),
        })
    }

    /// Reads one post-composition slot after display initialization.
    ///
    /// Returns `EINVAL` for an invalid slot and `EIO` for an all-ones response.
    pub fn display_postcomp_caps(&self, index: usize) -> Result<DisplayPostcompCaps> {
        let caps = self.gpu.display_postcomp_caps(index)?;
        Ok(DisplayPostcompCaps {
            scaler: caps.sclr_present(),
        })
    }
}
