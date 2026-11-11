// SPDX-License-Identifier: GPL-2.0

//! Scaler helpers shared across DISP 4.x and 5.x platforms.

use kernel::prelude::*;

use nova_core::api::NovaCoreApiHandle;

use super::tables;

/// Encodes writes to the GPU's scaler coefficient table.
///
/// Each method parameter specifies which coefficient slot to update (INDEX)
/// and the signed coefficient value to store (DATA). Coefficients are read
/// from SCALER_5TAP_COEFFICIENTS in table order, with indices starting at zero.
///
/// INDEX occupies bits 19:12 and DATA occupies bits 9:0.
pub(super) fn scaler_coefficients() -> impl Iterator<Item = u32> {
    const DATA_MASK: u32 = 0x3ff;
    const INDEX_SHIFT: u32 = 12;

    let ratios = tables::SCALER_5TAP_COEFFICIENTS.iter();
    let phases = ratios.flatten();
    let coefficients = phases.flatten();

    (0u32..).zip(coefficients).map(|(index, coefficient)| {
        let index_bits = index << INDEX_SHIFT;
        // Preserve the low ten bits of the signed value for two's-complement encoding.
        let data_bits = u32::from(coefficient.cast_unsigned()) & DATA_MASK;

        index_bits | data_bits
    })
}

/// Checks the selected postcomp slots for a scaler.
///
/// The mask uses capability slot indices, which match head IDs on DISP 4.x.
pub(super) fn has_postcomp_scaler(api: &NovaCoreApiHandle<'_>, postcomp_mask: u32) -> Result<bool> {
    for index in 0..u32::BITS {
        if postcomp_mask & (1 << index) != 0
            && api
                .with(|core| core.display_postcomp_caps(index as usize))?
                .scaler
        {
            return Ok(true);
        }
    }
    Ok(false)
}
