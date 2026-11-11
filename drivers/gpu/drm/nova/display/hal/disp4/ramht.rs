// SPDX-License-Identifier: GPL-2.0

//! RAM hash table (RAMHT) bindings for DISP 4.x display channels.
//!
//! The RAMHT lives in instance memory and maps a Context DMA handle, client ID
//! and display channel to a descriptor offset. Display hardware uses this lookup
//! to resolve handles supplied by channel methods.
//!
//! Each entry is two 32-bit words (8 bytes). The 8 KiB table holds 1024 entries:
//!
//! ```text
//! +-------------+------+-------+--------------------+
//! | Byte offset | Word | Bits  | Field              |
//! +-------------+------+-------+--------------------+
//! | 0x00        | 0    | 31:0  | Context DMA handle |
//! | 0x04        | 1    | 13:0  | CLIENT_ID          |
//! | 0x04        | 1    | 24:14 | INSTANCE           |
//! | 0x04        | 1    | 31:25 | CHN                |
//! +-------------+------+-------+--------------------+
//! ```
//!
//! GSP-RM programs a DMA channel's RAMHT client ID into NV_PDISP_FE_PBCLIENT,
//! the channel's pushbuffer client register, during channel setup. For example:
//!
//! ```text
//! Display channel 1
//!   CHN = 1, PBCLIENT(1).CLIENT_ID = 3
//!       |
//!       | executes SET_CONTEXT_DMA_ISO(0), data = 7
//!       v
//! RAMHT lookup
//!   handle = 7, CLIENT_ID = 3, CHN = 1
//!       |
//!       v
//! Matched entry: INSTANCE = 0x102
//!       |
//!       v
//! Descriptor at instance_memory_base + 0x2040
//! ```
//!
//! INSTANCE is a descriptor offset in 32-byte units; zero marks an invalid entry.

use kernel::{
    prelude::*,
    sizes::SZ_8K, //
};

use nova_core::api::VramMapping;

use super::context_dma::{
    ContextDmaDescriptorId,
    ContextDmaPool, //
};

/// RAMHT table and its active descriptor bindings.
pub(super) struct RamhtPool {
    entries: KVec<Option<RamhtEntry>>,
}

/// One descriptor binding for a client and display channel.
struct RamhtEntry {
    /// Descriptor allocation referenced by this binding.
    context_dma_descriptor: ContextDmaDescriptorId,
    /// Encoded client, descriptor offset and display channel.
    encoded_word1: u32,
}

impl RamhtEntry {
    const SIZE: usize = 2 * size_of::<u32>();
    const CLIENT_MASK: u32 = 0x3fff;
    const CHANNEL_MASK: u32 = 0x7f;
    const CHANNEL_SHIFT: u32 = 25;
    const INSTANCE_SHIFT: u32 = 14;
}

impl RamhtEntry {
    fn new(context_dma: ContextDmaDescriptorId, channel: u32) -> Self {
        let offset = ContextDmaPool::BASE + context_dma.index * ContextDmaPool::STRIDE;
        // CAST: The descriptor offset is within the 64 KiB instance-memory region.
        let instance = (offset / ContextDmaPool::STRIDE) as u32;
        let encoded_word1 = channel | (instance << Self::INSTANCE_SHIFT);

        Self {
            context_dma_descriptor: context_dma,
            encoded_word1,
        }
    }

    fn encode_channel(ramht_client_id: u32, display_channel: u32) -> Result<u32> {
        if ramht_client_id > Self::CLIENT_MASK || display_channel > Self::CHANNEL_MASK {
            return Err(EINVAL);
        }
        Ok(ramht_client_id | (display_channel << Self::CHANNEL_SHIFT))
    }

    fn matches_channel(&self, channel: u32) -> bool {
        let mask = Self::CLIENT_MASK | (Self::CHANNEL_MASK << Self::CHANNEL_SHIFT);
        self.encoded_word1 & mask == channel
    }
}

impl RamhtPool {
    const COUNT: usize = SZ_8K / RamhtEntry::SIZE;

    fn hash(handle: u32, ramht_client_id: u32, display_channel: u32) -> usize {
        let hash = (handle & 0x3ff)
            ^ ((handle >> 10) & 0x3ff)
            ^ ((handle >> 20) & 0x3ff)
            ^ (((ramht_client_id & 0xff) << 2) | (handle >> 30))
            ^ (((display_channel & 0xf) << 6) | ((ramht_client_id >> 8) & 0x3f))
            ^ ((display_channel >> 4) & 0x7);
        // CAST: The hash is a 10-bit RAMHT entry index.
        hash as usize
    }

