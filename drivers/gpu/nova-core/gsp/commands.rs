// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::{
    array,
    convert::Infallible,
    ffi::FromBytesUntilNulError,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    device,
    pci,
    prelude::*,
    time::{
        Instant,
        Monotonic, //
    },
    transmute::AsBytes, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::{
            Cmdq,
            CommandToGsp,
            MessageFromGsp,
            QueuePointers, //
        },
        fw::{
            self,
            commands::{
                GspInitRequest,
                GspInitResponse,
                GspInitResponseSchema,
                RegKey,
                VfInfo, //
            },
            MsgFunction,
            GMCAPI_CMD_GSP_INIT, //
        },
        nvkv::{
            Decoder,
            Encodeable,
            Encoder,
            UnknownKeyPolicy, //
        },
    },
    sbuffer::SBufferIter,
    vgpu::VgpuState, //
};

pub(crate) use fw::commands::{
    encode_vgpu_bootload,
    ChannelMapEntry,
    Dbdf,
    VgpuProperties, //
};

/// Upper bound on entries in the hardware FIFO engine table.
pub(crate) const MAX_FIFO_ENGINES: usize = 64;

/// Bit mask for `NVGMC_SC_ENGINE_FLAGS_IS_HOST_DRIVEN` (bits 0:0).
const ENGINE_FLAGS_IS_HOST_DRIVEN: u32 = 1 << 0;

