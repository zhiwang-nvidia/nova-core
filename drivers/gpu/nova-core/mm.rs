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
pub(crate) mod pramin;
pub(super) mod tlb;
pub(super) mod vmm;
pub(crate) mod vram;

use core::ops::Range;

use kernel::{
    bitfield,
    gpu::buddy::{
        GpuBuddy,
        GpuBuddyParams, //
    },
    num::Bounded,
    prelude::*,
    ptr::{
        Alignable,
        Alignment, //
    },
    sizes::SZ_4K, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset, //
};

pub(crate) use tlb::Tlb;

/// GPU Memory Manager - owns all core MM components.
///
/// Provides centralized ownership of memory management resources:
/// - [`GpuBuddy`] allocators for VRAM page table allocation.
/// - [`pramin::Pramin`] for direct VRAM access.
/// - [`Tlb`] manager for translation buffer flush operations.
#[pin_data]
pub(crate) struct GpuMm<'gpu> {
    buddies: KVec<GpuBuddy>,
    #[pin]
    pramin: pramin::Pramin<'gpu>,
    #[pin]
    tlb: Tlb<'gpu>,
}

impl<'gpu> GpuMm<'gpu> {
    /// Create a pin-initializer for `GpuMm`.
    ///
    /// `pramin_vram_region` is the full physical VRAM range (including GSP-reserved
    /// areas). PRAMIN window accesses are validated against this range.
    ///
    /// `buddy_base_alignment` is applied to the absolute start of every
    /// usable VRAM region. Individual allocations must still request their
    /// required alignment from the buddy allocator.
    pub(crate) fn new(
        bar: Bar0<'gpu>,
        chipset: Chipset,
        buddy_params: KVec<GpuBuddyParams>,
        buddy_base_alignment: Alignment,
        pramin_vram_region: Range<VramAddress>,
    ) -> Result<impl PinInit<Self> + 'gpu> {
        if buddy_params.is_empty() {
            return Err(EINVAL);
        }

        let mut buddies = KVec::new();
        for mut params in buddy_params {
            let end = params
                .base_offset
                .checked_add(params.size)
                .ok_or(EOVERFLOW)?;
            let start = params
                .base_offset
                .align_up(buddy_base_alignment)
                .ok_or(EOVERFLOW)?;
            if start >= end {
                continue;
            }
            params.base_offset = start;
            params.size = end - start;
            buddies.push(GpuBuddy::new(params)?, GFP_KERNEL)?;
        }

        if buddies.is_empty() {
            return Err(ENOSPC);
        }

        let pramin_init = pramin::Pramin::new(bar, chipset, pramin_vram_region)?;
        let tlb_init = Tlb::new(bar);

        Ok(pin_init!(Self {
            buddies,
            pramin <- pramin_init,
            tlb <- tlb_init,
        }))
    }

    /// Access the [`GpuBuddy`] allocators.
    pub(crate) fn buddies(&self) -> &[GpuBuddy] {
        &self.buddies
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
