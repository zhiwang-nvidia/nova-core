// SPDX-License-Identifier: GPL-2.0

//! MCTP/NVDM protocol support for inter-processor communication.
//!
//! MCTP (Management Component Transport Protocol, DMTF DSP0236) provides a
//! transport-agnostic message exchange format. NVDM (NVIDIA Data Model) is a
//! vendor-defined message layer on top of MCTP.
//!
//! Multiple GPU subsystems (FSP, GSP, SEC2) use MCTP/NVDM framing. This module
//! provides the shared header types and helpers for all of them.
//!
//! Open RM reference:
//!   `arch/nvalloc/common/inc/mctp_format.h`
//!   `arch/nvalloc/common/inc/nvdm_format.h`

use kernel::bitfield;

// MCTP transport header bit layout (DMTF DSP0236):
//
//  31  30  29:28  27  26:24  23:16  15:8  7:4   3:0
// [SOM|EOM| SEQ  | TO| TAG | SEID | DEID| RSVD| VER]
//
// TAG, TO, and RSVD are always zero in current usage.
bitfield! {
    pub(crate) struct TransportHeader(u32) {
        31:31 som => bool;
        30:30 eom => bool;
        29:28 seq;
        23:16 seid => u8;
        15:8  deid => u8;
        3:0   version;
    }
}

impl TransportHeader {
    /// Create a new MCTP transport header.
    pub(crate) fn new(som: bool, eom: bool, seid: u8, deid: u8, seq: u8) -> Self {
        Self::from_raw(0)
            .with_const_version::<1>()
            .with_som(som)
            .with_eom(eom)
            .with_seid(seid)
            .with_deid(deid)
            .try_with_seq(seq).unwrap_or(Self::from_raw(0))
    }
}

// NVDM over MCTP message header bit layout:
//
//  31:24      23:8        7     6:0
// [NVDM_TYPE | VENDOR_ID | IC | MSG_TYPE]
//
// IC (Instance ID) is always zero in current usage.
bitfield! {
    pub(crate) struct NvdmHeader(u32) {
        31:24 nvdm_type => u8;
        23:8  vendor_id => u16;
        6:0   msg_type;
    }
}

/// MCTP message type for vendor-defined PCI messages (DMTF DSP0236).
pub(crate) const MSG_TYPE_VENDOR_PCI: u8 = 0x7e;

/// NVIDIA PCI Vendor ID.
pub(crate) const VENDOR_ID_NV: u16 = 0x10de;

impl NvdmHeader {
    /// Create a new NVDM header for NVIDIA vendor-defined PCI messages.
    pub(crate) fn new(nvdm_type: u8) -> Self {
        Self::from_raw(0)
            .with_const_msg_type::<0x7e>()
            .with_vendor_id(VENDOR_ID_NV)
            .with_nvdm_type(nvdm_type)
    }
}

/// NVDM message type constants.
///
/// These values come from a shared namespace across all GPU subsystems.
/// Open RM reference: `arch/nvalloc/common/inc/nvdm_format.h`
pub(crate) mod nvdm_type {
    /// Chain of Trust (FSP boot).
    pub(crate) const COT: u8 = 0x14;
    /// FSP/SEC2 command response.
    pub(crate) const FSP_RESPONSE: u8 = 0x15;
    /// RM RPC message (GSP command queue).
    pub(crate) const RM_RPC: u8 = 0x25;
    /// GMC API message (GSP command queue).
    pub(crate) const GMCAPI: u8 = 0x26;
}
