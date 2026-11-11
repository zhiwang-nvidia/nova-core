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

/// Hardware reads bits 27:18 to know how many parameter words follow the header.
/// Shift the count into that field when encoding a method packet.
const METHOD_COUNT_SHIFT: u32 = 18;

/// The count field is 10 bits wide. Reject larger packets so the count cannot
/// spill into other header fields.
const METHOD_COUNT_MAX: u32 = 0x3ff;

/// DISP 4.x encodes the method byte offset in bits 13:2 of the header.
/// Reject offsets that exceed this field or are not aligned to a 32-bit word.
const METHOD_OFFSET_MASK: u32 = 0x3ffc;

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

    pub(crate) const HEAD_COUNT: u32 = 8;
    pub(crate) const HEAD_STRIDE: u32 = 0x0400;
    pub(crate) const HEAD_SET_DESKTOP_COLOR_ALPHA_RED: u32 = 0x2220;
    pub(crate) const HEAD_SET_DESKTOP_COLOR_GREEN_BLUE: u32 = 0x2224;
}

pub(crate) mod window {
    //! Method definitions shared by DISP 4.x window channels.
    //!
    //! These definitions match the C57E, C67E and C87E class interfaces.

    pub(crate) const SET_CONTEXT_DMA_NOTIFIER: u32 = 0x021c;
}
