// SPDX-License-Identifier: GPL-2.0

//! VRAM allocation helpers backed by the GPU buddy allocator.

use kernel::{
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddyAllocFlag,
        GpuBuddyAllocMode, //
    },
    prelude::*,
    ptr::Alignment, //
};

use super::GpuMm;

/// VRAM allocation with RAII lifetime. Drop frees blocks back to buddy allocator.
#[expect(dead_code)]
pub(crate) struct VramBlock {
    _blocks: Pin<KBox<AllocatedBlocks>>,
    pub addr: u64,
    pub size: u64,
}

#[expect(dead_code)]
pub(crate) fn alloc_vram(mm: &GpuMm, size: u64, align: u64) -> Result<VramBlock> {
    let min_block_size =
        Alignment::new_checked(core::cmp::max(align, 4096) as usize).ok_or(EINVAL)?;
    let blocks = KBox::pin_init(
        mm.buddy().alloc_blocks(
            GpuBuddyAllocMode::Simple,
            size,
            min_block_size,
            GpuBuddyAllocFlag::Contiguous,
        ),
        GFP_KERNEL,
    )?;
    let addr = blocks.as_ref().iter().next().ok_or(ENOMEM)?.offset();
    Ok(VramBlock {
        _blocks: blocks,
        addr,
        size,
    })
}
