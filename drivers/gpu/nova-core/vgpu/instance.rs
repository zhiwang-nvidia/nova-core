// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::num::NonZeroUsize;

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SizeConstants, //
};

use crate::{
    driver::Bar0,
    gpu::ChannelIdReservation,
    gsp::{
        cmdq::Cmdq,
        commands::{
            decode_vgpu_properties,
            Dbdf,
            FifoEngineList,
            VgpuProperties, //
        }, //
    },
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
    vgpu::{
        bootload::{
            bootload,
            cleanup,
            shutdown, //
        },
        consts::gmc,
        fw::CommBufferRegion,
        plugin_rpc::{
            PluginConfigParams,
            PluginRpc, //
        },
        vram::{
            VgpuVramLayout,
            VgpuVramSlot,
            VgpuVramSlotAllocator, //
        },
        VgpuManager, //
    }, //
};

/// Guest Function ID. GFID 0 is reserved for the PF; VFs start at 1.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gfid(pub(crate) u32);

/// vGPU type descriptor populated from a typed NVKV properties response.
#[expect(dead_code)]
pub(crate) struct VgpuType {
    name: [u8; 64],
    class: [u8; 64],
    vgpu_type_id: u32,
    bar1_length: u64,
    max_instance: u32,
    ecc_supported: u32,
    profile_size: u64,
    max_fps: u32,
    num_heads: u32,
    max_res_x: u32,
    max_res_y: u32,
    pci_dev_id: u32,
    pci_subsys_id: u32,
    fb_length: u64,
    gsp_heap_size: u64,
    fb_reservation: u64,
}

impl VgpuType {
    pub(crate) const fn vgpu_type_id(&self) -> u32 {
        self.vgpu_type_id
    }

    fn from_properties(properties: &VgpuProperties) -> Self {
        let mut name = [0; 64];
        let name_len = properties.name.len().min(name.len());
        name[..name_len].copy_from_slice(&properties.name[..name_len]);

        let mut class = [0; 64];
        let class_len = properties.class.len().min(class.len());
        class[..class_len].copy_from_slice(&properties.class[..class_len]);

        Self {
            name,
            class,
            vgpu_type_id: properties.type_id,
            bar1_length: properties.bar1_length,
            max_instance: properties.max_instance,
            ecc_supported: properties.ecc,
            profile_size: properties.profile_size,
            max_fps: properties.max_fps,
            num_heads: properties.num_heads,
            max_res_x: properties.max_res_x,
            max_res_y: properties.max_res_y,
            pci_dev_id: properties.dev_id,
            pci_subsys_id: properties.subsystem_id,
            fb_length: properties.fb_length,
            gsp_heap_size: properties.gsp_heap_size,
            fb_reservation: properties.fb_reservation,
        }
    }
}

/// A vGPU instance and the resources reserved for it.
#[expect(dead_code)]
pub(crate) struct VgpuInstance<'gpu> {
    pub(crate) gfid: Gfid,
    pub(crate) dbdf: Dbdf,
    pub(crate) vgpu_type: VgpuType,
    pub(crate) vm_pid: u32,
    pub(crate) chids: ChannelIdReservation<'gpu>,
    pub(crate) num_plugin_channels: u32,
    pub(crate) vram_slot: VgpuVramSlot,
    pub(crate) plugin_rpc: PluginRpc<'gpu>,
}

impl<'gpu> VgpuInstance<'gpu> {
    /// Unmap the plugin communication buffer and return the slot release token.
    fn unmap_and_take_slot(
        self,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
    ) -> Result<VgpuVramSlot> {
        let Self {
            plugin_rpc,
            vram_slot,
            ..
        } = self;
        plugin_rpc.destroy(bar_user, mm)?;
        Ok(vram_slot)
    }
}

/// Identity and firmware profile used to allocate an instance.
pub(crate) struct InstanceInfo {
    pub(crate) gfid: Gfid,
    pub(crate) dbdf: Dbdf,
    pub(crate) vgpu_type: VgpuType,
    pub(crate) vm_pid: u32,
}

#[expect(dead_code)]
impl InstanceInfo {
    pub(crate) const fn new(gfid: Gfid, dbdf: Dbdf, vgpu_type: VgpuType, vm_pid: u32) -> Self {
        Self {
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
        }
    }
}

/// Registry of live vGPU instances.
pub(crate) struct VgpuInstances<'gpu> {
    /// Declared before `vram_slots` so instance regions are dropped before their backing pool.
    instances: KVec<VgpuInstance<'gpu>>,
    vram_slots: Option<VgpuVramSlotAllocator>,
}

