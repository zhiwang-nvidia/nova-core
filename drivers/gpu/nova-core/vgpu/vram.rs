// SPDX-License-Identifier: GPL-2.0

//! VRAM placement allocation for non-MIG vGPU instances.

#![expect(dead_code)]

use core::ops::Range;

use kernel::prelude::*;

use crate::mm::{
    placement::{
        Placement,
        PlacementAllocation,
        PlacementEntry, //
    },
    vram::VramRegion,
    GpuMm,
    GpuMmAllocator, //
};

const VRAM_PLACEMENT_MIN_ALIGN: u64 = 4096;

/// VRAM requirements shared by all placements for one vGPU type.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct VgpuVramConfig {
    pub type_id: u32,
    pub max_slots: u32,
    pub fb_size: u64,
    pub heap_size: u64,
    pub fb_align: u64,
}

impl VgpuVramConfig {
    fn validated(mut self) -> Result<Self> {
        if self.max_slots == 0 || self.fb_size == 0 || self.heap_size == 0 {
            return Err(EINVAL);
        }

        self.fb_align = self.fb_align.max(VRAM_PLACEMENT_MIN_ALIGN);
        if !self.fb_align.is_power_of_two()
            || !self.fb_size.is_multiple_of(self.fb_align)
            || !self.heap_size.is_multiple_of(VRAM_PLACEMENT_MIN_ALIGN)
        {
            return Err(EINVAL);
        }
        Ok(self)
    }

    fn pool_sizes(self) -> Result<(u64, u64)> {
        let count = u64::from(self.max_slots);
        let total_fb_size = self.fb_size.checked_mul(count).ok_or(EOVERFLOW)?;
        let total_heap_size = self.heap_size.checked_mul(count).ok_or(EOVERFLOW)?;
        let pool_size = total_fb_size
            .checked_add(total_heap_size)
            .ok_or(EOVERFLOW)?;
        Ok((total_fb_size, pool_size))
    }
}

/// Concrete physical VRAM layout for one vGPU configuration.
///
/// Guest framebuffer ranges are followed by the management heap ranges:
///
/// ```text
/// base | fb[0] ... fb[N - 1] | heap[0] ... heap[N - 1] |
/// ```
pub(super) struct VgpuVramLayout {
    config: VgpuVramConfig,
    base: u64,
    total_fb_size: u64,
}

impl VgpuVramLayout {
    fn placement_map(&self) -> Result<KVec<PlacementEntry>> {
        let count = usize::try_from(self.config.max_slots).map_err(|_| EOVERFLOW)?;
        let mut entries = KVec::new();
        entries.reserve(count, GFP_KERNEL)?;

        for placement_id in 0..self.config.max_slots {
            let (fb_range, heap_range) = self.ranges(placement_id)?;
            let mut ranges = KVec::new();
            ranges.reserve(2, GFP_KERNEL)?;
            ranges.push_within_capacity(fb_range)?;
            ranges.push_within_capacity(heap_range)?;
            entries.push_within_capacity(PlacementEntry::new(placement_id, ranges)?)?;
        }
        Ok(entries)
    }

    fn slot(&self, placement: PlacementAllocation) -> Result<VgpuVramSlot> {
        let id = placement.id();
        let (expected_fb, expected_heap) = self.ranges(id)?;
        if placement.range_count() != 2
            || placement.range(0) != Some(&expected_fb)
            || placement.range(1) != Some(&expected_heap)
        {
            return Err(EIO);
        }

        let fbmem = VramRegion::from_placement(placement.placement_ref(), expected_fb)?;
        let mgmt_heap = VramRegion::from_placement(placement.placement_ref(), expected_heap)?;
        Ok(VgpuVramSlot {
            index: id,
            fbmem,
            mgmt_heap,
            placement,
        })
    }

    fn ranges(&self, id: u32) -> Result<(Range<u64>, Range<u64>)> {
        if id >= self.config.max_slots {
            return Err(EINVAL);
        }

        let index = u64::from(id);
        let fb_start = self
            .base
            .checked_add(self.config.fb_size.checked_mul(index).ok_or(EOVERFLOW)?)
            .ok_or(EOVERFLOW)?;
        let fb_end = fb_start.checked_add(self.config.fb_size).ok_or(EOVERFLOW)?;
        let heap_start = self
            .base
            .checked_add(self.total_fb_size)
            .and_then(|base| {
                self.config
                    .heap_size
                    .checked_mul(index)
                    .and_then(|offset| base.checked_add(offset))
            })
            .ok_or(EOVERFLOW)?;
        let heap_end = heap_start
            .checked_add(self.config.heap_size)
            .ok_or(EOVERFLOW)?;
        Ok((fb_start..fb_end, heap_start..heap_end))
    }
}

/// Owns the placement allocator for one vGPU VRAM configuration.
pub(super) struct VgpuVramAllocator {
    layout: VgpuVramLayout,
    allocator: GpuMmAllocator<Placement>,
}

impl VgpuVramAllocator {
    pub(super) fn new(mm: &GpuMm<'_>, config: VgpuVramConfig) -> Result<Self> {
        let config = config.validated()?;
        let (total_fb_size, pool_size) = config.pool_sizes()?;
        let backing = mm.alloc_core_vram(pool_size, config.fb_align)?;
        let layout = VgpuVramLayout {
            config,
            base: backing.address(),
            total_fb_size,
        };
        let entries = layout.placement_map()?;

        let mut backings = KVec::new();
        backings.push(backing, GFP_KERNEL)?;
        let allocator = GpuMmAllocator::<Placement>::new(backings, entries)?;

        Ok(Self { layout, allocator })
    }

    pub(super) fn matches_config(&self, config: VgpuVramConfig) -> Result<bool> {
        Ok(self.layout.config == config.validated()?)
    }

    pub(super) fn alloc(&self) -> Result<VgpuVramSlot> {
        for id in 0..self.layout.config.max_slots {
            match self.allocator.alloc(id) {
                Ok(placement) => return self.layout.slot(placement),
                Err(error) if error == ENOSPC => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ENOSPC)
    }

    pub(super) fn free(&self, placement: PlacementAllocation) -> Result {
        self.allocator.free(placement)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.allocator.is_empty()
    }
}

/// One framebuffer and management heap placement.
pub(crate) struct VgpuVramSlot {
    pub index: u32,
    pub fbmem: VramRegion,
    pub mgmt_heap: VramRegion,
    placement: PlacementAllocation,
}

impl VgpuVramSlot {
    /// Drop direct region references before returning the unique allocation.
    pub(super) fn into_placement(self) -> PlacementAllocation {
        let Self {
            placement,
            fbmem,
            mgmt_heap,
            ..
        } = self;
        drop(fbmem);
        drop(mgmt_heap);
        placement
    }
}
