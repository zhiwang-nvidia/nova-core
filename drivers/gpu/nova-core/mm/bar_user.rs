// SPDX-License-Identifier: GPL-2.0

//! BAR1 user interface for CPU access to GPU virtual memory. Used for USERD
//! for GPU work submission, and applications to access GPU buffers via mmap().

use kernel::{
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
        vram::VramRegion,
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
pub(crate) struct BarUser<'gpu> {
    #[pin]
    vmm: Mutex<Vmm>,
    bar1: Arc<Bar1<'gpu>>,
}

impl<'gpu> BarUser<'gpu> {
    /// Create a pin-initializer for [`BarUser`].
    pub(crate) fn new(
        pdb_addr: VramAddress,
        chipset: Chipset,
        va_size: u64,
        bar1: Bar1<'gpu>,
    ) -> Result<impl PinInit<Self> + 'gpu> {
        let vmm = Vmm::new(pdb_addr, chipset.mmu_version(), va_size)?;
        let bar1 = Arc::new(bar1, GFP_KERNEL)?;
        Ok(pin_init!(Self {
            vmm <- new_mutex!(vmm, "bar_user_vmm"),
            bar1,
        }))
    }

    /// Map physical pages to a contiguous BAR1 virtual range.
    pub(crate) fn map<'access>(
        &'access self,
        mm: &'access GpuMm<'gpu>,
        pfns: &[Pfn],
        writable: bool,
    ) -> Result<BarUserAccess<'access, 'gpu>> {
        if pfns.is_empty() {
            return Err(EINVAL);
        }
        let mut vmm = self.vmm.lock();
        let mapped = vmm.map_pages(mm, pfns, None, writable)?;

        Ok(BarUserAccess {
            bar_user: self,
            mm,
            mapped: Some(mapped),
        })
    }
}

/// Access object for a mapped BAR1 region.
pub(crate) struct BarUserAccess<'access, 'gpu> {
    bar_user: &'access BarUser<'gpu>,
    mm: &'access GpuMm<'gpu>,
    /// [`BarUserAccess::release`] [`Option::take`]s this; `Some` at
    /// drop time means `release()` was never called.
    mapped: Option<MappedRange>,
}

impl BarUserAccess<'_, '_> {
    /// Tear down the BAR1 mapping.
    pub(crate) fn release(mut self) -> Result {
        let mapped = self.mapped.take().ok_or(EINVAL)?;
        let mut vmm = self.bar_user.vmm.lock();
        vmm.unmap_pages(self.mm, mapped)?;
        Ok(())
    }

    /// Returns the active mapping.
    fn mapped(&self) -> &MappedRange {
        // `mapped` is only `None` after `take()` in `release`; hence unwrap()
        // cannot panic here.
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

    /// Translate an offset within this mapping to a BAR1 aperture offset.
    fn bar_offset(&self, offset: usize) -> Result<usize> {
        if offset >= self.size() {
            return Err(EINVAL);
        }

        let base_vfn: usize = self.mapped().vfn_start.raw().into_safe_cast();
        let base = base_vfn.checked_mul(PAGE_SIZE).ok_or(EOVERFLOW)?;
        base.checked_add(offset).ok_or(EOVERFLOW)
    }

    // Fallible accessors with runtime bounds checking.

    /// Read a 32-bit value at the given offset.
    pub(crate) fn try_read32(&self, offset: usize) -> Result<u32> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.as_ref().try_read32(off)
    }

    /// Write a 32-bit value at the given offset.
    pub(crate) fn try_write32(&self, value: u32, offset: usize) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.as_ref().try_write32(value, off)
    }

    /// Read a 64-bit value at the given offset.
    pub(crate) fn try_read64(&self, offset: usize) -> Result<u64> {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.as_ref().try_read64(off)
    }

    /// Write a 64-bit value at the given offset.
    pub(crate) fn try_write64(&self, value: u64, offset: usize) -> Result {
        let off = self.bar_offset(offset)?;
        self.bar_user.bar1.as_ref().try_write64(value, off)
    }
}

impl Drop for BarUserAccess<'_, '_> {
    fn drop(&mut self) {
        if self.mapped.is_some() {
            kernel::pr_warn!(
                "BarUserAccess dropped without calling release(). BarUser address space will leak.\n"
            );
        }
        // The inner `MappedRange`'s own `MustUnmapGuard` will also fire,
        // identifying the leaked VA range.
    }
}

/// An owned BAR1 mapping of a region within a live [`VramBlock`](super::vram::VramBlock).
///
/// The mapping retains the region's backing allocation, so the VRAM cannot be
/// returned to the buddy allocator while GPU PTEs still refer to it. The
/// logical region may start or end between page boundaries; the containing
/// pages are mapped, while all CPU accessors remain bounded to the requested
/// byte range.
pub(crate) struct Bar1Map<'gpu> {
    bar1: Arc<Bar1<'gpu>>,
    mapped: MappedRange,
    region: VramRegion,
    page_bias: usize,
    logical_size: usize,
}

