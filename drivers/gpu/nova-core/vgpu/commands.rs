// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! vGPU firmware command operations.
//!
//! Sends requests, checks responses and coordinates the firmware command
//! sequences used by instance lifecycle operations.

use kernel::{
    device,
    num::casts::usize_into_u32,
    prelude::*,
    time::Delta,
    transmute::AsBytes, //
};

use crate::gsp::{
    cmdq::Cmdq,
    nvkv::{
        nvkv_words,
        Decoder,
        UnknownKeyPolicy, //
    },
};

use crate::{
    driver::Bar0,
    mm::PAGE_SIZE, //
};

use super::{
    fw::RpcMessage,
    gsp_plugin_rpc::PluginRpc,
    instance::Gfid, //
};

use super::fw::commands::VgpuPropertiesSchema;

pub(super) use super::fw::commands::{
    Dbdf,
    VgpuProperties, //
};

use super::fw::commands::encode_plugin_set_bme;

use super::fw::{
    GMCAPI_CMD_BOOTLOAD_GSP_VGPU_PLUGIN_TASK,
    GMCAPI_CMD_CLEANUP_GSP_VGPU_PLUGIN_RESOURCES,
    GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE,
    GMCAPI_CMD_QUERY_VGPU_PROPERTIES,
    GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK,
    GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_COMPLETE, //
};

use super::fw::{
    commands::{
        AllocCeutilsRequest,
        AllocCeutilsResponse,
        FreeCeutilsRequest,
        ScrubGuestFbRequest,
        ScrubGuestFbResponse, //
    },
    GMCAPI_CMD_VGPU_MGR_ALLOC_GSP_CEUTILS,
    GMCAPI_CMD_VGPU_MGR_FREE_GSP_CEUTILS,
    GMCAPI_CMD_VGPU_MGR_SCRUB_GUEST_FB,
    NV_ADDR_FBMEM, //
};

/// Query the vGPU type assigned to a VF by its DBDF.
pub(super) fn query_assigned_vf_type(cmdq: &Cmdq<'_>, dbdf: Dbdf) -> Result<u32> {
    let request = u64::from(dbdf.into_raw()).to_le_bytes();
    let response =
        cmdq.send_gmc_and_receive(GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE, &request, 64)?;
    if response.status != 0 {
        return Err(EIO);
    }
    let bytes = response.payload.get(..4).ok_or(ENODEV)?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| EINVAL)?))
}

/// Query and decode the firmware properties of one vGPU type.
pub(super) fn query_vgpu_properties(cmdq: &Cmdq<'_>, type_id: u32) -> Result<KBox<VgpuProperties>> {
    let response = cmdq.send_gmc_and_receive(
        GMCAPI_CMD_QUERY_VGPU_PROPERTIES,
        &type_id.to_le_bytes(),
        4096,
    )?;
    if response.status != 0 {
        return Err(EIO);
    }

    let properties = decode_vgpu_properties(&response.payload)?;
    if properties.type_id != type_id || properties.max_instance == 0 {
        return Err(EINVAL);
    }
    Ok(properties)
}

/// Decodes a byte-oriented GMC vGPU-properties response with the typed NVKV schema.
fn decode_vgpu_properties(payload: &[u8]) -> Result<KBox<VgpuProperties>> {
    let words = nvkv_words(payload, &[])?;
    let decoder = Decoder::new(&words, UnknownKeyPolicy::Ignore);
    let mut schema = VgpuPropertiesSchema::default();
    let properties = KBox::try_init(decoder.decode(&mut schema)?, GFP_KERNEL)?;
    Ok(properties)
}

/// Send BOOTLOAD and check its firmware status.
pub(super) fn send_bootload(cmdq: &Cmdq<'_>, payload: &[u64]) -> Result {
    let response = cmdq.send_gmc_and_receive_timeout(
        GMCAPI_CMD_BOOTLOAD_GSP_VGPU_PLUGIN_TASK,
        AsBytes::as_bytes(payload),
        0,
        Delta::from_secs(10),
    )?;
    if response.status != 0 {
        return Err(EIO);
    }
    Ok(())
}

/// Shut down a vGPU plugin task and wait for its completion event.
pub(super) fn send_shutdown(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    gfid: Gfid,
) -> Result {
    let payload = gfid.0.to_le_bytes();

    cmdq.send_gmc_and_wait_event(
        GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK,
        &payload,
        Delta::from_secs(10),
        |command_id, _max_response_size, _sequence, payload_0, payload_1| {
            if command_id != GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_COMPLETE
                || !payload
                    .iter()
                    .copied()
                    .eq(Iterator::chain(payload_0.iter(), payload_1.iter())
                        .take(payload.len())
                        .copied())
            {
                return Ok(false);
            }
            Ok(true)
        },
        |command_id, _max_response_size, _sequence, _payload_0, _payload_1| {
            dev_dbg!(
                dev,
                "shutdown: ignoring unrelated event command={:#x}\n",
                command_id,
            );
            Ok(())
        },
    )?;
    Ok(())
}

/// Release firmware resources after a plugin task has stopped.
pub(super) fn send_cleanup(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    gfid: Gfid,
) -> Result {
    cmdq.send_gmc_and_check_status(
        GMCAPI_CMD_CLEANUP_GSP_VGPU_PLUGIN_RESOURCES,
        &gfid.0.to_le_bytes(),
    )?;
    dev_dbg!(dev, "cleanup: gfid={} done\n", gfid.0);
    Ok(())
}

/// Negotiate the host protocol with a bootloaded GSP plugin.
pub(super) fn negotiate_plugin_version(
    dev: &device::Device<device::Bound>,
    bar0: Bar0<'_>,
    gfid: Gfid,
    rpc: &mut PluginRpc<'_, '_>,
) -> Result {
    rpc.rpc_call(dev, bar0, gfid, RpcMessage::VersionNegotiation, &[])
}

