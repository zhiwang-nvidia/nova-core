// SPDX-License-Identifier: GPL-2.0

//! fwctl driver for nova-core GMC API pass-through.

use kernel::{
    fwctl,
    fwctl::{
        DeviceType,
        FwRpcResponse,
        RpcScope, //
    },
    prelude::*,
    sync::atomic::{
        Acquire,
        Atomic,
        AtomicFlag,
        Release, //
    },
    transmute::AsBytes, //
};

use crate::{
    gsp::{
        cmdq::Cmdq,
        nvkv::{
            Decoder,
            DecoderValue,
            Encoder,
            Index,
            KeyId,
            Schema,
            UnknownKeyPolicy, //
        }, //
    },
    vgpu::{
        GMCAPI_CMD_ADD_VGPU_TYPE,
        GMCAPI_CMD_ASSIGN_VGPU_TYPE,
        GMCAPI_CMD_DEASSIGN_VGPU_TYPE,
        GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE,
        GMCAPI_CMD_QUERY_CREATABLE_VGPU_TYPES,
        GMCAPI_CMD_QUERY_SUPPORTED_VGPU_TYPES,
        GMCAPI_CMD_QUERY_VGPU_PROPERTIES, //
    },
};

const GMCAPI_CMD_UMEM_CREATE: u32 = 0x0004_0002;
const GMCAPI_CMD_UMEM_DESTROY: u32 = 0x0004_0003;
const GMCAPI_CMD_RUSD_INIT: u32 = 0x0006_0001;
const GMCAPI_CMD_RUSD_BIND: u32 = 0x0006_0002;
const GMCAPI_CMD_RUSD_START: u32 = 0x0006_0003;
const GMCAPI_CMD_RUSD_STOP: u32 = 0x0006_0004;
const GMCAPI_CMD_RUSD_UNBIND: u32 = 0x0006_0005;
const GMCAPI_CMD_RUSD_STATS: u32 = 0x0006_0006;
const GMCAPI_CMD_RUSD_SHUTDOWN: u32 = 0x0006_0007;
const RUSD_VERSION: u32 = 2;
const RUSD_VERSION_KEY: KeyId = 0x0001;

// UMEM control keys from firmware `gmcapi_rusd.h`.
const UMEM_VERSION: u32 = 1;
const UMEM_VERSION_KEY: KeyId = 0x0001;
const UMEM_FLAGS_KEY: KeyId = 0x0002;
const UMEM_ID_KEY: KeyId = 0x0003;
const UMEM_DMA_ADDRESS_KEY: KeyId = 0x1000;
const UMEM_LENGTH_KEY: KeyId = 0x1001;
const UMEM_REPLY_BYTES: u32 = 16;

#[derive(Default)]
struct UmemReply {
    version: Option<u32>,
    id: Option<u32>,
}

impl Schema for UmemReply {
    type Target = u32;

    fn visit(&mut self, key: KeyId, index: Index, value: DecoderValue<'_>) -> Result<bool> {
        let slot = match key {
            UMEM_VERSION_KEY => &mut self.version,
            UMEM_ID_KEY => &mut self.id,
            _ => return Ok(false),
        };
        if index != Index::new::<0>() || slot.is_some() {
            return Err(EPROTO);
        }
        *slot = Some(u32::try_from(value).map_err(|_| EPROTO)?);
        Ok(true)
    }

    fn finish(&mut self) -> impl Init<Self::Target, Error> + '_ {
        if self.version.take() != Some(UMEM_VERSION) {
            return Err(EPROTO);
        }
        self.id.take().filter(|&id| id != 0).ok_or(EPROTO)
    }
}

