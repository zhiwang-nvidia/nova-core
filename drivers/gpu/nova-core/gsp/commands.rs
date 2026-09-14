// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::{
    pci,
    prelude::*,
    transmute::AsBytes, //
};

use crate::{
    gsp::{
        cmdq::Cmdq,
        fw::{
            self,
            commands::{
                GspInitRequest,
                GspInitResponseSchema,
                VfInfo, //
            },
            GspGmcMsgElement,
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
        GspBootContext, //
    },
    vgpu::VgpuState, //
};

pub(crate) use fw::commands::{
    FifoEngineList,
    GspStaticInfo, //
};

/// Builds the NVKV-encoded payload of a `GSP_INIT` request for `pdev`.
///
/// # Errors
///
/// - `ENOMEM` if the request or the encoder buffer cannot be allocated.
/// - `ENODEV` if vGPU mode is enabled but the SR-IOV capability is missing.
///
/// Errors reading the PCI configuration or decoding the VF BAR layout are propagated as-is.
pub(super) fn build_gsp_init_payload(ctx: &GspBootContext<'_, '_>) -> Result<EncodedStream> {
    let mut encoder = Encoder::new();
    let vf_info = build_vf_info(ctx)?;
    GspInitRequest::new(ctx.pdev, ctx.chipset, *ctx.vgpu_state, vf_info)?.encode(&mut encoder)?;

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

/// Largest `GSP_INIT` response that the driver accepts.
const GSP_INIT_MAX_RESPONSE_SIZE: u32 = 48 * 1024;

/// Sends `GSP_INIT` and returns the static GPU configuration that its reply carries.
///
/// Every GMC (GPU Management Controller) element that arrives before the reply is passed to
/// `on_unsolicited_element` with the headers that open it and its payload, as two slices because
/// the ring may wrap. The load-and-execute events that GSP-RM raises while it starts arrive this
/// way.
///
/// `payload` is the stream from [`build_gsp_init_payload`].
///
/// # Errors
///
/// - `EIO` if GSP-RM reports a failure status.
/// - `ETIMEDOUT` if the reply does not arrive within [`Cmdq::RECEIVE_TIMEOUT`] of the send,
///   however many events arrive while waiting.
///
/// Errors from `on_unsolicited_element` and from decoding the reply are propagated as-is.
pub(crate) fn gsp_init(
    cmdq: &Cmdq<'_>,
    payload: &[u64],
    on_unsolicited_element: impl FnMut(&GspGmcMsgElement, &[u8], &[u8]) -> Result,
) -> Result<GspStaticInfo> {
    // Qualified because `zerocopy::IntoBytes` also gives `[T]` an `as_bytes`.
    let payload = AsBytes::as_bytes(payload);

    let sequence =
        cmdq.send_gmc_no_wait(GMCAPI_CMD_GSP_INIT, payload, GSP_INIT_MAX_RESPONSE_SIZE)?;

    cmdq.await_gmc_response(
        GMCAPI_CMD_GSP_INIT,
        sequence,
        on_unsolicited_element,
        decode_gsp_init_reply,
    )
}

/// Decodes the `GSP_INIT` reply from its payload, which the ring may have split in two.
///
/// # Errors
///
/// - `EINVAL` if the payload is not a whole number of NVKV words, or if the stream is malformed
///   or omits a required key, or the FIFO engine count exceeds the supported table capacity.
/// - `ENOMEM` if the words or the decoded regions cannot be allocated.
fn decode_gsp_init_reply(payload_0: &[u8], payload_1: &[u8]) -> Result<GspStaticInfo> {
    let words = nvkv_words(payload_0, payload_1)?;

    let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
    let mut schema = GspInitResponseSchema::default();
    let info = KBox::try_init(decoder.decode(&mut schema)?, GFP_KERNEL)?;

    Ok(KBox::into_inner(info))
}

pub(crate) use fw::commands::PowerStateLevel;

/// Sends `GSP_SUSPEND`, which GSP-RM does not answer (see [`GMCAPI_CMD_GSP_SUSPEND`]).
///
/// # Errors
///
/// Errors from [`Cmdq::send_gmc_no_reply`] are propagated as-is.
pub(crate) fn gsp_suspend(cmdq: &Cmdq<'_>, level: PowerStateLevel) -> Result {
    let params = fw::commands::GspSuspend::new(level);

    cmdq.send_gmc_no_reply(GMCAPI_CMD_GSP_SUSPEND, AsBytes::as_bytes(&params))
}
