// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::num::NonZeroUsize;

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SizeConstants, //
};

use crate::gsp::cmdq::Cmdq;

use crate::{
    gpu::ChannelIdReservation,
    mm::GpuMm, //
};

use super::{
    vram::{
        VgpuVramLayout,
        VgpuVramSlot,
        VgpuVramSlotAllocator, //
    },
    VgpuManager, //
};

use super::commands::{
    query_vgpu_properties,
    Dbdf,
    VgpuProperties, //
};

/// Guest Function ID. GFID 0 is reserved for the PF; VFs start at 1.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Gfid(pub(super) u32);

/// Resource requirements and device identity for one vGPU type.
#[expect(dead_code)]
pub(super) struct VgpuType {
    vgpu_type_id: u32,
    bar1_length: u64,
    max_instance: u32,
    pci_dev_id: u32,
    pci_subsys_id: u32,
    fb_length: u64,
    gsp_heap_size: u64,
}

impl VgpuType {
    fn from_properties(properties: &VgpuProperties) -> Self {
        Self {
            vgpu_type_id: properties.type_id,
            bar1_length: properties.bar1_length,
            max_instance: properties.max_instance,
            pci_dev_id: properties.dev_id,
            pci_subsys_id: properties.subsystem_id,
            fb_length: properties.fb_length,
            gsp_heap_size: properties.gsp_heap_size,
        }
    }
}

/// A vGPU instance and the resources reserved for it.
#[expect(dead_code)]
pub(super) struct VgpuInstance<'gpu> {
    pub(super) gfid: Gfid,
    dbdf: Dbdf,
    vgpu_type: VgpuType,
    vm_pid: u32,
    chids: ChannelIdReservation<'gpu>,
    vram_slot: VgpuVramSlot,
}

/// Identity and firmware profile used to allocate an instance.
pub(super) struct InstanceInfo {
    gfid: Gfid,
    dbdf: Dbdf,
    vgpu_type: VgpuType,
    vm_pid: u32,
}

#[expect(dead_code)]
impl InstanceInfo {
    pub(super) const fn new(gfid: Gfid, dbdf: Dbdf, vgpu_type: VgpuType, vm_pid: u32) -> Self {
        Self {
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
        }
    }
}

/// Registry of live vGPU instances.
pub(super) struct VgpuInstances<'gpu> {
    instances: KVec<VgpuInstance<'gpu>>,
    vram_slots: Option<VgpuVramSlotAllocator>,
}

#[expect(dead_code)]
impl<'gpu> VgpuInstances<'gpu> {
    pub(super) const fn new() -> Self {
        Self {
            instances: KVec::new(),
            vram_slots: None,
        }
    }

    fn alloc_vram_slot(&mut self, mm: &GpuMm<'_>, layout: VgpuVramLayout) -> Result<VgpuVramSlot> {
        let replace_empty_pool = match self.vram_slots.as_ref() {
            Some(allocator) if allocator.is_empty() => !allocator.matches_layout(layout)?,
            _ => false,
        };
        if replace_empty_pool {
            self.vram_slots = None;
        }

        if let Some(allocator) = self.vram_slots.as_mut() {
            return allocator.alloc(layout);
        }

        let mut allocator = VgpuVramSlotAllocator::new(mm, layout)?;
        let slot = allocator.alloc(layout)?;
        self.vram_slots = Some(allocator);
        Ok(slot)
    }

    fn release_vram_slot(&mut self, slot: VgpuVramSlot) -> Result {
        self.vram_slots.as_mut().ok_or(EIO)?.release(slot);
        Ok(())
    }

    /// Allocate resources and register a new inactive vGPU instance.
    pub(super) fn allocate_instance(
        &mut self,
        mm: &GpuMm<'_>,
        vgpu: &VgpuManager<'gpu>,
        info: InstanceInfo,
    ) -> Result<Gfid> {
        let InstanceInfo {
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
        } = info;

        if self
            .instances
            .iter()
            .any(|instance| instance.gfid == gfid || instance.dbdf == dbdf)
        {
            return Err(EEXIST);
        }
        let profile_instances = self
            .instances
            .iter()
            .filter(|instance| instance.vgpu_type.vgpu_type_id == vgpu_type.vgpu_type_id)
            .count();
        if vgpu_type.max_instance == 0
            || profile_instances
                >= usize::try_from(vgpu_type.max_instance).map_err(|_| EOVERFLOW)?
        {
            return Err(ENOSPC);
        }
        // Reserve registry capacity before acquiring resources so publishing
        // the completed instance cannot fail due to memory pressure.
        self.instances.reserve(1, GFP_KERNEL)?;

        let num_chid = vgpu
            .total_channels()
            .checked_div(vgpu_type.max_instance)
            .filter(|count| *count != 0)
            .ok_or(EINVAL)?;
        let chids = vgpu.chid_pool.reserve_ids(
            NonZeroUsize::new(usize::try_from(num_chid).map_err(|_| EOVERFLOW)?).ok_or(EINVAL)?,
            Alignment::SZ_1,
        )?;
        let layout = VgpuVramLayout {
            type_id: vgpu_type.vgpu_type_id,
            max_slots: vgpu_type.max_instance,
            fb_size: vgpu_type.fb_length,
            heap_size: vgpu_type.gsp_heap_size,
            fb_align: vgpu.vmmu_segment_size(),
        };
        let vram_slot = self.alloc_vram_slot(mm, layout)?;

        let instance = VgpuInstance {
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
            chids,
            vram_slot,
        };
        match self.instances.push_within_capacity(instance) {
            Ok(()) => Ok(gfid),
            Err(error) => {
                let VgpuInstance { vram_slot, .. } = error.0;
                self.release_vram_slot(vram_slot)?;
                Err(EIO)
            }
        }
    }

    /// Remove an instance and release its channel and VRAM reservations.
    pub(super) fn destroy_instance(&mut self, gfid: Gfid) -> Result {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        let VgpuInstance { vram_slot, .. } = instance;
        self.release_vram_slot(vram_slot)
    }
}

/// Query and decode one vGPU type using the typed NVKV schema.
#[expect(dead_code)]
pub(super) fn query_vgpu_type(cmdq: &Cmdq<'_>, type_id: u32) -> Result<VgpuType> {
    let properties = query_vgpu_properties(cmdq, type_id)?;
    Ok(VgpuType::from_properties(&properties))
}
