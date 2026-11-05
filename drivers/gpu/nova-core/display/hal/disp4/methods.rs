// SPDX-License-Identifier: GPL-2.0

//! Common channel method definitions for DISP 4.x.
//!
//! Channel methods share compatible definitions across DISP 4.x classes.
//!
//! A method packet contains a 32-bit header followed by `count` 32-bit parameter
//! words. The incrementing METHOD opcode (0) advances the method address by four
//! bytes for each parameter:
//!
//! ```text
//! Header bits:
//! +--------+----+-------+-------+-------------+-----+
//! | 31:29  | 28 | 27:18 | 17:14 | 13:2        | 1:0 |
//! +--------+----+-------+-------+-------------+-----+
//! | opcode | 0  | count | 0     | method >> 2 | 0   |
//! +--------+----+-------+-------+-------------+-----+
//!
//! Packet words:
//! +------------+------------------------------------+
//! | Word 0     | Method header                      |
//! | Word 1     | Parameter for method               |
//! | Word 2     | Parameter for method + 4           |
//! | ...        | ...                                |
//! | Word count | Parameter for method + 4*(count-1) |
//! +------------+------------------------------------+
//! ```

use kernel::prelude::*;

pub(crate) mod c67e;
pub(crate) mod c77d;
pub(crate) mod csc;

/// Hardware reads bits 27:18 to know how many parameter words follow the header.
/// Shift the count into that field when encoding a method packet.
const METHOD_COUNT_SHIFT: u32 = 18;

/// The count field is 10 bits wide. Reject larger packets so the count cannot
/// spill into other header fields.
const METHOD_COUNT_MAX: u32 = 0x3ff;

/// DISP 4.x encodes the method byte offset in bits 13:2 of the header.
/// Reject offsets that exceed this field or are not aligned to a 32-bit word.
const METHOD_OFFSET_MASK: u32 = 0x3ffc;

pub(crate) const METHOD_ARRAY_STRIDE: u32 = 4;

pub(crate) fn encode_method(method: u32, count: usize) -> Result<u32> {
    let count = u32::try_from(count).map_err(|_| EINVAL)?;
    if !(1..=METHOD_COUNT_MAX).contains(&count) {
        return Err(EINVAL);
    }

    if !method.is_multiple_of(4) || method > METHOD_OFFSET_MASK {
        return Err(EINVAL);
    }

    let last_method = method + (count - 1) * 4;
    if last_method > METHOD_OFFSET_MASK {
        return Err(EINVAL);
    }

    Ok((count << METHOD_COUNT_SHIFT) | method)
}

pub(crate) mod core {
    //! Method definitions shared by DISP 4.x core channels.
    //!
    //! These definitions match the C57D, C67D, C77D and C87D class interfaces.

    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = 0x0208;

    pub(crate) const HEAD_STRIDE: u32 = 0x0400;
    pub(crate) const COORDINATE_MAX: u32 = 0x7fff;
    pub(crate) const HEAD_SET_DISPLAY_ID: u32 = 0x2020;
    pub(crate) const HEAD_SET_CONTEXT_DMA_CURSOR: u32 = 0x2088;
    pub(crate) const HEAD_SET_OFFSET_CURSOR: u32 = 0x2090;
    pub(crate) const HEAD_SET_CONTROL_CURSOR: u32 = 0x209c;
    pub(crate) const HEAD_CURSOR_FORMAT_A8R8G8B8: u32 = 0xcf;
    pub(crate) const HEAD_SET_CONTEXT_DMA_CRC: u32 = 0x2180;
    pub(crate) const HEAD_SET_CRC_CONTROL: u32 = 0x2184;
    pub(crate) const HEAD_SET_OUTPUT_SCALER_COEFF_VALUE: u32 = 0x2214;
    pub(crate) const HEAD_SET_DESKTOP_COLOR_ALPHA_RED: u32 = 0x2220;
    pub(crate) const HEAD_SET_DESKTOP_COLOR_GREEN_BLUE: u32 = 0x2224;
    pub(crate) const HEAD_SET_OCSC0CONTROL: u32 = 0x2240;
    pub(crate) const HEAD_SET_CONTEXT_DMA_OLUT: u32 = 0x2288;
    pub(crate) const HEAD_SET_OFFSET_OLUT: u32 = 0x228c;
    pub(crate) use crate::display::methods::core::HEAD_COUNT;
}

pub(crate) mod window {
    //! Method definitions shared by DISP 4.x window channels.
    //!
    //! These definitions match the C57E, C67E and C87E class interfaces.

