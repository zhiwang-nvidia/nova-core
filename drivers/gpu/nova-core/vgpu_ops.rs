// SPDX-License-Identifier: GPL-2.0

//! PF-side vGPU lifecycle operations exported to the VFIO variant driver.
//!
//! Implements the `nova_vgpu_vfio_ops` callbacks (open/close/reset) that wrap
//! the internal [`Vgpu`] lifecycle management.  The VFIO driver calls these
//! through the C function pointer table obtained via [`nova_vgpu_get_vfio_ops`].

use core::ptr::addr_of_mut;

use kernel::prelude::*;

use crate::{
    driver::NovaCore,
    vgpu::{
        Dbdf,
        Gfid,
        Vgpu,
        VgpuInstance, //
    },
};

/// Static ops table returned by `nova_vgpu_get_vfio_ops`.
#[used]
static NOVA_VFIO_OPS: NovaVgpuVfioOps = NovaVgpuVfioOps {
    open: nova_vgpu_open,
    close: nova_vgpu_close,
    reset: nova_vgpu_reset,
};

/// Mirrors `struct nova_vgpu_vfio_ops` from `include/drm/nova_vgpu_vfio.h`.
#[repr(C)]
pub(crate) struct NovaVgpuVfioOps {
    open: unsafe extern "C" fn(*mut core::ffi::c_void, i32, u32, u32) -> i32,
    close: unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> i32,
    reset: unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> i32,
}

/// Helper that extracts mutable per-field references from the opaque driver
/// data pointer without ever creating `&NovaCore` or `&Gpu`, which would
/// violate the aliasing rules when obtaining `&mut` to interior fields.
///
/// # Safety
///
/// - `pf_drvdata` must be a valid `*mut NovaCore` from `pci_get_drvdata()`.
/// - The caller must guarantee exclusive access (VFIO driver serialises calls).
struct GpuFields<'a> {
    vgpu: &'a mut crate::vgpu::Vgpu,
    bar: &'a kernel::sync::Arc<kernel::devres::Devres<crate::driver::Bar0>>,
    bar1: &'a kernel::sync::Arc<kernel::devres::Devres<crate::driver::Bar1>>,
    mm: &'a crate::mm::GpuMm,
    cmdq: &'a crate::gsp::cmdq::Cmdq,
    bar_user: &'a mut Option<crate::mm::bar_user::BarUser>,
}

impl<'a> GpuFields<'a> {
    /// # Safety
    ///
    /// `pf_drvdata` must be a valid `*mut NovaCore` from `pci_get_drvdata()`.
    /// The caller must guarantee exclusive access (VFIO driver serialises calls).
    unsafe fn from_drvdata(pf_drvdata: *mut core::ffi::c_void) -> Self {
        let nova_ptr = pf_drvdata.cast::<NovaCore>();
        // SAFETY: `nova_ptr` is a valid `*mut NovaCore` from `pci_get_drvdata`.
        // We derive raw pointers to individual fields without creating
        // intermediate `&NovaCore` or `&Gpu` references, so no aliasing
        // rules are violated.  The VFIO driver serialises all lifecycle
        // calls per PF, so there are no concurrent mutable accesses.
        unsafe {
            let gpu_ptr = addr_of_mut!((*nova_ptr).gpu);
            Self {
                vgpu: &mut *addr_of_mut!((*gpu_ptr).vgpu),
                bar: &*core::ptr::addr_of!((*gpu_ptr).bar),
                bar1: &*core::ptr::addr_of!((*gpu_ptr).bar1),
                mm: &*core::ptr::addr_of!((*gpu_ptr).mm),
                cmdq: {
                    let gsp_ptr = core::ptr::addr_of!((*gpu_ptr).gsp);
                    &*core::ptr::addr_of!((*gsp_ptr).cmdq)
                },
                bar_user: &mut *addr_of_mut!((*gpu_ptr).bar_user),
            }
        }
    }
}

