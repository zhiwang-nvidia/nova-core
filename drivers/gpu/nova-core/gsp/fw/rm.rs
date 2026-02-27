// SPDX-License-Identifier: GPL-2.0

use kernel::{
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use super::{
    r570_144 as r570,
    NvStatus, //
};

/// Command code for RM control RPCs sent using [`MsgFunction::GspRmControl`].
#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub(crate) enum RmControlMsgFunction {
    /// Get the CE fault method buffer size.
    CeGetFaultMethodBufferSize = r570::NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE,
}

impl TryFrom<u32> for RmControlMsgFunction {
    type Error = kernel::error::Error;

    fn try_from(value: u32) -> Result<Self> {
        match value {
            r570::NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE => {
                Ok(Self::CeGetFaultMethodBufferSize)
            }
            _ => Err(EINVAL),
        }
    }
}

impl From<RmControlMsgFunction> for u32 {
    fn from(value: RmControlMsgFunction) -> Self {
        // CAST: `RmControlMsgFunction` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

/// RM control message element structure.
#[allow(dead_code)] // Public API for RM control RPCs.
#[derive(Zeroable)]
#[repr(transparent)]
pub(crate) struct GspRmControl {
    inner: r570::rpc_gsp_rm_control_v03_00,
}

#[allow(dead_code)] // new/status are public API for RM control RPCs.
impl GspRmControl {
    /// Creates a new RM control command.
    pub(crate) fn new(
        h_client: u32,
        h_object: u32,
        cmd: RmControlMsgFunction,
        params_size: u32,
    ) -> Self {
        Self {
            inner: r570::rpc_gsp_rm_control_v03_00 {
                hClient: h_client,
                hObject: h_object,
                cmd: u32::from(cmd),
                status: 0,
                paramsSize: params_size,
                flags: 0,
                params: Default::default(),
            },
        }
    }

    /// Returns the status from the RM control response.
    pub(crate) fn status(&self) -> NvStatus {
        NvStatus::from(self.inner.status)
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for GspRmControl {}

// SAFETY: This struct contains no padding.
unsafe impl AsBytes for GspRmControl {}

/// Wrapper for [`r570::NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS`].
#[derive(Zeroable)]
#[repr(transparent)]
pub(crate) struct CeGetFaultMethodBufferSizeParams(
    r570::NV2080_CTRL_CE_GET_FAULT_METHOD_BUFFER_SIZE_PARAMS,
);

impl CeGetFaultMethodBufferSizeParams {
    /// Returns the CE fault method buffer size in bytes.
    pub(crate) fn size(&self) -> u32 {
        self.0.size
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for CeGetFaultMethodBufferSizeParams {}
