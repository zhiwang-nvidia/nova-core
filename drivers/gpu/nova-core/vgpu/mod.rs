// SPDX-License-Identifier: GPL-2.0

use core::num::NonZero;

use kernel::{
    device,
    pci,
    prelude::*, //
};

use crate::{
    fsp::{
        Fsp,
        VgpuMode, //
    },
    gpu::{
        ChannelIdPool,
        Chipset, //
    },
    gsp::commands::NVGMC_ENGINE_TYPE_COUNT, //
};

mod fw;
mod hal;
mod vram;

/// vGPU state detected during GPU construction.
#[derive(Debug, Clone, Copy)]
pub(crate) enum VgpuState {
    /// vGPU mode is not enabled for this boot.
    Disabled,
    /// vGPU mode is enabled for this boot.
    Enabled {
        /// Total number of SR-IOV VFs supported by this device.
        total_vfs: NonZero<u16>,
    },
}

/// vGPU state manager.
pub(crate) struct VgpuManager<'gpu> {
    /// Channel ID pool the per-VF areas are reserved from.
    #[expect(dead_code)]
    pub(crate) chid_pool: &'gpu ChannelIdPool,
    state: VgpuState,
    vmmu_segment_size: Option<u64>,
    total_channels: Option<u32>,
    engine_masks: Option<[u64; NVGMC_ENGINE_TYPE_COUNT]>,
}

impl<'gpu> VgpuManager<'gpu> {
    /// Creates an empty vGPU manager for initialization during GPU construction.
    pub(crate) const fn new(chid_pool: &'gpu ChannelIdPool) -> Self {
        Self {
            chid_pool,
            state: VgpuState::Disabled,
            vmmu_segment_size: None,
            total_channels: None,
            engine_masks: None,
        }
    }

    /// Detects and stores vGPU state before GSP boot.
    pub(crate) fn detect_state(
        &mut self,
        pdev: &pci::Device<device::Core<'_>>,
        chipset: Chipset,
        fsp: Option<&mut Fsp<'_>>,
    ) {
        let state: Result<VgpuState> = (|| {
            if !hal::vgpu_hal(chipset).supports_vgpu() {
                return Ok(VgpuState::Disabled);
            }

            let Some(total_vfs) = pdev.sriov_get_totalvfs() else {
                return Ok(VgpuState::Disabled);
            };

            if total_vfs.get() < 2 {
                // The current vGPU path does not support single-VF SR-IOV devices yet.
                // Treat one total VF as vGPU-disabled for now; single-VF support can relax
                // this gate once the manager handles that topology.
                return Ok(VgpuState::Disabled);
            }

            let fsp = fsp.ok_or(ENODEV)?;

            match fsp.read_vgpu_mode(pdev.as_ref())? {
                VgpuMode::Enabled => Ok(VgpuState::Enabled { total_vfs }),
                VgpuMode::Disabled => Ok(VgpuState::Disabled),
            }
        })();

        self.state = state.unwrap_or_else(|e| {
            dev_warn!(
                pdev,
                "vGPU state detection failed: {:?}; disabling vGPU\n",
                e
            );
            VgpuState::Disabled
        });
        dev_dbg!(pdev, "vGPU state: {:?}\n", self.state);
    }

    /// Returns the detected vGPU state for this boot.
    pub(crate) fn state(&self) -> VgpuState {
        self.state
    }

    /// Initializes the runtime parameters returned by GSP_INIT.
    pub(crate) fn init(
        &mut self,
        gmc_engine_masks: &[u64; NVGMC_ENGINE_TYPE_COUNT],
        vmmu_segment_size: u64,
        total_channels: u32,
    ) {
        if matches!(self.state, VgpuState::Enabled { .. }) {
            self.vmmu_segment_size = Some(vmmu_segment_size);
            self.total_channels = Some(total_channels);
            self.engine_masks = Some(*gmc_engine_masks);
        }
    }

    /// Returns the firmware-reported VMMU segment size when vGPU is enabled.
    pub(crate) const fn vmmu_segment_size(&self) -> Option<u64> {
        self.vmmu_segment_size
    }

    /// Returns the number of channel IDs available to vGPU instances.
    #[expect(dead_code)]
    pub(crate) const fn total_channels(&self) -> Option<u32> {
        self.total_channels
    }

    /// Returns the available engine-instance masks.
    #[expect(dead_code)]
    pub(crate) fn engine_masks(&self) -> Result<&[u64; NVGMC_ENGINE_TYPE_COUNT]> {
        self.engine_masks.as_ref().ok_or(ENODEV)
    }
}
