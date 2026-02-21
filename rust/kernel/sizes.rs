// SPDX-License-Identifier: GPL-2.0

//! Commonly used sizes.
//!
//! C headers: [`include/linux/sizes.h`](srctree/include/linux/sizes.h).

/// 0x00000400
pub const SZ_1K: usize = bindings::SZ_1K as usize;
/// 0x00000800
pub const SZ_2K: usize = bindings::SZ_2K as usize;
/// 0x00001000
pub const SZ_4K: usize = bindings::SZ_4K as usize;
/// 0x00002000
pub const SZ_8K: usize = bindings::SZ_8K as usize;
/// 0x00004000
pub const SZ_16K: usize = bindings::SZ_16K as usize;
/// 0x00008000
pub const SZ_32K: usize = bindings::SZ_32K as usize;
/// 0x00010000
pub const SZ_64K: usize = bindings::SZ_64K as usize;
/// 0x00020000
pub const SZ_128K: usize = bindings::SZ_128K as usize;
/// 0x00040000
pub const SZ_256K: usize = bindings::SZ_256K as usize;
/// 0x00080000
pub const SZ_512K: usize = bindings::SZ_512K as usize;
/// 0x00100000
pub const SZ_1M: usize = bindings::SZ_1M as usize;
/// 0x00200000
pub const SZ_2M: usize = bindings::SZ_2M as usize;
/// 0x00400000
pub const SZ_4M: usize = bindings::SZ_4M as usize;
/// 0x00800000
pub const SZ_8M: usize = bindings::SZ_8M as usize;
/// 0x01000000
pub const SZ_16M: usize = bindings::SZ_16M as usize;
/// 0x02000000
pub const SZ_32M: usize = bindings::SZ_32M as usize;
/// 0x04000000
pub const SZ_64M: usize = bindings::SZ_64M as usize;
/// 0x08000000
pub const SZ_128M: usize = bindings::SZ_128M as usize;
/// 0x10000000
pub const SZ_256M: usize = bindings::SZ_256M as usize;
/// 0x20000000
pub const SZ_512M: usize = bindings::SZ_512M as usize;
/// 0x40000000
pub const SZ_1G: usize = bindings::SZ_1G as usize;
/// 0x80000000
pub const SZ_2G: usize = bindings::SZ_2G as usize;

// `u64` variants of the size constants. These are the same values as the
// `usize` constants above, but typed as `u64` to avoid repeated conversion
// boilerplate in code that operates on 64-bit address spaces.
//
// CAST: every SZ_* value below fits in u64, so `as u64` is always lossless.

/// [`SZ_1K`] as a [`u64`].
pub const SZ_1K_U64: u64 = SZ_1K as u64;
/// [`SZ_2K`] as a [`u64`].
pub const SZ_2K_U64: u64 = SZ_2K as u64;
/// [`SZ_4K`] as a [`u64`].
pub const SZ_4K_U64: u64 = SZ_4K as u64;
/// [`SZ_8K`] as a [`u64`].
pub const SZ_8K_U64: u64 = SZ_8K as u64;
/// [`SZ_16K`] as a [`u64`].
pub const SZ_16K_U64: u64 = SZ_16K as u64;
/// [`SZ_32K`] as a [`u64`].
pub const SZ_32K_U64: u64 = SZ_32K as u64;
/// [`SZ_64K`] as a [`u64`].
pub const SZ_64K_U64: u64 = SZ_64K as u64;
/// [`SZ_128K`] as a [`u64`].
pub const SZ_128K_U64: u64 = SZ_128K as u64;
/// [`SZ_256K`] as a [`u64`].
pub const SZ_256K_U64: u64 = SZ_256K as u64;
/// [`SZ_512K`] as a [`u64`].
pub const SZ_512K_U64: u64 = SZ_512K as u64;
/// [`SZ_1M`] as a [`u64`].
pub const SZ_1M_U64: u64 = SZ_1M as u64;
/// [`SZ_2M`] as a [`u64`].
pub const SZ_2M_U64: u64 = SZ_2M as u64;
/// [`SZ_4M`] as a [`u64`].
pub const SZ_4M_U64: u64 = SZ_4M as u64;
/// [`SZ_8M`] as a [`u64`].
pub const SZ_8M_U64: u64 = SZ_8M as u64;
/// [`SZ_16M`] as a [`u64`].
pub const SZ_16M_U64: u64 = SZ_16M as u64;
/// [`SZ_32M`] as a [`u64`].
pub const SZ_32M_U64: u64 = SZ_32M as u64;
/// [`SZ_64M`] as a [`u64`].
pub const SZ_64M_U64: u64 = SZ_64M as u64;
/// [`SZ_128M`] as a [`u64`].
pub const SZ_128M_U64: u64 = SZ_128M as u64;
/// [`SZ_256M`] as a [`u64`].
pub const SZ_256M_U64: u64 = SZ_256M as u64;
/// [`SZ_512M`] as a [`u64`].
pub const SZ_512M_U64: u64 = SZ_512M as u64;
/// [`SZ_1G`] as a [`u64`].
pub const SZ_1G_U64: u64 = SZ_1G as u64;
/// [`SZ_2G`] as a [`u64`].
pub const SZ_2G_U64: u64 = SZ_2G as u64;
