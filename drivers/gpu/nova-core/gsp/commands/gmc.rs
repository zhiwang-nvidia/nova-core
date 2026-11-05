// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GMC (GPU Management Controller) API commands.

use core::convert::Infallible;

use kernel::{
    device,
    pci,
    prelude::*,
    transmute::AsBytes, //
};

use crate::{
    gpu::Chipset,
    gsp::{
        cmdq::{
            gmc::{
                GmcCommand,
                GmcReply, //
            },
            NoReply, //
        },
        commands::{
            DisplayAssignStreamRouteReply,
            DisplayCaps,
            DisplayChannelKind,
            DisplayCheckConfigurationReply,
            DisplayConfiguration,
            DisplayCreateChannelReply,
            DisplayDestroyChannelReply,
            DisplayGetPortCapsReply,
            DisplayGetPortStateReply,
            DisplayHdmiSetSinkCapabilitiesReply,
            DisplayHdmiSetTmdsProtocolReply,
            DisplayHwStaticInfo,
            DisplayModesetBeginReply,
            DisplayModesetEndMode,
            DisplayModesetEndReply,
            DisplayReadEdidReply,
            DisplayReleaseStreamRouteReply,
            DisplaySetInstanceMemoryReply,
            DisplayShutdownReply,
            GspStaticInfo, //
        },
        fw::{
            self,
            commands::{
                DisplayAssignStreamRouteRequest,
                DisplayAssignStreamRouteResponseSchema,
                DisplayCheckConfigurationRequest,
                DisplayCheckConfigurationResponseSchema,
                DisplayCreateChannelRequest,
                DisplayCreateChannelResponseSchema,
                DisplayDestroyChannelRequest,
                DisplayGetPortCapsRequest,
                DisplayGetPortCapsResponseSchema,
                DisplayGetPortStateRequest,
                DisplayGetPortStateResponseSchema,
                DisplayHdmiRequest,
                DisplayHdmiSetSinkCapabilitiesResponseSchema,
                DisplayInitRequest,
                DisplayInitResponseSchema,
                DisplayModesetBeginRequest,
                DisplayModesetBeginResponseSchema,
                DisplayModesetEndRequest,
                DisplayReadEdidRequest,
                DisplayReadEdidResponseSchema,
                DisplayReleaseStreamRouteRequest,
                DisplaySetInstanceMemoryRequest,
                DisplayStaticInfoResponseSchema,
                GspInitRequest,
                GspInitResponseSchema, //
            },
            GmcCommandId, //
        },
        nvkv::{
            Decoder,
            Encodable,
            Encoder,
            Schema,
            UnknownKeyPolicy, //
        },
    },
    num,
    sbuffer::SBufferIter,
    vgpu::VgpuState, //
};

/// The `GSP_INIT` command, which carries the system information and registry keys to GSP-RM.
///
/// GSP-RM answers it with the static GPU configuration once it has finished starting.
pub(crate) struct GspInit {
    /// The NVKV-encoded request.
    payload: KVVec<u64>,
}

