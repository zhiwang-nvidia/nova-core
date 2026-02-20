// SPDX-License-Identifier: GPL-2.0

//! MCTP/NVDM protocol types for NVIDIA GPU firmware communication.
//!
//! MCTP (Management Component Transport Protocol) carries NVDM (NVIDIA
//! Device Management) messages between the kernel driver and GPU firmware
//! processors such as FSP and GSP.

#![expect(dead_code)]

/// NVDM message type identifiers carried over MCTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum NvdmType {
    /// Chain of Trust boot message.
    Cot = 0x14,
    /// FSP command response.
    FspResponse = 0x15,
}

/// MCTP transport header for NVIDIA firmware messages.
///
/// Bit layout: `[31] SOM | [30] EOM | [29:28] SEQ | [23:16] SEID`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MctpHeader(u32);

impl MctpHeader {
    const SOM_SHIFT: u32 = 31;
    const EOM_SHIFT: u32 = 30;

    /// Build a single-packet MCTP header (SOM=1, EOM=1, SEQ=0, SEID=0).
    pub(crate) const fn single_packet() -> Self {
        Self((1 << Self::SOM_SHIFT) | (1 << Self::EOM_SHIFT))
    }

    /// Return the raw packed u32.
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }

    /// Check if this is a complete single-packet message (SOM=1 and EOM=1).
    pub(crate) const fn is_single_packet(self) -> bool {
        let som = (self.0 >> Self::SOM_SHIFT) & 1;
        let eom = (self.0 >> Self::EOM_SHIFT) & 1;
        som == 1 && eom == 1
    }
}

impl From<u32> for MctpHeader {
    fn from(raw: u32) -> Self {
        Self(raw)
    }
}

/// MCTP message type for PCI vendor-defined messages.
const MSG_TYPE_VENDOR_PCI: u32 = 0x7e;

/// NVIDIA PCI vendor ID.
const VENDOR_ID_NV: u32 = 0x10de;

/// NVIDIA Vendor-Defined Message (NVDM) header over MCTP.
///
/// Bit layout: `[6:0] msg_type | [23:8] vendor_id | [31:24] nvdm_type`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NvdmHeader(u32);

impl NvdmHeader {
    const MSG_TYPE_MASK: u32 = 0x7f;
    const VENDOR_ID_SHIFT: u32 = 8;
    const VENDOR_ID_MASK: u32 = 0xffff;
    const TYPE_SHIFT: u32 = 24;
    const TYPE_MASK: u32 = 0xff;

    /// Build an NVDM header for the given message type.
    pub(crate) const fn new(nvdm_type: NvdmType) -> Self {
        Self(
            MSG_TYPE_VENDOR_PCI
                | (VENDOR_ID_NV << Self::VENDOR_ID_SHIFT)
                | ((nvdm_type as u32) << Self::TYPE_SHIFT),
        )
    }

    /// Return the raw packed u32.
    pub(crate) const fn raw(self) -> u32 {
        self.0
    }

    /// Extract the NVDM type field as a raw value.
    pub(crate) const fn nvdm_type_raw(self) -> u32 {
        (self.0 >> Self::TYPE_SHIFT) & Self::TYPE_MASK
    }

    /// Validate this header against the expected NVIDIA NVDM format and type.
    pub(crate) const fn validate(self, expected_type: NvdmType) -> bool {
        let msg_type = self.0 & Self::MSG_TYPE_MASK;
        let vendor_id = (self.0 >> Self::VENDOR_ID_SHIFT) & Self::VENDOR_ID_MASK;
        msg_type == MSG_TYPE_VENDOR_PCI
            && vendor_id == VENDOR_ID_NV
            && self.nvdm_type_raw() == expected_type as u32
    }
}

impl From<u32> for NvdmHeader {
    fn from(raw: u32) -> Self {
        Self(raw)
    }
}