/// Send an instance's encoded configuration to the GSP plugin.
pub(super) fn send_plugin_config(
    dev: &device::Device<device::Bound>,
    bar0: Bar0<'_>,
    gfid: Gfid,
    rpc: &mut PluginRpc<'_, '_>,
    config: &[u64],
) -> Result {
    rpc.rpc_call_nvkv(
        dev,
        bar0,
        gfid,
        RpcMessage::SetupConfigParamsAndInit,
        config,
    )
}

/// Update the bus-mastering state reported to the GSP plugin.
pub(super) fn set_plugin_bme(
    dev: &device::Device<device::Bound>,
    bar0: Bar0<'_>,
    gfid: Gfid,
    rpc: &mut PluginRpc<'_, '_>,
    enable: bool,
) -> Result {
    let bme = encode_plugin_set_bme(enable)?;
    rpc.rpc_call_nvkv(dev, bar0, gfid, RpcMessage::UpdateBmeState, &bme)
}

/// Reset an active GSP plugin.
pub(super) fn reset_plugin(
    dev: &device::Device<device::Bound>,
    bar: Bar0<'_>,
    gfid: Gfid,
    rpc: &mut PluginRpc<'_, '_>,
) -> Result {
    rpc.rpc_call(dev, bar, gfid, RpcMessage::Reset, &[])
}

/// Whether a failed allocation may still have transferred CHID ownership to firmware.
pub(super) enum CeUtilsAllocError {
    /// A matching firmware response explicitly rejected the allocation.
    NotOwned(Error),
    /// The request may have completed despite a transport or response-validation error.
    MayOwn(Error),
}

/// Allocate a CeUtils channel and validate its semaphore description.
pub(super) fn alloc_ceutils(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    gfid: Gfid,
    chid: u32,
) -> core::result::Result<u64, CeUtilsAllocError> {
    let request = AllocCeutilsRequest {
        gfid: gfid.0.to_le(),
        fixed_chid: chid.to_le(),
        force_ceid: u32::MAX.to_le(),
        swizz_id: 0,
    };

    dev_dbg!(dev, "alloc CeUtils: gfid={} chid={}\n", gfid.0, chid,);

    let response = cmdq
        .send_gmc_and_receive(
            GMCAPI_CMD_VGPU_MGR_ALLOC_GSP_CEUTILS,
            <AllocCeutilsRequest as IntoBytes>::as_bytes(&request),
            usize_into_u32::<{ size_of::<AllocCeutilsResponse>() }>(),
        )
        .map_err(CeUtilsAllocError::MayOwn)?;
    if response.status != 0 {
        return Err(CeUtilsAllocError::NotOwned(EIO));
    }

    (|| {
        let bytes = response
            .payload
            .get(..size_of::<AllocCeutilsResponse>())
            .ok_or(EMSGSIZE)?;
        let response = AllocCeutilsResponse::read_from_bytes(bytes).map_err(|_| EINVAL)?;
        let semaphore_address = u64::from_le(response.semaphore_address);
        let semaphore_aperture = u32::from_le(response.semaphore_aperture);
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;

        if semaphore_address == 0
            || !semaphore_address.is_multiple_of(page_size)
            || semaphore_aperture != NV_ADDR_FBMEM
        {
            return Err(EINVAL);
        }

        dev_dbg!(
            dev,
            "alloc CeUtils: gfid={} semaphore={:#x}\n",
            gfid.0,
            semaphore_address,
        );
        Ok(semaphore_address)
    })()
    .map_err(CeUtilsAllocError::MayOwn)
}

/// Release a CeUtils allocation, including one whose allocation reply was lost.
pub(super) fn free_ceutils(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    gfid: Gfid,
) -> Result {
    let request = FreeCeutilsRequest {
        gfid: gfid.0.to_le(),
    };

    dev_dbg!(dev, "free CeUtils: gfid={}\n", gfid.0);
    cmdq.send_gmc_and_check_status(
        GMCAPI_CMD_VGPU_MGR_FREE_GSP_CEUTILS,
        <FreeCeutilsRequest as IntoBytes>::as_bytes(&request),
    )
}

/// Submit an asynchronous guest FB scrub and return its work identifier.
pub(super) fn submit_ceutils_scrub(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    gfid: Gfid,
    fb_offset: u64,
    fb_size: u64,
) -> Result<u32> {
    let request = ScrubGuestFbRequest {
        gfid: gfid.0.to_le(),
        reserved: 0,
        fb_offset: fb_offset.to_le(),
        fb_size: fb_size.to_le(),
    };

    dev_dbg!(
        dev,
        "submit scrub: gfid={} offset={:#x} size={:#x}\n",
        gfid.0,
        fb_offset,
        fb_size,
    );

    let response = cmdq.send_gmc_and_receive(
        GMCAPI_CMD_VGPU_MGR_SCRUB_GUEST_FB,
        <ScrubGuestFbRequest as IntoBytes>::as_bytes(&request),
        usize_into_u32::<{ size_of::<ScrubGuestFbResponse>() }>(),
    )?;
    if response.status != 0 {
        return Err(EIO);
    }

    let bytes = response
        .payload
        .get(..size_of::<ScrubGuestFbResponse>())
        .ok_or(EMSGSIZE)?;
    let response = ScrubGuestFbResponse::read_from_bytes(bytes).map_err(|_| EINVAL)?;
    let work_id = u32::try_from(u64::from_le(response.work_id)).map_err(|_| EOVERFLOW)?;

    Ok(work_id)
}
