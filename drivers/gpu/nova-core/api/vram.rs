// SPDX-License-Identifier: GPL-2.0

//! VRAM resources available through the core API.

use core::ops::Range;

use kernel::{
    prelude::*,
    ptr::Alignment,
    sync::Arc, //
};

use crate::mm::vram;

use super::NovaCoreApi;

impl NovaCoreApi<'_> {
    /// Allocates contiguous VRAM with a power-of-two size and matching alignment.
    ///
    /// The allocation must be at least 4 KiB. Its last owner returns it to the allocator.
    pub fn alloc_vram(&self, size: Alignment) -> Result<VramBlock> {
        Ok(VramBlock {
            block: Arc::new(self.gpu.alloc_vram(size)?, GFP_KERNEL)?,
            owner: self.vram_owner.clone(),
        })
    }
}

/// An owned, physically contiguous VRAM allocation.
///
/// Regions keep the allocation alive independently of this object.
/// The allocator metadata can outlive device unbind; hardware access cannot.
pub struct VramBlock {
    block: Arc<vram::VramBlock>,
    owner: Arc<()>,
}

impl VramBlock {
    /// Returns the physical address in GPU video memory.
    pub fn address(&self) -> u64 {
        self.block.address()
    }

    /// Returns the allocation size in bytes.
    pub fn size(&self) -> u64 {
        self.block.size
    }

    /// Creates a nonempty byte range relative to this allocation's start.
    pub fn region(&self, range: Range<u64>) -> Result<VramRegion> {
        Ok(VramRegion {
            region: self.block.region(range)?,
            owner: self.owner.clone(),
        })
    }
}

/// A byte range retaining its backing VRAM allocation.
#[derive(Clone)]
pub struct VramRegion {
    region: vram::VramRegion,
    owner: Arc<()>,
}

impl VramRegion {
    /// Returns the physical address of this region in GPU video memory.
    pub fn address(&self) -> u64 {
        self.region.address()
    }

    /// Returns the region size in bytes.
    pub fn size(&self) -> u64 {
        self.region.size()
    }

    /// Creates a nonempty byte range relative to this region's start.
    pub fn subregion(&self, range: Range<u64>) -> Result<Self> {
        Ok(Self {
            region: self.region.subregion(range)?,
            owner: self.owner.clone(),
        })
    }
}
