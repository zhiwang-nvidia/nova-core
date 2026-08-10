// SPDX-License-Identifier: GPL-2.0

//! Memory management subsystems for nova-core.

#![allow(dead_code)]

/// Implements `From` conversions between a frame-number type and `Bounded<u64, N>`.
///
/// Each MMU version module should invoke this for the specific bit widths used by that version's
/// PTE/PDE bitfield definitions.
macro_rules! impl_frame_number_bounded {
    ($type:ty, $bits:literal) => {
        impl From<Bounded<u64, $bits>> for $type {
            fn from(val: Bounded<u64, $bits>) -> Self {
                Self::new(val.get())
            }
        }

        impl From<$type> for Bounded<u64, $bits> {
            fn from(v: $type) -> Self {
                Bounded::from_expr(v.raw() & ::kernel::bits::genmask_u64(0..=($bits - 1)))
            }
        }
    };
}

/// Implements `From` conversions between [`Pfn`] and `Bounded<u64, N>` for bitfield interop.
macro_rules! impl_pfn_bounded {
    ($bits:literal) => {
        impl_frame_number_bounded!(Pfn, $bits);
    };
}

pub(crate) mod bar_user;
pub(super) mod pagetable;
pub(crate) mod placement;
pub(crate) mod pramin;
pub(super) mod tlb;
pub(super) mod vmm;
pub(crate) mod vram;

use core::ops::Range;

use kernel::{
    bitfield,
    device,
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddy,
        GpuBuddyAllocFlags,
        GpuBuddyAllocMode,
        GpuBuddyParams, //
    },
    num::Bounded,
    pci,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K,
    sync::Arc, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset, //
};

pub(crate) use tlb::Tlb;

use self::vram::VramBlock;

/// Memory owned by a [`GpuMmAllocator`].
pub(super) enum AllocatorBacking {
    /// Physical device ranges owned directly by the core allocator.
    Device,
    /// Allocations retained from another allocator.
    Parent(KVec<Arc<VramBlock>>),
}

impl AllocatorBacking {
    pub(super) fn from_blocks(blocks: KVec<Arc<VramBlock>>) -> Result<Arc<Self>> {
        if blocks.is_empty() {
            return Err(EINVAL);
        }

        for (index, block) in blocks.iter().enumerate() {
            let range = block.range()?;
            for other in &blocks[index + 1..] {
                if ranges_overlap(&range, &other.range()?) {
                    return Err(EINVAL);
                }
            }
        }

        Ok(Arc::new(Self::Parent(blocks), GFP_KERNEL)?)
    }

    pub(super) fn contains_range(&self, range: &Range<u64>) -> bool {
        match self {
            Self::Device => false,
            Self::Parent(blocks) => blocks.iter().any(|block| {
                block
                    .range()
                    .is_ok_and(|backing| range_contains(&backing, range))
            }),
        }
    }
}

/// Buddy allocator state for [`GpuMmAllocator<Buddy>`].
///
/// Each disjoint physical range has its own [`GpuBuddy`].
pub(crate) struct Buddy {
    regions: KVec<GpuBuddy>,
}

/// Applies one allocation policy to owned VRAM backing.
///
/// The core [`GpuMmAllocator<Buddy>`] owns all usable VRAM. Child buddy
/// allocators divide a reservation from the core into smaller allocations.
/// A [`GpuMmAllocator<placement::Placement>`] selects predefined ranges from
/// core-backed memory by placement ID.
///
/// `A` is the allocator implementation selected at compile time. `backend`
/// stores that implementation's state; it is not a mode selected at runtime.
pub(crate) struct GpuMmAllocator<A> {
    pub(super) backend: A,
    pub(super) backing: Arc<AllocatorBacking>,
}

/// Blocks allocated from a [`GpuMmAllocator<Buddy>`].
///
/// This object retains the allocator backing until the blocks are freed.
pub(crate) struct BuddyAllocation {
    blocks: Pin<KBox<AllocatedBlocks>>,
    _backing: Arc<AllocatorBacking>,
}