fn do_open(
    pf_drvdata: *mut core::ffi::c_void,
    vf_id: i32,
    vgpu_type_id: u32,
    vm_pid: u32,
) -> Result<i32> {
    // SAFETY: pf_drvdata is a valid *mut NovaCore; VFIO serialises calls.
    let f = unsafe { GpuFields::from_drvdata(pf_drvdata) };

    let gfid = Gfid((vf_id + 1) as u32);
    let dbdf = Dbdf(0); // TODO: encode from VF PCI BDF

    let vgpu_type_idx = f
        .vgpu
        .vgpu_types
        .iter()
        .position(|t| t.vgpu_type_id == vgpu_type_id)
        .ok_or(EINVAL)?;

    let instance = VgpuInstance {
        id: vf_id,
        gfid,
        dbdf,
        vgpu_type_idx,
        vm_pid,
        chid_offset: 0,
        num_chid: 0,
        num_plugin_channels: 0,
        fbmem_heap: None,
        mgmt_heap: None,
        active: false,
    };

    let bar_guard = f.bar.try_access().ok_or(ENXIO)?;
    let idx = f.vgpu.create_instance(f.mm, f.cmdq, &bar_guard, instance)?;

    let inst = &f.vgpu.instances[idx];
    let comm_size = f.vgpu.comm_layout.total_size;

    if let Some(ref mut bar_user) = f.bar_user {
        let bar1_guard = f.bar1.try_access().ok_or(ENXIO)?;
        Vgpu::wait_plugin_ready(inst, bar_user, f.mm, &bar1_guard, comm_size)?;
        f.vgpu
            .setup_plugin_rpc(inst, bar_user, f.mm, &bar_guard, &bar1_guard, 0)?;
    }

    Ok(0)
}

fn do_close(pf_drvdata: *mut core::ffi::c_void, vf_id: i32) -> Result<i32> {
    // SAFETY: pf_drvdata is a valid *mut NovaCore; VFIO serialises calls.
    let f = unsafe { GpuFields::from_drvdata(pf_drvdata) };

    let bar_guard = f.bar.try_access().ok_or(ENXIO)?;
    f.vgpu.destroy_instance(f.cmdq, &bar_guard, vf_id)?;

    Ok(0)
}

fn do_reset(pf_drvdata: *mut core::ffi::c_void, vf_id: i32) -> Result<i32> {
    // SAFETY: pf_drvdata is a valid *mut NovaCore; VFIO serialises calls.
    let f = unsafe { GpuFields::from_drvdata(pf_drvdata) };

    let inst = f
        .vgpu
        .instances
        .iter()
        .find(|i| i.id == vf_id)
        .ok_or(ENODEV)?;

    if !inst.active {
        return Err(ENODEV);
    }

    let comm_size = f.vgpu.comm_layout.total_size;
    let bar_user = f.bar_user.as_mut().ok_or(ENODEV)?;
    let bar0_guard = f.bar.try_access().ok_or(ENXIO)?;
    let bar1_guard = f.bar1.try_access().ok_or(ENXIO)?;
    f.vgpu
        .rpc_reset(inst, bar_user, f.mm, &bar0_guard, &bar1_guard, comm_size)?;

    Ok(0)
}

fn result_to_errno(r: Result<i32>) -> i32 {
    match r {
        Ok(v) => v,
        Err(e) => e.to_errno(),
    }
}

/// # Safety
///
/// `pf_drvdata` must be a valid `*mut NovaCore` from `pci_get_drvdata()`.
unsafe extern "C" fn nova_vgpu_open(
    pf_drvdata: *mut core::ffi::c_void,
    vf_id: i32,
    vgpu_type_id: u32,
    vm_pid: u32,
) -> i32 {
    result_to_errno(do_open(pf_drvdata, vf_id, vgpu_type_id, vm_pid))
}

/// # Safety
///
/// `pf_drvdata` must be a valid `*mut NovaCore` from `pci_get_drvdata()`.
unsafe extern "C" fn nova_vgpu_close(pf_drvdata: *mut core::ffi::c_void, vf_id: i32) -> i32 {
    result_to_errno(do_close(pf_drvdata, vf_id))
}

/// # Safety
///
/// `pf_drvdata` must be a valid `*mut NovaCore` from `pci_get_drvdata()`.
unsafe extern "C" fn nova_vgpu_reset(pf_drvdata: *mut core::ffi::c_void, vf_id: i32) -> i32 {
    result_to_errno(do_reset(pf_drvdata, vf_id))
}

/// Return the static ops table for the VFIO variant driver.
///
/// # Safety
///
/// `_pf_drvdata` is unused but included for ABI compatibility.
#[no_mangle]
#[allow(unreachable_pub)]
pub unsafe extern "C" fn nova_vgpu_get_vfio_ops(
    _pf_drvdata: *mut core::ffi::c_void,
) -> *const NovaVgpuVfioOps {
    &NOVA_VFIO_OPS
}
