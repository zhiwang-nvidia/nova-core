// SPDX-License-Identifier: GPL-2.0

//! fwctl driver for nova-core GMC API pass-through.

use kernel::{
    fwctl,
    fwctl::{
        DeviceType,
        FwRpcResponse,
        RpcScope, //
    },
    prelude::*, //
    sync::Arc,  //
};

use crate::{
    driver::Bar0,
    gsp::cmdq::Cmdq,
    vgpu::consts::gmc, //
};

/// Resources used by fwctl callbacks while the GPU is bound.
#[pin_data]
pub(crate) struct NovaCoreFwCtlData<'a> {
    bar: Bar0<'a>,
    cmdq: Arc<Cmdq>,
}

impl<'a> NovaCoreFwCtlData<'a> {
    pub(crate) fn new(bar: Bar0<'a>, cmdq: Arc<Cmdq>) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { bar, cmdq })
    }
}

/// Per-file fwctl context for nova-core.
#[pin_data]
pub(crate) struct NovaCoreFwCtl {}

impl fwctl::Operations for NovaCoreFwCtl {
    type RegistrationData<'a> = NovaCoreFwCtlData<'a>;

    const DEVICE_TYPE: DeviceType = DeviceType::NovaCore;

    fn open<'a>(
        _device: &fwctl::Device<Self>,
        _reg_data: &Self::RegistrationData<'a>,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {})
    }

    fn fw_rpc<'a>(
        _this: Pin<&Self>,
        _device: &fwctl::Device<Self>,
        reg_data: &Self::RegistrationData<'a>,
        scope: RpcScope,
        rpc_buf: &mut [u8],
        _max_output_len: usize,
    ) -> Result<FwRpcResponse, Error> {
        if rpc_buf.len() < RPC_HEADER_SIZE {
            return Err(EINVAL);
        }

        let (header, payload) = rpc_buf.split_at(RPC_HEADER_SIZE);
        let command_id = u32::from_le_bytes(header[..4].try_into().map_err(|_| EINVAL)?);
        let reserved = u32::from_le_bytes(header[4..].try_into().map_err(|_| EINVAL)?);

        if reserved != 0 {
            return Err(EINVAL);
        }
        if !is_command_permitted(scope, command_id) {
            return Err(EPERM);
        }

        let response = reg_data.cmdq.send_gmc_and_receive(
            reg_data.bar,
            command_id,
            payload,
            GSP_GMC_MAX_RESPONSE_SIZE,
        )?;
        if response.status != 0 {
            return Err(EIO);
        }

        let response_len = RPC_HEADER_SIZE
            .checked_add(response.payload.len())
            .ok_or(EOVERFLOW)?;
        let mut out = KVec::with_capacity(response_len, GFP_KERNEL)?;
        out.extend_from_slice(&command_id.to_le_bytes(), GFP_KERNEL)?;
        out.extend_from_slice(&0u32.to_le_bytes(), GFP_KERNEL)?;
        out.extend_from_slice(&response.payload, GFP_KERNEL)?;

        Ok(FwRpcResponse::NewBuffer(out))
    }
}

/// Size of `fwctl_rpc_nova_core` (`command_id` plus `reserved`).
const RPC_HEADER_SIZE: usize = 8;

const GSP_GMC_MAX_RESPONSE_SIZE: u32 = 16_384;

/// Returns whether `command_id` may be issued under `scope`.
fn is_command_permitted(scope: RpcScope, command_id: u32) -> bool {
    match scope {
        RpcScope::Configuration => matches!(
            command_id,
            gmc::VGPU_MGMT_ADD_TYPE
                | gmc::VGPU_MGMT_QUERY_SUPPORTED
                | gmc::VGPU_MGMT_QUERY_CREATABLE
                | gmc::VGPU_MGMT_ASSIGN_TYPE
                | gmc::VGPU_MGMT_DEASSIGN_TYPE
                | gmc::VGPU_MGMT_QUERY_PROPERTIES
                | gmc::VGPU_MGMT_QUERY_ASSIGNED_VF
        ),
        _ => false,
    }
}
