// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::{
    ffi::FromBytesUntilNulError,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    pci,
    prelude::*,
    transmute::AsBytes, //
};

use crate::{
    gsp::{
        cmdq::{
            Cmdq,
            QueuePointers, //
        },
        fw::{
            self,
            commands::{
                GspInitRequest,
                GspInitResponse,
                GspInitResponseSchema,
                RegKey,
                VfInfo,
                MAX_FIFO_ENGINES, //
            },
            GMCAPI_CMD_GSP_INIT,
            GMCAPI_CMD_GSP_SUSPEND, //
        },
        nvkv::{
            nvkv_words,
            Decoder,
            Encodable,
            EncodedStream,
            Encoder,
            UnknownKeyPolicy, //
        },
        GspBootContext,
    },
    vgpu::VgpuState, //
};

/// Bit mask for `NVGMC_SC_ENGINE_FLAGS_IS_HOST_DRIVEN`.
const ENGINE_FLAGS_IS_HOST_DRIVEN: u32 = 1 << 0;

/// Host-driven GMC engine IDs in hardware FIFO order, including any repeated IDs.
///
/// # Invariants
///
/// `count` is at most [`MAX_FIFO_ENGINES`]. The first `count` slots are the retained engine IDs.
#[derive(Copy, Clone)]
pub(crate) struct FifoEngineList {
    gmc_ids: [u32; MAX_FIFO_ENGINES],
    count: usize,
}

impl FifoEngineList {
    pub(crate) fn gmc_ids(&self) -> &[u32] {
        // PANIC: The type invariant bounds `count` by the array capacity.
        &self.gmc_ids[..self.count]
    }
}

/// The static GPU configuration, as decoded from the `GSP_INIT` reply.
pub(crate) struct GetGspStaticInfoReply {
    gpu_name: [u8; 64],
    /// BAR1 Page Directory Entry base address.
    pub(crate) bar1_pde_base: u64,
    /// Usable FB (VRAM) regions for driver memory allocation.
    pub(crate) usable_fb_regions: KVec<Range<u64>>,
    /// Exclusive end of the FB physical address space.
    pub(crate) total_fb_end: u64,
    /// VMMU segment size in bytes, or zero if GSP-RM omitted it.
    pub(crate) vmmu_segment_size: u64,
    pub(crate) fifo_engine_list: FifoEngineList,
}

/// Error type for [`GetGspStaticInfoReply::gpu_name`].
#[derive(Debug)]
pub(crate) enum GpuNameError {
    /// The GPU name string does not contain a null terminator.
    NoNullTerminator(FromBytesUntilNulError),

    /// The GPU name string contains invalid UTF-8.
    #[expect(dead_code)]
    InvalidUtf8(Utf8Error),
}

impl GetGspStaticInfoReply {
    /// Returns the name of the GPU as a string.
    ///
    /// Returns an error if the string given by the GSP does not contain a null terminator or
    /// contains invalid UTF-8.
    pub(crate) fn gpu_name(&self) -> core::result::Result<&str, GpuNameError> {
        CStr::from_bytes_until_nul(&self.gpu_name)
            .map_err(GpuNameError::NoNullTerminator)?
            .to_str()
            .map_err(GpuNameError::InvalidUtf8)
    }
}

/// Registry entries the driver sends to GSP-RM on every boot.
///
/// `RMSecBusResetEnable` enables PCI secondary bus reset. `RMForcePcieConfigSave` makes GSP-RM
/// preserve PCI configuration registers across any PCI reset. `RMDevidCheckIgnore` lets GSP-RM
/// boot when the PCI device id is absent from its product name database.
const REGISTRY_ENTRIES: &[(&[u8], u32)] = &[
    (b"RMSecBusResetEnable\0", 1),
    (b"RMForcePcieConfigSave\0", 1),
    (b"RMDevidCheckIgnore\0", 1),
];

