// SPDX-License-Identifier: GPL-2.0

//! FFI exports for the VFIO variant driver.

use kernel::{
    device,
    pci,
    prelude::*, //
};

use crate::{
    driver::NovaCore,
    gsp::nvkv::Dbdf,
    vgpu::instance::{
        activate_instance,
        query_assigned_vf_type,
        query_vgpu_type,
        Gfid,
        InstanceInfo,
        VgpuType, //
    }, //
};

/// Transparent wrapper over the C `struct nvidia_vgpu_type_info`.
#[repr(transparent)]
pub(crate) struct VgpuTypeInfo(kernel::bindings::nvidia_vgpu_type_info);

impl VgpuTypeInfo {
    fn from_vgpu_type(vgpu_type: &VgpuType) -> Self {
        Self(kernel::bindings::nvidia_vgpu_type_info {
            pci_dev_id: vgpu_type.pci_dev_id,
            pci_subsys_id: vgpu_type.pci_subsys_id,
            bar1_length: vgpu_type.bar1_length,
        })
    }
}

/// Run `f` with the nova-core driver data and bound device for a PF.
///
/// The higher-ranked callback prevents references reconstructed from the raw
/// PCI device and its driver data from escaping this call.
///
/// # Safety
///
/// If `pf_pdev` is non-null, it must point to a live physical PCI function and
/// remain valid for the duration of this call. The function must remain bound
/// to nova-core, without concurrent unbind, for the duration of this call.
unsafe fn with_nova_core<R>(
    pf_pdev: *mut kernel::bindings::pci_dev,
    gfid: u32,
    f: impl for<'a> FnOnce(Pin<&'a NovaCore<'a>>, &'a device::Device<device::Bound>) -> Result<R>,
) -> Result<R> {
    if pf_pdev.is_null() {
        return Err(EINVAL);
    }

    // SAFETY: The caller guarantees that `pf_pdev` points to a live PCI
    // device that remains bound for this call. `pci::Device` is transparent
    // over `bindings::pci_dev`.
    let pf: &pci::Device<device::Bound> = unsafe { &*pf_pdev.cast() };
    if pf.is_virtfn() || !pf.is_physfn() {
        return Err(EINVAL);
    }

    // Before interpreting drvdata as `NovaCore`, verify that this PF is still
    // bound to the nova-core PCI driver. `managed_sriov` keeps the PF bound for
    // the lifetime of its VFs.
    // SAFETY: `pf_pdev` is valid and its `driver` pointer, when non-null,
    // remains valid while the device is bound.
    let driver = unsafe { (*pf_pdev).driver };
    if driver.is_null()
        // SAFETY: `driver` was checked for null above.
        || !unsafe { (*driver).managed_sriov }
        // SAFETY: `driver` was checked for null above.
        || unsafe { (*driver).name } != crate::MODULE_NAME.as_char_ptr()
        // SAFETY: `driver` was checked for null above.
        || unsafe { (*driver).driver.owner } != crate::THIS_MODULE.as_ptr()
    {
        return Err(ENODEV);
    }

    let pf_dev: &device::Device<device::Bound> = pf.as_ref();
    // SAFETY: `pf_pdev` is valid, so its embedded device is valid too.
    let drvdata =
        unsafe { kernel::bindings::dev_get_drvdata(core::ptr::addr_of_mut!((*pf_pdev).dev)) };
    if drvdata.is_null() {
        return Err(ENODEV);
    }

    // SAFETY: The driver identity check above establishes that drvdata was
    // installed by nova-core as `NovaCore`. The caller guarantees that the PF
    // cannot be unbound during this call, and PCI driver data stores the
    // pointer returned by `Pin<KBox<NovaCore>>::into_foreign()`. Lifetimes do
    // not affect layout. The callback's HRTB prevents the reconstructed
    // reference, including NovaCore's bound-device lifetime, from escaping.
    let nova_core = unsafe { Pin::new_unchecked(&*drvdata.cast::<NovaCore<'_>>()) };

    let total_vfs = nova_core.gpu.vgpu_total_vfs().ok_or(ENODEV)?;
    if gfid == 0 || gfid > u32::from(total_vfs.get()) {
        return Err(EINVAL);
    }

    f(nova_core, pf_dev)
}

fn nvidia_vgpu_open_inner<'a>(
    nova_core: Pin<&'a NovaCore<'a>>,
    dev: &'a device::Device<device::Bound>,
    gfid: u32,
    dbdf: u32,
    vm_pid: u32,
) -> Result<VgpuTypeInfo> {
    let gpu = &nova_core.gpu;
    let gfid = Gfid(gfid);
    let dbdf = Dbdf::from_raw(dbdf);

    dev_dbg!(
        dev,
        "vgpu_open: gfid={} dbdf={:#x}\n",
        gfid.0,
        dbdf.into_raw()
    );

    let bar = gpu.bar0();
    let cmdq = gpu.cmdq();
    let vgpu = gpu.vgpu_manager();

    let type_id = query_assigned_vf_type(&cmdq, bar, dbdf)?;
    dev_dbg!(
        dev,
        "vgpu_open: gfid={} assigned type_id={}\n",
        gfid.0,
        type_id
    );

    let vgpu_type = query_vgpu_type(&cmdq, bar, type_id)?;
    dev_dbg!(
        dev,
        "vgpu_open: gfid={} vgpu_type={} fb_length={:#x}\n",
        gfid.0,
        vgpu_type.vgpu_type_id,
        vgpu_type.fb_length
    );

    let type_info = VgpuTypeInfo::from_vgpu_type(&vgpu_type);
    let engine_masks = vgpu.engine_masks()?;
    let mut instances = vgpu.instances().lock();
    let instance = instances.allocate_instance(
        dev,
        &cmdq,
        bar,
        gpu.bar_user(),
        gpu.mm(),
        vgpu,
        InstanceInfo::new(gfid, dbdf, vgpu_type, vm_pid),
    )?;

    activate_instance(
        &mut instances,
        dev,
        &cmdq,
        bar,
        gpu.bar_user(),
        gpu.mm(),
        instance,
        engine_masks,
        gpu.chipset(),
        gpu.build_id(),
    )?;

    Ok(type_info)
}

fn nvidia_vgpu_close_inner<'a>(
    nova_core: Pin<&'a NovaCore<'a>>,
    dev: &'a device::Device<device::Bound>,
    gfid: u32,
) -> Result {
    let gpu = &nova_core.gpu;
    let gfid = Gfid(gfid);

    dev_dbg!(dev, "vgpu_close: gfid={}\n", gfid.0);

    let cmdq = gpu.cmdq();
    let mut instances = gpu.vgpu_manager().instances().lock();
    let result = instances.destroy_instance(dev, &cmdq, gpu.bar0(), gpu.bar_user(), gpu.mm(), gfid);
    if let Err(error) = result {
        dev_err!(dev, "vgpu_close: gfid={} failed: {:?}\n", gfid.0, error);
    }
    result
}

