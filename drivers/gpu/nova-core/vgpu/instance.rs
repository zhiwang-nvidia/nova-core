// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::num::NonZeroUsize;

use kernel::{
    debugfs,
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SizeConstants,
    str::CString,
    sync::Mutex, //
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
    firmware::BuildId,
    gpu::{
        ChannelIdReservation,
        Chipset, //
    },
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
};

use super::{
    gsp_plugin_comm::{
        CommBufferRegion,
        MappedPluginLogBuffers, //
    },
    gsp_plugin_rpc::PluginRpc,
    log::VgpuLogBuffers,
    scrubber::CeUtils,
    vram::{
        VgpuVramLayout,
        VgpuVramSlot,
        VgpuVramSlotAllocator, //
    },
    VgpuManager, //
};

use super::commands::{
    free_ceutils,
    negotiate_plugin_version,
    query_vgpu_properties,
    reset_plugin,
    send_bootload,
    send_cleanup,
    send_plugin_config,
    send_shutdown,
    set_plugin_bme,
    CeUtilsAllocError,
    Dbdf,
    VgpuProperties, //
};

use super::fw::commands::{
    encode_plugin_config_params,
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

    pub(super) const fn bar1_length(&self) -> u64 {
        self.bar1_length
    }

    pub(super) const fn pci_dev_id(&self) -> u32 {
        self.pci_dev_id
    }

    pub(super) const fn pci_subsys_id(&self) -> u32 {
        self.pci_subsys_id
    }

    pub(super) const fn fb_length(&self) -> u64 {
        self.fb_length
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
    debugfs_logs: Option<Pin<KBox<debugfs::Scope<VgpuLogBuffers<'gpu>>>>>,
    vram_slot: VgpuVramSlot,
    pub(super) plugin_rpc: PluginRpc<'gpu, 'gpu>,
    ceutils: Option<CeUtils>,
    initialized: bool,
    active: bool,
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

    fn configure_plugin(&mut self, dev: &device::Device<device::Bound>, bar0: Bar0<'_>) -> Result {
        let config = encode_plugin_config_params(
            [0; 16],
            self.dbdf,
            self.vgpu_type.vgpu_type_id(),
            self.vm_pid,
            u32::try_from(self.chids.len().checked_sub(1).ok_or(EINVAL)?).map_err(|_| EOVERFLOW)?,
            self.num_plugin_channels,
        )?;

        send_plugin_config(dev, bar0, self.gfid, &mut self.plugin_rpc, &config)
    }

    /// Stop the plugin when firmware may own instance resources.
    fn shutdown(&mut self, dev: &device::Device<device::Bound>, cmdq: &Cmdq<'_>) -> Result {
        if self.needs_teardown {
            send_shutdown(dev, cmdq, self.gfid)?;
            dev_dbg!(dev, "shutdown: gfid={} stopped\n", self.gfid.0);
        }
        self.active = false;
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

fn create_debugfs_logs<'gpu>(
    buffers: MappedPluginLogBuffers<'gpu>,
    dbdf: Dbdf,
    chipset: Chipset,
    build_id: Option<&BuildId>,
) -> Result<Pin<KBox<debugfs::Scope<VgpuLogBuffers<'gpu>>>>> {
    let logs = VgpuLogBuffers::new(buffers, chipset, build_id);
    let raw_dbdf = dbdf.into_raw();
    let domain = raw_dbdf >> 16;
    let bus = (raw_dbdf >> 8) & 0xff;
    let device = (raw_dbdf >> 3) & 0x1f;
    let function = raw_dbdf & 0x07;
    let directory = CString::try_from_fmt(fmt!(
        "{:04x}:{:02x}:{:02x}.{:x}-vgpu",
        domain,
        bus,
        device,
        function,
    ))?;

    #[allow(static_mut_refs)]
    // SAFETY: The root is initialized before driver registration and cleared
    // only after driver unregistration has drained all users.
    let root = unsafe { crate::DEBUGFS_ROOT.as_ref() }.ok_or(ENODEV)?;

    KBox::pin_init(
        root.scope(logs, &directory, |logs, directory| {
            VgpuLogBuffers::register_debugfs(logs, directory);
        }),
        GFP_KERNEL,
    )
}

/// Registry of live vGPU instances.
pub(super) struct VgpuInstances<'gpu> {
    instances: KVec<VgpuInstance<'gpu>>,
    vram_slots: Option<VgpuVramSlotAllocator>,
}

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
            debugfs_logs,
            plugin_rpc,
            vram_slot,
            ..
        } = instance;
        drop(debugfs_logs);
        let result = plugin_rpc.destroy(mm);
        self.release_vram_slot(vram_slot)?;
        result
    }

    /// Allocate resources, scrub the framebuffer and register an inactive instance.
    pub(super) fn allocate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
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
            .filter(|count| *count > 1)
            .ok_or(EINVAL)?;
        let chids = vgpu.chid_pool.reserve_ids(
            NonZeroUsize::new(usize::try_from(num_chid).map_err(|_| EOVERFLOW)?).ok_or(EINVAL)?,
            Alignment::SZ_1,
        )?;
        let ceutils_chid =
            u32::try_from(chids.end.checked_sub(1).ok_or(EINVAL)?).map_err(|_| EOVERFLOW)?;
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
            debugfs_logs: None,
            vram_slot,
            plugin_rpc: PluginRpc::new(comm),
            ceutils: None,
            initialized: false,
            active: false,
            needs_teardown: false,
        };
        // Register ownership before firmware work so an uncertain result leaves
        // the reservations reachable and unavailable to another instance.
        let index = self.instances.len();
        if let Err(error) = self.instances.push_within_capacity(instance) {
            self.release_instance(error.0, mm)?;
            return Err(EIO);
        }

        let ceutils = match CeUtils::allocate(dev, cmdq, gfid, ceutils_chid) {
            Ok(ceutils) => ceutils,
            Err(error) => {
                let error = match error {
                    CeUtilsAllocError::NotOwned(error) => error,
                    CeUtilsAllocError::MayOwn(error) => {
                        dev_err!(dev, "CeUtils allocation failed: {:?}\n", error);
                        return Err(error);
                    }
                };
                let instance = self.instances.remove(index).map_err(|_| EIO)?;
                self.release_instance(instance, mm)?;
                return Err(error);
            }
        };
        let instance = self.instances.get_mut(index).ok_or(EIO)?;
        let result = ceutils.scrub_guest_fb(dev, cmdq, bar_user, mm, &instance.vram_slot.fbmem);
        instance.ceutils = Some(ceutils);
        result?;
        instance.initialized = true;
        Ok(gfid)
    }

    /// Boot and configure the GSP plugin for a registered instance.
    pub(super) fn activate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        vgpu: &VgpuManager<'gpu>,
    ) -> Result {
        let instance = self
            .instances
            .iter_mut()
            .find(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        if !instance.initialized {
            return Err(EINVAL);
        }
        instance.bootload(dev, cmdq, vgpu.fifo_engine_list())?;

        instance.plugin_rpc.init_rpc()?;
        negotiate_plugin_version(dev, bar0, gfid, &mut instance.plugin_rpc)?;
        instance.configure_plugin(dev, bar0)?;
        set_plugin_bme(dev, bar0, gfid, &mut instance.plugin_rpc, true)?;

        if instance.debugfs_logs.is_none() {
            match instance
                .plugin_rpc
                .comm()
                .mapped_plugin_logs()
                .and_then(|buffers| {
                    create_debugfs_logs(
                        buffers,
                        instance.dbdf,
                        vgpu.chipset,
                        vgpu.build_id.as_ref(),
                    )
                }) {
                Ok(logs) => instance.debugfs_logs = Some(logs),
                Err(error) => dev_warn!(
                    dev,
                    "debugfs logs unavailable for gfid={}: {:?}\n",
                    gfid.0,
                    error,
                ),
            }
        }

        Ok(())
    }

    /// Reset an active instance and scrub its guest VRAM.
    pub(super) fn reset_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
        gfid: Gfid,
    ) -> Result {
        let instance = self
            .instances
            .iter_mut()
            .find(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        if !instance.active {
            return Err(EBUSY);
        }

        reset_plugin(dev, bar, instance.gfid, &mut instance.plugin_rpc)?;
        instance.ceutils.as_ref().ok_or(EINVAL)?.scrub_guest_fb(
            dev,
            cmdq,
            bar_user,
            mm,
            &instance.vram_slot.fbmem,
        )
    }

    /// Stop the plugin and release the instance's firmware and host resources.
    pub(super) fn destroy_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar_user: &BarUser<'_>,
        mm: &mut GpuMm<'_>,
        gfid: Gfid,
    ) -> Result {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;
        let instance = self.instances.get_mut(index).ok_or(EIO)?;
        instance.initialized = false;
        instance.shutdown(dev, cmdq)?;
        if let Some(ceutils) = instance.ceutils.as_ref() {
            ceutils.scrub_guest_fb(dev, cmdq, bar_user, mm, &instance.vram_slot.fbmem)?;
        }
        instance.ceutils = None;
        free_ceutils(dev, cmdq, gfid)?;
        if instance.needs_teardown {
            send_cleanup(dev, cmdq, gfid)?;
        }
        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        self.release_instance(instance, mm)
    }
}

