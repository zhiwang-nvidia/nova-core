// SPDX-License-Identifier: GPL-2.0

//! Context DMA descriptors for DISP 4.x display channels.
//!
//! A descriptor defines a buffer's address range, memory target, access permissions
//! and layout. The descriptor lives in instance memory; image pixels and
//! notification data reside in the target buffer.
//!
//! A hardware descriptor contains five 32-bit words (20 bytes) in a 32-byte slot:
//!
//! ```text
//! +-------------+------+------+----------------------------------------+
//! | Byte offset | Word | Bits | Field                                  |
//! +-------------+------+------+----------------------------------------+
//! | 0x00        | 0    | 1:0  | TARGET_NODE                            |
//! |             |      |      |   1 = VRAM                             |
//! |             |      |      |   2 = PCI non-coherent                 |
//! |             |      |      |   3 = PCI coherent                     |
//! | 0x00        | 0    | 2    | ACCESS (0 = read-only, 1 = read/write) |
//! | 0x00        | 0    | 20   | KIND (0 = pitch, 1 = block-linear)     |
//! | 0x04        | 1    | 31:0 | BASE_LO = base[39:8]                   |
//! | 0x08        | 2    | 6:0  | BASE_HI = base[46:40]                  |
//! | 0x0c        | 3    | 31:0 | LIMIT_LO = limit[39:8]                 |
//! | 0x10        | 4    | 6:0  | LIMIT_HI = limit[46:40]                |
//! | 0x14..0x1f  |      |      | Slot padding                           |
//! +-------------+------+------+----------------------------------------+
//! ```
//!
//! `base` and `limit` are physical byte addresses.

use kernel::{
    prelude::*,
    sizes::{
        SizeConstants,
        SZ_64K,
        SZ_8K, //
    }, //
};

use nova_core::api::{
    VramMapping,
    VramRegion, //
};

/// Context DMA descriptor pool for descriptors + handles.
pub(super) struct ContextDmaPool {
    descriptors: KVec<Option<ContextDmaDescriptor>>,
    /// Complete instance-memory region.
    instance_memory_region: VramRegion,
    /// Last successfully allocated handle.
    last_handle: u32,
}

/// Identifies one descriptor allocation within its instance-memory pool.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextDmaDescriptorId {
    /// Handle used by channel methods and RAMHT lookup.
    pub(crate) handle: u32,
    /// Slot index within the pool.
    index: usize,
}

/// A context DMA descriptor.
struct ContextDmaDescriptor {
    id: ContextDmaDescriptorId,
    /// Target VRAM region described by this descriptor.
    _target_region: VramRegion,
}

impl ContextDmaDescriptor {
    const ADDRESS_LIMIT: u64 = 1 << 47;
    const ADDRESS_SHIFT: u32 = 8;
    const TARGET_PHYSICAL_NVM: u32 = 1;
    const ACCESS_READ_WRITE: u32 = 1 << 2;
    const KIND_BLOCK_LINEAR: u32 = 1 << 20;
}

impl ContextDmaDescriptor {
    fn encode_descriptor(
        target_region: &VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<[u32; 5]> {
        let alignment = u64::SZ_4K;
        if !target_region.address().is_multiple_of(alignment)
            || !target_region.size().is_multiple_of(alignment)
        {
            return Err(EINVAL);
        }
        let limit = target_region
            .address()
            .checked_add(target_region.size() - 1)
            .ok_or(EOVERFLOW)?;
        if limit >= Self::ADDRESS_LIMIT {
            return Err(ERANGE);
        }

        let mut attributes = Self::TARGET_PHYSICAL_NVM;
        if writable {
            attributes |= Self::ACCESS_READ_WRITE;
        }
        if block_linear {
            attributes |= Self::KIND_BLOCK_LINEAR;
        }

        let base = target_region.address() >> Self::ADDRESS_SHIFT;
        let limit = limit >> Self::ADDRESS_SHIFT;
        // CAST: Split the encoded address into low 32 bits and at most seven high bits.
        Ok([
            attributes,
            base as u32,
            (base >> u32::BITS) as u32,
            limit as u32,
            (limit >> u32::BITS) as u32,
        ])
    }
}

impl ContextDmaPool {
    const BASE: usize = SZ_8K;
    const STRIDE: usize = 32;
    const COUNT: usize = (SZ_64K - Self::BASE) / Self::STRIDE;
}

impl ContextDmaPool {
    pub(super) fn new(instance_memory_region: VramRegion) -> Result<Self> {
        let mut descriptors = KVec::with_capacity(Self::COUNT, GFP_KERNEL)?;
        for _ in 0..Self::COUNT {
            descriptors.push(None, GFP_KERNEL)?;
        }

        Ok(Self {
            descriptors,
            instance_memory_region,
            last_handle: 0,
        })
    }

    pub(super) fn alloc(
        &mut self,
        instance_memory_mapping: &VramMapping<'_>,
        target_region: VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<ContextDmaDescriptorId> {
        let words =
            ContextDmaDescriptor::encode_descriptor(&target_region, writable, block_linear)?;
        let handle = self.last_handle.checked_add(1).ok_or(EOVERFLOW)?;
        // The first slot is reserved.
        let mut index = 1;
        while index < self.descriptors.len() && self.descriptors[index].is_some() {
            index += 1;
        }
        if index == self.descriptors.len() {
            return Err(ENOSPC);
        }
        let offset = Self::BASE + index * Self::STRIDE;
        let end = u64::try_from(offset + Self::STRIDE).map_err(|_| EOVERFLOW)?;
        if end > self.instance_memory_region.size() {
            return Err(EINVAL);
        }

        let id = ContextDmaDescriptorId { handle, index };
        let context_dma = ContextDmaDescriptor {
            id,
            _target_region: target_region,
        };

        for (word, value) in words.into_iter().enumerate() {
            instance_memory_mapping.write32(value, offset + word * size_of::<u32>())?;
        }

        self.descriptors[index] = Some(context_dma);
        self.last_handle = handle;
        Ok(id)
    }

    fn lookup(&mut self, id: ContextDmaDescriptorId) -> Result<&mut ContextDmaDescriptor> {
        self.descriptors
            .get_mut(id.index)
            .and_then(Option::as_mut)
            .filter(|descriptor| descriptor.id == id)
            .ok_or(ENOENT)
    }

    pub(super) fn free(&mut self, id: ContextDmaDescriptorId) -> Result {
        self.lookup(id)?;
        self.descriptors[id.index] = None;
        Ok(())
    }
}