#[expect(dead_code)]
impl<'gpu> VgpuInstances<'gpu> {
    pub(crate) const fn new() -> Self {
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

    fn release_vram_slot(&mut self, slot: VgpuVramSlot) {
        let Some(allocator) = self.vram_slots.as_mut() else {
            // A live slot proves that its pool exists. If that invariant is ever broken,
            // leaking the slot is safer than allowing its backing VRAM to be reused.
            core::mem::forget(slot);
            return;
        };
        allocator.release(slot);
    }

    /// Allocate resources, map the management communication region, and
    /// register a new inactive vGPU instance.
    pub(crate) fn allocate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
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
            .ok_or(ENODEV)?
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
            fb_align: vgpu.vmmu_segment_size().ok_or(ENODEV)?,
        };
        let vram_slot = self.alloc_vram_slot(mm, layout)?;
        let comm = match CommBufferRegion::new(bar_user, mm, &vram_slot.mgmt_heap) {
            Ok(comm) => comm,
            Err(error) => {
                // A failed page-table update may have installed a partial mapping without
                // returning a handle that can unmap it. Keep the slot reserved so its backing
                // VRAM cannot be reused while stale BAR1 PTEs may still reference it.
                dev_err!(
                    dev,
                    "allocate_instance: retaining slot {} after BAR1 map error {:?}\n",
                    vram_slot.index(),
                    error,
                );
                return Err(error);
            }
        };

        let instance = VgpuInstance {
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
            chids,
            num_plugin_channels: 3,
            vram_slot,
            plugin_rpc: PluginRpc::new(comm),
        };
        match self.instances.push_within_capacity(instance) {
            Ok(()) => Ok(gfid),
            Err(error) => match error.0.unmap_and_take_slot(bar_user, mm) {
                Ok(vram_slot) => {
                    self.release_vram_slot(vram_slot);
                    Err(EIO)
                }
                Err(error) => Err(error),
            },
        }
    }

    /// Shut down and remove an instance, then release its reservations.
    pub(crate) fn destroy_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
        gfid: Gfid,
    ) -> Result {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;

        shutdown(dev, cmdq, bar, gfid)?;
        cleanup(dev, cmdq, bar, gfid)?;
        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        let vram_slot = instance.unmap_and_take_slot(bar_user, mm)?;
        self.release_vram_slot(vram_slot);
        Ok(())
    }
}

/// Query the vGPU type assigned to a VF by its DBDF.
#[expect(dead_code)]
pub(crate) fn query_assigned_vf_type(cmdq: &Cmdq, bar: Bar0<'_>, dbdf: Dbdf) -> Result<u32> {
    let request = u64::from(dbdf.into_raw()).to_le_bytes();
    let response =
        cmdq.send_gmc_and_receive(bar, gmc::VGPU_MGMT_QUERY_ASSIGNED_VF, &request, 64)?;
    if response.status != 0 {
        return Err(EIO);
    }
    let bytes = response.payload.get(..4).ok_or(ENODEV)?;
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| EINVAL)?))
}

/// Query and decode one vGPU type using the typed NVKV schema.
#[expect(dead_code)]
pub(crate) fn query_vgpu_type(cmdq: &Cmdq, bar: Bar0<'_>, type_id: u32) -> Result<VgpuType> {
    let response = cmdq.send_gmc_and_receive(
        bar,
        gmc::VGPU_MGMT_QUERY_PROPERTIES,
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
    Ok(VgpuType::from_properties(&properties))
}

/// Start the vGPU plugin and establish its RPC channel.
///
/// Ask GSP to create the plugin task, wait for its BAR1 ready marker,
/// initialize the shared RPC buffers, negotiate the protocol, send the
/// instance configuration, and enable bus mastering.
#[expect(dead_code)]
pub(crate) fn activate_instance(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    instance: &mut VgpuInstance<'_>,
    fifo_engine_list: &FifoEngineList,
) -> Result {
    bootload(dev, cmdq, bar, instance, fifo_engine_list)?;

    let params = PluginConfigParams::new(
        [0; 16],
        instance.dbdf,
        instance.vgpu_type.vgpu_type_id,
        instance.vm_pid,
        u32::try_from(instance.chids.len()).map_err(|_| EOVERFLOW)?,
        instance.num_plugin_channels,
    );
    let gfid = instance.gfid;
    let rpc = &mut instance.plugin_rpc;
    rpc.init_rpc()?;
    rpc.negotiate_rpc_version(dev, bar, gfid)?;
    rpc.send_config_params(dev, bar, gfid, &params)?;
    rpc.set_bme(dev, bar, gfid, true)?;

    Ok(())
}
