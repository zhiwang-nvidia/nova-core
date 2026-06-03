// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! vGPU firmware command operations.
//!
//! Sends requests, checks responses and coordinates the firmware command
//! sequences used by instance lifecycle operations.

use kernel::prelude::*;

use crate::gsp::{
    cmdq::Cmdq,
    nvkv::{
        nvkv_words,
        Decoder,
        UnknownKeyPolicy, //
    },
};

use super::fw::commands::VgpuPropertiesSchema;

pub(super) use super::fw::commands::{
    Dbdf,
    VgpuProperties, //
};

use super::fw::{
    GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE,
    GMCAPI_CMD_QUERY_VGPU_PROPERTIES, //
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
#[expect(dead_code)]
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