/// Builds the NVKV-encoded payload of a `GSP_INIT` request.
///
/// The payload carries the system information GSP-RM reads before it starts, and
/// [`REGISTRY_ENTRIES`] as `REGKEY_NAME` and `REGKEY_VALUE_U32` pairs. GSP-RM requires each name
/// to be followed by its value, which is the order [`RegKey`] declares them in.
///
/// # Errors
///
/// - `ENOMEM` if the registry list or the encoder buffer cannot be allocated.
/// - `ENODEV` if vGPU mode is enabled but the SR-IOV capability is missing.
///
/// Errors reading the PCI configuration or decoding the VF BAR layout are propagated as-is.
pub(super) fn build_gsp_init_payload(ctx: &GspBootContext<'_, '_>) -> Result<EncodedStream> {
    let mut regkeys = KVVec::new();
    for &(name, value) in REGISTRY_ENTRIES {
        regkeys.push(RegKey::new(name, value), GFP_KERNEL)?;
    }
    if matches!(*ctx.vgpu_state, VgpuState::Enabled { .. }) {
        regkeys.push(RegKey::new(b"RMSetSriovMode\0", 1), GFP_KERNEL)?;
    }

    let vf_info = build_vf_info(ctx)?;

    let mut encoder = Encoder::new();
    GspInitRequest::new(ctx.pdev, ctx.chipset, regkeys, vf_info).encode(&mut encoder)?;

    Ok(encoder.finish())
}

/// Builds the optional VF topology portion of the `GSP_INIT` request.
fn build_vf_info(ctx: &GspBootContext<'_, '_>) -> Result<Option<VfInfo>> {
    let VgpuState::Enabled { total_vfs } = *ctx.vgpu_state else {
        return Ok(None);
    };

    let sriov = ctx
        .pdev
        .config_space_extended()?
        .find_ext_capability::<pci::ExtSriovRegs>()?
        .ok_or(ENODEV)?;

    let mut vf_bars = sriov.vf_bars()?;
    let bar0 = vf_bars.next().ok_or(EINVAL)?;
    let bar1 = vf_bars.next().ok_or(EINVAL)?;
    let bar2 = vf_bars.next().ok_or(EINVAL)?;

    let flags = u64::from(bar0.is_64bit)
        | (u64::from(bar1.is_64bit) << 1)
        | (u64::from(bar2.is_64bit) << 2);

    Ok(Some(VfInfo::new(
        u32::from(total_vfs.get()),
        u32::from(sriov.first_vf_offset()),
        flags,
        bar0.address,
        bar1.address,
        bar2.address,
    )))
}

/// Size of the buffer GSP-RM may fill with static configuration, matching the allocation Open RM
/// makes in `kgspSendInitRpcs`.
const GSP_INIT_MAX_RESPONSE_SIZE: u32 = 48 * 1024;

/// Sends `GSP_INIT` and returns the static configuration its reply carries.
///
/// GSP-RM raises load-and-execute events between the request and the reply, and it cannot finish
/// starting until the driver has serviced them, so each one goes to `on_boot_event`, which
/// reports back the [`QueuePointers`] state its handler left. GSP-RM sends the reply once it is
/// up, so the reply doubles as the signal that boot is complete.
///
/// `payload` is the blob from [`build_gsp_init_payload`].
///
/// # Errors
///
/// - `EIO` if GSP-RM reports a failure status, or if the reply is not a whole number of NVKV
///   words.
/// - `ETIMEDOUT` if neither the reply nor another element arrives within
///   [`Cmdq::RECEIVE_TIMEOUT`].
///
/// Errors from `on_boot_event` and from decoding the reply are propagated as-is.
pub(crate) fn gsp_init(
    cmdq: &Cmdq<'_>,
    payload: &[u64],
    mut on_boot_event: impl FnMut(u32, &[u8]) -> Result<QueuePointers>,
) -> Result<GetGspStaticInfoReply> {
    // Qualified because `zerocopy::IntoBytes` also gives `[T]` an `as_bytes`.
    let payload = AsBytes::as_bytes(payload);

    let sequence =
        cmdq.send_gmc_no_wait(GMCAPI_CMD_GSP_INIT, payload, GSP_INIT_MAX_RESPONSE_SIZE)?;

    loop {
        let reply = match cmdq.receive_gmc_and_dispatch(
            Cmdq::RECEIVE_TIMEOUT,
            |header, payload_0, payload_1| {
                if header.is_response_to(GMCAPI_CMD_GSP_INIT, sequence) {
                    (
                        Some(decode_gsp_init_reply(
                            header.gmc.max_resp_or_status,
                            payload_0,
                            payload_1,
                        )),
                        QueuePointers::Unchanged,
                    )
                } else {
                    // A boot event. Keep waiting for the reply unless handling it failed.
                    match on_boot_event(header.gmc.command_id(), payload_0) {
                        Ok(queue_pointers) => (None, queue_pointers),
                        // A handler can fail after it has already reset the GSP, so the pointer
                        // registers cannot be assumed intact on this path.
                        Err(e) => (Some(Err(e)), QueuePointers::Reset),
                    }
                }
            },
        ) {
            Ok(reply) => reply,
            Err(ERANGE) => continue,
            Err(error) => return Err(error),
        };

        if let Some(reply) = reply {
            return reply;
        }
    }
}

