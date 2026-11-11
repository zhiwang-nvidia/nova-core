// SPDX-License-Identifier: GPL-2.0

//! HDMI packet programming for Ada's C771 interface.
//!
//! SF_USER access and AVI programming share the DISP 4.x and 5.x layout.
//! GCP uses the 9171/C771 slot-3 layout shared by DISP 4.1, 4.2 and 4.4;
//! C871 moves GCP to slot 1, and DISP 5.x uses a different audio gate.
//! Shared packet-slot shutdown remains class-specific even within DISP 4.x.

use kernel::{
    io::{
        io_project,
        register::{
            Array,
            RegisterArray, //
        },
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

use super::{
    Architecture,
    Gpu, //
};

const AVI_SLOT: usize = 0;
const GCP_SLOT: usize = 3;

impl Gpu<'_> {
    fn display_hdmi_bar(&self) -> Result<Bar0<'_>> {
        if !matches!(self.spec.chipset.arch(), Architecture::Ada) {
            return Err(EOPNOTSUPP);
        }
        Ok(self.gsp_resources.bar)
    }

    #[expect(dead_code)]
    pub(crate) fn display_configure_hdmi_packets(&self, head: u32, avi: &[u8; 17]) -> Result {
        let bar = self.display_hdmi_bar()?;
        validate_avi(avi)?;
        let sf = sf_user_head(bar, head)?;
        self.display_disable_hdmi_packets(head)?;
        program_avi(sf, avi)
    }

    #[expect(dead_code)]
    pub(crate) fn display_set_hdmi_avmute(&self, head: u32, mute: bool) -> Result {
        let bar = self.display_hdmi_bar()?;
        set_avmute(bar, head, mute)
    }

    pub(crate) fn display_disable_hdmi_packets(&self, head: u32) -> Result {
        let bar = self.display_hdmi_bar()?;
        let sf = sf_user_head(bar, head)?;
        mute_audio(bar, head)?;
        // C771 exposes five legacy slots and six shared packet slots.
        for slot in 0..regs::NV_PDISP_SF_HDMI_INFO_CTRL::SIZE {
            let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::try_at(slot).ok_or(EINVAL)?);
            if control.into_raw() == u32::MAX {
                return Err(EIO);
            }
            sf.write(
                regs::NV_PDISP_SF_HDMI_INFO_CTRL::try_at(slot).ok_or(EINVAL)?,
                control.with_enable(false),
            );
        }
        // Slot 2 is ACR, not an Audio InfoFrame. Its frequency selector has a
        // separate enable. Neither field alone gates hardware-generated audio.
        let acr = sf.read(regs::NV_PDISP_SF_HDMI_ACR_CTRL);
        sf.write_reg(acr.with_packet_enable(false).with_freqs_enable(false));
        for slot in 0..regs::NV_PDISP_SF_SHARED_GENERIC_CTRL::SIZE {
            let control =
                sf.read(regs::NV_PDISP_SF_SHARED_GENERIC_CTRL::try_at(slot).ok_or(EINVAL)?);
            if control.into_raw() == u32::MAX {
                return Err(EIO);
            }
            sf.write(
                regs::NV_PDISP_SF_SHARED_GENERIC_CTRL::try_at(slot).ok_or(EINVAL)?,
                control.with_enable(false),
            );
        }
        flush_avi(sf)
    }
}

/// Projects one head's SF_USER bank through the existing BAR0 mapping.
fn sf_user_head(bar: Bar0<'_>, head: u32) -> Result<Mmio<'_, regs::SfUserHeadRegisters>> {
    let index: usize = head.into_safe_cast();
    let heads = io_project!(bar, build: regs::NV_PDISP_SF_USER);
    Ok(io_project!(heads, [try: index]))
}

/// Checks a version-2 AVI packet for the driver's RGB output without repetition.
fn validate_avi(avi: &[u8; 17]) -> Result {
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
fn flush_avi(sf: Mmio<'_, regs::SfUserHeadRegisters>) -> Result {
    let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(AVI_SLOT));
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(())
}

/// Programs a validated AVI packet after the caller disables inherited packets.
fn program_avi(sf: Mmio<'_, regs::SfUserHeadRegisters>, avi: &[u8; 17]) -> Result {
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

/// Drains posted GCP writes; this does not prove delivery to the sink.
fn flush_gcp(sf: Mmio<'_, regs::SfUserHeadRegisters>) -> Result {
    let control = sf.read(regs::NV_PDISP_SF_HDMI_INFO_CTRL::at(GCP_SLOT));
    if control.into_raw() == u32::MAX {
        return Err(EIO);
    }
    Ok(())
}

/// Mutes the DISP 4.x audio gate; DISP 5.x uses a different register.
fn mute_audio(bar: Bar0<'_>, head: u32) -> Result {
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
fn set_avmute(bar: Bar0<'_>, head: u32, mute: bool) -> Result {
    let sf = sf_user_head(bar, head)?;
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
