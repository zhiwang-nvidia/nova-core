// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! vGPU firmware command operations.
//!
//! Sends requests, checks responses and coordinates the firmware command
//! sequences used by instance lifecycle operations.

use kernel::{
    device,
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

use super::instance::Gfid;

use super::fw::commands::VgpuPropertiesSchema;

pub(super) use super::fw::commands::{
    Dbdf,
    VgpuProperties, //
};

use super::fw::{
    GMCAPI_CMD_BOOTLOAD_GSP_VGPU_PLUGIN_TASK,
    GMCAPI_CMD_CLEANUP_GSP_VGPU_PLUGIN_RESOURCES,
    GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE,
    GMCAPI_CMD_QUERY_VGPU_PROPERTIES,
    GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK,
    GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_COMPLETE, //
};

/// Query the vGPU type assigned to a VF by its DBDF.
#[expect(dead_code)]
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
