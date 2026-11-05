// SPDX-License-Identifier: GPL-2.0

//! C67E definitions that differ across DISP 4.x window classes.
//!
//! Definitions follow the published `clc67e.h` class interface.

pub(crate) const SET_ACQ_SEMAPHORE_CONTROL: u32 = 0x0330;
pub(crate) const SET_ACQ_SEMAPHORE_VALUE: u32 = 0x0334;
pub(crate) const SET_CONTEXT_DMA_ACQ_SEMAPHORE: u32 = 0x0338;
