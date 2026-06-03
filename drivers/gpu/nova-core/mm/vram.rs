// SPDX-License-Identifier: GPL-2.0

//! VRAM allocation and allocation-relative region helpers.

use core::ops::Range;

use kernel::{
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddyAllocFlags,
        GpuBuddyAllocMode, //
    },
    prelude::*,
    ptr::Alignment,
    sync::Arc, //
};

use super::{
    GpuMm,
    PAGE_SIZE, //
};

/// A physically contiguous VRAM allocation with shared RAII ownership.
///
/// Subregions and BAR1 mappings retain an [`Arc`] to this object, preventing
/// the buddy allocation from being returned while any mapping can still
/// access it.
pub(crate) struct VramBlock {
    _blocks: Pin<KBox<AllocatedBlocks>>,
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

    /// Create a checked allocation-relative region.
    pub(crate) fn region(self: &Arc<Self>, range: Range<u64>) -> Result<VramRegion> {
        VramRegion::new(self.clone(), range)
    }

    /// Create a region spanning the complete allocation.
    pub(crate) fn full_region(self: &Arc<Self>) -> VramRegion {
        VramRegion {
            backing: self.clone(),
            address: self.address,
            size: self.size,
        }
    }
}

/// A byte range within a shared [`VramBlock`].
#[derive(Clone)]
pub(crate) struct VramRegion {
    backing: Arc<VramBlock>,
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
            backing,
            address,
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
            backing: self.backing.clone(),
            address,
            size,
        })
    }
}

/// Allocate an exact VRAM range relative to a usable region's buddy base.
pub(crate) fn alloc_vram_range(
    mm: &GpuMm<'_>,
    range: Range<u64>,
    align: u64,
) -> Result<Arc<VramBlock>> {
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
    let min_block_size = Alignment::new_checked(align_usize).ok_or(EINVAL)?;

    for buddy in &mm.buddies {
        if range.end > buddy.size() {
            continue;
        }

        let blocks = match KBox::pin_init(
            buddy.alloc_blocks(
                GpuBuddyAllocMode::Range(range.clone()),
                size,
                min_block_size,
                GpuBuddyAllocFlags::default(),
            ),
            GFP_KERNEL,
        ) {
            Ok(blocks) => blocks,
            Err(error) if error == ENOSPC => continue,
            Err(error) => return Err(error),
        };

        let mut address = None;
        let mut allocation_end = None;
        let mut covered = 0u64;
        for block in blocks.as_ref().iter() {
            let block_address = block.offset();
            let block_size = block.size();
            let block_end = block_address.checked_add(block_size).ok_or(EOVERFLOW)?;
            address = Some(address.map_or(block_address, |start: u64| start.min(block_address)));
            allocation_end = Some(allocation_end.map_or(block_end, |end: u64| end.max(block_end)));
            covered = covered.checked_add(block_size).ok_or(EOVERFLOW)?;
        }

        let address = address.ok_or(ENOMEM)?;
        let allocation_end = allocation_end.ok_or(ENOMEM)?;
        let expected_address = buddy
            .base_offset()
            .checked_add(range.start)
            .ok_or(EOVERFLOW)?;
        if address != expected_address
            || covered != size
            || allocation_end.checked_sub(address).ok_or(EIO)? != size
            || !address.is_multiple_of(align)
        {
            return Err(EIO);
        }

        return Ok(Arc::new(
            VramBlock {
                _blocks: blocks,
                address,
                size,
            },
            GFP_KERNEL,
        )?);
    }

    Err(ENOSPC)
}
