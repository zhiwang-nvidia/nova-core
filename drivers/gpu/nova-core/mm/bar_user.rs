// SPDX-License-Identifier: GPL-2.0

//! BAR1 user interface for CPU access to GPU virtual memory. Used for USERD
//! for GPU work submission, and applications to access GPU buffers via mmap().

use kernel::{
    device,
    devres::Devres,
    io::Io,
    new_mutex,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    },
};

use crate::{
    driver::Bar1,
    gpu::Chipset,
    mm::{
        vmm::{
            MappedRange,
            Vmm, //
        },
        GpuMm,
        Pfn,
        Vfn,
        VirtualAddress,
        VramAddress,
        PAGE_SIZE, //
    },
    num::IntoSafeCast,
};

/// BAR1 user interface for virtual memory mappings.
///
/// Owns the [`Vmm`] for the BAR1 address space.
#[pin_data]
pub(crate) struct BarUser {
    #[pin]
    vmm: Mutex<Vmm>,
    mm: Arc<GpuMm>,
    bar1: Arc<Devres<Bar1>>,
}

impl BarUser {
    /// Create a pin-initializer for [`BarUser`].
    pub(crate) fn new(
        pdb_addr: VramAddress,
        chipset: Chipset,
        va_size: u64,
        mm: Arc<GpuMm>,
        bar1: Arc<Devres<Bar1>>,
    ) -> Result<impl PinInit<Self>> {
        let vmm = Vmm::new(pdb_addr, chipset.mmu_version(), va_size)?;
        Ok(pin_init!(Self {
            vmm <- new_mutex!(vmm, "bar_user_vmm"),
            mm,
            bar1,
        }))
    }

    /// Returns a reference to the BAR1 devres for direct MMIO access.
    fn bar1(&self) -> &Devres<Bar1> {
        &self.bar1
    }

    /// Map physical pages into the BAR1 address space, returning a [`MappedRange`].
    ///
    /// Unlike [`BarUser::map()`], the caller is responsible for calling
    /// [`BarUser::unmap_pages()`] to release the mapping.
    fn map_pages(
        &self,
        dev: &device::Device<device::Bound>,
        pfns: &[Pfn],
        writable: bool,
    ) -> Result<MappedRange> {
        self.vmm.lock().map_pages(dev, &self.mm, pfns, None, writable)
    }

    /// Unmap a previously mapped range, invalidating PTEs and freeing VA.
    fn unmap_pages(
        &self,
        dev: &device::Device<device::Bound>,
        range: MappedRange,
    ) -> Result {
        self.vmm.lock().unmap_pages(dev, &self.mm, range)
    }

    /// Map physical pages to a contiguous BAR1 virtual range.
    pub(crate) fn map(
        self: &Arc<Self>,
        dev: &device::Device<device::Bound>,
        pfns: &[Pfn],
        writable: bool,
    ) -> Result<BarUserAccess> {
        if pfns.is_empty() {
            return Err(EINVAL);
        }
        let mut vmm = self.vmm.lock();
        let mapped = vmm.map_pages(dev, &self.mm, pfns, None, writable)?;

        Ok(BarUserAccess {
            bar_user: self.clone(),
            mapped: Some(mapped),
        })
    }
}

/// Access object for a mapped BAR1 region.
pub(crate) struct BarUserAccess {
    bar_user: Arc<BarUser>,
    /// [`BarUserAccess::release`] [`Option::take`]s this; `Some` at
    /// drop time means `release()` was never called.
    mapped: Option<MappedRange>,
}

impl BarUserAccess {
    /// Tear down the BAR1 mapping using a caller-supplied bound device.
    pub(crate) fn release(mut self, dev: &device::Device<device::Bound>) -> Result {
        let mapped = self.mapped.take().ok_or(EINVAL)?;
        let mut vmm = self.bar_user.vmm.lock();
        vmm.unmap_pages(dev, &self.bar_user.mm, mapped)?;
        Ok(())
    }

    fn mapped(&self) -> &MappedRange {
        self.mapped.as_ref().unwrap()
    }

    /// Get the base virtual address of this mapping.
    pub(crate) fn base(&self) -> VirtualAddress {
        VirtualAddress::from(self.mapped().vfn_start)
    }

    /// Get the total size of the mapped region in bytes.
    pub(crate) fn size(&self) -> usize {
        self.mapped().num_pages * PAGE_SIZE
    }

    /// Get the starting virtual frame number.
    pub(crate) fn vfn_start(&self) -> Vfn {
        self.mapped().vfn_start
    }

