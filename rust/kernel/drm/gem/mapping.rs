// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Fault-driven mappings for driver-backed GEM objects.

use super::{
    DriverAllocImpl,
    DriverObject,
    IntoGEMObject, //
};
use crate::{
    bindings,
    error::from_result,
    mm::virt::VmaNew,
    page::PAGE_SIZE,
    prelude::*, //
};
use core::marker::PhantomData;

/// An admitted range within a GEM object, measured in bytes.
///
/// Only shared I/O mappings are supported. The fake GEM mmap offset remains in
/// the VMA so that DRM unplug can invalidate its PTEs through the inode mapping.
pub struct Mmap<'a> {
    offset: usize,
    size: usize,
    _vma: PhantomData<&'a mut bindings::vm_area_struct>,
}

impl Mmap<'_> {
    /// Returns the mapping's byte offset within the GEM object.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Returns the mapping's size in bytes.
    pub fn size(&self) -> usize {
        self.size
    }
}

/// The result of servicing a GEM page fault.
pub struct FaultResult(bindings::vm_fault_t);

impl FaultResult {
    /// The page cannot be accessed.
    pub const SIGBUS: Self = Self(bindings::vm_fault_reason_VM_FAULT_SIGBUS);

    /// The fault could not be serviced due to memory allocation failure.
    pub const OOM: Self = Self(bindings::vm_fault_reason_VM_FAULT_OOM);
}

/// A page fault in a shared GEM I/O mapping.
///
/// # Invariants
///
/// `raw` remains valid throughout the borrow. Its VMA holds a GEM reference and
/// has the shared PFNMAP flags installed by `mmap_callback`.
pub struct Fault<'a> {
    raw: *mut bindings::vm_fault,
    _borrow: PhantomData<&'a mut bindings::vm_fault>,
}

impl Fault<'_> {
    /// Returns the faulting page's byte offset within the GEM object.
    ///
    /// The complete page must fit within the object.
    pub fn offset(&self) -> Result<usize> {
        // SAFETY: These callback fields and the VMA's GEM reference remain valid.
        let (obj, pgoff) = unsafe {
            let fault = &(*self.raw).__bindgen_anon_1;
            ((*fault.vma).vm_private_data.cast(), fault.pgoff)
        };
        // SAFETY: The VMA owns a reference to this initialized GEM object.
        unsafe { object_offset(obj, pgoff, PAGE_SIZE) }
    }

    /// Inserts a device PFN at the faulting virtual address.
    ///
    /// # Safety
    ///
    /// `pfn` must name the object's device-memory page at [`Self::offset`], and
    /// allow write-combined userspace access. Keep the backing valid until its
    /// PTEs have been revoked, and hold the device's unplug guard across this
    /// call so unplug's invalidation cannot race a new insertion.
    pub unsafe fn insert_pfn(&self, pfn: usize) -> FaultResult {
        if self.offset().is_err() {
            return FaultResult::SIGBUS;
        }
        // SAFETY: The VMA is a shared PFNMAP, the fault covers a complete object
        // page, and the caller guarantees the backing and unplug synchronization.
        FaultResult(unsafe {
            let fault = &(*self.raw).__bindgen_anon_1;
            bindings::vmf_insert_pfn(fault.vma, fault.address, pfn)
        })
    }
}

/// Checks an object-relative byte range derived from the fake mmap page offset.
///
/// # Safety
///
/// `obj` must be a live GEM object with a stable VMA node and size.
unsafe fn object_offset(
    obj: *mut bindings::drm_gem_object,
    pgoff: usize,
    size: usize,
) -> Result<usize> {
    // SAFETY: The caller guarantees that the GEM object and its VMA node are live.
    let (start, object_size) = unsafe {
        (
            bindings::drm_vma_node_start(&raw const (*obj).vma_node),
            (*obj).size,
        )
    };
    let offset = pgoff.checked_sub(start).ok_or(EINVAL)?;
    let offset = offset.checked_mul(PAGE_SIZE).ok_or(EOVERFLOW)?;
    if offset.checked_add(size).ok_or(EOVERFLOW)? > object_size {
        return Err(EINVAL);
    }
    Ok(offset)
}

pub(super) extern "C" fn mmap_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_vma: *mut bindings::vm_area_struct,
) -> c_int {
    from_result(|| {
        // SAFETY: DRM calls this from f_ops->mmap during initial VMA setup.
        let vma = unsafe { VmaNew::from_raw(raw_vma) };
        let size = vma.end() - vma.start();
        // SAFETY: The VMA remains valid throughout this callback.
        let pgoff = unsafe { (*raw_vma).vm_pgoff };
        if vma.flags() & bindings::VM_SHARED == 0 {
            return Err(EINVAL);
        }
        let mapping = Mmap {
            // SAFETY: DRM holds a reference to raw_obj throughout this callback.
            offset: unsafe { object_offset(raw_obj, pgoff, size) }?,
            size,
            _vma: PhantomData,
        };
        // SAFETY: This callback belongs to T's base allocation, which is fully
        // initialized before being published. DRM holds the object reference.
        let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };
        T::mmap(obj, &mapping)?;
        // SAFETY: raw_vma is exclusively initialized here and was checked shared.
        // Only flags/protection change; no PTE is installed before VMA linkage.
        unsafe { bindings::drm_gem_prepare_io_vma(raw_vma) };
        Ok(0)
    })
}

pub(super) struct VmaOps<T>(PhantomData<T>);

impl<T: DriverObject> VmaOps<T> {
    pub(super) const OPS: bindings::vm_operations_struct = bindings::vm_operations_struct {
        open: Some(bindings::drm_gem_vm_open),
        close: Some(bindings::drm_gem_vm_close),
        fault: Some(Self::fault_callback),
        ..pin_init::zeroed()
    };

    extern "C" fn fault_callback(raw: *mut bindings::vm_fault) -> bindings::vm_fault_t {
        // SAFETY: The MM invokes this callback with a live fault and VMA. Its
        // vm_private_data is the GEM object retained by drm_gem_mmap_obj/vm_open.
        let raw_obj = unsafe { (*(*raw).__bindgen_anon_1.vma).vm_private_data.cast() };
        // SAFETY: Our VMA operations belong to T's fully initialized allocation,
        // and the VMA retains its GEM reference throughout this callback.
        let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };
        let fault = Fault {
            raw,
            _borrow: PhantomData,
        };
        T::fault(obj, &fault).0
    }
}
