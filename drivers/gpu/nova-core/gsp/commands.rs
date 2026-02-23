// SPDX-License-Identifier: GPL-2.0

use core::{
    array,
    convert::Infallible,
    ffi::FromBytesUntilNulError,
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
            commands::*,
            GspVfInfo,
            MsgFunction, //
        },
    },
    sbuffer::SBufferIter,
};

/// The `GspSetSystemInfo` command.
pub(crate) struct SetSystemInfo<'a> {
    pdev: &'a pci::Device<device::Bound>,
    chipset: Chipset,
    vf_info: Option<GspVfInfo>,
}

impl<'a> SetSystemInfo<'a> {
    /// Creates a new `GspSetSystemInfo` command using the parameters of `pdev`.
    pub(crate) fn new(
        pdev: &'a pci::Device<device::Bound>,
        chipset: Chipset,
        vf_info: Option<GspVfInfo>,
    ) -> Self {
        Self {
            pdev,
            chipset,
            vf_info,
        }
    }
}

impl<'a> CommandToGsp for SetSystemInfo<'a> {
    const FUNCTION: MsgFunction = MsgFunction::GspSetSystemInfo;
    const IS_ASYNC: bool = true;
    type Command = GspSetSystemInfo;
    type Reply = NoReply;
    type InitError = Error;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        GspSetSystemInfo::init(self.pdev, self.chipset, self.vf_info.clone())
    }
}

struct RegistryEntry {
    key: &'static str,
    value: u32,
}

/// The `SetRegistry` command.
///
/// Registry entries are built dynamically at runtime based on the current
/// configuration (e.g. whether vGPU is enabled).
pub(crate) struct SetRegistry {
    entries: KVec<RegistryEntry>,
}

impl SetRegistry {
    /// Creates a new `SetRegistry` command.
    ///
    /// The base set of registry entries is always included. Additional entries
    /// are appended dynamically based on runtime conditions (e.g. vGPU).
    pub(crate) fn new(vgpu_requested: bool) -> Result<Self> {
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

        // RMSetSriovMode - required when vGPU is enabled.
        if vgpu_requested {
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
    type Command = PackedRegistryTable;
    type Reply = NoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        PackedRegistryTable::init(
            self.entries.len() as u32,
            self.variable_payload_len() as u32,
        )
    }

    fn variable_payload_len(&self) -> usize {
        let mut key_size = 0;
        for entry in self.entries.iter() {
            key_size += entry.key.len() + 1; // +1 for NULL terminator
        }
        self.entries.len() * size_of::<PackedRegistryEntry>() + key_size
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<core::array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        let string_data_start_offset = size_of::<PackedRegistryTable>()
            + self.entries.len() * size_of::<PackedRegistryEntry>();

        // Array for string data.
        let mut string_data = KVec::new();

        for entry in self.entries.iter() {
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

/// The `GetGspStaticInfo` command.
struct GetGspStaticInfo;

impl CommandToGsp for GetGspStaticInfo {
    const FUNCTION: MsgFunction = MsgFunction::GetGspStaticInfo;
    type Command = GspStaticConfigInfo;
    type Reply = GetGspStaticInfoReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        GspStaticConfigInfo::init_zeroed()
    }
}

/// The reply from the GSP to the [`GetGspInfo`] command.
pub(crate) struct GetGspStaticInfoReply {
    gpu_name: [u8; 64],
    h_client: u32,
    h_subdevice: u32,
    bar1_pde_base: u64,
    /// First usable FB region `(base, size)` for memory allocation.
    usable_fb_region: Option<(u64, u64)>,
}

impl MessageFromGsp for GetGspStaticInfoReply {
    const FUNCTION: MsgFunction = MsgFunction::GetGspStaticInfo;
    type Message = GspStaticConfigInfo;
    type InitError = Infallible;

    fn read(
        msg: &Self::Message,
        _sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(GetGspStaticInfoReply {
            gpu_name: msg.gpu_name_str(),
            h_client: msg.h_internal_client(),
            h_subdevice: msg.h_internal_subdevice(),
            bar1_pde_base: msg.bar1_pde_base(),
            usable_fb_region: msg.first_usable_fb_region(),
        })
    }
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

    /// Returns the internal client handle allocated by GSP-RM.
    pub(crate) fn h_client(&self) -> u32 {
        self.h_client
    }

    /// Returns the internal subdevice handle allocated by GSP-RM.
    pub(crate) fn h_subdevice(&self) -> u32 {
        self.h_subdevice
    }

    /// Returns the BAR1 Page Directory Entry base address.
    #[expect(dead_code)]
    pub(crate) fn bar1_pde_base(&self) -> u64 {
        self.bar1_pde_base
    }

    /// Returns the usable FB region `(base, size)` for driver allocation which is
    /// already retrieved from the GSP.
    pub(crate) fn usable_fb_region(&self) -> Option<(u64, u64)> {
        self.usable_fb_region
    }
}

/// Send the [`GetGspInfo`] command and awaits for its reply.
pub(crate) fn get_gsp_info(cmdq: &Cmdq, bar: &Bar0) -> Result<GetGspStaticInfoReply> {
    cmdq.send_command(bar, GetGspStaticInfo)
}