    /// Get the number of pages in this mapping.
    pub(crate) fn num_pages(&self) -> usize {
        self.mapped().num_pages
    }

    fn bar_offset(&self, offset: usize) -> Result<usize> {
        if offset >= self.size() {
            return Err(EINVAL);
        }

        let base_vfn: usize = self.mapped().vfn_start.raw().into_safe_cast();
        let base = base_vfn.checked_mul(PAGE_SIZE).ok_or(EOVERFLOW)?;
        base.checked_add(offset).ok_or(EOVERFLOW)
    }

    /// Read a 32-bit value at the given offset.
    pub(crate) fn try_read32(
        &self,
        dev: &device::Device<device::Bound>,
        offset: usize,
    ) -> Result<u32> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_read32(off)
    }

    /// Write a 32-bit value at the given offset.
    pub(crate) fn try_write32(
        &self,
        dev: &device::Device<device::Bound>,
        value: u32,
        offset: usize,
    ) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_write32(value, off)
    }

    /// Read a 64-bit value at the given offset.
    pub(crate) fn try_read64(
        &self,
        dev: &device::Device<device::Bound>,
        offset: usize,
    ) -> Result<u64> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_read64(off)
    }

    /// Write a 64-bit value at the given offset.
    pub(crate) fn try_write64(
        &self,
        dev: &device::Device<device::Bound>,
        value: u64,
        offset: usize,
    ) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.access(dev)?.try_write64(value, off)
    }
}

impl Drop for BarUserAccess {
    fn drop(&mut self) {
        if self.mapped.is_some() {
            kernel::pr_warn!(
                "BarUserAccess dropped without calling release(). BarUser address space will leak.\n"
            );
        }
    }
}

/// BAR1 sub-mapping backed by GPU page tables.
///
/// Maps a contiguous VRAM region into the BAR1 virtual address space via
/// [`BarUser`].  CPU accesses go through `bar1[gpu_va_addr + off]`.
/// Must be explicitly destroyed via [`Bar1Map::destroy()`] to release the
/// GPU page table mapping.
#[expect(dead_code)]
pub(crate) struct Bar1Map {
    bar_user: Arc<BarUser>,
    mapped: MappedRange,
    /// Physical VRAM address of the mapped region.
    pub fbmem_addr: u64,
    /// Size of the mapped VRAM region in bytes.
    pub fbmem_size: u64,
    /// GPU virtual address within BAR1 aperture (from page table mapping).
    pub gpu_va_addr: u64,
    /// Size of the GPU VA region in bytes.
    pub gpu_va_size: u64,
}

#[expect(dead_code)]
impl Bar1Map {
    pub(crate) fn new(
        bar_user: &Arc<BarUser>,
        dev: &device::Device<device::Bound>,
        fbmem_addr: u64,
        fbmem_size: u64,
    ) -> Result<Self> {
        let num_pages = (fbmem_size as usize).div_ceil(PAGE_SIZE);
        let mut pfns = KVec::new();
        for i in 0..num_pages {
            let addr = fbmem_addr + (i * PAGE_SIZE) as u64;
            pfns.push(Pfn::from(VramAddress::new(addr)), GFP_KERNEL)?;
        }

        let mapped = bar_user.map_pages(dev, &pfns, true)?;
        let gpu_va_addr = mapped.vfn_start.raw() * PAGE_SIZE as u64;
        let gpu_va_size = mapped.num_pages as u64 * PAGE_SIZE as u64;

        Ok(Self {
            bar_user: bar_user.clone(),
            mapped,
            fbmem_addr,
            fbmem_size,
            gpu_va_addr,
            gpu_va_size,
        })
    }

    pub(crate) fn read32(&self, dev: &device::Device<device::Bound>, off: u64) -> Result<u32> {
        let bar1 = self.bar_user.bar1().access(dev)?;
        bar1.try_read32((self.gpu_va_addr + off) as usize)
    }

    pub(crate) fn write32(
        &self,
        dev: &device::Device<device::Bound>,
        off: u64,
        val: u32,
    ) -> Result {
        let bar1 = self.bar_user.bar1().access(dev)?;
        bar1.try_write32(val, (self.gpu_va_addr + off) as usize)
    }

    /// Explicitly destroy the mapping, releasing the GPU VA and invalidating PTEs.
    pub(crate) fn destroy(self, dev: &device::Device<device::Bound>) -> Result {
        self.bar_user.unmap_pages(dev, self.mapped)
    }
}