impl BuddyAllocation {
    pub(crate) fn blocks(&self) -> &AllocatedBlocks {
        self.blocks.as_ref().get_ref()
    }
}

impl GpuMmAllocator<Buddy> {
    /// Create the core allocator over the physical usable VRAM ranges.
    pub(crate) fn new(ranges: &[Range<u64>]) -> Result<Self> {
        let backing = Arc::new(AllocatorBacking::Device, GFP_KERNEL)?;
        Self::new_with_backing(ranges, backing)
    }

    /// Create a buddy allocator over reservations from another allocator.
    pub(crate) fn from_backing(blocks: KVec<Arc<VramBlock>>) -> Result<Self> {
        let backing = AllocatorBacking::from_blocks(blocks)?;
        let AllocatorBacking::Parent(blocks) = &*backing else {
            return Err(EINVAL);
        };

        let mut ranges = KVec::new();
        ranges.reserve(blocks.len(), GFP_KERNEL)?;
        for block in blocks {
            ranges.push_within_capacity(block.range()?)?;
        }

        Self::new_with_backing(&ranges, backing)
    }

    /// Create the buddy regions and attach their ownership backing.
    fn new_with_backing(ranges: &[Range<u64>], backing: Arc<AllocatorBacking>) -> Result<Self> {
        if ranges.is_empty() {
            return Err(ENOSPC);
        }

        let mut regions = KVec::new();
        regions.reserve(ranges.len(), GFP_KERNEL)?;
        for (index, range) in ranges.iter().enumerate() {
            validate_fb_range(range)?;
            if ranges[index + 1..]
                .iter()
                .any(|other| ranges_overlap(range, other))
            {
                return Err(EINVAL);
            }
            regions.push_within_capacity(GpuBuddy::new(buddy_params(range.clone())?)?)?;
        }

        Ok(Self {
            backend: Buddy { regions },
            backing,
        })
    }

    /// Allocate from any managed buddy region.
    pub(crate) fn alloc(
        &self,
        size: u64,
        min_block_size: Alignment,
        flags: GpuBuddyAllocFlags,
    ) -> Result<BuddyAllocation> {
        for region in &self.backend.regions {
            let blocks =
                region.alloc_blocks(GpuBuddyAllocMode::Simple, size, min_block_size, flags);
            match self.finish_allocation(blocks) {
                Ok(allocation) => return Ok(allocation),
                Err(error) if error == ENOSPC => continue,
                Err(error) => return Err(error),
            }
        }
        Err(ENOSPC)
    }

    /// Reserve exactly one absolute physical range.
    ///
    /// The caller has already chosen the address. If that range is unavailable,
    /// this returns `ENOSPC` instead of choosing a different address.
    pub(crate) fn reserve_range(
        &self,
        range: Range<u64>,
        min_block_size: Alignment,
        flags: GpuBuddyAllocFlags,
    ) -> Result<BuddyAllocation> {
        validate_fb_range(&range)?;
        let size = range.end.checked_sub(range.start).ok_or(EOVERFLOW)?;

        for region in &self.backend.regions {
            let region_start = region.base_offset();
            let region_end = region_start.checked_add(region.size()).ok_or(EOVERFLOW)?;
            if range.start < region_start || range.end > region_end {
                continue;
            }

            let relative = range.start - region_start..range.end - region_start;
            let blocks = region.alloc_blocks(
                GpuBuddyAllocMode::Range(relative),
                size,
                min_block_size,
                flags,
            );
            return self.finish_allocation(blocks);
        }
        Err(ENOSPC)
    }