    fn find_entry(&self, handle: u32, channel: u32) -> Option<usize> {
        for (index, entry) in self.entries.iter().enumerate() {
            let Some(entry) = entry else {
                continue;
            };
            if entry.context_dma_descriptor.handle == handle && entry.matches_channel(channel) {
                return Some(index);
            }
        }
        None
    }

    fn find_free_entry(
        &self,
        handle: u32,
        ramht_client_id: u32,
        display_channel: u32,
    ) -> Result<usize> {
        let hash = Self::hash(handle, ramht_client_id, display_channel);
        let mut index = hash;
        while self.entries[index].is_some() {
            index += 1;
            if index == Self::COUNT {
                index = 0;
            }
            if index == hash {
                return Err(ENOSPC);
            }
        }
        Ok(index)
    }

    fn write_entry(
        instance_memory_mapping: &VramMapping<'_>,
        index: usize,
        entry: Option<&RamhtEntry>,
    ) -> Result {
        let offset = index * RamhtEntry::SIZE;
        match entry {
            Some(entry) => {
                instance_memory_mapping.write32(entry.context_dma_descriptor.handle, offset)?;
                instance_memory_mapping.write32(entry.encoded_word1, offset + size_of::<u32>())
            }
            None => {
                instance_memory_mapping.write32(0, offset + size_of::<u32>())?;
                instance_memory_mapping.write32(0, offset)
            }
        }
    }

    fn clear_entry(
        &mut self,
        instance_memory_mapping: &VramMapping<'_>,
        context_dma_pool: &mut ContextDmaPool,
        index: usize,
    ) -> Result {
        let context_dma = self.entries[index]
            .as_ref()
            .ok_or(ENOENT)?
            .context_dma_descriptor;
        let descriptor = context_dma_pool.lookup(context_dma)?;
        let bindings = descriptor.bindings.checked_sub(1).ok_or(EINVAL)?;
        Self::write_entry(instance_memory_mapping, index, None)?;
        descriptor.bindings = bindings;
        self.entries[index] = None;
        Ok(())
    }
}

impl RamhtPool {
    pub(super) fn new() -> Result<Self> {
        let mut entries = KVec::with_capacity(Self::COUNT, GFP_KERNEL)?;
        for _ in 0..Self::COUNT {
            entries.push(None, GFP_KERNEL)?;
        }
        Ok(Self { entries })
    }

    pub(super) fn bind(
        &mut self,
        instance_memory_mapping: &VramMapping<'_>,
        context_dma_pool: &mut ContextDmaPool,
        context_dma: ContextDmaDescriptorId,
        ramht_client_id: u32,
        display_channel: u32,
    ) -> Result {
        let descriptor = context_dma_pool.lookup(context_dma)?;
        let bindings = descriptor.bindings.checked_add(1).ok_or(EOVERFLOW)?;
        let channel = RamhtEntry::encode_channel(ramht_client_id, display_channel)?;
        if self.find_entry(context_dma.handle, channel).is_some() {
            return Err(EEXIST);
        }
        let index = self.find_free_entry(context_dma.handle, ramht_client_id, display_channel)?;
        let entry = RamhtEntry::new(context_dma, channel);

        Self::write_entry(instance_memory_mapping, index, Some(&entry))?;
        descriptor.bindings = bindings;
        self.entries[index] = Some(entry);
        Ok(())
    }

    pub(super) fn unbind(
        &mut self,
        instance_memory_mapping: &VramMapping<'_>,
        context_dma_pool: &mut ContextDmaPool,
        handle: u32,
        ramht_client_id: u32,
        display_channel: u32,
    ) -> Result {
        let channel = RamhtEntry::encode_channel(ramht_client_id, display_channel)?;
        let index = self.find_entry(handle, channel).ok_or(ENOENT)?;
        self.clear_entry(instance_memory_mapping, context_dma_pool, index)
    }

    pub(super) fn unbind_all_for_channel(
        &mut self,
        instance_memory_mapping: &VramMapping<'_>,
        context_dma_pool: &mut ContextDmaPool,
        ramht_client_id: u32,
        display_channel: u32,
    ) -> Result {
        let channel = RamhtEntry::encode_channel(ramht_client_id, display_channel)?;
        for index in 0..Self::COUNT {
            if self.entries[index]
                .as_ref()
                .is_some_and(|entry| entry.matches_channel(channel))
            {
                self.clear_entry(instance_memory_mapping, context_dma_pool, index)?;
            }
        }
        Ok(())
    }
}
