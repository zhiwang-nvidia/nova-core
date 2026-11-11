// SPDX-License-Identifier: GPL-2.0

//! VRAM resources available through the core API.

use core::ops::Range;

use kernel::{
    io::{
        Io,
        IoBackend,
        IoBase,
        MmioBackend, //
    },
    new_mutex,
    page::PAGE_SIZE,
    prelude::*,
    ptr::Alignment,
    sync::{
        Arc,
        Mutex, //
    }, //
};

use crate::{
    driver::{
        self,
        Bar1, //
    },
    mm::{
        bar_user::BarMapping,
        vram, //
    }, //
};

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

    /// Maps a region of this GPU's VRAM for CPU access through BAR1.
    ///
    /// The mapping retains the region and cannot outlive the device binding.
    /// A region from another GPU or an earlier binding is rejected before mapping.
    pub fn map_vram<'a>(&'a self, region: VramRegion, writable: bool) -> Result<VramMapping<'a>> {
        if !Arc::ptr_eq(&self.vram_owner, &region.owner) {
            return Err(EXDEV);
        }

        let bar_index = driver::bar1_resource_index(self.pdev)?;
        let physical_base = self.pdev.resource_start(bar_index)?;
        let mapping = self.gpu.map_vram(region.region, writable)?;
        Ok(VramMapping {
            mapping: Arc::pin_init(new_mutex!(mapping, "vram_mapping"), GFP_KERNEL)?,
            bar1: self.gpu.vram_bar(),
            physical_base,
            writable,
        })
    }
}

/// An owned, physically contiguous VRAM allocation.
///
/// Regions and mappings keep the allocation alive independently of this object.
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
    pub(super) owner: Arc<()>,
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

/// A CPU mapping of VRAM through BAR1.
///
/// Accesses are bounded by the mapped region. Accessing or dropping the mapping may sleep.
/// Dropping it unmaps the region.
/// This does not stop GPU engines using the allocation; their owner must stop them first.
pub struct VramMapping<'bound> {
    mapping: Arc<dyn Mapping + 'bound>,
    bar1: Bar1<'bound>,
    physical_base: u64,
    writable: bool,
}

impl VramMapping<'_> {
    /// Returns the region size in bytes.
    pub fn size(&self) -> usize {
        self.mapping.size()
    }

    /// Returns the region's offset in the BAR1 aperture, while mapped.
    pub fn bar1_offset(&self) -> Result<u64> {
        self.mapping.bar1_offset()
    }

    /// Returns the CPU physical address of a complete page in this mapping.
    ///
    /// The page must remain mapped until every installed CPU page-table entry is revoked.
    pub fn physical_page_address(&self, offset: usize) -> Result<u64> {
        if !offset.is_multiple_of(PAGE_SIZE)
            || offset.checked_add(PAGE_SIZE).ok_or(EOVERFLOW)? > self.size()
        {
            return Err(EINVAL);
        }
        let offset = u64::try_from(offset).map_err(|_| EOVERFLOW)?;
        let address = self
            .physical_base
            .checked_add(self.mapping.bar1_offset()?)
            .and_then(|base| base.checked_add(offset))
            .ok_or(EOVERFLOW)?;
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
        if !address.is_multiple_of(page_size) {
            return Err(EINVAL);
        }
        Ok(address)
    }

    /// Returns the kernel I/O address of the mapped region.
    ///
    /// # Safety
    ///
    /// The caller must keep this mapping and its device binding alive, prevent unmapping
    /// while the pointer is used, and restrict I/O accesses to this mapping's size and
    /// permissions. The pointer must not be dereferenced as ordinary system memory.
    pub unsafe fn iomem_ptr(&self) -> Result<*mut c_void> {
        let offset = usize::try_from(self.mapping.bar1_offset()?).map_err(|_| EOVERFLOW)?;
        if offset.checked_add(self.size()).ok_or(EOVERFLOW)? > self.bar1.size() {
            return Err(EINVAL);
        }
        let base = MmioBackend::as_ptr(self.bar1.as_view()).cast::<u8>();
        Ok(base.wrapping_add(offset).cast())
    }

    /// Reads an aligned 32-bit value at a byte offset within the region.
    pub fn read32(&self, offset: usize) -> Result<u32> {
        self.mapping.read32(offset)
    }

    /// Writes an aligned 32-bit value at a byte offset within a writable region.
    pub fn write32(&self, value: u32, offset: usize) -> Result {
        if !self.writable {
            return Err(EPERM);
        }
        self.mapping.write32(value, offset)
    }

    /// Writes an aligned 64-bit value at a byte offset within a writable region.
    pub fn write64(&self, value: u64, offset: usize) -> Result {
        if !self.writable {
            return Err(EPERM);
        }
        self.mapping.write64(value, offset)
    }

    /// Unmaps the region, reporting errors before drop.
    ///
    /// Further accesses fail after unmapping or an unmap error. On failure, the
    /// mapping retains its backing and drop leaks that reference to prevent reuse.
    pub fn unmap(&mut self) -> Result {
        self.mapping.unmap()
    }
}

// Keep the MM implementation's invariant lifetime behind the API boundary.
// The mutex permits sharing the mapping while serializing access to its state.
trait Mapping: Send + Sync {
    fn size(&self) -> usize;
    fn bar1_offset(&self) -> Result<u64>;
    fn read32(&self, offset: usize) -> Result<u32>;
    fn write32(&self, value: u32, offset: usize) -> Result;
    fn write64(&self, value: u64, offset: usize) -> Result;
    fn unmap(&self) -> Result;
}

impl Mapping for Mutex<BarMapping<'_, '_>> {
    fn size(&self) -> usize {
        self.lock().size()
    }

    fn bar1_offset(&self) -> Result<u64> {
        self.lock().gpu_va_addr()
    }

    fn read32(&self, offset: usize) -> Result<u32> {
        self.lock().try_read32(offset)
    }

    fn write32(&self, value: u32, offset: usize) -> Result {
        self.lock().try_write32(value, offset)
    }

    fn write64(&self, value: u64, offset: usize) -> Result {
        self.lock().try_write64(value, offset)
    }

    fn unmap(&self) -> Result {
        self.lock().unmap()
    }
}