impl GspInit {
    /// Builds the request for `pdev`.
    ///
    /// # Errors
    ///
    /// - `ENOMEM` if the request or the encoder buffer cannot be allocated.
    pub(crate) fn new(
        pdev: &pci::Device<device::Bound>,
        chipset: Chipset,
        vgpu_state: VgpuState,
    ) -> Result<Self> {
        let mut encoder = Encoder::new();
        GspInitRequest::new(pdev, chipset, vgpu_state)?.encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    /// Returns the request as the bytes that the GSP reads.
    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for GspInit {
    const COMMAND: GmcCommandId = GmcCommandId::GspInit;
    /// Largest reply that the driver accepts, as in OpenRM's `kgspSendInitRpcs`.
    const MAX_REPLY_SIZE: u32 = 48 * 1024;
    type Command = ();
    type Reply = GspStaticInfo;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

pub(crate) use fw::commands::PowerStateLevel;

/// The `GSP_SUSPEND` command, which suspends GSP-RM.
///
/// GSP-RM sends no reply on the message queue. It reports the completed suspend in the GSP
/// falcon's `MAILBOX0` register instead.
pub(crate) struct GspSuspend {
    level: PowerStateLevel,
}

impl GspSuspend {
    /// Creates a `GSP_SUSPEND` command for the given [`PowerStateLevel`].
    pub(crate) fn new(level: PowerStateLevel) -> Self {
        Self { level }
    }
}

impl GmcCommand for GspSuspend {
    const COMMAND: GmcCommandId = GmcCommandId::GspSuspend;
    type Command = fw::commands::GspSuspend;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        fw::commands::GspSuspend::new(self.level)
    }
}

/// Copies `src`, which GSP-RM sends with its NUL terminator, to the start of `dst`.
///
/// # Errors
///
/// - `EINVAL` if `src` is longer than `dst`.
fn copy_name<const N: usize>(dst: &mut [u8; N], src: &[u8]) -> Result {
    dst.get_mut(..src.len()).ok_or(EINVAL)?.copy_from_slice(src);

    Ok(())
}

/// Copies a byte payload into the aligned little-endian words required by the NVKV decoder.
///
/// Returns `EINVAL` for a partial word, or `ENOMEM` if allocation fails.
fn nvkv_words(bytes: &[u8]) -> Result<KVVec<u64>> {
    let chunks = bytes.chunks_exact(size_of::<u64>());
    if !chunks.remainder().is_empty() {
        return Err(EINVAL);
    }
    let mut words = KVVec::with_capacity(chunks.len(), GFP_KERNEL)?;
    for chunk in chunks {
        // `chunks_exact` yields slices of exactly `size_of::<u64>()` bytes.
        let word = chunk.try_into().map_err(|_| EINVAL)?;
        words.push(u64::from_le_bytes(word), GFP_KERNEL)?;
    }
    Ok(words)
}

impl GmcReply for GspStaticInfo {
    const COMMAND: GmcCommandId = GmcCommandId::GspInit;
    type InitError = Error;

    /// Decodes the `GSP_INIT` reply, an NVKV stream, into the static configuration.
    ///
    /// # Errors
    ///
    /// - `EINVAL` if the payload is not a whole number of NVKV words, if the stream is malformed
    ///   or omits a required key, or if GSP-RM reported no framebuffer region.
    /// - `ENOMEM` if the words or the decoded regions cannot be allocated.
    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;

        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(GspInitResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;

        let mut gpu_name = [0u8; GspStaticInfo::MAX_GPU_NAME_LEN];
        copy_name(&mut gpu_name, reply.gpu_name())?;
        let mut gpu_short_name = [0u8; GspStaticInfo::MAX_GPU_NAME_LEN];
        copy_name(&mut gpu_short_name, reply.gpu_short_name())?;
        let mut gpu_gid = [0u8; GspStaticInfo::GID_LEN];
        copy_name(&mut gpu_gid, reply.gpu_gid())?;

        let mut usable_fb_regions = KVec::new();
        for region in reply.usable_fb_regions() {
            usable_fb_regions.push(region, GFP_KERNEL)?;
        }

        Ok(GspStaticInfo {
            gpu_name,
            gpu_short_name,
            gpu_gid,
            bar1_pde_base: reply.bar1_pde_base(),
            usable_fb_regions,
            total_fb_end: reply.total_fb_end().ok_or(EINVAL)?,
        })
    }
}

pub(crate) struct DisplayInit {
    /// The NVKV-encoded request.
    payload: KVVec<u64>,
}

impl DisplayInit {
    /// Builds the request with its required zero flags field.
    pub(crate) fn new() -> Result<Self> {
        let mut encoder = Encoder::new();
        DisplayInitRequest::new().encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    /// Returns the request as the bytes that the GSP reads.
    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayInit {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayInit;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayCaps;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Ok(())
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayCaps {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayInit;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayInit::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayInitResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if reply.common_caps.len() != Self::COMMON_CAPS_LEN
            || reply.display_caps.len() != Self::DISPLAY_CAPS_LEN
        {
            return Err(EINVAL);
        }
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplayShutdown;

impl GmcCommand for DisplayShutdown {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayShutdown;
    type Command = ();
    type Reply = DisplayShutdownReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }
}

impl GmcReply for DisplayShutdownReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayShutdown;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}

pub(crate) struct DisplayGetStaticInfo;

impl GmcCommand for DisplayGetStaticInfo {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetStaticInfo;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayHwStaticInfo;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }
}

impl GmcReply for DisplayHwStaticInfo {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetStaticInfo;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayGetStaticInfo::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayStaticInfoResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplayGetPortState {
    payload: KVVec<u64>,
}

impl DisplayGetPortState {
    pub(crate) fn new(display_id: u32) -> Result<Self> {
        if !display_id.is_power_of_two() {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayGetPortStateRequest::new(display_id).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayGetPortState {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetPortState;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayGetPortStateReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayGetPortStateReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetPortState;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayGetPortState::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayGetPortStateResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if !reply.display_id.is_power_of_two() || reply.connected > 1 {
            return Err(EINVAL);
        }
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplayReadEdid {
    payload: KVVec<u64>,
}

impl DisplayReadEdid {
    pub(crate) fn new(display_id: u32) -> Result<Self> {
        if !display_id.is_power_of_two() {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayReadEdidRequest::new(display_id).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayReadEdid {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayReadEdid;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayReadEdidReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayReadEdidReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayReadEdid;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayReadEdid::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayReadEdidResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        let prefix = reply
            .edid
            .get(..num::u32_as_usize(reply.size))
            .ok_or(EINVAL)?;
        let mut edid = KVec::with_capacity(prefix.len(), GFP_KERNEL)?;
        edid.extend_from_slice(prefix, GFP_KERNEL)?;
        Ok(Self { edid })
    }
}

pub(crate) struct DisplayGetPortCaps {
    payload: KVVec<u64>,
}

impl DisplayGetPortCaps {
    pub(crate) fn new(display_id: u32) -> Result<Self> {
        if !display_id.is_power_of_two() {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayGetPortCapsRequest::new(display_id).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayGetPortCaps {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetPortCaps;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayGetPortCapsReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayGetPortCapsReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayGetPortCaps;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayGetPortCaps::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayGetPortCapsResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplaySetInstanceMemory {
    /// The NVKV-encoded request.
    payload: KVVec<u64>,
}

impl DisplaySetInstanceMemory {
    pub(crate) fn new(address: u64, size: u64) -> Result<Self> {
        let mut encoder = Encoder::new();
        DisplaySetInstanceMemoryRequest::new(address, size).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplaySetInstanceMemory {
    const COMMAND: GmcCommandId = GmcCommandId::DisplaySetInstanceMemory;
    type Command = ();
    type Reply = DisplaySetInstanceMemoryReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplaySetInstanceMemoryReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplaySetInstanceMemory;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}

pub(crate) struct DisplayCreateChannel {
    payload: KVVec<u64>,
}

impl DisplayCreateChannel {
    pub(crate) fn new(
        kind: DisplayChannelKind,
        instance: u32,
        pb_address: u64,
        pb_size: u64,
    ) -> Result<Self> {
        let mut encoder = Encoder::new();
        DisplayCreateChannelRequest::new(kind, instance, pb_address, pb_size)
            .encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayCreateChannel {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayCreateChannel;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayCreateChannelReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayCreateChannelReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayCreateChannel;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayCreateChannel::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayCreateChannelResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplayDestroyChannel {
    payload: KVVec<u64>,
}

impl DisplayDestroyChannel {
    pub(crate) fn new(channel: u64) -> Result<Self> {
        let mut encoder = Encoder::new();
        DisplayDestroyChannelRequest::new(channel).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayDestroyChannel {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayDestroyChannel;
    type Command = ();
    type Reply = DisplayDestroyChannelReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayDestroyChannelReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayDestroyChannel;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}

pub(crate) struct DisplayHdmiSetSinkCapabilities {
    payload: KVVec<u64>,
}

impl DisplayHdmiSetSinkCapabilities {
    pub(crate) fn new(stream_id: u64, modeset_id: u64, capabilities: u32) -> Result<Self> {
        if stream_id == 0 || modeset_id == 0 || capabilities & !7 != 0 {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayHdmiRequest::new(stream_id, modeset_id, capabilities).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayHdmiSetSinkCapabilities {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayHdmiSetSinkCapabilities;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayHdmiSetSinkCapabilitiesReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayHdmiSetSinkCapabilitiesReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayHdmiSetSinkCapabilities;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayHdmiSetSinkCapabilities::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(
            DisplayHdmiSetSinkCapabilitiesResponseSchema::init(),
            GFP_KERNEL,
        )?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if reply.sink_snapshot_id == 0 {
            return Err(EINVAL);
        }
        Ok(Self)
    }
}

pub(crate) struct DisplayAssignStreamRoute {
    payload: KVVec<u64>,
}

impl DisplayAssignStreamRoute {
    pub(crate) fn new(
        modeset_id: u64,
        display_id: u32,
        head: u32,
        exclude_mask: u32,
    ) -> Result<Self> {
        if !display_id.is_power_of_two()
            || modeset_id == 0
            || exclude_mask >= (1 << DisplayAssignStreamRouteReply::NUM_SORS)
        {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayAssignStreamRouteRequest::new(modeset_id, display_id, head, exclude_mask)
            .encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayAssignStreamRoute {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayAssignStreamRoute;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayAssignStreamRouteReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayAssignStreamRouteReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayAssignStreamRoute;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayAssignStreamRoute::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayAssignStreamRouteResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if reply.stream_id == 0 || reply.assigned.len() != Self::NUM_SORS {
            return Err(EINVAL);
        }
        let reply = KBox::into_inner(reply);
        Ok(Self {
            stream_id: reply.stream_id,
            assigned: reply.assigned,
        })
    }
}

pub(crate) struct DisplayCheckConfiguration {
    payload: KVVec<u64>,
}

impl DisplayCheckConfiguration {
    pub(crate) fn new(configuration: &DisplayConfiguration) -> Result<Self> {
        if !configuration.display_id.is_power_of_two() {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayCheckConfigurationRequest::new(configuration).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayCheckConfiguration {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayCheckConfiguration;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayCheckConfigurationReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayCheckConfigurationReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayCheckConfiguration;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayCheckConfiguration::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayCheckConfigurationResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if reply.possible > 1 || reply.checked_domains != 1 {
            return Err(EINVAL);
        }
        Ok(Self {
            possible: reply.possible,
            checked_domains: reply.checked_domains,
        })
    }
}

pub(crate) struct DisplayModesetBegin {
    payload: KVVec<u64>,
}

impl DisplayModesetBegin {
    pub(crate) fn new(display_mask: u32, target_display_mask: u32) -> Result<Self> {
        if display_mask == 0 {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayModesetBeginRequest::new(display_mask, target_display_mask).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayModesetBegin {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayModesetBegin;
    const MAX_REPLY_SIZE: u32 = 4096;
    type Command = ();
    type Reply = DisplayModesetBeginReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayModesetBeginReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayModesetBegin;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        let bytes = payload.flush_into_kvec(GFP_KERNEL)?;
        if bytes.len() > num::u32_as_usize(DisplayModesetBegin::MAX_REPLY_SIZE) {
            return Err(EINVAL);
        }
        let words = nvkv_words(&bytes)?;

        let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
        let mut schema = KBox::init(DisplayModesetBeginResponseSchema::init(), GFP_KERNEL)?;
        let reply = KBox::try_init(decoder.decode(&mut *schema)?, GFP_KERNEL)?;
        if reply.modeset_id == 0 {
            return Err(EINVAL);
        }
        Ok(KBox::into_inner(reply))
    }
}

pub(crate) struct DisplayModesetEnd {
    payload: KVVec<u64>,
}

impl DisplayModesetEnd {
    pub(crate) fn new(modeset_id: u64, end_mode: DisplayModesetEndMode) -> Result<Self> {
        if modeset_id == 0 {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayModesetEndRequest::new(modeset_id, end_mode).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayModesetEnd {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayModesetEnd;
    type Command = ();
    type Reply = DisplayModesetEndReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayModesetEndReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayModesetEnd;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}

pub(crate) struct DisplayReleaseStreamRoute {
    payload: KVVec<u64>,
}

impl DisplayReleaseStreamRoute {
    pub(crate) fn new(stream_id: u64, modeset_id: u64) -> Result<Self> {
        if stream_id == 0 || modeset_id == 0 {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayReleaseStreamRouteRequest::new(stream_id, modeset_id).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayReleaseStreamRoute {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayReleaseStreamRoute;
    type Command = ();
    type Reply = DisplayReleaseStreamRouteReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayReleaseStreamRouteReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayReleaseStreamRoute;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}

pub(crate) struct DisplayHdmiSetTmdsProtocol {
    payload: KVVec<u64>,
}

impl DisplayHdmiSetTmdsProtocol {
    #[expect(dead_code)]
    pub(crate) fn new(stream_id: u64, modeset_id: u64, hdmi: bool) -> Result<Self> {
        if stream_id == 0 || modeset_id == 0 {
            return Err(EINVAL);
        }
        let mut encoder = Encoder::new();
        DisplayHdmiRequest::new(stream_id, modeset_id, u32::from(hdmi)).encode(&mut encoder)?;

        Ok(Self {
            payload: encoder.finish(),
        })
    }

    fn payload_bytes(&self) -> &[u8] {
        AsBytes::as_bytes(self.payload.as_slice())
    }
}

impl GmcCommand for DisplayHdmiSetTmdsProtocol {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayHdmiSetTmdsProtocol;
    type Command = ();
    type Reply = DisplayHdmiSetTmdsProtocolReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        pin_init::init_zeroed()
    }

    fn variable_payload_len(&self) -> usize {
        self.payload_bytes().len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.payload_bytes())
    }
}

impl GmcReply for DisplayHdmiSetTmdsProtocolReply {
    const COMMAND: GmcCommandId = GmcCommandId::DisplayHdmiSetTmdsProtocol;
    type InitError = Error;

    fn read(payload: &mut SBufferIter<core::array::IntoIter<&[u8], 2>>) -> Result<Self> {
        if payload.is_empty() {
            Ok(Self)
        } else {
            Err(EINVAL)
        }
    }
}
