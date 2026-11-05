// SPDX-License-Identifier: GPL-2.0

//! Common window helpers for DISP 4.x.

use kernel::prelude::*;

use crate::{
    display::{
        channel::{
            Channel,
            WindowDma, //
        },
        hal::DisplayMethodHal, //
    },
    regs, //
};

use super::methods::{
    self,
    window as window_methods, //
};

/// Emits methods to disable framebuffer/LUT and release semaphore accesses.
///
/// Acquire semaphores are absent from C57E and remain a separate HAL operation.
pub(crate) fn emit_disable_window_methods(
    hal: &dyn DisplayMethodHal,
    window: &mut Channel<'_, '_, WindowDma>,
    caps: regs::NV_PDISP_FE_SW_PRECOMP_WIN_PIPE_HDR_CAPA,
) -> Result {
    for method in [
        window_methods::SET_CONTEXT_DMA_SEMAPHORE,
        window_methods::SET_SEMAPHORE_CONTROL,
        window_methods::SET_SEMAPHORE_RELEASE,
    ] {
        window.emit(hal, method, &[0])?;
    }
    for slot in 0..window_methods::ISO_CONTEXT_COUNT {
        let offset = slot * methods::METHOD_ARRAY_STRIDE;
        window.emit(hal, window_methods::SET_CONTEXT_DMA_ISO + offset, &[0])?;
        window.emit(hal, window_methods::SET_OFFSET + offset, &[0])?;
    }
    window.emit(hal, window_methods::SET_CONTEXT_DMA_ILUT, &[0])?;
    window.emit(hal, window_methods::SET_OFFSET_ILUT, &[0])?;
    if caps.tmo_present() {
        window.emit(hal, window_methods::SET_CONTEXT_DMA_TMO_LUT, &[0])?;
        window.emit(hal, window_methods::SET_OFFSET_TMO_LUT, &[0])?;
    }
    Ok(())
}
