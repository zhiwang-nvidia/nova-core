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

use kernel::{
    bitfield,
    num::Bounded,
    prelude::*, //
};

// MCTP transport header bit layout (DMTF DSP0236):
//
//  31  30  29:28  27  26:24  23:16  15:8  7:4   3:0
// [SOM|EOM| SEQ  | TO| TAG | SEID | DEID| RSVD| VER]
//
// TAG, TO, and RSVD are always zero in current usage.
bitfield! {
    /// MCTP transport header (first DWORD).
    pub(crate) struct TransportHeader(u32) {
        /// Start of Message flag.
        31:31 som => bool;
        /// End of Message flag.
        30:30 eom => bool;
        /// Packet sequence number (0-3, wraps modulo 4).
        29:28 seq;
        /// Source Endpoint ID.
        23:16 seid;
        /// Destination Endpoint ID.
        15:8  deid;
        /// Header version (1 for MCTP 1.0).
        3:0   version;
    }
}

impl TransportHeader {
    /// Create a new MCTP transport header.
    ///
    /// TAG and TO fields are always zero (not needed when messages are
    /// single-packet and do not use request/response tag correlation).
    pub(crate) fn new(som: bool, eom: bool, seid: u8, deid: u8, seq: u8) -> Self {
        Self::zeroed()
            .with_const_version::<1>()
            .with_som(som)
            .with_eom(eom)
            .with_seid(Bounded::try_new(u32::from(seid)).expect("seid overflow"))
            .with_deid(Bounded::try_new(u32::from(deid)).expect("deid overflow"))
            .with_seq(Bounded::try_new(u32::from(seq)).expect("seq overflow"))
    }
}

// NVDM over MCTP message header bit layout:
//
//  31:24      23:8        7     6:0
// [NVDM_TYPE | VENDOR_ID | IC | MSG_TYPE]
//
// IC (Instance ID) is always zero in current usage.
bitfield! {
    /// NVDM over MCTP message header (second DWORD).
    pub(crate) struct NvdmHeader(u32) {
        /// NVDM message type (subsystem-specific).
        31:24 nvdm_type;
        /// PCI Vendor ID.
        23:8  vendor_id;
        /// MCTP message type.
        6:0   msg_type;
    }
}

/// MCTP message type for vendor-defined PCI messages (DMTF DSP0236).
pub(crate) const MSG_TYPE_VENDOR_PCI: u32 = 0x7e;

/// NVIDIA PCI Vendor ID.
pub(crate) const VENDOR_ID_NV: u32 = 0x10de;

impl NvdmHeader {
    /// Create a new NVDM header for NVIDIA vendor-defined PCI messages.
    ///
    /// Sets MCTP message type to Vendor Defined PCI (0x7E) and vendor ID to
    /// NVIDIA (0x10DE). The IC (Instance ID) field is always zero.
    pub(crate) fn new(nvdm_type: u8) -> Self {
        Self::zeroed()
            .with_const_msg_type::<{ MSG_TYPE_VENDOR_PCI }>()
            .with_const_vendor_id::<{ VENDOR_ID_NV }>()
            .with_nvdm_type(Bounded::try_new(u32::from(nvdm_type)).expect("nvdm_type overflow"))
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