    /// Find and reserve an aligned, physically contiguous range.
    ///
    /// Unlike [`Self::reserve_range`], the caller supplies no address. This
    /// searches the managed regions until it finds a suitable free range.
    pub(crate) fn reserve_aligned(
        &self,
        size: u64,
        address_align: u64,
        min_block_size: Alignment,
        flags: GpuBuddyAllocFlags,
    ) -> Result<BuddyAllocation> {
        if size == 0 || !address_align.is_power_of_two() {
            return Err(EINVAL);
        }
        let align_mask = address_align - 1;

        for region in &self.backend.regions {
            let region_start = region.base_offset();
            let region_end = region_start.checked_add(region.size()).ok_or(EOVERFLOW)?;
            let Some(mut address) = region_start
                .checked_add(align_mask)
                .map(|value| value & !align_mask)
            else {
                continue;
            };

            loop {
                let Some(end) = address.checked_add(size) else {
                    break;
                };
                if end > region_end {
                    break;
                }

                let relative = address - region_start..end - region_start;
                let blocks = region.alloc_blocks(
                    GpuBuddyAllocMode::Range(relative),
                    size,
                    min_block_size,
                    flags,
                );
                match self.finish_allocation(blocks) {
                    Ok(allocation) => return Ok(allocation),
                    Err(error) if error == ENOSPC => {}
                    Err(error) => return Err(error),
                }

                let Some(next) = address.checked_add(address_align) else {
                    break;
                };
                address = next;
            }
        }
        Err(ENOSPC)
    }

    /// Retain the parent backing for the lifetime of an allocation.
    fn finish_allocation(
        &self,
        blocks: impl PinInit<AllocatedBlocks, Error>,
    ) -> Result<BuddyAllocation> {
        let blocks = KBox::pin_init(blocks, GFP_KERNEL)?;
        Ok(BuddyAllocation {
            blocks,
            _backing: self.backing.clone(),
        })
    }
}

/// GPU memory manager with core and internal buddy allocators.
///
/// The core allocator owns all usable VRAM. The internal allocator subdivides
/// a core reservation for driver-owned page tables and similar objects.
#[pin_data]
pub(crate) struct GpuMm<'gpu> {
    internal: GpuMmAllocator<Buddy>,
    core: GpuMmAllocator<Buddy>,
    #[pin]
    pramin: pramin::Pramin<'gpu>,
    #[pin]
    tlb: Tlb<'gpu>,
}

impl<'gpu> GpuMm<'gpu> {
    /// Return the FB space needed for nova-core-owned BAR1 page tables.
    pub(crate) fn internal_fb_size(
        pdev: &pci::Device<device::Bound>,
        chipset: Chipset,
    ) -> Result<u64> {
        let bar1_idx = crate::driver::bar1_resource_index(pdev)?;
        let bar1_size = pdev.resource_len(bar1_idx)?;
        chipset.mmu_version().page_table_memory_size(bar1_size)
    }

    /// Choose the internal range from the smallest usable range that fits.
    ///
    /// Memory is taken from the end to leave the start available for larger
    /// workload reservations.
    pub(crate) fn select_internal_fb_range(
        usable_fb_regions: &[Range<u64>],
        size: u64,
    ) -> Result<Range<u64>> {
        let page_size = u64::try_from(SZ_4K).map_err(|_| EOVERFLOW)?;
        if size == 0 || !size.is_multiple_of(page_size) {
            return Err(EINVAL);
        }

        let mut selected: Option<(u64, Range<u64>)> = None;
        for usable in usable_fb_regions {
            validate_fb_range(usable)?;
            let usable_size = usable.end.checked_sub(usable.start).ok_or(EINVAL)?;
            if usable_size < size {
                continue;
            }
            if let Some((best_size, best_range)) = selected.as_ref() {
                if *best_size < usable_size
                    || (*best_size == usable_size && best_range.end >= usable.end)
                {
                    continue;
                }
            }

            let start = usable.end.checked_sub(size).ok_or(EOVERFLOW)?;
            selected = Some((usable_size, start..usable.end));
        }
        selected.map(|(_, range)| range).ok_or(ENOSPC)
    }