impl<'gpu> Bar1Map<'gpu> {
    /// Map a VRAM allocation region through BAR1.
    pub(crate) fn new(
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
        region: VramRegion,
        writable: bool,
    ) -> Result<Self> {
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
        let region_start = region.address();
        let region_end = region_start.checked_add(region.size()).ok_or(EOVERFLOW)?;
        let map_start = region_start - region_start % page_size;
        let map_end =
            region_end.checked_add(page_size - 1).ok_or(EOVERFLOW)? / page_size * page_size;
        let map_size = map_end.checked_sub(map_start).ok_or(EINVAL)?;
        let num_pages = usize::try_from(map_size / page_size).map_err(|_| EOVERFLOW)?;
        if num_pages == 0 {
            return Err(EINVAL);
        }
        let page_bias = usize::try_from(region_start - map_start).map_err(|_| EOVERFLOW)?;
        let logical_size = usize::try_from(region.size()).map_err(|_| EOVERFLOW)?;

        let mut pfns = KVec::new();
        for page in 0..num_pages {
            let byte_offset = u64::try_from(page)
                .map_err(|_| EOVERFLOW)?
                .checked_mul(page_size)
                .ok_or(EOVERFLOW)?;
            let address = map_start.checked_add(byte_offset).ok_or(EOVERFLOW)?;
            pfns.push(Pfn::from(VramAddress::new(address)), GFP_KERNEL)?;
        }

        let mapped = bar_user.vmm.lock().map_pages(mm, &pfns, None, writable)?;

        Ok(Self {
            bar1: bar_user.bar1.clone(),
            mapped,
            region,
            page_bias,
            logical_size,
        })
    }

    /// Return the mapped physical VRAM region.
    pub(crate) fn region(&self) -> &VramRegion {
        &self.region
    }

    /// Return the logical GPU virtual address visible through BAR1.
    pub(crate) fn gpu_va_addr(&self) -> Result<u64> {
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
        let base = self
            .mapped
            .vfn_start
            .raw()
            .checked_mul(page_size)
            .ok_or(EOVERFLOW)?;
        base.checked_add(u64::try_from(self.page_bias).map_err(|_| EOVERFLOW)?)
            .ok_or(EOVERFLOW)
    }

    /// Return the requested logical mapping size.
    pub(crate) const fn size(&self) -> usize {
        self.logical_size
    }

    fn bar_offset(&self, offset: usize, access_width: usize) -> Result<usize> {
        let access_end = offset.checked_add(access_width).ok_or(EOVERFLOW)?;
        if access_end > self.logical_size {
            return Err(EINVAL);
        }

        let base_vfn = usize::try_from(self.mapped.vfn_start.raw()).map_err(|_| EOVERFLOW)?;
        let base = base_vfn.checked_mul(PAGE_SIZE).ok_or(EOVERFLOW)?;
        let logical_base = base.checked_add(self.page_bias).ok_or(EOVERFLOW)?;
        let bar_offset = logical_base.checked_add(offset).ok_or(EOVERFLOW)?;
        let bar_end = bar_offset.checked_add(access_width).ok_or(EOVERFLOW)?;
        if bar_end > self.bar1.size() || !bar_offset.is_multiple_of(access_width) {
            return Err(EINVAL);
        }

        Ok(bar_offset)
    }

    pub(crate) fn try_read32(&self, offset: usize) -> Result<u32> {
        self.bar1
            .as_ref()
            .try_read32(self.bar_offset(offset, size_of::<u32>())?)
    }

    pub(crate) fn try_write8(&self, value: u8, offset: usize) -> Result {
        self.bar1
            .as_ref()
            .try_write8(value, self.bar_offset(offset, size_of::<u8>())?)
    }

    pub(crate) fn try_write32(&self, value: u32, offset: usize) -> Result {
        self.bar1
            .as_ref()
            .try_write32(value, self.bar_offset(offset, size_of::<u32>())?)
    }

    pub(crate) fn try_read64(&self, offset: usize) -> Result<u64> {
        self.bar1
            .as_ref()
            .try_read64(self.bar_offset(offset, size_of::<u64>())?)
    }

    pub(crate) fn try_write64(&self, value: u64, offset: usize) -> Result {
        self.bar1
            .as_ref()
            .try_write64(value, self.bar_offset(offset, size_of::<u64>())?)
    }

    /// Invalidate the PTEs and release the BAR1 virtual address.
    ///
    /// The backing [`VramRegion`] remains owned until unmapping completes.
    pub(crate) fn destroy(self, bar_user: &BarUser<'gpu>, mm: &GpuMm<'gpu>) -> Result {
        let result = bar_user.vmm.lock().unmap_pages(mm, self.mapped);
        drop(self.region);
        result
    }
}
