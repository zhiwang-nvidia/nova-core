// SPDX-License-Identifier: GPL-2.0

//! Memory management subsystems for nova-core.

#![expect(dead_code)]

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

use core::ops::Range;

use kernel::{
    bitfield,
    num::Bounded,
    prelude::*, //
};

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
}

// Allow VRAM addresses to be printed with the `{:#x}` format specifier.
impl core::fmt::LowerHex for VramAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::LowerHex::fmt(&self.raw(), f)
    }
}

impl From<Pfn> for VramAddress {
    fn from(pfn: Pfn) -> Self {
        Self::zeroed().with_frame_number(pfn)
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
