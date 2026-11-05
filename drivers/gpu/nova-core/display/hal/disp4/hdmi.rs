// SPDX-License-Identifier: GPL-2.0

//! HDMI operations shared by DISP 4.1, 4.2 and 4.4.
//!
//! These GCP operations use the 9171/C771 slot-3 layout; C871 moves GCP to
//! slot 1. Shared-packet shutdown remains class-specific even within DISP 4.x.

use kernel::{
    io::{
        register::Array,
        Io,
        Mmio, //
    },
    prelude::*, //
};

use crate::{
    display::hdmi,
    driver::Bar0,
    num::IntoSafeCast,
    regs, //
};

const GCP_SLOT: usize = 3;

/// Drains posted GCP writes; this does not prove delivery to the sink.
fn flush_gcp(sf: Mmio<'_, regs::SfUserHeadRegisters>) -> Result {
    let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(GCP_SLOT));
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(())
}

/// Mutes the DISP 4.x audio gate; DISP 5.x uses a different register.
pub(crate) fn mute_audio(bar: Bar0<'_>, head: u32) -> Result {
    let index: usize = head.into_safe_cast();
    let control = bar.read(regs::NV_PDISP_SF_HDMI_CTRL::try_at(index).ok_or(EINVAL)?);
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    // Match sfSetHdmiAudioMuteCtrl: preserve the firmware-owned source enable,
    // rekey and packet-limit fields. Caller serializes this with source ON/OFF.
    bar.write(
        regs::NV_PDISP_SF_HDMI_CTRL::try_at(index).ok_or(EINVAL)?,
        control.with_audio(false),
    );
    let control = bar.read(regs::NV_PDISP_SF_HDMI_CTRL::try_at(index).ok_or(EINVAL)?);
    if control.into_raw() == u32::MAX || control.audio() {
        return Err(EIO);
    }
    Ok(())
}

/// Sends Set_AVMUTE or Clear_AVMUTE for RGB8 through the 9171/C771 GCP layout.
pub(crate) fn set_avmute(bar: Bar0<'_>, head: u32, mute: bool) -> Result {
    let sf = hdmi::sf_user_head(bar, head)?;
    mute_audio(bar, head)?;
    let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(GCP_SLOT));
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    sf.write(
        regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(GCP_SLOT),
        control.with_enable(false),
    );
    flush_gcp(sf)?;

    let subpack = sf.read(regs::NV_PDISP_SF_HDMI_GCP_SUBPACK);
    // RGB8 uses software SB1 control and zero color-depth/packing fields,
    // matching hdmiProgramSb1Ctrl_v04_01 for Bpp_24_444.
    sf.write_reg(
        subpack
            .with_sb0(if mute { 0x01u8 } else { 0x10u8 })
            .with_const_sb1::<0>()
            .with_const_sb2::<0>()
            .with_sb1_hw(false),
    );
    // The dedicated GCP control has no checksum field. Set only its valid
    // ENABLE bit for every-frame transmission; the header is implicit.
    sf.write(
        regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(GCP_SLOT),
        regs::NV_PDISP_SF_HDMI_INFO_CTRL::zeroed().with_enable(true),
    );
    flush_gcp(sf)
}
