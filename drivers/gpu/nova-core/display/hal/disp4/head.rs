// SPDX-License-Identifier: GPL-2.0

//! Common head helpers for DISP 4.x.

use kernel::prelude::*;

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
    channel: &mut Channel<'_, '_, Core>,
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
