// SPDX-License-Identifier: GPL-2.0

//! AVI packet helpers for the common DISP 4.x and 5.x register layout.
//!
//! The SF_USER aperture and AVI registers are shared; other packet slots and
//! audio control remain generation- or class-specific.

use kernel::{
    io::{
        io_project,
        register::Array,
        Io,
        Mmio, //
    },
    prelude::*, //
};

use crate::{
    driver::Bar0,
    num::IntoSafeCast,
    regs, //
};

const AVI_SLOT: usize = 0;

/// Projects one head's SF_USER bank through the existing BAR0 mapping.
pub(crate) fn sf_user_head(
    bar: Bar0<'_>,
    head: u32,
) -> Result<Mmio<'_, regs::SfUserHeadRegisters>> {
    let index: usize = head.into_safe_cast();
    let heads = io_project!(bar, build: regs::NV_PDISP_SF_USER);
    Ok(io_project!(heads, [try: index]))
}

/// Checks a version-2 AVI packet for the driver's RGB output without repetition.
pub(crate) fn validate_avi(avi: &[u8; 17]) -> Result {
    if avi[..3] != [0x82, 2, 13]
        || avi.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)) != 0
        || avi[4] & 0xe0 != 0
        || avi[8] & 0x0f != 0
    {
        return Err(EINVAL);
    }
    Ok(())
}

/// Drains posted packet writes and rejects an inaccessible SF_USER bank.
/// This readback does not prove that a packet reached the sink.
pub(crate) fn flush_avi(sf: Mmio<'_, regs::SfUserHeadRegisters>) -> Result {
    let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(AVI_SLOT));
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(())
}

/// Programs a validated AVI packet after the caller disables inherited packets.
pub(crate) fn program_avi(sf: Mmio<'_, regs::SfUserHeadRegisters>, avi: &[u8; 17]) -> Result {
    // Version 2 uses PB0..13; clear the PB14..17 extension.
    sf.write_reg(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_SUBPACK2_LOW::zeroed());
    let header = sf.read(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_HEADER);
    sf.write_reg(header.with_hb0(avi[0]).with_hb1(avi[1]).with_hb2(avi[2]));
    sf.write_reg(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_SUBPACK0_LOW::from_raw(
        u32::from_le_bytes([avi[3], avi[4], avi[5], avi[6]]),
    ));
    let high = sf.read(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_SUBPACK0_HIGH);
    sf.write_reg(high.with_pb4(avi[7]).with_pb5(avi[8]).with_pb6(avi[9]));
    sf.write_reg(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_SUBPACK1_LOW::from_raw(
        u32::from_le_bytes([avi[10], avi[11], avi[12], avi[13]]),
    ));
    let high = sf.read(regs::NV_PDISP_SF_HDMI_AVI_INFOFRAME_SUBPACK1_HIGH);
    sf.write_reg(
        high.with_pb11(avi[14])
            .with_pb12(avi[15])
            .with_pb13(avi[16]),
    );

    // Enable every-frame transmission with the verified software checksum.
    // All other control bits remain zero.
    sf.write(
        regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(AVI_SLOT),
        regs::NV_PDISP_SF_HDMI_INFO_CTRL::zeroed().with_enable(true),
    );
    flush_avi(sf)
}
