// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::{
    convert::Infallible,
    ffi::FromBytesUntilNulError,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    device,
    pci,
    prelude::*,
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
            NoReply,
            QueuePointers, //
        },
        fw::{
            self,
            commands::{
                GspInitRequest,
                GspInitResponse,
                GspInitResponseSchema,
                RegKey, //
            },
            CommandId,
            CommandInfo, //
        },
        nvkv::{
            Decoder,
            Encodable,
            EncodedStream,
            Encoder,
            UnknownKeyPolicy, //
        },
    },
    sbuffer::SBufferIter,
    vgpu::VgpuState, //
};

/// The static GPU configuration, as decoded from the `GSP_INIT` reply.
pub(crate) struct GetGspStaticInfoReply {
    gpu_name: [u8; 64],
    /// Usable FB (VRAM) regions for driver memory allocation.
    pub(crate) usable_fb_regions: KVec<Range<u64>>,
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

impl MessageFromGsp for GetGspStaticInfoReply {
    type Message = ();
    type InitError = Error;

    fn read(
        _message: &Self::Message,
        payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        decode_gsp_info(&nvkv_words(payload)?)
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
pub(crate) fn build_gsp_init_payload(
    pdev: &pci::Device<device::Bound>,
    chipset: Chipset,
    vgpu_state: VgpuState,
) -> Result<EncodedStream> {
    let mut regkeys = KVVec::new();
    for &(name, value) in REGISTRY_ENTRIES {
        regkeys.push(RegKey::new(name, value), GFP_KERNEL)?;
    }
    if matches!(vgpu_state, VgpuState::Enabled { .. }) {
        regkeys.push(RegKey::new(b"RMSetSriovMode\0", 1), GFP_KERNEL)?;
    }

    let mut encoder = Encoder::new();
    GspInitRequest::new(pdev, chipset, regkeys).encode(&mut encoder)?;

    Ok(encoder.finish())
}

/// Size of the buffer GSP-RM may fill with static configuration, matching the allocation Open RM
/// makes in `kgspSendInitRpcs`.
const GSP_INIT_MAX_RESPONSE_SIZE: u32 = 48 * 1024;

/// Typed `GSP_INIT` command.
struct GspInit<'a> {
    payload: &'a [u64],
}

impl<'a> CommandToGsp for GspInit<'a> {
    const INFO: CommandInfo = CommandInfo::gmc(CommandId::GSP_INIT, GSP_INIT_MAX_RESPONSE_SIZE);
    type Command = ();
    type Reply = GetGspStaticInfoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        <()>::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        size_of_val(self.payload)
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        // Qualified because `zerocopy::IntoBytes` also gives `[T]` an `as_bytes`.
        dst.write_all(AsBytes::as_bytes(self.payload))
    }
}

/// Sends `GSP_INIT` and returns the static configuration its reply carries.
///
/// GSP-RM raises load-and-execute events between the request and the reply, and it cannot finish
/// starting until the driver has serviced them, so each one goes to `on_boot_event`. GSP-RM
/// sends the reply once it is up, so the reply doubles as the signal that boot is complete.
///
/// `payload` is the blob from [`build_gsp_init_payload`].
///
/// # Errors
///
/// - `EIO` if GSP-RM reports a failure status, or if the reply is not a whole number of NVKV
///   words.
/// - `ETIMEDOUT` if the reply does not arrive within [`Cmdq::RECEIVE_TIMEOUT`], however many
///   events arrive while waiting.
///
/// Errors from `on_boot_event` and from decoding the reply are propagated as-is.
pub(crate) fn gsp_init(
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    payload: &[u64],
    mut on_boot_event: impl FnMut(CommandId, &[u8]) -> Result,
) -> Result<GetGspStaticInfoReply> {
    cmdq.send_command_with_events(bar, GspInit { payload }, |command, payload_0, _| {
        on_boot_event(command, payload_0).map(|()| QueuePointers::Reset)
    })
}

/// Joins the two halves of a wrapped payload into the `u64` words an NVKV stream is made of.
///
/// # Errors
///
/// - `EIO` if the combined length is not a whole number of words.
/// - `ENOMEM` if the buffer cannot be allocated.
fn nvkv_words(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<KVVec<u64>> {
    let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
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

    Ok(GetGspStaticInfoReply {
        gpu_name,
        usable_fb_regions,
    })
}

pub(crate) use fw::commands::PowerStateLevel;

/// Typed `GSP_SUSPEND` command.
struct GspSuspend(PowerStateLevel);

impl CommandToGsp for GspSuspend {
    const INFO: CommandInfo = CommandInfo::gmc(CommandId::GSP_SUSPEND, 0);
    type Command = fw::commands::GspSuspend;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init(self.0)
    }
}

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
pub(crate) fn gsp_suspend(cmdq: &Cmdq, bar: Bar0<'_>, level: PowerStateLevel) -> Result {
    cmdq.send_command_no_wait(bar, GspSuspend(level))
}
