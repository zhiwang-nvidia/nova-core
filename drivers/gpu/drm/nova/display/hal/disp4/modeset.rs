// SPDX-License-Identifier: GPL-2.0

//! Common modeset helpers for DISP 4.x.

use kernel::prelude::*;

use crate::display::{
    channel::{
        Channel,
        Core, //
    },
    hal::DisplayMethodHal,
    mode::DisplayMode,
    sor, //
};

use super::{
    head,
    methods::core, //
};

/// Clears every SOR's head owner mask and unsets the selected heads' display IDs.
///
/// Used to clear initial head/SOR associations before assigning any driver-owned
/// routes. The caller supplies the SOR count and submits the core UPDATE.
pub(crate) fn emit_detach_heads_from_sors_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head_mask: u32,
    sor_count: u32,
) -> Result {
    for sor in 0..sor_count {
        sor::emit_sor_control_methods(hal, channel, sor, 0, 0)?;
    }
    for head in 0..core::HEAD_COUNT {
        if head_mask & (1 << head) != 0 {
            head::emit_head_display_id_methods(hal, channel, head, 0)?;
        }
    }
    Ok(())
}

/// DISP 4.x raster coordinates are 15 bits; DISP 5.x uses wider raster fields.
pub(crate) fn validate_mode(mode: &DisplayMode) -> Result {
    mode.validate()?;
    if mode.htotal > core::COORDINATE_MAX || mode.vtotal > core::COORDINATE_MAX {
        return Err(ERANGE);
    }
    Ok(())
}
