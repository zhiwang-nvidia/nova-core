// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    pci,
    prelude::*, //
};

use crate::{
    gpu::Chipset,
    module_parameters, //
};

pub(crate) struct Vgpu {
    pub(crate) vgpu_requested: bool,
    pub(crate) vgpu_enabled: bool,
    pub(crate) total_vfs: u16,
}

impl Vgpu {
    pub(crate) fn new(pdev: &pci::Device<device::Bound>, chipset: Chipset) -> Result<Vgpu> {
        let total_vfs = if chipset.arch().supports_vgpu() {
            match *module_parameters::vgpu_support.value() {
                0 => 0,
                _ => pdev.sriov_get_totalvfs().unwrap_or(0),
            }
        } else {
            0
        };

        Ok(Vgpu {
            vgpu_requested: total_vfs > 0,
            vgpu_enabled: false,
            total_vfs,
        })
    }
}
