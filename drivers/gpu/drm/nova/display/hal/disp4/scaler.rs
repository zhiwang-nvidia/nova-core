// SPDX-License-Identifier: GPL-2.0

//! Scaler coefficient initialization for DISP 4.x channels.
//!
//! The methods used here are compatible across the DISP 4.x channel classes.

use kernel::prelude::*;

use crate::display::{
    channel::{
        Channel,
        Core,
        WindowDma, //
    },
    hal::DisplayMethodHal,
    scaler, //
};

use super::methods::{
    core,
    window, //
};

pub(crate) fn emit_postcomp_scaler_coeffs_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head_mask: u32,
) -> Result {
    for value in scaler::scaler_coefficients() {
        for head in 0..core::HEAD_COUNT {
            if head_mask & (1 << head) == 0 {
                continue;
            }
            let method = core::HEAD_SET_OUTPUT_SCALER_COEFF_VALUE + head * core::HEAD_STRIDE;
            channel.emit(hal, method, &[value])?;
        }
    }
    Ok(())
}

pub(crate) fn emit_precomp_scaler_coeffs_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, WindowDma>,
) -> Result {
    for value in scaler::scaler_coefficients() {
        channel.emit(hal, window::SET_INPUT_SCALER_COEFF_VALUE, &[value])?;
    }
    Ok(())
}
