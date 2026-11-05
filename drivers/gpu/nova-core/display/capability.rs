// SPDX-License-Identifier: GPL-2.0

//! Capability register access shared by DISP 4.x and 5.x discrete GPUs.
//!
//! The CAPA offsets and fields used here are shared; other fields differ by class.
//! Indices select hardware capability slots. On tiled DISP 5.x hardware, postcomp
//! slots describe tiles rather than logical heads. These readers use the discrete
//! GPU BAR0 aperture; Tegra requires a different display MMIO base.
//!
//! An all-ones read is rejected as a suspected failed MMIO response.

use kernel::{
    io::{
        register::Array,
        Io, //
    },
    prelude::*, //
};

use crate::{
    driver::Bar0,
    regs, //
};

/// Reads the capability word for one postcomp slot.
pub(super) fn read_postcomp_caps(
    bar: Bar0<'_>,
    index: usize,
) -> Result<regs::NV_PDISP_FE_SW_POSTCOMP_HEAD_HDR_CAPA> {
    let caps = bar.read(regs::NV_PDISP_FE_SW_POSTCOMP_HEAD_HDR_CAPA::try_at(index).ok_or(EINVAL)?);
    if caps.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(caps)
}

/// Reads the capability word for one precomp slot.
pub(super) fn read_precomp_caps(
    bar: Bar0<'_>,
    index: usize,
) -> Result<regs::NV_PDISP_FE_SW_PRECOMP_WIN_PIPE_HDR_CAPA> {
    let caps =
        bar.read(regs::NV_PDISP_FE_SW_PRECOMP_WIN_PIPE_HDR_CAPA::try_at(index).ok_or(EINVAL)?);
    if caps.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(caps)
}