/// Query and decode one vGPU type using the typed NVKV schema.
pub(super) fn query_vgpu_type(cmdq: &Cmdq<'_>, type_id: u32) -> Result<VgpuType> {
    let properties = query_vgpu_properties(cmdq, type_id)?;
    Ok(VgpuType::from_properties(&properties))
}

/// Activate an instance already owned by the live-instance registry.
///
/// If activation fails, attempt full teardown before returning the original
/// error.
#[expect(clippy::too_many_arguments)]
fn activate_registered_instance<'gpu>(
    instances: &mut VgpuInstances<'gpu>,
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'_>,
    bar: Bar0<'_>,
    bar_user: &BarUser<'gpu>,
    mm: &mut GpuMm<'_>,
    gfid: Gfid,
    vgpu: &VgpuManager<'gpu>,
) -> Result {
    let index = instances
        .instances
        .iter()
        .position(|instance| instance.gfid == gfid)
        .ok_or(EIO)?;
    let activation_result = instances.activate_instance(dev, cmdq, bar, gfid, vgpu);

    if let Err(original_error) = activation_result {
        if let Err(cleanup_error) = instances.destroy_instance(dev, cmdq, bar_user, mm, gfid) {
            dev_err!(
                dev,
                "vgpu_open: cleanup failed for gfid={} after activation error {:?}: {:?}\n",
                gfid.0,
                original_error,
                cleanup_error,
            );
        }
        return Err(original_error);
    }

    instances.instances[index].active = true;
    Ok(())
}

