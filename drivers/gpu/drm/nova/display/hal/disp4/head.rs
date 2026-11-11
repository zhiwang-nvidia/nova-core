// SPDX-License-Identifier: GPL-2.0

//! Common head helpers for DISP 4.x.

use kernel::prelude::*;

use nova_core::api::DisplayPostcompCaps;

use crate::display::{
    channel::{
        Channel,
        Core, //
    },
    hal::DisplayMethodHal, //
};

use super::methods::core;

pub(crate) fn emit_desktop_color_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head_mask: u32,
) -> Result {
    for head in 0..core::HEAD_COUNT {
        if head_mask & (1 << head) == 0 {
            continue;
        }
        let offset = head * core::HEAD_STRIDE;
        channel.emit(
            hal,
            core::HEAD_SET_DESKTOP_COLOR_ALPHA_RED + offset,
            &[0xff],
        )?;
        channel.emit(hal, core::HEAD_SET_DESKTOP_COLOR_GREEN_BLUE + offset, &[0])?;
    }
    Ok(())
}

/// Disables cursor, output LUT and CRC memory accesses, and the output CSC0 matrix.
pub(crate) fn emit_disable_head_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head: u32,
    caps: &DisplayPostcompCaps,
) -> Result {
    let offset = head * core::HEAD_STRIDE;
    // Keep a valid cursor format while disabling the cursor and unsetting its handles.
    channel.emit(
        hal,
        core::HEAD_SET_CONTROL_CURSOR + offset,
        &[core::HEAD_CURSOR_FORMAT_A8R8G8B8],
    )?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_CURSOR + offset, &[0, 0])?;
    channel.emit(hal, core::HEAD_SET_OFFSET_CURSOR + offset, &[0, 0])?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_OLUT + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_OFFSET_OLUT + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_CRC + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_CRC_CONTROL + offset, &[0])?;
    if caps.ocsc0 {
        channel.emit(hal, core::HEAD_SET_OCSC0CONTROL + offset, &[0])?;
    }
    Ok(())
}

/// Emits a head's GSP-RM display ID; zero unsets the association.
pub(crate) fn emit_head_display_id_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head: u32,
    display_id: u32,
) -> Result {
    channel.emit(
        hal,
        core::HEAD_SET_DISPLAY_ID + head * core::HEAD_STRIDE,
        &[display_id],
    )
}
