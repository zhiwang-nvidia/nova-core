// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::num::NonZeroUsize;

use kernel::{
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SizeConstants, //
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
};

use crate::gsp::{
    cmdq::Cmdq,
    commands::FifoEngineList, //
};

use crate::{
    driver::Bar0,
    gpu::ChannelIdReservation,
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
};

use super::{
    gsp_plugin_comm::CommBufferRegion,
    gsp_plugin_rpc::PluginRpc,
    vram::{
        VgpuVramLayout,
        VgpuVramSlot,
        VgpuVramSlotAllocator, //
    },
    VgpuManager, //
};

use super::commands::{
    negotiate_plugin_version,
    query_vgpu_properties,
    send_bootload,
    send_cleanup,
    send_shutdown,
    Dbdf,
    VgpuProperties, //
};

use super::fw::commands::{
    encode_vgpu_bootload,
    ChannelMapEntry, //
};

/// Ready limit used by `vmiopd_negotiate_cpu_gsp_version()` for the same marker.
const PLUGIN_READY_TIMEOUT: Delta = Delta::from_secs(10);

/// Build the typed channel mapping from the GSP FIFO engine list.
fn channel_mapping(
    fifo_engine_list: &FifoEngineList,
    chid_offset: u32,
) -> Result<KVVec<ChannelMapEntry>> {
    let mut mapping = KVVec::new();
    for &gmc_id in fifo_engine_list.gmc_ids() {
        let engine_type = (gmc_id & 0xffff) as usize;
        let index = gmc_id >> 16;
        mapping.push(
            ChannelMapEntry::new(engine_type, index, chid_offset)?,
            GFP_KERNEL,
        )?;
    }
    Ok(mapping)
}

fn wait_plugin_ready(
    dev: &device::Device<device::Bound>,
    comm: &CommBufferRegion<'_, '_>,
) -> Result {
    let start = Instant::<Monotonic>::now();

    loop {
        if comm.is_plugin_ready()? {
            dev_dbg!(dev, "vGPU plugin ready after {:?}\n", start.elapsed());
            return Ok(());
        }
        if start.elapsed() >= PLUGIN_READY_TIMEOUT {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
}

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
    pub(super) const fn vgpu_type_id(&self) -> u32 {
        self.vgpu_type_id
    }

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
pub(super) struct VgpuInstance<'gpu> {
    pub(super) gfid: Gfid,
    dbdf: Dbdf,
    vgpu_type: VgpuType,
    vm_pid: u32,
    chids: ChannelIdReservation<'gpu>,
    num_plugin_channels: u32,
    vram_slot: VgpuVramSlot,
    pub(super) plugin_rpc: PluginRpc<'gpu, 'gpu>,
    needs_teardown: bool,
}

impl VgpuInstance<'_> {
    /// Bootload the GSP vGPU plugin and wait for its BAR1 ready indication.
    fn bootload(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        fifo_engine_list: &FifoEngineList,
    ) -> Result {
        let fb = &self.vram_slot.fbmem;
        let mgmt = &self.vram_slot.mgmt_heap;
        let logs = self.plugin_rpc.comm().plugin_logs();

        let payload = encode_vgpu_bootload(
            self.dbdf,
            self.gfid.0,
            self.vgpu_type.vgpu_type_id(),
            self.vm_pid,
            u32::try_from(self.chids.len()).map_err(|_| EOVERFLOW)?,
            self.num_plugin_channels,
            channel_mapping(
                fifo_engine_list,
                u32::try_from(self.chids.start).map_err(|_| EOVERFLOW)?,
            )?,
            fb.address(),
            fb.size(),
            mgmt.address(),
            mgmt.size(),
            0,
            logs.init().address(),
            logs.init().size(),
            logs.vgpu().address(),
            logs.vgpu().size(),
            logs.kernel().address(),
            logs.kernel().size(),
        )?;

        dev_dbg!(
            dev,
            "bootload: gfid={} sending {} typed NVKV bytes\n",
            self.gfid.0,
            payload.len() * size_of::<u64>(),
        );

        self.plugin_rpc.comm().clear_plugin_ready()?;
        self.needs_teardown = true;
        send_bootload(cmdq, &payload)?;

        wait_plugin_ready(dev, self.plugin_rpc.comm())?;

        dev_dbg!(dev, "bootload: gfid={} plugin ready\n", self.gfid.0);
        Ok(())
    }

    /// Stop the plugin when firmware may own instance resources.
    fn shutdown(&mut self, dev: &device::Device<device::Bound>, cmdq: &Cmdq<'_>) -> Result {
        if self.needs_teardown {
            send_shutdown(dev, cmdq, self.gfid)?;
            dev_dbg!(dev, "shutdown: gfid={} stopped\n", self.gfid.0);
        }
        Ok(())
    }
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

    fn release_instance(&mut self, instance: VgpuInstance<'gpu>, mm: &mut GpuMm<'_>) -> Result {
        let VgpuInstance {
            plugin_rpc,
            vram_slot,
            ..
        } = instance;
        let result = plugin_rpc.destroy(mm);
        self.release_vram_slot(vram_slot)?;
        result
    }

    /// Allocate resources and register a new inactive vGPU instance.
    pub(super) fn allocate_instance(
        &mut self,
        bar_user: &'gpu BarUser<'gpu>,
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
        let comm = match CommBufferRegion::new(bar_user, mm, &vram_slot.mgmt_heap) {
            Ok(comm) => comm,
            Err(error) => {
                self.release_vram_slot(vram_slot)?;
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
            needs_teardown: false,
        };
        match self.instances.push_within_capacity(instance) {
            Ok(()) => Ok(gfid),
            Err(error) => {
                self.release_instance(error.0, mm)?;
                Err(EIO)
            }
        }
    }

    /// Boot the GSP plugin and negotiate its RPC version.
    pub(super) fn activate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        fifo_engine_list: &FifoEngineList,
    ) -> Result {
        let instance = self
            .instances
            .iter_mut()
            .find(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        instance.bootload(dev, cmdq, fifo_engine_list)?;

        instance.plugin_rpc.init_rpc()?;
        negotiate_plugin_version(dev, bar0, gfid, &mut instance.plugin_rpc)
    }

    /// Stop the plugin and release the instance's firmware and host resources.
    pub(super) fn destroy_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        mm: &mut GpuMm<'_>,
        gfid: Gfid,
    ) -> Result {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        let instance = &mut self.instances[index];
        instance.shutdown(dev, cmdq)?;
        if instance.needs_teardown {
            send_cleanup(dev, cmdq, gfid)?;
        }
        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        self.release_instance(instance, mm)
    }
}

/// Query and decode one vGPU type using the typed NVKV schema.
#[expect(dead_code)]
pub(super) fn query_vgpu_type(cmdq: &Cmdq<'_>, type_id: u32) -> Result<VgpuType> {
    let properties = query_vgpu_properties(cmdq, type_id)?;
    Ok(VgpuType::from_properties(&properties))
}
