// SPDX-License-Identifier: GPL-2.0

use core::num::NonZeroUsize;

use kernel::{
    debugfs,
    device,
    prelude::*,
    ptr::Alignment,
    str::CString, //
};

use crate::{
    driver::Bar0,
    firmware::BuildId,
    gpu::{
        ChannelIdArea,
        Chipset, //
    },
    gsp::{
        cmdq::Cmdq,
        commands::NVGMC_ENGINE_TYPE_COUNT,
        nvkv::{
            Dbdf,
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
        },
        log::VgpuLogBuffers,
        plugin_rpc::{
            PluginConfigParams,
            PluginRpc, //
        },
        scrubber::CeUtils,
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
    pub(crate) name: [u8; 64],
    pub(crate) class: [u8; 64],
    pub(crate) vgpu_type_id: u32,
    pub(crate) bar1_length: u64,
    pub(crate) max_instance: u32,
    pub(crate) ecc_supported: u32,
    pub(crate) profile_size: u64,
    pub(crate) max_fps: u32,
    pub(crate) num_heads: u32,
    pub(crate) max_res_x: u32,
    pub(crate) max_res_y: u32,
    pub(crate) pci_dev_id: u32,
    pub(crate) pci_subsys_id: u32,
    pub(crate) fb_length: u64,
    pub(crate) gsp_heap_size: u64,
    pub(crate) fb_reservation: u64,
}

impl VgpuType {
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
#[expect(dead_code)]
pub(crate) struct VgpuInstance<'gpu> {
    pub(crate) id: u32,
    pub(crate) gfid: Gfid,
    pub(crate) dbdf: Dbdf,
    pub(crate) vgpu_type: VgpuType,
    pub(crate) vm_pid: u32,
    pub(crate) chids: ChannelIdArea<'gpu>,
    pub(crate) num_plugin_channels: u32,
    ceutils: Option<CeUtils>,
    pub(crate) vram_slot: VgpuVramSlot,
    debugfs_logs: Option<Pin<KBox<debugfs::Scope<VgpuLogBuffers>>>>,
    pub(crate) plugin_rpc: PluginRpc<'gpu>,
}

impl<'gpu> VgpuInstance<'gpu> {
    /// Scrub the instance framebuffer with its owned CeUtils allocation.
    pub(crate) fn scrub_guest_fb(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
    ) -> Result {
        self.ceutils.as_ref().ok_or(EIO)?.scrub_guest_fb(
            dev,
            cmdq,
            bar,
            bar_user,
            mm,
            &self.vram_slot.fbmem,
        )
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
    next_instance_id: u32,
}

#[expect(dead_code)]
impl<'gpu> VgpuInstances<'gpu> {
    pub(crate) const fn new() -> Self {
        Self {
            instances: KVec::new(),
            vram_slots: None,
            next_instance_id: 0,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.instances.len()
    }

    fn next_id(&mut self) -> Result<u32> {
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.checked_add(1).ok_or(ENOSPC)?;
        Ok(id)
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

    fn free_vram_slot(&mut self, index: u32) -> Result {
        let allocator = self.vram_slots.as_mut().ok_or(EINVAL)?;
        allocator.free(index)
    }

    /// Allocate resources and map the management communication region.
    #[expect(clippy::too_many_arguments)]
    pub(crate) fn allocate_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
        vgpu: &VgpuManager<'gpu>,
        info: InstanceInfo,
    ) -> Result<VgpuInstance<'gpu>> {
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
        let id = self.next_id()?;

        let num_chid = vgpu
            .total_channels()
            .ok_or(ENODEV)?
            .checked_div(vgpu_type.max_instance)
            .filter(|count| *count != 0)
            .ok_or(EINVAL)?;
        let chids = vgpu.chid_pool.alloc_area(
            NonZeroUsize::new(usize::try_from(num_chid).map_err(|_| EOVERFLOW)?).ok_or(EINVAL)?,
            Alignment::new::<1>(),
        )?;
        let chid_offset = u32::try_from(chids.start).map_err(|_| EOVERFLOW)?;
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
            Err(error) => {
                let slot_index = vram_slot.index;
                drop(vram_slot);
                if let Err(free_error) = self.free_vram_slot(slot_index) {
                    dev_err!(
                        dev,
                        "allocate_instance: failed to release slot {}: {:?}\n",
                        slot_index,
                        free_error,
                    );
                }
                return Err(error);
            }
        };
        if let Err(error) = ceutils.scrub_guest_fb(dev, cmdq, bar, bar_user, mm, &vram_slot.fbmem) {
            if let Err(release_error) = ceutils.release(dev, cmdq, bar) {
                dev_err!(
                    dev,
                    "failed to release CeUtils after scrub error {:?}: {:?}\n",
                    error,
                    release_error,
                );
                // The firmware may still own the reserved channel.
                core::mem::forget(chids);
            }
            dev_err!(
                dev,
                "retaining VRAM slot {} after scrub error {:?}\n",
                vram_slot.index,
                error,
            );
            return Err(error);
        }
        let comm = match CommBufferRegion::new(bar_user, mm, &vram_slot.mgmt_heap) {
            Ok(comm) => comm,
            Err(error) => {
                if let Err(release_error) = ceutils.release(dev, cmdq, bar) {
                    dev_err!(
                        dev,
                        "failed to release CeUtils after BAR1 error {:?}: {:?}\n",
                        error,
                        release_error,
                    );
                    // The firmware may still own the reserved channel.
                    core::mem::forget(chids);
                    return Err(error);
                }

                let slot_index = vram_slot.index;
                drop(vram_slot);
                if let Err(free_error) = self.free_vram_slot(slot_index) {
                    dev_err!(
                        dev,
                        "allocate_instance: failed to release slot {}: {:?}\n",
                        slot_index,
                        free_error,
                    );
                }
                return Err(error);
            }
        };
        let fbmem = &vram_slot.fbmem;
        let mgmt = &vram_slot.mgmt_heap;

        dev_dbg!(
            dev,
            "allocate_instance: gfid={} dbdf={:#x} type={} slot={} chid={}..{} ceutils_chid={} fb={:#x}+{:#x} heap={:#x}+{:#x} sema={:#x}\n",
            gfid.0,
            dbdf.into_raw(),
            vgpu_type.vgpu_type_id,
            vram_slot.index,
            chid_offset,
            u64::from(chid_offset) + u64::from(num_chid),
            ceutils.chid(),
            fbmem.address(),
            fbmem.size(),
            mgmt.address(),
            mgmt.size(),
            ceutils.semaphore_address(),
        );

        Ok(VgpuInstance {
            id,
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
            chids,
            num_plugin_channels: 3,
            ceutils: Some(ceutils),
            vram_slot,
            debugfs_logs: None,
            plugin_rpc: PluginRpc::new(comm),
        })
    }

    /// Shut down an instance, scrub its guest FB, and release its reservations.
    pub(crate) fn destroy_instance(
        &mut self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
        gfid: Gfid,
    ) -> Result {
        let index = self
            .instances
            .iter()
            .position(|instance| instance.gfid == gfid)
            .ok_or(ENOENT)?;

        shutdown(dev, cmdq, bar, gfid)?;
        self.instances[index].scrub_guest_fb(dev, cmdq, bar, bar_user, mm)?;
        self.instances[index]
            .ceutils
            .as_ref()
            .ok_or(EIO)?
            .release(dev, cmdq, bar)?;
        self.instances[index].ceutils = None;
        cleanup(dev, cmdq, bar, gfid)?;
        // Remove the files and drain active readers before tearing down the
        // BAR1 mapping that backs them.
        drop(self.instances[index].debugfs_logs.take());
        self.instances[index].plugin_rpc.destroy(bar_user, mm)?;

        let instance = self.instances.remove(index).map_err(|_| EIO)?;
        let slot_index = instance.vram_slot.index;
        drop(instance);
        self.free_vram_slot(slot_index)?;
        dev_dbg!(dev, "destroy_instance: gfid={} done\n", gfid.0);
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

    let properties = VgpuProperties::decode(&response.payload)?;
    if properties.type_id != type_id || properties.max_instance == 0 {
        return Err(EINVAL);
    }
    Ok(VgpuType::from_properties(&properties))
}

/// Bootload the GSP plugin and negotiate its RPC channel.
///
/// Called without holding the runtime lock.
#[expect(dead_code)]
pub(crate) fn activate_instance(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    instance: &mut VgpuInstance<'_>,
    engine_masks: &[u64; NVGMC_ENGINE_TYPE_COUNT],
    chipset: Chipset,
    build_id: Option<&BuildId>,
) -> Result {
    bootload(dev, cmdq, bar, instance, engine_masks)?;

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
