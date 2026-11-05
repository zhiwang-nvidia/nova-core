// SPDX-License-Identifier: GPL-2.0

//! Common modeset helpers for DISP 4.x.

use kernel::prelude::*;

use crate::display::mode::DisplayMode;

use super::methods::core;

/// DISP 4.x raster coordinates are 15 bits; DISP 5.x uses wider raster fields.
pub(crate) fn validate_mode(mode: &DisplayMode) -> Result {
    mode.validate()?;
    if mode.htotal > core::COORDINATE_MAX || mode.vtotal > core::COORDINATE_MAX {
        return Err(ERANGE);
    }
    Ok(())
}
