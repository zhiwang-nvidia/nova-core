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
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    gsp::{
        cmdq::{
            Cmdq,
            CommandToGsp,
            MessageFromGsp, //
        },
        fw::{
            self,
            MsgFunction, //
        },
        nvkv, //
    },
    sbuffer::SBufferIter,
    vgpu::VgpuState, //
};

/// Maximum response size for the `GSP_INIT` reply.
const GSP_INIT_MAX_RESPONSE_SIZE: u32 = 8192;

/// GMC command id for `GSP_INIT`.
///
/// Matches `GMCAPI_COMMANDS_GMCAPI_CMD_GSP_INIT` in the r000 bindings.
const CMD_GSP_INIT: u32 = 0x0001_0001;

/// Hardcoded registry entries the driver always sends to GSP-RM.
///
/// `RMSecBusResetEnable` enables PCI secondary bus reset. `RMForcePcieConfigSave`
/// forces GSP-RM to preserve PCI configuration registers across any PCI reset.
/// `RMDevidCheckIgnore` allows GSP-RM to boot even if the PCI device id is not
/// found in its internal product name database.
const REGISTRY_ENTRIES: &[(&[u8], u32)] = &[
    (b"RMSecBusResetEnable\0", 1),
    (b"RMForcePcieConfigSave\0", 1),
    (b"RMDevidCheckIgnore\0", 1),
];

/// The reply from the GSP to the `GSP_INIT` GMC command.
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

/// Decodes the static GPU information returned by a GMC command.
fn decode_gsp_info(payload: &[u8]) -> Result<GetGspStaticInfoReply> {
    let decoder = nvkv::Decoder::new(payload, nvkv::UnknownKeyPolicy::Ignore)?;
    let decoded = KBox::try_init(
        decoder.decode(nvkv::GspInitResponseSchema::default())?,
        GFP_KERNEL,
    )?;

    let mut gpu_name = [0u8; 64];
    let name = decoded.gpu_name();
    gpu_name[..name.len()].copy_from_slice(name);

    let mut usable_fb_regions = KVec::new();
    for region in decoded.usable_fb_regions() {
        usable_fb_regions.push(region, GFP_KERNEL)?;
    }

    Ok(GetGspStaticInfoReply {
        gpu_name,
        usable_fb_regions,
    })
}

/// Builds an NVKV-encoded `GSP_INIT` request payload.
///
/// The blob carries the system-info keys with values the driver actually
/// has, plus the registry entries from [`REGISTRY_ENTRIES`] as
/// `REGKEY_NAME` plus `REGKEY_VALUE_U32` pairs.
pub(crate) fn build_gsp_init_payload(
    pdev: &pci::Device<device::Bound>,
    chipset: Chipset,
    vgpu_state: VgpuState,
) -> Result<KVVec<u8>> {
    let mirror = chipset.pci_config_mirror_range();
    let mirror_size = mirror.end.checked_sub(mirror.start).ok_or(EINVAL)?;
    let oor_arch = if cfg!(target_arch = "x86_64") {
        nvkv::OorArch::X86_64
    } else if cfg!(target_arch = "aarch64") {
        nvkv::OorArch::Aarch64
    } else if cfg!(target_arch = "powerpc64") {
        nvkv::OorArch::Ppc64le
    } else if cfg!(target_arch = "arm") {
        nvkv::OorArch::Arm
    } else if cfg!(target_arch = "riscv64") {
        nvkv::OorArch::Riscv64
    } else {
        nvkv::OorArch::None
    };

    let mut regkeys = KVVec::new();
    for &(name, value) in REGISTRY_ENTRIES {
        regkeys.push(nvkv::RegKey::new(name, value), GFP_KERNEL)?;
    }
    if matches!(vgpu_state, VgpuState::Enabled { .. }) {
        regkeys.push(nvkv::RegKey::new(b"RMSetSriovMode\0", 1), GFP_KERNEL)?;
    }

    let request = nvkv::GspInitRequest::new(
        (u32::from(pdev.device_id()) << 16) | u32::from(pdev.vendor_id().as_raw()),
        (u32::from(pdev.subsystem_device_id()) << 16) | u32::from(pdev.subsystem_vendor_id()),
        u32::from(pdev.revision_id()),
        mirror.start,
        mirror_size,
        oor_arch,
        u64::from(pdev.dev_id()),
        regkeys,
    );
    let mut encoder = nvkv::Encoder::new();
    nvkv::Encodeable::encode(&request, &mut encoder)?;

    Ok(encoder.finish())
}

/// Sends `GSP_INIT` and drives the receive loop until the reply arrives.
///
/// `payload` is the NVKV-encoded blob from [`build_gsp_init_payload`].
/// GSP-RM may interleave boot events between the send and the reply, so the
/// caller supplies an `on_boot_event` closure that handles those events.
/// The loop terminates when a GMC message arrives whose command id matches
/// [`CMD_GSP_INIT`]; the reply payload is decoded and returned.
pub(crate) fn gsp_init(
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    payload: &[u8],
    mut on_boot_event: impl FnMut(u32, &[u8]) -> Result,
) -> Result<GetGspStaticInfoReply> {
    cmdq.send_gmc_no_wait(bar, CMD_GSP_INIT, payload, GSP_INIT_MAX_RESPONSE_SIZE)?;

    loop {
        let reply = cmdq.receive_gmc_and_dispatch(
            bar,
            Cmdq::RECEIVE_TIMEOUT,
            |id, status, p0, p1| -> Result<Option<GetGspStaticInfoReply>> {
                if id == CMD_GSP_INIT {
                    if status != 0 {
                        return Err(EIO);
                    }

                    let mut blob = KVVec::with_capacity(p0.len() + p1.len(), GFP_KERNEL)?;
                    blob.extend_from_slice(p0, GFP_KERNEL)?;
                    blob.extend_from_slice(p1, GFP_KERNEL)?;
                    Ok(Some(decode_gsp_info(&blob)?))
                } else {
                    on_boot_event(id, p0)?;
                    Ok(None)
                }
            },
        )??;

        if let Some(reply) = reply {
            return Ok(reply);
        }
    }
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
