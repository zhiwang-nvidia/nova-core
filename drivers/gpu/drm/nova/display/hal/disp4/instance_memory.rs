// SPDX-License-Identifier: GPL-2.0

//! Display instance-memory backing for DISP 4.x.
//!
//! Instance memory stores the RAMHT and Context DMA descriptors. To fetch
//! framebuffer pixels, the display engine resolves a handle to a descriptor,
//! then uses its address range and attributes together with the surface offset.
//!
//! Layout of the 64 KiB region, with byte offsets from its aligned VRAM base:
//!
//! ```text
//! 0x00000 +------------------------------------------------+
//!         | RAMHT: 8 KiB (1024 entries x 8 bytes)          |
//!         | Handle + client/channel + descriptor offset    |
//! 0x02000 +------------------------------------------------+
//!         | Context DMA storage: 56 KiB                    |
//!         | 32-byte allocations, aligned to 32 bytes       |
//!         | Each descriptor uses 20 bytes                  |
//!         | Base/limit, memory target, access, layout      |
//! 0x10000 +------------------------------------------------+ (end)
//! ```
//!
//! The RAMHT encodes each descriptor's offset in units of 32 bytes.
//! Before submission, handle H must be bound to a descriptor for this channel.
//!
//! ```text
//! Driver framebuffer flip
//!                     |
//!                     v
//! Window DMA pushbuffer (C67E example)
//!   NVC67E_SET_CONTEXT_DMA_ISO(0) = H
//!   NVC67E_SET_OFFSET(0)          = surface_offset >> 8
//!   ... remaining surface state and UPDATE ...
//!                     |
//!                     | submit; display hardware executes the methods
//!                     v
//! Context DMA handle + client/channel identity
//!                     |
//!                     v
//! Instance memory: RAMHT entry ---> Context DMA descriptor
//!                                            |
//!                                base + surface offset
//!                                            |
//!                                            v
//!                                    Framebuffer memory
//! ```

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K, //
};

use nova_core::api::{
    NovaCoreApiHandle,
    VramMapping,
    VramRegion, //
};

pub(crate) struct InstanceMemory<'bound> {
    region: VramRegion,
    _mapping: VramMapping<'bound>,
}

impl<'bound> InstanceMemory<'bound> {
    /// Size and physical alignment of the instance-memory region.
    const INSTANCE_MEMORY_SIZE: Alignment = Alignment::new::<SZ_64K>();

    /// Exclusive physical byte-address limit of the instance-memory descriptor.
    // NV_PDISP_FE_INST_MEM1.ADDR holds bits 46:16 of the physical address.
    const INSTANCE_MEMORY_ADDRESS_LIMIT: u64 = 1 << 47;

    pub(crate) fn new(api: &NovaCoreApiHandle<'bound>) -> Result<Self> {
        let backing = api.with(|core| core.get_ref().alloc_vram(Self::INSTANCE_MEMORY_SIZE))?;
        let end = backing
            .address()
            .checked_add(backing.size())
            .ok_or(EOVERFLOW)?;
        if end > Self::INSTANCE_MEMORY_ADDRESS_LIMIT {
            return Err(ERANGE);
        }

        let region = backing.region(0..backing.size())?;
        let mapping = api.with(|core| core.get_ref().map_vram(region.clone(), true))?;
        for offset in (0..mapping.size()).step_by(size_of::<u32>()) {
            mapping.write32(0, offset)?;
        }

        Ok(Self {
            region,
            _mapping: mapping,
        })
    }

    pub(crate) fn region(&self) -> &VramRegion {
        &self.region
    }
}
