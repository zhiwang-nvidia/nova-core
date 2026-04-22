// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! C VFIO entry points into Nova's typed PF API.
//!
//! Each call borrows the PF API through the VF's managed SR-IOV relationship.
//! The C driver drains VFIO callbacks before returning from remove.

use kernel::{
    bindings,
    device,
    error::from_result,
    pci,
    prelude::*,
    types::ForLt, //
};
use nova_core::NovaCoreVfApi;

/// # Safety
///
/// `vf` must be a valid PCI device whose driver remains bound for this call,
/// including calls during probe and remove. A device reference alone does not
/// guarantee that its driver remains bound.
unsafe fn with_pf_api<R>(
    vf: *mut bindings::pci_dev,
    f: impl for<'borrow, 'data> FnOnce(Pin<&'borrow NovaCoreVfApi<'data>>) -> Result<R>,
) -> Result<R> {
    if vf.is_null() {
        return Err(EINVAL);
    }

    // SAFETY: The caller guarantees a valid PCI device and a driver binding
    // that lasts throughout this call, including the PF API borrow below.
    let dev = unsafe { device::Device::<device::Bound>::from_raw(&raw mut (*vf).dev) };
    let vf: &pci::Device<device::Bound> = dev.try_into()?;
    vf.vf_registration_data_with::<ForLt!(NovaCoreVfApi<'_>), _>(f)?
}

/// # Safety
///
/// `vf` must satisfy [`with_pf_api`]'s requirements.
#[kernel::macros::export]
unsafe extern "C" fn nvidia_vgpu_is_available(vf: *mut bindings::pci_dev) -> bool {
    // SAFETY: The caller guarantees `with_pf_api`'s device and binding requirements.
    unsafe { with_pf_api(vf, |api| Ok(api.is_available())) }.unwrap_or(false)
}

/// # Safety
///
/// `vf` must satisfy [`with_pf_api`]'s requirements. If non-null, `type_info`
/// must point to aligned, writable storage with no concurrent access.
#[kernel::macros::export]
unsafe extern "C" fn nvidia_vgpu_open(
    vf: *mut bindings::pci_dev,
    gfid: core::ffi::c_uint,
    sbdf: core::ffi::c_uint,
    vm_pid: core::ffi::c_uint,
    type_info: *mut bindings::nvidia_vgpu_type_info,
) -> core::ffi::c_int {
    from_result(|| {
        if type_info.is_null() {
            return Err(EINVAL);
        }

        // SAFETY: The caller guarantees `with_pf_api`'s device and binding requirements.
        let info = unsafe { with_pf_api(vf, |api| api.open_instance(gfid, sbdf, vm_pid)) }?;
        // SAFETY: `type_info` is non-null, and the caller guarantees aligned,
        // writable storage with no concurrent access.
        unsafe {
            type_info.write(bindings::nvidia_vgpu_type_info {
                pci_dev_id: info.pci_dev_id,
                pci_subsys_id: info.pci_subsys_id,
                bar1_length: info.bar1_length,
            })
        };
        Ok(0)
    })
}

/// # Safety
///
/// `vf` must satisfy [`with_pf_api`]'s requirements.
#[kernel::macros::export]
unsafe extern "C" fn nvidia_vgpu_close(vf: *mut bindings::pci_dev, gfid: core::ffi::c_uint) {
    // SAFETY: The caller guarantees `with_pf_api`'s device and binding requirements.
    let _ = unsafe { with_pf_api(vf, |api| api.close_instance(gfid)) };
}

/// # Safety
///
/// `vf` must satisfy [`with_pf_api`]'s requirements.
#[kernel::macros::export]
unsafe extern "C" fn nvidia_vgpu_reset(
    vf: *mut bindings::pci_dev,
    gfid: core::ffi::c_uint,
) -> core::ffi::c_int {
    from_result(|| {
        // SAFETY: The caller guarantees `with_pf_api`'s device and binding requirements.
        unsafe { with_pf_api(vf, |api| api.reset_instance(gfid)) }?;
        Ok(0)
    })
}
