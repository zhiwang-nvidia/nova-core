// SPDX-License-Identifier: GPL-2.0

//! NVKV key/value decoder for GMC API responses.
//!
//! NVKV is a pushbuffer-like encoding used by GSP-RM to transmit structured
//! data as an array of `u64` values. Each entry starts with a header `u64`
//! containing the opcode, key, index, and count (or immediate value),
//! followed by zero or more data `u64`s.
//!
//! Open RM reference: `inc/libraries/nvkv/nvkv.h`

use kernel::prelude::*;

const OPCODE_IMM32: u32 = 0x0;
const OPCODE_SEQ32: u32 = 0x1;
const OPCODE_SEQ64: u32 = 0x2;
const OPCODE_ARRAY8: u32 = 0x3;
const OPCODE_ARRAY32: u32 = 0x4;
const OPCODE_ARRAY64: u32 = 0x5;

fn header_key(hdr: u64) -> u16 {
    (hdr & 0xFFFF) as u16
}

fn header_opcode(hdr: u64) -> u32 {
    ((hdr >> 28) & 0xF) as u32
}

fn header_count(hdr: u64) -> u32 {
    (hdr >> 32) as u32
}

/// Number of data `u64`s that follow a given header.
fn data_u64s(opcode: u32, count: u32) -> Result<usize> {
    match opcode {
        OPCODE_IMM32 => Ok(0),
        OPCODE_SEQ32 => Ok(count.div_ceil(2) as usize),
        OPCODE_SEQ64 => Ok(count as usize),
        OPCODE_ARRAY8 => Ok((count as usize).div_ceil(8)),
        OPCODE_ARRAY32 => Ok((count as usize * 4).div_ceil(8)),
        OPCODE_ARRAY64 => Ok(count as usize),
        _ => Err(EINVAL),
    }
}

/// GSP static config key constants.
///
/// Open RM reference: `interface/gmcapi/gmcapi_gsp_config.h`
pub(crate) mod gsp_config_key {
    /// GPU name string (ARRAY8, null-terminated).
    pub(crate) const GPU_NAME_STRING: u16 = 0x2000;
}

/// Find a key in an NVKV-encoded byte buffer and return its raw data bytes.
///
/// The `payload` is the raw response bytes from a `GMCAPI_DYNAMIC` command.
/// Returns `None` if the key is not found.
///
/// # Errors
///
/// Returns `EINVAL` if the NVKV data is malformed (bad opcode or truncated).
pub(crate) fn find_array8<'a>(payload: &'a [u8], target_key: u16) -> Result<Option<&'a [u8]>> {
    if payload.len() % 8 != 0 {
        return Err(EINVAL);
    }

    let qwords = payload.len() / 8;
    let mut pos = 0usize;

    while pos < qwords {
        let hdr = u64::from_le_bytes(
            payload[pos * 8..(pos + 1) * 8]
                .try_into()
                .map_err(|_| EINVAL)?,
        );
        pos += 1;

        let opcode = header_opcode(hdr);
        let key = header_key(hdr);
        let count = header_count(hdr);
        let n_data = data_u64s(opcode, count)?;

        if pos + n_data > qwords {
            return Err(EINVAL);
        }

        if key == target_key && opcode == OPCODE_ARRAY8 {
            let byte_offset = pos * 8;
            let byte_count = count as usize;
            if byte_offset + byte_count > payload.len() {
                return Err(EINVAL);
            }
            return Ok(Some(&payload[byte_offset..byte_offset + byte_count]));
        }

        pos += n_data;
    }

    Ok(None)
}
