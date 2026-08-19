// SPDX-License-Identifier: GPL-2.0

//! Basic utilities for dealing with memory, values, and types.

use crate::prelude::*;

/// Version of `transmute` that performs size check at monomorphization-time.
///
/// Use this instead of [`core::mem::transmute`] when it is known that sizes are identical but this
/// cannot be proven by the compiler during type checking.
///
/// The signature is equivalent after Rust standard library's unstable `transmute_neo` and that of
/// [RFC 3844](https://github.com/rust-lang/rfcs/pull/3844).
///
/// # Safety
///
/// Same as [`core::mem::transmute`].
#[inline(always)]
pub const unsafe fn transmute_unchecked<Src, Dst>(val: Src) -> Dst {
    const_assert!(size_of::<Src>() == size_of::<Dst>());

    // SAFETY: This is identical to `transmute` except that we bypassed the size check; which we
    // used `const_assert!` to check above.
    unsafe { core::mem::transmute_copy(&core::mem::ManuallyDrop::new(val)) }
}

/// Safely transmutes a value of one type to a value of another type of the same size.
///
/// The sizes are checked during monomorphization.
///
/// This can be considered as generic version of [`zerocopy::transmute!`] macro that defers the size
/// check and thus can be used in more cases.
#[inline(always)]
pub const fn transmute<Src: IntoBytes, Dst: FromBytes>(val: Src) -> Dst {
    // SAFETY: transmute is safe with `IntoBytes` and `FromBytes` bounds.
    unsafe { transmute_unchecked(val) }
}
