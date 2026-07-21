// SPDX-License-Identifier: GPL-2.0

//! VRAM slot allocation for vGPU instances.

#![expect(dead_code)]

use kernel::{
    bitmap::BitmapVec,
    prelude::*,
    sync::Arc, //
};

use crate::mm::{
    vram::{
        alloc_vram_range,
        VramBlock,
        VramRegion, //
    },
    GpuMm, //
};

const VRAM_SLOT_MIN_ALIGN: u64 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct VgpuVramLayout {
    pub type_id: u32,
    pub max_slots: u32,
    pub fb_size: u64,
    pub heap_size: u64,
    pub fb_align: u64,
}

impl VgpuVramLayout {
    fn validated(mut self) -> Result<Self> {
        if self.max_slots == 0 || self.fb_size == 0 || self.heap_size == 0 {
            return Err(EINVAL);
        }
        self.fb_align = core::cmp::max(self.fb_align, VRAM_SLOT_MIN_ALIGN);
        if !self.fb_align.is_power_of_two() {
            return Err(EINVAL);
        }
        if self.fb_size & (self.fb_align - 1) != 0
            || self.heap_size & (VRAM_SLOT_MIN_ALIGN - 1) != 0
        {
            return Err(EINVAL);
        }
        Ok(self)
    }
}

#[derive(Clone)]
pub(crate) struct VgpuVramSlot {
    pub index: u32,
    pub fbmem: VramRegion,
    pub mgmt_heap: VramRegion,
}

pub(super) struct VgpuVramSlotAllocator {
    backing: Arc<VramBlock>,
    layout: VgpuVramLayout,
    fb_region_size: u64,
    used: BitmapVec,
}

impl VgpuVramSlotAllocator {
    pub(super) fn new(mm: &GpuMm<'_>, layout: VgpuVramLayout) -> Result<Self> {
        let layout = layout.validated()?;
        let max_slots = u64::from(layout.max_slots);
        // Lay out all framebuffer slots first, followed by all management
        // heap slots: fbmem[0..N], heap[0..N].
        let fb_region_size = layout.fb_size.checked_mul(max_slots).ok_or(EINVAL)?;
        let heap_region_size = layout.heap_size.checked_mul(max_slots).ok_or(EINVAL)?;
        let pool_size = fb_region_size.checked_add(heap_region_size).ok_or(EINVAL)?;

        let used = BitmapVec::new(
            usize::try_from(layout.max_slots).map_err(|_| EINVAL)?,
            GFP_KERNEL,
        )?;
        let backing = alloc_vram_range(mm, 0..pool_size, VRAM_SLOT_MIN_ALIGN)?;
        if !backing.address().is_multiple_of(layout.fb_align) {
            return Err(EINVAL);
        }

        Ok(Self {
            backing,
            layout,
            fb_region_size,
            used,
        })
    }

    pub(super) fn matches_layout(&self, layout: VgpuVramLayout) -> Result<bool> {
        Ok(self.layout == layout.validated()?)
    }

    pub(super) fn alloc(&mut self, layout: VgpuVramLayout) -> Result<VgpuVramSlot> {
        if !self.matches_layout(layout)? {
            return Err(EBUSY);
        }

        let bitmap_index = self.used.next_zero_bit(0).ok_or(ENOSPC)?;
        let slot = u64::try_from(bitmap_index).map_err(|_| EINVAL)?;
        let fb_offset = self.layout.fb_size.checked_mul(slot).ok_or(EINVAL)?;
        let fb_end = fb_offset.checked_add(self.layout.fb_size).ok_or(EINVAL)?;
        let heap_offset = self
            .fb_region_size
            .checked_add(self.layout.heap_size.checked_mul(slot).ok_or(EINVAL)?)
            .ok_or(EINVAL)?;
        let heap_end = heap_offset
            .checked_add(self.layout.heap_size)
            .ok_or(EINVAL)?;
        let index = u32::try_from(bitmap_index).map_err(|_| EINVAL)?;
        let fbmem = self.backing.region(fb_offset..fb_end)?;
        let mgmt_heap = self.backing.region(heap_offset..heap_end)?;

        self.used.set_bit(bitmap_index);

        Ok(VgpuVramSlot {
            index,
            fbmem,
            mgmt_heap,
        })
    }

    pub(super) fn free(&mut self, index: u32) -> Result {
        let index = usize::try_from(index).map_err(|_| EINVAL)?;
        if index >= self.used.len() || self.used.next_bit(index) != Some(index) {
            return Err(EINVAL);
        }

        self.used.clear_bit(index);
        Ok(())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.used.last_bit().is_none()
    }
}
