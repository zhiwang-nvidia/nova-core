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
};

use crate::{
    driver::Bar0,
    firmware::BuildId,
    gpu::{
        ChannelIdReservation,
        Chipset, //
    },
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
        fw::{
            CommBufferRegion,
            MappedPluginLogBuffers, //
            RpcMessage,
        },
        log::VgpuLogBuffers,
        plugin_rpc::{
            PluginConfigParams,
            PluginRpc, //
        },
        scrubber::{
            CeUtils,
            CeUtilsAllocError, //
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

    pub(crate) const fn bar1_length(&self) -> u64 {
        self.bar1_length
    }

    pub(crate) const fn pci_dev_id(&self) -> u32 {
        self.pci_dev_id
    }

    pub(crate) const fn pci_subsys_id(&self) -> u32 {
        self.pci_subsys_id
    }

    pub(crate) const fn fb_length(&self) -> u64 {
        self.fb_length
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

/// A live vGPU instance with allocated resources.
///
/// Field ordering is load-bearing for drop: `debugfs_logs` must be declared
/// before `plugin_rpc` so that debugfs entries are removed (and in-progress
/// readers drained) before the underlying `Bar1Map` is destroyed.
pub(crate) struct VgpuInstance<'gpu> {
    pub(crate) gfid: Gfid,
    pub(crate) dbdf: Dbdf,
    pub(crate) vgpu_type: VgpuType,
    pub(crate) vm_pid: u32,
    pub(crate) chids: ChannelIdReservation<'gpu>,
    pub(crate) num_plugin_channels: u32,
    ceutils: CeUtils,
    pub(crate) vram_slot: VgpuVramSlot,
    debugfs_logs: Option<Pin<KBox<debugfs::Scope<VgpuLogBuffers>>>>,
    pub(crate) plugin_rpc: PluginRpc<'gpu>,
    active: bool,
}

impl<'gpu> VgpuInstance<'gpu> {
    /// Request the idempotent firmware release of this instance's CeUtils.
    fn release_ceutils(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
    ) -> Result {
        self.ceutils.release(dev, cmdq, bar)
    }

    /// Unmap the plugin communication buffer and return the slot release token.
    fn unmap_and_take_slot(
        self,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
    ) -> Result<VgpuVramSlot> {
        let Self {
            debugfs_logs,
            plugin_rpc,
            vram_slot,
            ..
        } = self;
        // Remove the files and drain active readers before tearing down the
        // BAR1 mapping that backs them.
        drop(debugfs_logs);
        plugin_rpc.destroy(bar_user, mm)?;
        Ok(vram_slot)
    }

    /// Scrub the instance framebuffer with its owned CeUtils allocation.
    pub(crate) fn scrub_guest_fb(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
    ) -> Result {
        self.ceutils
            .scrub_guest_fb(dev, cmdq, bar, bar_user, mm, &self.vram_slot.fbmem)
    }
}

/// Keep channel IDs unavailable when firmware ownership cannot be determined.
fn quarantine_channel_ids(chids: ChannelIdReservation<'_>) {
    // A failed GMC response cannot distinguish a command that was never
    // executed from one whose reply was lost, and there is no ownership query
    // for CeUtils. Running the reservation's destructor could therefore let a
    // second owner reuse a firmware-owned CHID. Skipping it leaves those bits
    // reserved for the remaining lifetime of the device's channel-ID pool.
    core::mem::forget(chids);
}

/// Keep an invariant-violating slot and its backing VRAM unavailable for reuse.
fn quarantine_vram_slot(slot: VgpuVramSlot) {
    // A live slot without its allocator should be impossible. If it happens,
    // stale BAR1 mappings may still refer to this VRAM. There is no recovery
    // path without the allocator, so permanently retaining the backing
    // allocation is safer than exposing it again.
    core::mem::forget(slot);
}

/// Preserve every guard when publishing a fully built instance unexpectedly fails.
fn quarantine_instance(instance: VgpuInstance<'_>) {
    // allocate_instance() reserves registry capacity before acquiring any
    // resource, so this is an invariant-failure fallback. If firmware release
    // is also unconfirmed, retaining the complete instance prevents its CHID,
    // BAR1 mapping, and VRAM from being independently reused.
    core::mem::forget(instance);
}

/// Identity and firmware profile used to allocate an instance.
pub(crate) struct InstanceInfo {
    pub(crate) gfid: Gfid,
    pub(crate) dbdf: Dbdf,
    pub(crate) vgpu_type: VgpuType,
    pub(crate) vm_pid: u32,
}

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

fn create_debugfs_logs(
    buffers: MappedPluginLogBuffers,
    dbdf: Dbdf,
    chipset: Chipset,
    build_id: Option<&BuildId>,
) -> Result<Pin<KBox<debugfs::Scope<VgpuLogBuffers>>>> {
    let logs = VgpuLogBuffers::new(buffers, chipset, build_id)?;
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
pub(crate) struct VgpuInstances<'gpu> {
    /// Declared before `vram_slots` so instance regions are dropped before their backing pool.
    instances: KVec<VgpuInstance<'gpu>>,
    vram_slots: Option<VgpuVramSlotAllocator>,
}

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
            quarantine_vram_slot(slot);
            return;
        };
        allocator.release(slot);
    }

    /// Allocate resources, map the management communication region, and
    /// register a new inactive vGPU instance.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn allocate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
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
            fb_align: vgpu.vmmu_segment_size().ok_or(ENODEV)?,
        };
        let vram_slot = self.alloc_vram_slot(mm, layout)?;
        let ceutils = match CeUtils::allocate(dev, cmdq, bar, gfid, ceutils_chid, 0) {
            Ok(ceutils) => ceutils,
            Err(alloc_error) => {
                let error = match alloc_error {
                    CeUtilsAllocError::NotOwned(error) => error,
                    CeUtilsAllocError::MayOwn(error) => {
                        if let Err(release_error) = CeUtils::release_gfid(dev, cmdq, bar, gfid) {
                            dev_err!(
                                dev,
                                "CeUtils alloc {:?}; firmware release unconfirmed: {:?}\n",
                                error,
                                release_error,
                            );
                            quarantine_channel_ids(chids);
                        }
                        error
                    }
                };

                // CeUtils allocation never receives the FB address, so the slot is safe to
                // recycle once its local regions have been dropped.
                self.release_vram_slot(vram_slot);
                return Err(error);
            }
        };
        if let Err(error) = ceutils.scrub_guest_fb(dev, cmdq, bar, bar_user, mm, &vram_slot.fbmem) {
            // This error may be an unmap failure, or firmware may still be scrubbing. Keep the
            // channel reservation and slot out of their allocators in either case.
            quarantine_channel_ids(chids);
            dev_err!(
                dev,
                "retaining CeUtils and VRAM slot {} after scrub error {:?}\n",
                vram_slot.index(),
                error,
            );
            return Err(error);
        }
        let comm = match CommBufferRegion::new(bar_user, mm, &vram_slot.mgmt_heap) {
            Ok(comm) => comm,
            Err(error) => {
                // A failed page-table update may have installed a partial mapping without
                // returning a handle that can unmap it. Keep the slot reserved so its backing
                // VRAM cannot be reused while stale BAR1 PTEs may still reference it.
                if let Err(release_error) = ceutils.release(dev, cmdq, bar) {
                    dev_err!(
                        dev,
                        "BAR1 error {:?}; CeUtils release unconfirmed: {:?}\n",
                        error,
                        release_error,
                    );
                    quarantine_channel_ids(chids);
                }
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
            ceutils,
            vram_slot,
            debugfs_logs: None,
            plugin_rpc: PluginRpc::new(comm),
            active: false,
        };
        match self.instances.push_within_capacity(instance) {
            Ok(()) => Ok(gfid),
            Err(error) => {
                let instance = error.0;
                if let Err(error) = instance.release_ceutils(dev, cmdq, bar) {
                    // Firmware may still own the final CHID. Keep every host resource
                    // quarantined rather than returning any of them to an allocator.
                    quarantine_instance(instance);
                    return Err(error);
                }
                match instance.unmap_and_take_slot(bar_user, mm) {
                    Ok(vram_slot) => {
                        self.release_vram_slot(vram_slot);
                        Err(EIO)
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    /// Reset an active instance and scrub its guest framebuffer.
    pub(crate) fn reset_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
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

        instance
            .plugin_rpc
            .rpc_call(dev, bar, gfid, RpcMessage::Reset, &[])?;
        instance.scrub_guest_fb(dev, cmdq, bar, bar_user, mm)
    }

    /// Shut down an instance, scrub its guest FB, and release its reservations.
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
        self.instances[index].active = false;
        self.instances[index].scrub_guest_fb(dev, cmdq, bar, bar_user, mm)?;
        self.instances[index].release_ceutils(dev, cmdq, bar)?;
        cleanup(dev, cmdq, bar, gfid)?;
        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        let vram_slot = instance.unmap_and_take_slot(bar_user, mm)?;
        self.release_vram_slot(vram_slot);
        Ok(())
    }
}

/// Query the vGPU type assigned to a VF by its DBDF.
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
fn activate_instance(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    instance: &mut VgpuInstance<'_>,
    fifo_engine_list: &FifoEngineList,
    chipset: Chipset,
    build_id: Option<&BuildId>,
) -> Result {
    bootload(dev, cmdq, bar, instance, fifo_engine_list)?;

    let params = PluginConfigParams::new(
        [0; 16],
        instance.dbdf,
        instance.vgpu_type.vgpu_type_id,
        instance.vm_pid,
        u32::try_from(instance.chids.len().checked_sub(1).ok_or(EINVAL)?).map_err(|_| EOVERFLOW)?,
        instance.num_plugin_channels,
    );
    let gfid = instance.gfid;
    let rpc = &mut instance.plugin_rpc;
    rpc.init_rpc()?;
    rpc.negotiate_rpc_version(dev, bar, gfid)?;
    rpc.send_config_params(dev, bar, gfid, &params)?;
    rpc.set_bme(dev, bar, gfid, true)?;

    // Publish the log files only after the plugin has initialized its
    // management heap. Debugfs is diagnostic, so failure must not undo an
    // otherwise usable vGPU instance.
    match rpc
        .mapped_plugin_logs()
        .and_then(|buffers| create_debugfs_logs(buffers, instance.dbdf, chipset, build_id))
    {
        Ok(logs) => instance.debugfs_logs = Some(logs),
        Err(error) => dev_warn!(
            dev,
            "debugfs logs unavailable for gfid={}: {:?}\n",
            gfid.0,
            error,
        ),
    }

    Ok(())
}

/// Activate an instance already owned by the live-instance registry.
///
/// If activation fails, attempt full teardown before returning the original
/// error.
#[expect(clippy::too_many_arguments)]
fn activate_registered_instance<'gpu>(
    instances: &mut VgpuInstances<'gpu>,
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    bar_user: &BarUser<'gpu>,
    mm: &mut GpuMm<'_>,
    gfid: Gfid,
    fifo_engine_list: &FifoEngineList,
    chipset: Chipset,
    build_id: Option<&BuildId>,
) -> Result {
    let index = instances
        .instances
        .iter()
        .position(|instance| instance.gfid == gfid)
        .ok_or(EIO)?;
    let activation_result = activate_instance(
        dev,
        cmdq,
        bar,
        &mut instances.instances[index],
        fifo_engine_list,
        chipset,
        build_id,
    );

    if let Err(original_error) = activation_result {
        if let Err(cleanup_error) = instances.destroy_instance(dev, cmdq, bar, bar_user, mm, gfid) {
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
    /// so duplicate checks and profile limits remain stable.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn create_instance(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &Mutex<GpuMm<'gpu>>,
        info: InstanceInfo,
        chipset: Chipset,
        build_id: Option<&BuildId>,
    ) -> Result {
        let fifo_engine_list = self.fifo_engine_list()?;
        let mut instances = self.instances().lock();
        // Global vGPU lock order: instances -> MM -> BAR-user VMM.
        let mut mm = mm.lock();
        let gfid = instances.allocate_instance(dev, cmdq, bar, bar_user, &mut mm, self, info)?;

        activate_registered_instance(
            &mut instances,
            dev,
            cmdq,
            bar,
            bar_user,
            &mut mm,
            gfid,
            fifo_engine_list,
            chipset,
            build_id,
        )
    }
}