impl<'gpu> VgpuManager<'gpu> {
    /// Allocate, register, and activate a vGPU instance.
    ///
    /// Keep the registry locked from allocation through activation or rollback
    /// so duplicate checks and vGPU type limits remain stable.
    pub(super) fn create_instance(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar: Bar0<'_>,
        bar_user: &'gpu BarUser<'gpu>,
        mm: &Mutex<GpuMm<'gpu>>,
        info: InstanceInfo,
    ) -> Result {
        let mut instances = self.instances().lock();
        // Global vGPU lock order: instances -> MM -> BAR-user VMM.
        let mut mm = mm.lock();
        let gfid = instances.allocate_instance(dev, cmdq, bar_user, &mut mm, self, info)?;

        activate_registered_instance(
            &mut instances,
            dev,
            cmdq,
            bar,
            bar_user,
            &mut mm,
            gfid,
            self,
        )
    }
    pub(super) fn close_instance(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &Mutex<GpuMm<'gpu>>,
        gfid: Gfid,
    ) -> Result {
        let mut instances = self.instances().lock();
        let mut mm = mm.lock();
        instances.destroy_instance(dev, cmdq, bar_user, &mut mm, gfid)
    }

    pub(super) fn reset_instance(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'_>,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &Mutex<GpuMm<'gpu>>,
        gfid: Gfid,
    ) -> Result {
        let mut instances = self.instances().lock();
        let mut mm = mm.lock();
        instances.reset_instance(dev, cmdq, bar, bar_user, &mut mm, gfid)
    }
}
