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
            NoReply, //
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

/// The `GspSetSystemInfo` command.
pub(crate) struct SetSystemInfo<'a> {
    pdev: &'a pci::Device<device::Bound>,
    chipset: Chipset,
}

impl<'a> SetSystemInfo<'a> {
    /// Creates a new `GspSetSystemInfo` command using the parameters of `pdev`.
    pub(crate) fn new(pdev: &'a pci::Device<device::Bound>, chipset: Chipset) -> Self {
        Self { pdev, chipset }
    }
}

impl<'a> CommandToGsp for SetSystemInfo<'a> {
    const FUNCTION: MsgFunction = MsgFunction::GspSetSystemInfo;
    const IS_ASYNC: bool = true;
    type Command = fw::commands::GspSetSystemInfo;
    type Reply = NoReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init(self.pdev, self.chipset)
    }
}

struct RegistryEntry {
    key: &'static str,
    value: u32,
}

/// The `SetRegistry` command.
pub(crate) struct SetRegistry {
    entries: KVec<RegistryEntry>,
}

impl SetRegistry {
    /// Creates a new `SetRegistry` command, using a set of hardcoded entries.
    pub(crate) fn new(vgpu_state: VgpuState) -> Result<Self> {
        let mut entries = KVec::new();

        // RMSecBusResetEnable - enables PCI secondary bus reset
        entries.push(
            RegistryEntry {
                key: "RMSecBusResetEnable",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        // RMForcePcieConfigSave - forces GSP-RM to preserve PCI configuration registers on
        // any PCI reset.
        entries.push(
            RegistryEntry {
                key: "RMForcePcieConfigSave",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        // RMDevidCheckIgnore - allows GSP-RM to boot even if the PCI dev ID is not found
        // in the internal product name database.
        entries.push(
            RegistryEntry {
                key: "RMDevidCheckIgnore",
                value: 1,
            },
            GFP_KERNEL,
        )?;

        if matches!(vgpu_state, VgpuState::Enabled { .. }) {
            // RMSetSriovMode - required when vGPU is enabled.
            entries.push(
                RegistryEntry {
                    key: "RMSetSriovMode",
                    value: 1,
                },
                GFP_KERNEL,
            )?;
        }

        Ok(Self { entries })
    }
}

impl CommandToGsp for SetRegistry {
    const FUNCTION: MsgFunction = MsgFunction::SetRegistry;
    const IS_ASYNC: bool = true;
    type Command = fw::commands::PackedRegistryTable;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        Self::Command::init(
            self.entries.len() as u32,
            self.variable_payload_len() as u32,
        )
    }

    fn variable_payload_len(&self) -> usize {
        let mut key_size = 0;
        for entry in self.entries.iter() {
            key_size += entry.key.len() + 1; // +1 for NULL terminator
        }
        self.entries.len() * size_of::<fw::commands::PackedRegistryEntry>() + key_size
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        let string_data_start_offset = size_of::<Self::Command>()
            + self.entries.len() * size_of::<fw::commands::PackedRegistryEntry>();

        // Array for string data.
        let mut string_data = KVec::new();

        for entry in self.entries.iter() {
            dst.write_all(
                fw::commands::PackedRegistryEntry::new(
                    (string_data_start_offset + string_data.len()) as u32,
                    entry.value,
                )
                .as_bytes(),
            )?;

            let key_bytes = entry.key.as_bytes();
            string_data.extend_from_slice(key_bytes, GFP_KERNEL)?;
            string_data.push(0, GFP_KERNEL)?;
        }

        dst.write_all(string_data.as_slice())
    }
}

/// GMC command ID for `GSP_GET_STATIC_INFO`.
const GMC_CMD_GSP_GET_STATIC_INFO: u32 = 0x0001_0001;

/// Maximum response size for `GSP_GET_STATIC_INFO`.
const GSP_GET_STATIC_INFO_MAX_RESPONSE: u32 = 8192;

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

/// The reply from the GSP to the `GSP_GET_STATIC_INFO` GMC command.
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

/// Sends `GSP_GET_STATIC_INFO` via GMC and parses the NVKV response.
pub(crate) fn get_gsp_info(cmdq: &Cmdq, bar: Bar0<'_>) -> Result<GetGspStaticInfoReply> {
    let response = cmdq.send_gmc_and_receive(
        bar,
        GMC_CMD_GSP_GET_STATIC_INFO,
        &[],
        GSP_GET_STATIC_INFO_MAX_RESPONSE,
    )?;

    if response.status != 0 {
        return Err(EIO);
    }

    decode_gsp_info(&response.payload)
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
#[expect(dead_code)]
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

/// Sends `GSP_INIT` via GMC and parses the NVKV response.
///
/// `payload` is the NVKV-encoded blob from [`build_gsp_init_payload`].
#[expect(dead_code)]
pub(crate) fn gsp_init(
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    payload: &[u8],
) -> Result<GetGspStaticInfoReply> {
    let response =
        cmdq.send_gmc_and_receive(bar, CMD_GSP_INIT, payload, GSP_GET_STATIC_INFO_MAX_RESPONSE)?;

    if response.status != 0 {
        return Err(EIO);
    }

    decode_gsp_info(&response.payload)
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
