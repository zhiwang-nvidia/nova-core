// SPDX-License-Identifier: GPL-2.0

//! C77D definitions that differ across DISP 4.x core classes.
//!
//! Definitions follow the published `clc77d.h` class interface.

pub(crate) const HEAD_SET_CONTEXT_DMA_RG_REL_SEMAPHORE: u32 = 0x21ac;
// C67D/C77D/C87D define six RG semaphores per head; C57D has no such methods.
pub(crate) const HEAD_RG_REL_SEMAPHORE_COUNT: u32 = 6;
