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
    device,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_64K,
    sync::{
        Arc,
        Mutex, //
    },
};

use crate::{
    display::hal::disp4::context_dma::{
        ContextDmaDescriptorId,
        ContextDmaPool, //
    },
    gsp::{
        cmdq::Cmdq,
        commands::gmc::DisplaySetInstanceMemory, //
    },
    mm::{
        bar_user::{
            BarMapping,
            BarUser, //
        },
        vram::VramRegion,
        GpuMm, //
    },
    num::IntoSafeCast, //
};

pub(crate) struct InstanceMemory<'a, 'gpu> {
    mapping: BarMapping<'a, 'gpu>,
    context_dma_pool: ContextDmaPool,
}

impl InstanceMemory<'_, '_> {
    /// Size and physical alignment of the instance-memory region.
    const INSTANCE_MEMORY_SIZE: Alignment = Alignment::new::<SZ_64K>();

    /// Exclusive physical byte-address limit of the instance-memory descriptor.
    // NV_PDISP_FE_INST_MEM1.ADDR holds bits 46:16 of the physical address.
    const INSTANCE_MEMORY_ADDRESS_LIMIT: u64 = 1 << 47;
}

impl<'a, 'gpu> InstanceMemory<'a, 'gpu> {
    pub(crate) fn new(
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
    ) -> Result<Self> {
        let backing = mm.lock().alloc_vram(Self::INSTANCE_MEMORY_SIZE)?;
        let end = backing
            .address()
            .checked_add(backing.size)
            .ok_or(EOVERFLOW)?;
        if end > Self::INSTANCE_MEMORY_ADDRESS_LIMIT {
            return Err(ERANGE);
        }

        let backing = Arc::new(backing, GFP_KERNEL)?;
        let region = backing.region(0..backing.size)?;
        let context_dma_pool = ContextDmaPool::new(region.clone())?;
        let mapping = BarMapping::new(bar_user, mm, region, true)?;

        for offset in (0..mapping.size()).step_by(size_of::<u32>()) {
            mapping.try_write32(0, offset)?;
        }

        let instance_size = mapping.size().into_safe_cast();
        let command = DisplaySetInstanceMemory::new(mapping.region().address(), instance_size)?;
        cmdq.send_gmc_command(command, |_| Ok(()))
            .inspect_err(|error| {
                dev_err!(dev, "SET_INSTANCE_MEMORY failed: {:?}\n", error);
            })?;

        Ok(Self {
            mapping,
            context_dma_pool,
        })
    }

    #[expect(dead_code)]
    pub(crate) fn alloc_context_dma(
        &mut self,
        target_region: VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<ContextDmaDescriptorId> {
        self.context_dma_pool
            .alloc(&self.mapping, target_region, writable, block_linear)
    }

    #[expect(dead_code)]
    pub(crate) fn free_context_dma(&mut self, context_dma: ContextDmaDescriptorId) -> Result {
        self.context_dma_pool.free(context_dma)
    }
}