    /// Create a pin-initializer for `GpuMm`.
    ///
    /// `pramin_vram_region` is the full physical VRAM range (including GSP-reserved
    /// areas). PRAMIN window accesses are validated against this range.
    pub(crate) fn new(
        bar: Bar0<'gpu>,
        chipset: Chipset,
        usable_fb_regions: &[Range<u64>],
        internal_fb_range: Range<u64>,
        pramin_vram_region: Range<VramAddress>,
    ) -> Result<impl PinInit<Self> + 'gpu> {
        let core = GpuMmAllocator::<Buddy>::new(usable_fb_regions)?;
        let page_size = u64::try_from(SZ_4K).map_err(|_| EOVERFLOW)?;
        let internal_backing = VramBlock::alloc_range(&core, internal_fb_range, page_size)?;
        let mut internal_blocks = KVec::new();
        internal_blocks.push(internal_backing, GFP_KERNEL)?;
        let internal = GpuMmAllocator::<Buddy>::from_backing(internal_blocks)?;

        let pramin_init = pramin::Pramin::new(bar, chipset, pramin_vram_region)?;
        let tlb_init = Tlb::new(bar);

        Ok(pin_init!(Self {
            internal,
            core,
            pramin <- pramin_init,
            tlb <- tlb_init,
        }))
    }

    /// Allocate VRAM from the internal allocator.
    ///
    /// Dropping the returned allocation releases the memory.
    pub(crate) fn alloc_internal_vram(
        &self,
        size: u64,
        min_block_size: Alignment,
        flags: GpuBuddyAllocFlags,
    ) -> Result<BuddyAllocation> {
        self.internal.alloc(size, min_block_size, flags)
    }

    /// Allocate aligned, contiguous VRAM from the core allocator.
    ///
    /// Dropping all references to the returned block releases the memory.
    pub(crate) fn alloc_core_vram(&self, size: u64, align: u64) -> Result<Arc<VramBlock>> {
        VramBlock::alloc_aligned(&self.core, size, align)
    }

    /// Access the [`pramin::Pramin`].
    pub(crate) fn pramin(&self) -> &pramin::Pramin<'gpu> {
        &self.pramin
    }

    /// Access the [`Tlb`] manager.
    pub(crate) fn tlb(&self) -> &Tlb<'gpu> {
        &self.tlb
    }
}

fn validate_fb_range(range: &Range<u64>) -> Result {
    let page_size = u64::try_from(SZ_4K).map_err(|_| EOVERFLOW)?;
    if range.start >= range.end
        || !range.start.is_multiple_of(page_size)
        || !range.end.is_multiple_of(page_size)
    {
        return Err(EINVAL);
    }
    Ok(())
}

fn range_contains(outer: &Range<u64>, inner: &Range<u64>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn ranges_overlap(left: &Range<u64>, right: &Range<u64>) -> bool {
    left.start < right.end && right.start < left.end
}

fn buddy_params(range: Range<u64>) -> Result<GpuBuddyParams> {
    validate_fb_range(&range)?;
    Ok(GpuBuddyParams {
        base_offset: range.start,
        size: range.end - range.start,
        chunk_size: Alignment::new::<SZ_4K>(),
    })
}

/// Page size in bytes (4 KiB).
pub(crate) const PAGE_SIZE: usize = SZ_4K;

bitfield! {
    /// Physical VRAM address in GPU video memory.
    pub(crate) struct VramAddress(u64) {
        /// Offset within 4KB page.
        11:0    offset;
        /// Physical frame number.
        63:12   frame_number => Pfn;
    }
}

impl VramAddress {
    /// Create a new VRAM address from a raw value.
    pub(crate) const fn new(addr: u64) -> Self {
        Self::from_raw(addr)
    }

    /// Get the raw address value as `u64`.
    pub(crate) const fn raw(&self) -> u64 {
        self.into_raw()
    }

    /// Align the address down to the given power-of-two `alignment`.
    pub(crate) const fn align_down(self, alignment: u64) -> Self {
        Self::new(self.raw() & !(alignment - 1))
    }

    /// Add `rhs` to this address, returning `None` on overflow.
    pub(crate) fn checked_add<O: IntoVramOffset>(self, rhs: O) -> Option<Self> {
        self.raw()
            .checked_add(rhs.into_vram_offset())
            .map(Self::new)
    }
}

/// Lossless conversion into a `u64` byte offset, for use as a [`VramAddress`] `checked_add()`
/// operand which can be either a `u64` or a `usize`.
pub(crate) trait IntoVramOffset {
    /// Convert `self` into a `u64` byte offset.
    fn into_vram_offset(self) -> u64;
}

impl IntoVramOffset for u64 {
    fn into_vram_offset(self) -> u64 {
        self
    }
}

impl IntoVramOffset for usize {
    fn into_vram_offset(self) -> u64 {
        use crate::num::IntoSafeCast;
        self.into_safe_cast()
    }
}

// Allow VRAM addresses to be printed with the `{:#x}` format specifier.
impl core::fmt::LowerHex for VramAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::LowerHex::fmt(&self.raw(), f)
    }
}