fn nvidia_vgpu_reset_inner<'a>(
    nova_core: Pin<&'a NovaCore<'a>>,
    dev: &'a device::Device<device::Bound>,
    gfid: u32,
) -> Result {
    let gpu = &nova_core.gpu;
    let gfid = Gfid(gfid);

    dev_dbg!(dev, "vgpu_reset: gfid={}\n", gfid.0);

    let cmdq = gpu.cmdq();
    let mut instances = gpu.vgpu_manager().instances().lock();
    instances.reset_instance(dev, &cmdq, gpu.bar0(), gpu.bar_user(), gpu.mm(), gfid)?;

    dev_dbg!(dev, "vgpu_reset: gfid={} done\n", gfid.0);
    Ok(())
}

/// # Safety
///
/// If `pf_pdev` is non-null, it must point to a live physical PCI function
/// that remains bound to nova-core, without concurrent unbind, for the
/// duration of this call. `gfid` is the Guest Function ID (VF index + 1).
/// `dbdf` is the VF's Domain:Bus:Device.Function encoded as
/// `(domain << 16) | (bus << 8) | devfn`. `vm_pid` is the thread-group ID of
/// the userspace VM process. If `type_info` is non-null, it must point to
/// aligned, writable storage for a `struct nvidia_vgpu_type_info`, and no
/// other thread may access that storage for the duration of the write.
#[export]
unsafe extern "C" fn nvidia_vgpu_open(
    pf_pdev: *mut kernel::bindings::pci_dev,
    gfid: core::ffi::c_uint,
    dbdf: core::ffi::c_uint,
    vm_pid: core::ffi::c_uint,
    type_info: *mut kernel::bindings::nvidia_vgpu_type_info,
) -> core::ffi::c_int {
    if type_info.is_null() {
        return EINVAL.to_errno();
    }

    // SAFETY: The caller upholds the exported function's contract. The HRTB
    // callback confines all references derived from `pf_pdev` to this call.
    let result = unsafe {
        with_nova_core(pf_pdev, gfid, |nova_core, dev| {
            nvidia_vgpu_open_inner(nova_core, dev, gfid, dbdf, vm_pid)
        })
    };

    match result {
        Ok(info) => {
            // SAFETY: `type_info` was checked for null above and the caller
            // guarantees that it points to writable storage.
            unsafe { type_info.write(info.0) };
            0
        }
        Err(error) => error.to_errno(),
    }
}

/// # Safety
///
/// If `pf_pdev` is non-null, it must point to a live physical PCI function
/// that remains bound to nova-core, without concurrent unbind, for the
/// duration of this call. `gfid` must identify one of that PF's VFs.
#[export]
unsafe extern "C" fn nvidia_vgpu_close(
    pf_pdev: *mut kernel::bindings::pci_dev,
    gfid: core::ffi::c_uint,
) {
    // SAFETY: The caller upholds the exported function's contract. The HRTB
    // callback confines all references derived from `pf_pdev` to this call.
    let _ = unsafe {
        with_nova_core(pf_pdev, gfid, |nova_core, dev| {
            nvidia_vgpu_close_inner(nova_core, dev, gfid)
        })
    };
}

/// # Safety
///
/// If `pf_pdev` is non-null, it must point to a live physical PCI function
/// that remains bound to nova-core, without concurrent unbind, for the
/// duration of this call. `gfid` must identify one of that PF's VFs.
#[export]
unsafe extern "C" fn nvidia_vgpu_reset(
    pf_pdev: *mut kernel::bindings::pci_dev,
    gfid: core::ffi::c_uint,
) -> core::ffi::c_int {
    // SAFETY: The caller upholds the exported function's contract. The HRTB
    // callback confines all references derived from `pf_pdev` to this call.
    let result = unsafe {
        with_nova_core(pf_pdev, gfid, |nova_core, dev| {
            nvidia_vgpu_reset_inner(nova_core, dev, gfid)
        })
    };

    match result {
        Ok(()) => 0,
        Err(error) => error.to_errno(),
    }
}
