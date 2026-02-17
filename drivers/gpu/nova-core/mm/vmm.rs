// SPDX-License-Identifier: GPL-2.0

//! Virtual Memory Manager for NVIDIA GPU page table management.
//!
//! The [`Vmm`] provides high-level page mapping and unmapping operations for GPU
//! virtual address spaces (Channels, BAR1, BAR2).

use kernel::{
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddy,
        GpuBuddyAllocFlag,
        GpuBuddyAllocMode,
        GpuBuddyParams, //
    },
    prelude::*,
    ptr::Alignment,
    rbtree::RBTree,
    sizes::SZ_4K, //
};

use core::{
    cell::Cell,
    ops::Range, //
};

use crate::{
    mm::{
        pagetable::{
            map::{
                PtMap, //
            },
            walk::{
                PtWalk,
                WalkResult, //
            },
            MmuVersion, //
        },
        GpuMm,
        Pfn,
        Vfn,
        VramAddress,
        PAGE_SIZE, //
    },
    num::{
        IntoSafeCast, //
    },
};

/// Multi-page prepared mapping -- VA range allocated, ready for execute.
///
/// Produced by [`Vmm::prepare_map()`], consumed by [`Vmm::execute_map()`].
/// The struct owns the VA space allocation between prepare and execute phases.
pub(crate) struct PreparedMapping {
    vfn_start: Vfn,
    num_pages: usize,
    vfn_alloc: Pin<KBox<AllocatedBlocks>>,
}

/// Result of a mapping operation -- tracks the active mapped range.
///
/// Returned by [`Vmm::execute_map()`] and [`Vmm::map_pages()`].
/// Owns the VA allocation; the VA range is freed when this is dropped.
/// Callers must call [`Vmm::unmap_pages()`] before dropping to invalidate
/// PTEs (dropping only frees the VA range, not the PTE entries).
pub(crate) struct MappedRange {
    pub(super) vfn_start: Vfn,
    pub(super) num_pages: usize,
    /// VA allocation -- freed when [`MappedRange`] is dropped.
    _vfn_alloc: Pin<KBox<AllocatedBlocks>>,
    /// Logs a warning if dropped without unmapping.
    _drop_guard: MustUnmapGuard,
}

/// Guard that logs a warning once if a [`MappedRange`] is dropped without
/// calling [`Vmm::unmap_pages()`].
struct MustUnmapGuard {
    armed: Cell<bool>,
}

impl MustUnmapGuard {
    const fn new() -> Self {
        Self {
            armed: Cell::new(true),
        }
    }

    fn disarm(&self) {
        self.armed.set(false);
    }
}

impl Drop for MustUnmapGuard {
    fn drop(&mut self) {
        if self.armed.get() {
            kernel::pr_warn!("MappedRange dropped without calling unmap_pages()\n");
        }
    }
}

/// Virtual Memory Manager for a GPU address space.
///
/// Each [`Vmm`] instance manages a single address space identified by its Page
/// Directory Base (`PDB`) address. Used for Channel, BAR1 and BAR2 mappings.
pub(crate) struct Vmm {
    /// Page Directory Base address for this address space.
    pdb_addr: VramAddress,
    /// Page table walker for reading existing mappings.
    pt_walk: PtWalk,
    /// Page table mapper for prepare/execute operations.
    pt_map: PtMap,
    /// Page table allocations required for mappings.
    page_table_allocs: KVec<Pin<KBox<AllocatedBlocks>>>,
    /// Buddy allocator for virtual address range tracking.
    virt_buddy: GpuBuddy,
    /// Prepared PT pages pending PDE installation, keyed by `install_addr`.
    ///
    /// Populated during prepare phase and drained in execute phase. Shared by all
    /// pending maps, preventing races on the same PDE slot.
    pt_pages: RBTree<VramAddress, super::pagetable::map::PreparedPtPage>,
}

impl Vmm {
    /// Create a new [`Vmm`] for the given Page Directory Base address.
    ///
    /// The [`Vmm`] will manage a virtual address space of `va_size` bytes.
    pub(crate) fn new(
        pdb_addr: VramAddress,
        mmu_version: MmuVersion,
        va_size: u64,
    ) -> Result<Self> {
        // Only MMU v2 is supported for now.
        if mmu_version != MmuVersion::V2 {
            return Err(ENOTSUPP);
        }

        let virt_buddy = GpuBuddy::new(GpuBuddyParams {
            base_offset: 0,
            size: va_size,
            chunk_size: Alignment::new::<SZ_4K>(),
        })?;

        Ok(Self {
            pdb_addr,
            pt_walk: PtWalk::new(pdb_addr, mmu_version),
            pt_map: PtMap::new(pdb_addr, mmu_version),
            page_table_allocs: KVec::new(),
            virt_buddy,
            pt_pages: RBTree::new(),
        })
    }