    pub(crate) const EYE_COUNT: u32 = 2;
    pub(crate) const PLANE_COUNT: u32 = 3;
    pub(crate) const ISO_CONTEXT_COUNT: u32 = EYE_COUNT * PLANE_COUNT;

    pub(crate) const SET_SEMAPHORE_CONTROL: u32 = 0x020c;
    pub(crate) const SET_SEMAPHORE_RELEASE: u32 = 0x0214;
    pub(crate) const SET_CONTEXT_DMA_SEMAPHORE: u32 = 0x0218;
    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = 0x021c;

    pub(crate) const SET_SIZE: u32 = 0x0224;
    pub(crate) const SET_SIZE_HEIGHT_SHIFT: u32 = 16;
    pub(crate) const SET_PLANAR_STORAGE: u32 = 0x0230;
    // Pitch-linear storage encodes the byte pitch in units of 64 bytes.
    pub(crate) const SET_PLANAR_STORAGE_PITCH_BYTE_SHIFT: u32 = 6;
    pub(crate) const SET_PLANAR_STORAGE_PITCH_MASK: u32 = 0x1fff;
    pub(crate) const SET_CONTEXT_DMA_ISO: u32 = 0x0240;
    pub(crate) const SET_OFFSET: u32 = 0x0260;
    pub(crate) const SET_POINT_IN: u32 = 0x0290;
    pub(crate) const SET_SIZE_IN: u32 = 0x0298;
    pub(crate) const SET_SIZE_OUT: u32 = 0x02a4;
    pub(crate) const SET_CONTROL_INPUT_SCALER: u32 = 0x02a8;
    pub(crate) const SET_CONTROL_INPUT_SCALER_VERTICAL_TAPS_2: u32 = 1;
    pub(crate) const SET_CONTROL_INPUT_SCALER_HORIZONTAL_TAPS_2: u32 = 1 << 4;
    pub(crate) const SET_INPUT_SCALER_COEFF_VALUE: u32 = 0x02ac;

    pub(crate) const SET_COMPOSITION_CONTROL: u32 = 0x02ec;
    pub(crate) const SET_COMPOSITION_CONTROL_BYPASS_ENABLE: u32 = 1 << 16;
    pub(crate) const SET_PRESENT_CONTROL: u32 = 0x0308;
    pub(crate) const SET_PRESENT_CONTROL_MIN_PRESENT_INTERVAL_ONE: u32 = 1;
    pub(crate) const SET_PRESENT_CONTROL_BEGIN_MODE_NON_TEARING: u32 = 0;
    pub(crate) const SET_PRESENT_CONTROL_TIMESTAMP_MODE_DISABLE: u32 = 0;
    pub(crate) const SET_PRESENT_CONTROL_STEREO_MODE_MONO: u32 = 0;
    pub(crate) const SET_UPDATE_TIMESTAMP_LO: u32 = 0x0348;
    pub(crate) const SET_UPDATE_TIMESTAMP_HI: u32 = 0x034c;

    pub(crate) const SET_FMT_COEFFICIENT_C00: u32 = 0x0400;
    // Row-major S5.16 coefficients for the RGB identity transform.
    pub(crate) const FMT_IDENTITY_COEFFICIENTS: [u32; 12] =
        [0x10000, 0, 0, 0, 0, 0x10000, 0, 0, 0, 0, 0x10000, 0];
    pub(crate) const SET_CONTEXT_DMA_ILUT: u32 = 0x0444;
    pub(crate) const SET_OFFSET_ILUT: u32 = 0x0448;
    pub(crate) const SET_CSC00CONTROL: u32 = 0x045c;
    pub(crate) const SET_CSC0LUT_CONTROL: u32 = 0x04a0;
    pub(crate) const SET_CSC0LUT_SEGMENT_SIZE: u32 = 0x04a4;
    pub(crate) const SET_CSC0LUT_ENTRY: u32 = 0x04a8;
    pub(crate) const SET_CSC01CONTROL: u32 = 0x04bc;
    pub(crate) const SET_CONTEXT_DMA_TMO_LUT: u32 = 0x0528;
    pub(crate) const SET_OFFSET_TMO_LUT: u32 = 0x052c;
    pub(crate) const SET_CSC10CONTROL: u32 = 0x053c;
    pub(crate) const SET_CSC1LUT_CONTROL: u32 = 0x0580;
    pub(crate) const SET_CSC1LUT_SEGMENT_SIZE: u32 = 0x0584;
    pub(crate) const SET_CSC1LUT_ENTRY: u32 = 0x0588;
    pub(crate) const SET_CSC11CONTROL: u32 = 0x059c;
    pub(crate) const LUT_VALUE_SHIFT: u32 = 16;
}
