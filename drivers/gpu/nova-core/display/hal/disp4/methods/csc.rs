// SPDX-License-Identifier: GPL-2.0

//! Common color space conversion (CSC) lookup table (LUT) helpers for DISP 4.x.
//!
//! Emits methods to initialize the LUTs used by the window color pipeline.

use kernel::prelude::*;

use crate::display::{
    channel::{
        Channel,
        WindowDma, //
    },
    hal::DisplayMethodHal,
    tables, //
};

use super::window;

/// Emits the PQ OETF into CSC0 and the PQ EOTF into CSC1.
pub(crate) fn emit_csc_lut_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, '_, WindowDma>,
) -> Result {
    emit_lut_methods(
        hal,
        channel,
        window::SET_CSC0LUT_SEGMENT_SIZE,
        window::SET_CSC0LUT_ENTRY,
        &tables::PQ_OETF_SEGMENT_SIZES_LOG2,
        &tables::PQ_OETF_ENTRIES,
    )?;
    emit_lut_methods(
        hal,
        channel,
        window::SET_CSC1LUT_SEGMENT_SIZE,
        window::SET_CSC1LUT_ENTRY,
        &tables::PQ_EOTF_SEGMENT_SIZES_LOG2,
        &tables::PQ_EOTF_ENTRIES,
    )
}

/// Emits the segment-size and entry methods for one CSC LUT.
///
/// Each parameter's IDX field selects the segment or LUT entry to write. VALUE
/// contains the log2 segment size for `segment_method`, or the table value for
/// `entry_method`.
fn emit_lut_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, '_, WindowDma>,
    segment_method: u32,
    entry_method: u32,
    segment_sizes_log2: &[u8],
    entries: &[u16],
) -> Result {
    for (index, size_log2) in (0u32..).zip(segment_sizes_log2) {
        let data = index | (u32::from(*size_log2) << window::LUT_VALUE_SHIFT);
        channel.emit(hal, segment_method, &[data])?;
    }
    for (index, value) in (0u32..).zip(entries) {
        let data = index | (u32::from(*value) << window::LUT_VALUE_SHIFT);
        channel.emit(hal, entry_method, &[data])?;
    }
    Ok(())
}