impl PartialOrd for VramAddress {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VramAddress {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.into_raw().cmp(&other.into_raw())
    }
}

impl From<Pfn> for VramAddress {
    fn from(pfn: Pfn) -> Self {
        Self::zeroed().with_frame_number(pfn)
    }
}

impl core::ops::Add<u64> for VramAddress {
    type Output = Self;

    fn add(self, rhs: u64) -> Self {
        Self::new(self.raw() + rhs)
    }
}

impl core::ops::Sub<VramAddress> for VramAddress {
    type Output = u64;

    fn sub(self, rhs: VramAddress) -> u64 {
        self.raw() - rhs.raw()
    }
}

/// Extension trait to convert a `Range<u64>` of byte addresses into a
/// `Range<VramAddress>`.
pub(crate) trait IntoVramRange {
    /// Convert this range of byte addresses into a `Range<VramAddress>`.
    fn into_vram_range(self) -> Range<VramAddress>;
}

impl IntoVramRange for Range<u64> {
    fn into_vram_range(self) -> Range<VramAddress> {
        VramAddress::new(self.start)..VramAddress::new(self.end)
    }
}

bitfield! {
    /// Virtual address in GPU address space.
    pub(crate) struct VirtualAddress(u64) {
        /// Offset within 4KB page.
        11:0    offset;
        /// Virtual frame number.
        63:12   frame_number => Vfn;
    }
}

impl VirtualAddress {
    /// Create a new virtual address from a raw value.
    pub(crate) const fn new(addr: u64) -> Self {
        Self::from_raw(addr)
    }
}

impl From<Vfn> for VirtualAddress {
    fn from(vfn: Vfn) -> Self {
        Self::zeroed().with_frame_number(vfn)
    }
}

/// Physical Frame Number.
///
/// Represents a physical page in VRAM.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pfn(u64);

impl Pfn {
    /// Create a new PFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VramAddress> for Pfn {
    fn from(addr: VramAddress) -> Self {
        addr.frame_number()
    }
}

impl From<u64> for Pfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Pfn> for u64 {
    fn from(pfn: Pfn) -> Self {
        pfn.0
    }
}

impl_pfn_bounded!(52);

/// Virtual Frame Number.
///
/// Represents a virtual page in GPU address space.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Vfn(u64);

impl Vfn {
    /// Create a new VFN from a frame number.
    pub(crate) const fn new(frame_number: u64) -> Self {
        Self(frame_number)
    }

    /// Get the raw frame number.
    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

impl From<VirtualAddress> for Vfn {
    fn from(addr: VirtualAddress) -> Self {
        addr.frame_number()
    }
}

impl From<u64> for Vfn {
    fn from(val: u64) -> Self {
        Self(val)
    }
}

impl From<Vfn> for u64 {
    fn from(vfn: Vfn) -> Self {
        vfn.0
    }
}

impl_frame_number_bounded!(Vfn, 52);
