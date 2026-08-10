// SPDX-License-Identifier: GPL-2.0

//! VRAM allocation and allocation-relative region helpers.

use core::ops::Range;

use kernel::{
    gpu::buddy::GpuBuddyAllocFlags,
    prelude::*,
    ptr::Alignment,
    sync::Arc, //
};

use super::{
    placement::PlacementRef,
    Buddy,
    BuddyAllocation,
    GpuMmAllocator,
    PAGE_SIZE, //
};

/// A physically contiguous VRAM allocation with shared RAII ownership.
///
/// Subregions and BAR1 mappings retain an [`Arc`] to this object, preventing
/// the buddy allocation from being returned while any mapping can still
/// access it.
pub(crate) struct VramBlock {
    _allocation: BuddyAllocation,
    address: u64,
    size: u64,
}

impl VramBlock {
    /// Return the physical start address of the allocation.
    pub(crate) const fn address(&self) -> u64 {
        self.address
    }

    /// Return the allocation size in bytes.
    pub(crate) const fn size(&self) -> u64 {
        self.size
    }

    pub(super) fn range(&self) -> Result<Range<u64>> {
        let end = self.address.checked_add(self.size).ok_or(EOVERFLOW)?;
        Ok(self.address..end)
    }

    /// Create a checked allocation-relative region.
    pub(crate) fn region(self: &Arc<Self>, range: Range<u64>) -> Result<VramRegion> {
        VramRegion::new(self.clone(), range)
    }

    /// Create a region spanning the complete allocation.
    pub(crate) fn full_region(self: &Arc<Self>) -> VramRegion {
        VramRegion {
            owner: VramRegionOwner::Buddy(self.clone()),
            address: self.address,
            size: self.size,
        }
    }

    /// Allocate an aligned, physically contiguous block.
    pub(super) fn alloc_aligned(
        allocator: &GpuMmAllocator<Buddy>,
        size: u64,
        align: u64,
    ) -> Result<Arc<Self>> {
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
        if size == 0 || !size.is_multiple_of(page_size) {
            return Err(EINVAL);
        }

        let align = align.max(page_size);
        let align_usize = usize::try_from(align).map_err(|_| EOVERFLOW)?;
        Alignment::new_checked(align_usize).ok_or(EINVAL)?;
        let allocation = allocator.reserve_aligned(
            size,
            align,
            Alignment::new::<PAGE_SIZE>(),
            GpuBuddyAllocFlags::default(),
        )?;
        Self::from_allocation(allocation, size, align, None)
    }

    /// Allocate one exact physical range.
    pub(super) fn alloc_range(
        allocator: &GpuMmAllocator<Buddy>,
        range: Range<u64>,
        align: u64,
    ) -> Result<Arc<Self>> {
        let expected_address = range.start;
        let (size, min_block_size, align) = validate_allocation(&range, align)?;
        if !expected_address.is_multiple_of(align) {
            return Err(EINVAL);
        }
        let allocation =
            allocator.reserve_range(range, min_block_size, GpuBuddyAllocFlags::default())?;
        Self::from_allocation(allocation, size, align, Some(expected_address))
    }

    fn from_allocation(
        allocation: BuddyAllocation,
        size: u64,
        align: u64,
        expected_address: Option<u64>,
    ) -> Result<Arc<Self>> {
        let mut address = None;
        let mut allocation_end = None;
        let mut covered = 0u64;
        for block in allocation.blocks().iter() {
            let block_address = block.offset();
            let block_size = block.size();
            let block_end = block_address.checked_add(block_size).ok_or(EOVERFLOW)?;
            address = Some(address.map_or(block_address, |start: u64| start.min(block_address)));
            allocation_end = Some(allocation_end.map_or(block_end, |end: u64| end.max(block_end)));
            covered = covered.checked_add(block_size).ok_or(EOVERFLOW)?;
        }

        let address = address.ok_or(ENOMEM)?;
        let allocation_end = allocation_end.ok_or(ENOMEM)?;
        if expected_address.is_some_and(|expected| address != expected)
            || covered != size
            || allocation_end.checked_sub(address).ok_or(EIO)? != size
            || !address.is_multiple_of(align)
        {
            return Err(EIO);
        }

        Ok(Arc::new(
            Self {
                _allocation: allocation,
                address,
                size,
            },
            GFP_KERNEL,
        )?)
    }
}

/// Reference that keeps a VRAM region in use.
#[derive(Clone)]
enum VramRegionOwner {
    Buddy(Arc<VramBlock>),
    Placement(Arc<PlacementRef>),
}

/// A byte range kept in use by a buddy allocation or placement reference.
#[derive(Clone)]
pub(crate) struct VramRegion {
    owner: VramRegionOwner,
    address: u64,
    size: u64,
}

impl VramRegion {
    fn new(backing: Arc<VramBlock>, range: Range<u64>) -> Result<Self> {
        let size = range
            .end
            .checked_sub(range.start)
            .filter(|size| *size != 0)
            .ok_or(EINVAL)?;
        if range.end > backing.size {
            return Err(EINVAL);
        }
        let address = backing.address.checked_add(range.start).ok_or(EOVERFLOW)?;
        backing.address.checked_add(range.end).ok_or(EOVERFLOW)?;

        Ok(Self {
            owner: VramRegionOwner::Buddy(backing),
            address,
            size,
        })
    }

    /// Create a physical region whose lifetime is tied to a placement.
    pub(crate) fn from_placement(
        placement_ref: Arc<PlacementRef>,
        range: Range<u64>,
    ) -> Result<Self> {
        if !placement_ref.contains_range(&range) {
            return Err(EINVAL);
        }
        let size = range
            .end
            .checked_sub(range.start)
            .filter(|size| *size != 0)
            .ok_or(EINVAL)?;
        Ok(Self {
            owner: VramRegionOwner::Placement(placement_ref),
            address: range.start,
            size,
        })
    }

    /// Return the physical address of the first byte in this region.
    pub(crate) const fn address(&self) -> u64 {
        self.address
    }

    /// Return the region size in bytes.
    pub(crate) const fn size(&self) -> u64 {
        self.size
    }

    /// Return a checked subregion relative to this region.
    pub(crate) fn subregion(&self, range: Range<u64>) -> Result<Self> {
        let size = range
            .end
            .checked_sub(range.start)
            .filter(|size| *size != 0)
            .ok_or(EINVAL)?;
        if range.end > self.size {
            return Err(EINVAL);
        }
        let address = self.address.checked_add(range.start).ok_or(EOVERFLOW)?;
        address.checked_add(size).ok_or(EOVERFLOW)?;

        Ok(Self {
            owner: self.owner.clone(),
            address,
            size,
        })
    }
}

fn validate_allocation(range: &Range<u64>, align: u64) -> Result<(u64, Alignment, u64)> {
    let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
    let size = range
        .end
        .checked_sub(range.start)
        .filter(|size| *size != 0)
        .ok_or(EINVAL)?;
    if !range.start.is_multiple_of(page_size) || !size.is_multiple_of(page_size) {
        return Err(EINVAL);
    }

    let align = align.max(page_size);
    let align_usize = usize::try_from(align).map_err(|_| EOVERFLOW)?;
    Alignment::new_checked(align_usize).ok_or(EINVAL)?;
    let min_block_size = Alignment::new::<PAGE_SIZE>();

    Ok((size, min_block_size, align))
}