/// Ordered list of host-driven GMC engine IDs from the hardware FIFO engine table.
#[derive(Copy, Clone)]
pub(crate) struct FifoEngineList {
    pub(crate) gmc_ids: [u32; MAX_FIFO_ENGINES],
    pub(crate) count: usize,
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
    /// VMMU segment size reported by GSP-RM, in bytes.
    pub(crate) vmmu_segment_size: u64,
    /// Ordered host-driven FIFO engine GMC IDs.
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

/// Registry entries the driver sends to GSP-RM on every boot, each with the NULL terminator that
/// Open RM counts in the encoded name length.
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
pub(crate) fn build_gsp_init_payload(
    pdev: &pci::Device<device::Bound>,
    chipset: Chipset,
    vgpu_state: VgpuState,
) -> Result<KVVec<u64>> {
    let mut regkeys = KVVec::new();
    for &(name, value) in REGISTRY_ENTRIES {
        regkeys.push(RegKey::new(name, value), GFP_KERNEL)?;
    }
    if matches!(vgpu_state, VgpuState::Enabled { .. }) {
        regkeys.push(RegKey::new(b"RMSetSriovMode\0", 1), GFP_KERNEL)?;
    }

    let vf_info = build_vf_info(pdev, vgpu_state)?;

    let mut encoder = Encoder::new();
    GspInitRequest::new(pdev, chipset, regkeys, vf_info).encode(&mut encoder)?;

    Ok(encoder.finish())
}

/// Builds the optional VF topology portion of the `GSP_INIT` request.
fn build_vf_info(
    pdev: &pci::Device<device::Bound>,
    vgpu_state: VgpuState,
) -> Result<Option<VfInfo>> {
    let VgpuState::Enabled { total_vfs } = vgpu_state else {
        return Ok(None);
    };

    let sriov = pdev
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
/// GSP-RM interleaves load-and-execute events between the request and the reply, and those events
/// drive the falcon loads that let it finish starting, so each one is passed to `on_boot_event`
/// rather than skipped. `on_boot_event` returns the [`QueuePointers`] state its handler left
/// behind, because a handler that resets the GSP also zeroes the queue's pointer registers. The
/// reply arrives only once GSP-RM is up, which is what makes it the signal that boot is complete.
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
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    payload: &[u64],
    mut on_boot_event: impl FnMut(u32, &[u8]) -> Result<QueuePointers>,
) -> Result<GetGspStaticInfoReply> {
    // Qualified because `zerocopy::IntoBytes` also gives `[T]` an `as_bytes`.
    let payload = AsBytes::as_bytes(payload);

    cmdq.send_gmc_no_wait(
        bar,
        GMCAPI_CMD_GSP_INIT,
        payload,
        GSP_INIT_MAX_RESPONSE_SIZE,
    )?;

    let deadline = Instant::<Monotonic>::now() + Cmdq::RECEIVE_TIMEOUT;
    loop {
        let remaining = deadline - Instant::<Monotonic>::now();
        if remaining.is_negative() {
            return Err(ETIMEDOUT);
        }

        let reply =
            match cmdq.receive_gmc_and_dispatch(bar, remaining, |header, payload_0, payload_1| {
                let command_id = header.command_id();
                if header.is_response() && command_id == GMCAPI_CMD_GSP_INIT {
                    (
                        Some(decode_gsp_init_reply(
                            header.max_resp_or_status,
                            payload_0,
                            payload_1,
                        )),
                        QueuePointers::Unchanged,
                    )
                } else if header.is_response() {
                    (None, QueuePointers::Unchanged)
                } else {
                    // A boot event. Keep waiting for the reply unless handling it failed.
                    match on_boot_event(command_id, payload_0) {
                        Ok(queue_pointers) => (None, queue_pointers),
                        // A handler can fail after it has already reset the GSP, so the pointer
                        // registers cannot be assumed intact on this path.
                        Err(e) => (Some(Err(e)), QueuePointers::Reset),
                    }
                }
            }) {
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

/// Joins the two halves of a wrapped payload into the `u64` words an NVKV stream is made of.
///
/// # Errors
///
/// - `EIO` if the combined length is not a whole number of words.
/// - `ENOMEM` if the buffer cannot be allocated.
fn nvkv_words(payload_0: &[u8], payload_1: &[u8]) -> Result<KVVec<u64>> {
    let bytes = SBufferIter::new_reader([payload_0, payload_1]).flush_into_kvec(GFP_KERNEL)?;
    let words = bytes.chunks_exact(size_of::<u64>());
    if !words.remainder().is_empty() {
        return Err(EIO);
    }

    let mut out = KVVec::with_capacity(bytes.len() / size_of::<u64>(), GFP_KERNEL)?;
    for word in words {
        let word: [u8; size_of::<u64>()] = word.try_into().map_err(|_| EIO)?;
        out.push(u64::from_le_bytes(word), GFP_KERNEL)?;
    }

    Ok(out)
}

/// Decodes a byte-oriented GMC vGPU-properties response with the typed NVKV schema.
pub(crate) fn decode_vgpu_properties(payload: &[u8]) -> Result<KBox<VgpuProperties>> {
    let words = nvkv_words(payload, &[])?;
    VgpuProperties::decode(&words)
}

/// Decodes the static GPU configuration from an NVKV stream.
///
/// # Errors
///
/// - `EINVAL` if the stream is malformed or omits a required key.
/// - `ENOMEM` if the decoded regions cannot be allocated.
fn decode_gsp_info(words: &[u64]) -> Result<GetGspStaticInfoReply> {
    let decoder = Decoder::new(words, UnknownKeyPolicy::Ignore);
    let decoded = KBox::try_init(
        decoder.decode(GspInitResponseSchema::default())?,
        GFP_KERNEL,
    )?;

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
    let total_fb_end = decoded.total_fb_end().ok_or(EINVAL)?;
    let vmmu_segment_size = decoded.vmmu_segment_size();
    let fifo_count = decoded.fifo_engine_count();
    let raw_ids = decoded.fifo_engine_gmc_ids();
    let raw_flags = decoded.fifo_engine_flags();
    let mut fifo_engine_list = FifoEngineList {
        gmc_ids: [0; MAX_FIFO_ENGINES],
        count: 0,
    };
    for index in 0..fifo_count {
        if raw_flags[index] & ENGINE_FLAGS_IS_HOST_DRIVEN != 0 {
            fifo_engine_list.gmc_ids[fifo_engine_list.count] = raw_ids[index];
            fifo_engine_list.count += 1;
        }
    }

    Ok(GetGspStaticInfoReply {
        gpu_name,
        bar1_pde_base: decoded.bar1_pde_base(),
        usable_fb_regions,
        total_fb_end,
        vmmu_segment_size,
        fifo_engine_list,
    })
}

pub(crate) use fw::commands::PowerStateLevel;

/// The `UnloadingGuestDriver` command, used to shut down the GSP.
///
/// Only used within the `gsp` module.
pub(super) struct UnloadingGuestDriver {
    level: PowerStateLevel,
}

impl UnloadingGuestDriver {
    /// Creates a new `UnloadingGuestDriver` command for the given [`PowerStateLevel`].
    pub(super) fn new(level: PowerStateLevel) -> Self {
        Self { level }
    }
}

impl CommandToGsp for UnloadingGuestDriver {
    const FUNCTION: MsgFunction = MsgFunction::UnloadingGuestDriver;
    type Command = fw::commands::UnloadingGuestDriver;
    type Reply = UnloadingGuestDriverReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        fw::commands::UnloadingGuestDriver::new(self.level)
    }
}

/// The reply from the GSP to the [`UnloadingGuestDriver`] command.
pub(super) struct UnloadingGuestDriverReply;

impl MessageFromGsp for UnloadingGuestDriverReply {
    const FUNCTION: MsgFunction = MsgFunction::UnloadingGuestDriver;
    type InitError = Infallible;
    type Message = ();

    fn read(
        _msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(UnloadingGuestDriverReply)
    }
}