/// Sends an internal UMEM command and decodes its version/device-ID acknowledgement.
fn umem_command(cmdq: &Cmdq<'_>, command: u32, encoder: Encoder) -> Result<u32> {
    let request = encoder.finish();
    let response =
        cmdq.send_gmc_and_receive(command, AsBytes::as_bytes(&*request), UMEM_REPLY_BYTES)?;
    if response.status != 0 {
        return Err(EIO);
    }
    if response.payload.len() != 16 {
        return Err(EPROTO);
    }
    // Decode the fixed, small reply without another allocation after CREATE.
    let words = [
        u64::from_le_bytes(response.payload[..8].try_into().map_err(|_| EPROTO)?),
        u64::from_le_bytes(response.payload[8..].try_into().map_err(|_| EPROTO)?),
    ];
    let decoder = Decoder::new(&words, UnknownKeyPolicy::Error);
    let mut schema = UmemReply::default();
    pin_init::stack_try_pin_init!(let id: u32 =? decoder.decode(&mut schema).map_err(|_| EPROTO)?);
    Ok(*id)
}

fn rusd_shutdown(cmdq: &Cmdq<'_>) -> Result {
    let mut encoder = Encoder::new();
    encoder.encode_u32(RUSD_VERSION_KEY, Index::new::<0>(), RUSD_VERSION)?;
    let request = encoder.finish();
    let payload = AsBytes::as_bytes(&*request);
    let response = cmdq.send_gmc_and_receive(GMCAPI_CMD_RUSD_SHUTDOWN, payload, 8)?;
    if response.status != 0 {
        return Err(EIO);
    }
    if response.payload.as_slice() != payload {
        return Err(EPROTO);
    }
    Ok(())
}

/// Resources used by fwctl callbacks while the GPU is bound.
#[pin_data]
pub(crate) struct NovaCoreFwCtlData<'a> {
    cmdq: &'a Cmdq<'a>,
    rusd_in_use: AtomicFlag,
}

impl<'a> NovaCoreFwCtlData<'a> {
    pub(crate) fn new(cmdq: &'a Cmdq<'a>) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            cmdq,
            rusd_in_use: AtomicFlag::new(false),
        })
    }
}

/// Per-file fwctl context for nova-core.
#[pin_data]
pub(crate) struct NovaCoreFwCtl {
    owns_rusd: Atomic<bool>,
}

impl NovaCoreFwCtl {
    fn claim_rusd(&self, reg_data: &NovaCoreFwCtlData<'_>) -> Result {
        if self.owns_rusd.load(Acquire) {
            return Ok(());
        }
        reg_data
            .rusd_in_use
            .cmpxchg(false, true, Acquire)
            .map_err(|_| EBUSY)?;
        self.owns_rusd.store(true, Release);
        Ok(())
    }
}

// SAFETY: Serial POC use registers a validated range and requires DESTROY acknowledgement.
// FIXME: Lost CREATE replies and failed core close still violate the UMEM safety contract.
#[vtable]
unsafe impl fwctl::Operations for NovaCoreFwCtl {
    type RegistrationData<'a> = NovaCoreFwCtlData<'a>;
    type UmemData = u32;

    const DEVICE_TYPE: DeviceType = DeviceType::NovaCore;