/// Decodes the `GSP_INIT` reply, whose `max_resp_or_status` field carries an `NV_STATUS`.
fn decode_gsp_init_reply(
    status: u32,
    payload_0: &[u8],
    payload_1: &[u8],
) -> Result<GetGspStaticInfoReply> {
    if status != 0 {
        return Err(EIO);
    }

    decode_gsp_info(&nvkv_words(payload_0, payload_1)?)
}

/// Decodes the static GPU configuration from an NVKV stream.
///
/// # Errors
///
/// - `EINVAL` if the stream is malformed or omits a required key.
/// - `ENOMEM` if the decoded regions cannot be allocated.
fn decode_gsp_info(words: &[u64]) -> Result<GetGspStaticInfoReply> {
    let decoder = Decoder::new(words, UnknownKeyPolicy::Ignore);
    let mut schema = GspInitResponseSchema::default();
    let decoded = KBox::try_init(decoder.decode(&mut schema)?, GFP_KERNEL)?;

    let mut gpu_name = [0u8; GspInitResponse::MAX_GPU_NAME_LEN];
    let name = decoded.gpu_name();
    gpu_name
        .get_mut(..name.len())
        .ok_or(EINVAL)?
        .copy_from_slice(name);

    let mut usable_fb_regions = KVec::new();
    for region in decoded.usable_fb_regions() {
        usable_fb_regions.push(region, GFP_KERNEL)?;
    }

    // INVARIANT: The list starts empty and appends at most one ID per supported input slot.
    let mut fifo_engine_list = FifoEngineList {
        gmc_ids: [0; MAX_FIFO_ENGINES],
        count: 0,
    };
    for (&gmc_id, &flags) in decoded
        .fifo_engine_gmc_ids()
        .iter()
        .zip(decoded.fifo_engine_flags())
        .take(decoded.fifo_engine_count())
    {
        if flags & ENGINE_FLAGS_IS_HOST_DRIVEN != 0 {
            // PANIC: At most one slot is filled per input, and the input has at most
            // MAX_FIFO_ENGINES entries, so the next retained ID always fits.
            fifo_engine_list.gmc_ids[fifo_engine_list.count] = gmc_id;
            fifo_engine_list.count += 1;
        }
    }

    Ok(GetGspStaticInfoReply {
        gpu_name,
        bar1_pde_base: decoded.bar1_pde_base(),
        usable_fb_regions,
        total_fb_end: decoded.total_fb_end().ok_or(EINVAL)?,
        vmmu_segment_size: decoded.vmmu_segment_size(),
        fifo_engine_list,
    })
}

pub(crate) use fw::commands::PowerStateLevel;

/// Tells GSP-RM to suspend.
///
/// GSP-RM sends no response to this command and reports the completed suspend through the GSP
/// falcon's `MAILBOX0`, so the caller polls that register rather than waiting on the queue. The
/// request carries no response buffer, hence a maximum response size of zero.
///
/// # Errors
///
/// - `EMSGSIZE` if the command exceeds the maximum queue element size.
/// - `ETIMEDOUT` if space does not become available within the timeout.
/// - `EIO` if the command header is not properly aligned.
pub(crate) fn gsp_suspend(cmdq: &Cmdq<'_>, level: PowerStateLevel) -> Result {
    let params = fw::commands::GspSuspend::new(level);

    cmdq.send_gmc_no_wait(GMCAPI_CMD_GSP_SUSPEND, AsBytes::as_bytes(&params), 0)
        .map(|_| ())
}
