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
    /// Whether the CSC00 color space conversion matrix is present.
    pub csc00: bool,
    /// Whether the CSC01 color space conversion matrix is present.
    pub csc01: bool,
    /// Whether the first color space conversion lookup table is present.
    pub csc0_lut: bool,
    /// Whether the CSC10 color space conversion matrix is present.
    pub csc10: bool,
    /// Whether the CSC11 color space conversion matrix is present.
    pub csc11: bool,
    /// Whether the second color space conversion lookup table is present.
    pub csc1_lut: bool,
    /// Whether a tone-mapping lookup table is present.
    pub tmo_lut: bool,
}

/// Capabilities of one display post-composition slot.
pub struct DisplayPostcompCaps {
    /// Whether an output scaler is present.
    pub scaler: bool,
    /// Whether output color space conversion matrix 0 is present.
    pub ocsc0: bool,
    /// Whether output color space conversion matrix 1 is present.
    pub ocsc1: bool,
}

impl NovaCoreApi<'_> {
    /// Reads one pre-composition slot after display initialization.
    ///
    /// Returns `EINVAL` for an invalid slot and `EIO` for an all-ones response.
    pub fn display_precomp_caps(&self, index: usize) -> Result<DisplayPrecompCaps> {
        let caps = self.gpu.display_precomp_caps(index)?;
        Ok(DisplayPrecompCaps {
            scaler: caps.sclr_present(),
            csc00: caps.csc00_present(),
            csc01: caps.csc01_present(),
            csc0_lut: caps.csc0lut_present(),
            csc10: caps.csc10_present(),
            csc11: caps.csc11_present(),
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
            ocsc0: caps.ocsc0_present(),
            ocsc1: caps.ocsc1_present(),
        })
    }
}