    fn open<'a>(
        _device: &fwctl::Device<Self>,
        _reg_data: &Self::RegistrationData<'a>,
    ) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            owns_rusd: Atomic::new(false),
        })
    }

    fn close<'a>(
        this: Pin<&mut Self>,
        device: &fwctl::Device<Self>,
        reg_data: &Self::RegistrationData<'a>,
    ) {
        if !this.owns_rusd.load(Acquire) {
            return;
        }
        match rusd_shutdown(reg_data.cmdq) {
            Ok(()) => reg_data.rusd_in_use.store(false, Release),
            Err(err) => dev_err!(device, "RUSD shutdown failed: {:?}\n", err),
        }
    }

    unsafe fn umem_reg<'a>(
        this: Pin<&Self>,
        _device: &fwctl::Device<Self>,
        reg_data: &Self::RegistrationData<'a>,
        request: &fwctl::UmemRegistration<'_>,
    ) -> Result<fwctl::Umem<Self::UmemData>> {
        if request.flags != 0 {
            return Err(EOPNOTSUPP);
        }
        if request.offset != 0 || request.len == 0 || request.len % kernel::page::PAGE_SIZE != 0 {
            return Err(EINVAL);
        }
        let mut segments = request.sgt.iter();
        let segment = segments.next().ok_or(EINVAL)?;
        if request.original_nents != 1 || segments.next().is_some() {
            return Err(EOPNOTSUPP);
        }
        let address = segment.dma_address();
        let length = u64::try_from(request.len).map_err(|_| EOVERFLOW)?;
        if address == 0
            || address % 4096 != 0
            || segment.dma_len() < length
            || address.checked_add(length).is_none()
        {
            return Err(EINVAL);
        }

        let mut encoder = Encoder::new();
        encoder.encode_u32(UMEM_VERSION_KEY, Index::new::<0>(), UMEM_VERSION)?;
        encoder.encode_u32(UMEM_FLAGS_KEY, Index::new::<0>(), 0)?;
        encoder.encode_u64(UMEM_DMA_ADDRESS_KEY, Index::new::<0>(), address)?;
        encoder.encode_u64(UMEM_LENGTH_KEY, Index::new::<0>(), length)?;
        this.claim_rusd(reg_data)?;
        let id = umem_command(reg_data.cmdq, GMCAPI_CMD_UMEM_CREATE, encoder)?;
        Ok(fwctl::Umem {
            data: id,
            umem_id: id,
            mkey_id: 0,
        })
    }

    fn umem_unreg<'a>(
        _this: Pin<&Self>,
        _device: &fwctl::Device<Self>,
        reg_data: &Self::RegistrationData<'a>,
        id: &mut Self::UmemData,
    ) -> Result {
        let mut encoder = Encoder::new();
        encoder.encode_u32(UMEM_VERSION_KEY, Index::new::<0>(), UMEM_VERSION)?;
        encoder.encode_u32(UMEM_ID_KEY, Index::new::<0>(), *id)?;
        let destroyed = umem_command(reg_data.cmdq, GMCAPI_CMD_UMEM_DESTROY, encoder)?;
        if destroyed != *id {
            return Err(EPROTO);
        }
        Ok(())
    }

    fn fw_rpc<'a>(
        this: Pin<&Self>,
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
        if matches!(
            command_id,
            GMCAPI_CMD_RUSD_INIT
                | GMCAPI_CMD_RUSD_BIND
                | GMCAPI_CMD_RUSD_START
                | GMCAPI_CMD_RUSD_STOP
                | GMCAPI_CMD_RUSD_UNBIND
                | GMCAPI_CMD_RUSD_SHUTDOWN
        ) {
            this.claim_rusd(reg_data)?;
        }

        let response =
            reg_data
                .cmdq
                .send_gmc_and_receive(command_id, payload, GSP_GMC_MAX_RESPONSE_SIZE)?;
        if response.status != 0 {
            return Err(EIO);
        }

        let response_len = RPC_HEADER_SIZE
            .checked_add(response.payload.len())
            .ok_or(EOVERFLOW)?;
        let mut out = KVVec::with_capacity(response_len, GFP_KERNEL)?;
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
            GMCAPI_CMD_ADD_VGPU_TYPE
                | GMCAPI_CMD_QUERY_SUPPORTED_VGPU_TYPES
                | GMCAPI_CMD_QUERY_CREATABLE_VGPU_TYPES
                | GMCAPI_CMD_ASSIGN_VGPU_TYPE
                | GMCAPI_CMD_DEASSIGN_VGPU_TYPE
                | GMCAPI_CMD_QUERY_VGPU_PROPERTIES
                | GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE
                | GMCAPI_CMD_RUSD_INIT
                | GMCAPI_CMD_RUSD_BIND
                | GMCAPI_CMD_RUSD_START
                | GMCAPI_CMD_RUSD_STOP
                | GMCAPI_CMD_RUSD_UNBIND
                | GMCAPI_CMD_RUSD_STATS
                | GMCAPI_CMD_RUSD_SHUTDOWN
        ),
        RpcScope::DebugReadOnly => command_id == GMCAPI_CMD_RUSD_STATS,
        _ => false,
    }
}
