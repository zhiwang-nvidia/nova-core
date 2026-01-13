// SPDX-License-Identifier: GPL-2.0

use core::{
    convert::Infallible,
    ffi::FromBytesUntilNulError,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    device,
    io::Io,
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
            NoReply, //
        },
        fw::{
            commands::*,
            MsgFunction, //
        },
        nvkv, //
    },
    regs,
    sbuffer::SBufferIter,
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
    type Command = GspSetSystemInfo;
    type Reply = NoReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        GspSetSystemInfo::init(self.pdev, self.chipset)
    }
}

struct RegistryEntry {
    key: &'static str,
    value: u32,
}

/// The `SetRegistry` command.
pub(crate) struct SetRegistry {
    entries: [RegistryEntry; Self::NUM_ENTRIES],
}

impl SetRegistry {
    // For now we hard-code the registry entries. Future work will allow others to
    // be added as module parameters.
    const NUM_ENTRIES: usize = 3;

    /// Creates a new `SetRegistry` command, using a set of hardcoded entries.
    pub(crate) fn new() -> Self {
        Self {
            entries: [
                // RMSecBusResetEnable - enables PCI secondary bus reset
                RegistryEntry {
                    key: "RMSecBusResetEnable",
                    value: 1,
                },
                // RMForcePcieConfigSave - forces GSP-RM to preserve PCI configuration registers on
                // any PCI reset.
                RegistryEntry {
                    key: "RMForcePcieConfigSave",
                    value: 1,
                },
                // RMDevidCheckIgnore - allows GSP-RM to boot even if the PCI dev ID is not found
                // in the internal product name database.
                RegistryEntry {
                    key: "RMDevidCheckIgnore",
                    value: 1,
                },
            ],
        }
    }
}

impl CommandToGsp for SetRegistry {
    const FUNCTION: MsgFunction = MsgFunction::SetRegistry;
    const IS_ASYNC: bool = true;
    type Command = PackedRegistryTable;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        PackedRegistryTable::init(Self::NUM_ENTRIES as u32, self.variable_payload_len() as u32)
    }

    fn variable_payload_len(&self) -> usize {
        let mut key_size = 0;
        for i in 0..Self::NUM_ENTRIES {
            key_size += self.entries[i].key.len() + 1; // +1 for NULL terminator
        }
        Self::NUM_ENTRIES * size_of::<PackedRegistryEntry>() + key_size
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        let string_data_start_offset =
            size_of::<PackedRegistryTable>() + Self::NUM_ENTRIES * size_of::<PackedRegistryEntry>();

        // Array for string data.
        let mut string_data = KVec::new();

        for entry in self.entries.iter().take(Self::NUM_ENTRIES) {
            dst.write_all(
                PackedRegistryEntry::new(
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

/// The reply from the GSP to the `GSP_GET_STATIC_INFO` GMC command.
pub(crate) struct GetGspStaticInfoReply {
    gpu_name: [u8; 64],
    /// Usable FB (VRAM) region for driver memory allocation.
    pub(crate) usable_fb_region: Range<u64>,
    /// End of VRAM.
    pub(crate) total_fb_end: u64,
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
pub(crate) fn get_gsp_info(cmdq: &Cmdq, bar: &Bar0) -> Result<GetGspStaticInfoReply> {
    let response = cmdq.send_gmc_and_receive(
        bar,
        GMC_CMD_GSP_GET_STATIC_INFO,
        &[],
        GSP_GET_STATIC_INFO_MAX_RESPONSE,
    )?;

    if response.status != 0 {
        return Err(EIO);
    }

    let mut gpu_name = [0u8; 64];
    if let Some(name_bytes) =
        nvkv::find_array8(&response.payload, nvkv::gsp_config_key::GPU_NAME_STRING)?
    {
        let len = name_bytes.len().min(gpu_name.len());
        gpu_name[..len].copy_from_slice(&name_bytes[..len]);
    }

    // TODO: Extract usable FB region from NVKV response once the GSP firmware
    // exposes the fbRegionInfoParams key. For now, derive from BAR0 registers.
    let usable_fb_size = bar
        .read(regs::NV_PFB_PRI_MMU_LOCAL_MEMORY_RANGE)
        .usable_fb_size();

    Ok(GetGspStaticInfoReply {
        gpu_name,
        usable_fb_region: 0..usable_fb_size,
        total_fb_end: usable_fb_size,
    })
}
