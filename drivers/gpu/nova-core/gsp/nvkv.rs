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

fn header_index(hdr: u64) -> u16 {
    ((hdr >> 16) & 0xFFF) as u16
}

/// GSP static config key constants.
///
/// Open RM reference: `interface/gmcapi/gmcapi_gsp_config.h`
pub(crate) mod gsp_config_key {
    pub(crate) const GPU_NAME_STRING: u16 = 0x2000;
    pub(crate) const BAR1_PDE_BASE: u16 = 0x1020;
    pub(crate) const ENGINE_MASK: u16 = 0x1100;
    pub(crate) const FB_LENGTH: u16 = 0x1010;
    pub(crate) const FB_REGION_COUNT: u16 = 0x0010;
    pub(crate) const FB_REGION_BASE: u16 = 0x1011;
    pub(crate) const FB_REGION_LIMIT: u16 = 0x1012;
    pub(crate) const SRIOV_MAX_GFID: u16 = 0x0200;
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

/// Decoded value from a single NVKV entry.
pub(crate) enum NvkvValue<'a> {
    /// IMM32: immediate 32-bit value (from header count field).
    Imm32(u32),
    /// SEQ32: sequence of 32-bit values.
    Seq32(&'a [u8]),
    /// SEQ64: sequence of 64-bit values.
    Seq64(&'a [u8]),
    /// ARRAY8: byte array.
    Array8(&'a [u8]),
    /// ARRAY64: array of 64-bit values.
    Array64(&'a [u8]),
}

/// Iterate over all key-value pairs in an NVKV payload, calling `f` for each.
///
/// The callback receives `(key, index, value)` where `index` is the NVKV
/// entry index (bits 16:27 of the header). For most keys index is 0; for
/// indexed arrays like `ENGINE_MASK` it indicates the starting element.
pub(crate) fn nvkv_decode<F>(payload: &[u8], mut f: F) -> Result
where
    F: FnMut(u16, u16, NvkvValue<'_>),
{
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
        let index = header_index(hdr);
        let count = header_count(hdr);
        let n_data = data_u64s(opcode, count)?;

        if pos + n_data > qwords {
            return Err(EINVAL);
        }

        let data_bytes = &payload[pos * 8..(pos + n_data) * 8];

        match opcode {
            OPCODE_IMM32 => f(key, index, NvkvValue::Imm32(count)),
            OPCODE_SEQ32 => f(key, index, NvkvValue::Seq32(data_bytes)),
            OPCODE_SEQ64 => f(key, index, NvkvValue::Seq64(data_bytes)),
            OPCODE_ARRAY8 => {
                let byte_count = count as usize;
                let byte_offset = pos * 8;
                if byte_offset + byte_count > payload.len() {
                    return Err(EINVAL);
                }
                f(key, index, NvkvValue::Array8(&payload[byte_offset..byte_offset + byte_count]));
            }
            OPCODE_ARRAY64 => f(key, index, NvkvValue::Array64(data_bytes)),
            _ => {}
        }

        pos += n_data;
    }

    Ok(())
}


/// Read a u32 from an NvkvValue (IMM32 or first element of SEQ32).
pub(crate) fn nvkv_read_u32(val: &NvkvValue<'_>) -> u32 {
    match val {
        NvkvValue::Imm32(v) => *v,
        NvkvValue::Seq32(data) if data.len() >= 4 => {
            u32::from_le_bytes(data[..4].try_into().unwrap_or([0; 4]))
        }
        _ => 0,
    }
}

/// Read a u64 from an NvkvValue (first element of SEQ64).
pub(crate) fn nvkv_read_u64(val: &NvkvValue<'_>) -> u64 {
    match val {
        NvkvValue::Seq64(data) if data.len() >= 8 => {
            u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))
        }
        NvkvValue::Imm32(v) => *v as u64,
        _ => 0,
    }
}

/// Read ARRAY64 elements into a u64 slice, starting at `start_idx` offset.
///
/// `start_idx` is the NVKV index from the header (e.g. NVGMC_ENGINE_TYPE_GR=1).
/// Each element is written to `dst[start_idx + i]` if within bounds.
pub(crate) fn nvkv_read_array64(val: &NvkvValue<'_>, start_idx: usize, dst: &mut [u64]) {
    if let NvkvValue::Array64(data) = val {
        let count = data.len() / 8;
        for i in 0..count {
            let idx = start_idx + i;
            if idx < dst.len() {
                dst[idx] = u64::from_le_bytes(
                    data[i * 8..(i + 1) * 8]
                        .try_into()
                        .unwrap_or([0; 8]),
                );
            }
        }
    }
}

/// Copy a string from an NvkvValue::Array8 into a fixed buffer.
pub(crate) fn nvkv_read_string8(val: &NvkvValue<'_>, dst: &mut [u8]) {
    if let NvkvValue::Array8(data) = val {
        let len = data.len().min(dst.len());
        dst[..len].copy_from_slice(&data[..len]);
    }
}

// --- NVKV Encoding ---

/// Encode an IMM32 key-value pair as two u64s.
pub(crate) fn nvkv_imm32(key: u16, value: u32) -> [u64; 2] {
    let header = ((OPCODE_IMM32 as u64) << 28) | (key as u64) | ((value as u64) << 32);
    [header, 0]
}

/// Encode a SEQ64 key-value pair: header + N data u64s.
pub(crate) fn nvkv_push_seq64(kvs: &mut KVec<u64>, key: u16, values: &[u64]) -> Result {
    let header =
        ((OPCODE_SEQ64 as u64) << 28) | (key as u64) | ((values.len() as u64) << 32);
    kvs.push(header, GFP_KERNEL)?;
    for &v in values {
        kvs.push(v, GFP_KERNEL)?;
    }
    Ok(())
}

/// Encode an IMM32 key-value pair and push to the kvs vector.
pub(crate) fn nvkv_push_imm32(kvs: &mut KVec<u64>, key: u16, value: u32) -> Result {
    let header = ((OPCODE_IMM32 as u64) << 28) | (key as u64) | ((value as u64) << 32);
    kvs.push(header, GFP_KERNEL)?;
    Ok(())
}

/// Encode an ARRAY8 key-value pair: header + ceil(len/8) data u64s.
pub(crate) fn nvkv_push_array8(kvs: &mut KVec<u64>, key: u16, data: &[u8]) -> Result {
    let count = data.len();
    let header = ((OPCODE_ARRAY8 as u64) << 28) | (key as u64) | ((count as u64) << 32);
    kvs.push(header, GFP_KERNEL)?;

    let n_qwords = count.div_ceil(8);
    for i in 0..n_qwords {
        let start = i * 8;
        let mut buf = [0u8; 8];
        let end = (start + 8).min(count);
        buf[..end - start].copy_from_slice(&data[start..end]);
        kvs.push(u64::from_le_bytes(buf), GFP_KERNEL)?;
    }
    Ok(())
}
