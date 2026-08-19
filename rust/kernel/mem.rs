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

/// Type that is layout-compatible with a primitive representation.
///
/// # Round-trip transmutability
///
/// `T` is round-trip transmutable to `U` if and only if both of these properties hold:
///
/// - Any valid bit pattern for `T` is also a valid bit pattern for `U`.
/// - Transmuting a value of type `T` to `U` and then to `T` again
///   yields a value that is in all aspects equivalent to the original value.
///
/// # Safety
///
/// - [`Self`] must have the same size and alignment as [`Self::Repr`].
/// - [`Self`] must be [round-trip transmutable] to  [`Self::Repr`].
///
/// [round-trip transmutable]: AsRepr#round-trip-transmutability
pub unsafe trait AsRepr: Sized {
    /// Primitive representation of this type.
    type Repr;

    /// Convert from [`AsRepr::Repr`] to `Self`.
    ///
    /// # Safety
    ///
    /// `repr` must be a valid bit patern of `Self`. If `repr` is previously obtained using
    /// [`AsRepr::into_repr`], then it will always be safe.
    #[inline(always)]
    unsafe fn from_repr_unchecked(repr: Self::Repr) -> Self {
        // SAFETY: Per safety requirement of the trait.
        unsafe { transmute_unchecked(repr) }
    }

    /// Convert from `Self` to [`AsRepr::Repr`].
    #[inline(always)]
    fn into_repr(this: Self) -> Self::Repr {
        // SAFETY: Per safety requirement of the trait.
        unsafe { transmute_unchecked(this) }
    }
}

/// Type that is bi-directionally transmutable with a primitive representation.
///
/// # Safety
///
/// - [`Self`] must be [transmutable] from [`Self::Repr`].
/// - Note that [`Self::Repr`] must be [transmutable] from `Self` as well, however that is a
///   requirement of the [`AsRepr`] super trait already.
///
/// [`transmutable`]: core::mem::transmute
pub unsafe trait AsReprMut: AsRepr {
    /// Convert from [`AsRepr::Repr`] to `Self`.
    #[inline(always)]
    fn from_repr(repr: Self::Repr) -> Self {
        // SAFETY: Per safety requirement of the trait.
        unsafe { transmute_unchecked(repr) }
    }
}

// SAFETY: `bool` has the same size and alignment as `u8`, and Rust guarantees that `bool` has
// only two valid bit patterns: 0 (false) and 1 (true). Those are valid `u8` values, so `bool` is
// round-trip transmutable to `u8`.
unsafe impl AsRepr for bool {
    type Repr = u8;
}

// SAFETY: `*mut T` has the same size and alignment with `*const c_void`, and is round-trip
// transmutable to `*const c_void`.
unsafe impl<T> AsRepr for *mut T {
    type Repr = *const c_void;
}

// SAFETY: `*mut T` is transmutable from `*const c_void`.
unsafe impl<T> AsReprMut for *mut T {}

// SAFETY: `*const T` has the same size and alignment with `*const c_void`, and is round-trip
// transmutable to `*const c_void`.
unsafe impl<T> AsRepr for *const T {
    type Repr = *const c_void;
}

// SAFETY: `*const T` is transmutable from `*const c_void`.
unsafe impl<T> AsReprMut for *const T {}

macro_rules! int_impl {
    ($($unsigned:ident $signed:ident ,)*) => {$(
        // SAFETY: $unsigned has the same size and alignment with itself, and is round-trip
        // transmutable to itself.
        unsafe impl AsRepr for $unsigned {
            type Repr = $unsigned;
        }

        // SAFETY: $unsigned is transmutable from itself.
        unsafe impl AsReprMut for $unsigned {}

        // SAFETY: $signed has the same size and alignment with $unsigned, and is round-trip
        // transmutable to it.
        unsafe impl AsRepr for $signed {
            type Repr = $unsigned;
        }

        // SAFETY: $signed is transmutable from $unsigned.
        unsafe impl AsReprMut for $signed {}
    )*};
}

int_impl! {
    u8 i8,
    u16 i16,
    u32 i32,
    u64 i64,
}

#[cfg(target_pointer_width = "32")]
const _: () = {
    // SAFETY: usize has the same size and alignment with u32, and is round-trip transmutable to it.
    unsafe impl AsRepr for usize {
        type Repr = u32;
    }

    // SAFETY: isize has the same size and alignment with u32, and is round-trip transmutable to it.
    unsafe impl AsRepr for isize {
        type Repr = u32;
    }

    // SAFETY: usize is transmutable from u32.
    unsafe impl AsReprMut for usize {}
    // SAFETY: isize is transmutable from u32.
    unsafe impl AsReprMut for isize {}
};

#[cfg(target_pointer_width = "64")]
const _: () = {
    // SAFETY: usize has the same size and alignment with u64, and is round-trip transmutable to it.
    unsafe impl AsRepr for usize {
        type Repr = u64;
    }

    // SAFETY: isize has the same size and alignment with u64, and is round-trip transmutable to it.
    unsafe impl AsRepr for isize {
        type Repr = u64;
    }

    // SAFETY: usize is transmutable from u64.
    unsafe impl AsReprMut for usize {}
    // SAFETY: isize is transmutable from u64.
    unsafe impl AsReprMut for isize {}
};
