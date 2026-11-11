// SPDX-License-Identifier: GPL-2.0

//! C67B definitions that differ across DISP 4.x window immediate classes.
//!
//! Definitions follow the published `clc67b.h` class interface.

pub(crate) const UPDATE: u32 = 0x0200;
pub(crate) const UPDATE_RELEASE_ELV: u32 = 1;
pub(crate) const UPDATE_INTERLOCK_WITH_WINDOW: u32 = 1 << 1;