    /// Allocate a contiguous virtual frame number range.
    fn alloc_vfn_range(
        &self,
        num_pages: usize,
        va_range: Option<Range<u64>>,
    ) -> Result<(Vfn, Pin<KBox<AllocatedBlocks>>)> {
        let num_pages: u64 = num_pages.into_safe_cast();
        let page_size: u64 = PAGE_SIZE.into_safe_cast();
        let size: u64 = num_pages.checked_mul(page_size).ok_or(EOVERFLOW)?;

        let mode = match va_range {
            Some(r) => {
                let range_size = r.end.checked_sub(r.start).ok_or(EOVERFLOW)?;
                if range_size != size {
                    return Err(EINVAL);
                }
                GpuBuddyAllocMode::Range(r)
            }
            None => GpuBuddyAllocMode::Simple,
        };

        let alloc = KBox::pin_init(
            self.virt_buddy.alloc_blocks(
                mode,
                size,
                Alignment::new::<SZ_4K>(),
                GpuBuddyAllocFlag::Contiguous,
            ),
            GFP_KERNEL,
        )?;

        let offset = alloc.iter().next().ok_or(ENOMEM)?.offset();
        let vfn = Vfn::new(offset / page_size);

        Ok((vfn, alloc))
    }

    /// Read the [`Pfn`] for a mapped [`Vfn`] if one is mapped.
    pub(super) fn read_mapping(&self, mm: &GpuMm, vfn: Vfn) -> Result<Option<Pfn>> {
        match self.pt_walk.walk_to_pte(mm, vfn)? {
            WalkResult::Mapped { pfn, .. } => Ok(Some(pfn)),
            WalkResult::Unmapped { .. } | WalkResult::PageTableMissing => Ok(None),
        }
    }

    /// Prepare resources for mapping `num_pages` pages.
    ///
    /// Allocates a contiguous VA range, then walks the hierarchy per-VFN to prepare pages
    /// for all missing PDEs. Returns a [`PreparedMapping`] with the VA allocation.
    ///
    /// If `va_range` is not `None`, the VA range is constrained to the given range. Safe
    /// to call outside the fence signalling critical path.
    pub(crate) fn prepare_map(
        &mut self,
        mm: &GpuMm,
        num_pages: usize,
        va_range: Option<Range<u64>>,
    ) -> Result<PreparedMapping> {
        if num_pages == 0 {
            return Err(EINVAL);
        }

        // Allocate contiguous VA range.
        let (vfn_start, vfn_alloc) = self.alloc_vfn_range(num_pages, va_range)?;

        self.pt_map.prepare_map(
            mm,
            vfn_start,
            num_pages,
            &mut self.page_table_allocs,
            &mut self.pt_pages,
        )?;

        Ok(PreparedMapping {
            vfn_start,
            num_pages,
            vfn_alloc,
        })
    }

    /// Execute a prepared multi-page mapping.
    ///
    /// Installs all prepared PDEs and writes PTEs into the page table, then flushes TLB.
    pub(crate) fn execute_map(
        &mut self,
        mm: &GpuMm,
        prepared: PreparedMapping,
        pfns: &[Pfn],
        writable: bool,
    ) -> Result<MappedRange> {
        if pfns.len() != prepared.num_pages {
            return Err(EINVAL);
        }

        let PreparedMapping {
            vfn_start,
            num_pages,
            vfn_alloc,
        } = prepared;

        self.pt_map.install_mappings(
            mm,
            &mut self.pt_pages,
            &mut self.page_table_allocs,
            vfn_start,
            pfns,
            writable,
        )?;

        Ok(MappedRange {
            vfn_start,
            num_pages,
            _vfn_alloc: vfn_alloc,
            _drop_guard: MustUnmapGuard::new(),
        })
    }

    /// Map pages doing prepare and execute in the same call.
    ///
    /// This is a convenience wrapper for callers outside the fence signalling critical
    /// path (e.g., BAR mappings). For DRM usecases, [`Vmm::prepare_map()`] and
    /// [`Vmm::execute_map()`] will be called separately.
    pub(crate) fn map_pages(
        &mut self,
        mm: &GpuMm,
        pfns: &[Pfn],
        va_range: Option<Range<u64>>,
        writable: bool,
    ) -> Result<MappedRange> {
        if pfns.is_empty() {
            return Err(EINVAL);
        }

        // Check if provided VA range is sufficient (if provided).
        if let Some(ref range) = va_range {
            let required: u64 = pfns
                .len()
                .checked_mul(PAGE_SIZE)
                .ok_or(EOVERFLOW)?
                .into_safe_cast();
            let available = range.end.checked_sub(range.start).ok_or(EINVAL)?;
            if available < required {
                return Err(EINVAL);
            }
        }

        let prepared = self.prepare_map(mm, pfns.len(), va_range)?;
        self.execute_map(mm, prepared, pfns, writable)
    }

    /// Unmap all pages in a [`MappedRange`] with a single TLB flush.
    pub(crate) fn unmap_pages(&mut self, mm: &GpuMm, range: MappedRange) -> Result {
        self.pt_map
            .invalidate_ptes(mm, range.vfn_start, range.num_pages)?;

        // TODO: Internal page table pages (PDE, PTE pages) are still kept around.
        // This is by design as repeated maps/unmaps will be fast. As a future TODO,
        // we can add a reclaimer here to reclaim if VRAM is short. For now, the PT
        // pages are dropped once the `Vmm` is dropped.

        // Unmap complete, safe to drop `MappedRange`.
        range._drop_guard.disarm();
        Ok(())
    }
}
