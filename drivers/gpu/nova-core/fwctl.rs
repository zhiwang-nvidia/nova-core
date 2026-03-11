// SPDX-License-Identifier: GPL-2.0

use kernel::{
    fwctl::{
        self,
        DeviceType,
        FwRpcResponse,
        Operations,
        RpcScope, //
    },
    prelude::*,
    transmute::{AsBytes, FromBytes},
    uapi, //
};

use crate::{
    driver::NovaCore,
    gsp::{
        RmControlMsgFunction,
        rm::commands::send_rm_control, //
    },
};

/// Byte-serializable wrapper for [`uapi::fwctl_rpc_nova_core_request_hdr`].
#[repr(transparent)]
struct FwctlNovaCoreReqHdr(uapi::fwctl_rpc_nova_core_request_hdr);

// SAFETY: All fields are plain `__u32` with no padding.
unsafe impl FromBytes for FwctlNovaCoreReqHdr {}

/// Byte-serializable wrapper for [`uapi::fwctl_rpc_nova_core_resp_hdr`].
#[repr(transparent)]
struct FwctlNovaCoreRespHdr(uapi::fwctl_rpc_nova_core_resp_hdr);

// SAFETY: All fields are plain `__u32` with no padding.
unsafe impl AsBytes for FwctlNovaCoreRespHdr {}

/// Per-FD fwctl user context and operations for nova-core.
pub(crate) struct NovaCoreFwCtl;

impl Operations for NovaCoreFwCtl {
    type DeviceData = ();

    const DEVICE_TYPE: DeviceType = DeviceType::NovaCore;

    fn open(_device: &fwctl::Device<Self>) -> Result<impl PinInit<Self, Error>, Error> {
        Ok(Ok(NovaCoreFwCtl))
    }

    fn fw_rpc(
        _this: &Self,
        device: &fwctl::Device<Self>,
        scope: RpcScope,
        rpc_in: &mut [u8],
    ) -> Result<FwRpcResponse, Error> {
        let hdr_size = size_of::<FwctlNovaCoreReqHdr>();

        if rpc_in.len() < hdr_size {
            return Err(EINVAL);
        }

        if scope != RpcScope::Configuration {
            return Err(EPERM);
        }

        let (hdr, _) = FwctlNovaCoreReqHdr::from_bytes_prefix(rpc_in).ok_or(EINVAL)?;
        let cmd = hdr.0.cmd;

        let rm_cmd = match cmd {
            uapi::fwctl_cmd_nova_core_FWCTL_CMD_NOVA_CORE_UPLOAD_VGPU_TYPE => {
                RmControlMsgFunction::VgpuMgrInternalPgpuAddVgpuType
            }
            _ => return Err(EINVAL),
        };

        let parent = device.parent();
        let data = parent.drvdata::<NovaCore>()?;
        let bar = data.gpu.bar.as_ref().access(parent)?;

        let params = &rpc_in[hdr_size..];
        let reply_params = send_rm_control(
            &data.gpu.gsp.cmdq,
            bar,
            data.gpu.gsp.h_client(),
            data.gpu.gsp.h_subdevice(),
            rm_cmd,
            params,
        )?;

        let resp_hdr = FwctlNovaCoreRespHdr(uapi::fwctl_rpc_nova_core_resp_hdr {
            mctp_header: 0,
            nvdm_header: 0,
        });
        let mut out = KVec::new();
        out.extend_from_slice(resp_hdr.as_bytes(), GFP_KERNEL)?;
        out.extend_from_slice(&reply_params, GFP_KERNEL)?;
        Ok(FwRpcResponse::NewBuffer(out))
    }
}
