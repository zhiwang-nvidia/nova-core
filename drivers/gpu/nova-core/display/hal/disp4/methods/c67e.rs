// SPDX-License-Identifier: GPL-2.0

//! C67E definitions that differ across DISP 4.x window classes.
//!
//! C67E selects pitch or block-linear layout through the Context DMA
//! descriptor rather than SET_STORAGE.
//!
//! Definitions follow the published `clc67e.h` class interface.

pub(crate) const SET_STORAGE: u32 = 0x0228;
pub(crate) const SET_STORAGE_BLOCK_HEIGHT_ONE_GOB: u32 = 0;
pub(crate) const SET_PARAMS: u32 = 0x022c;
pub(crate) const SET_PARAMS_FORMAT_X8R8G8B8: u32 = 0x00e6;
pub(crate) const SET_ACQ_SEMAPHORE_CONTROL: u32 = 0x0330;
pub(crate) const SET_ACQ_SEMAPHORE_VALUE: u32 = 0x0334;
pub(crate) const SET_CONTEXT_DMA_ACQ_SEMAPHORE: u32 = 0x0338;
pub(crate) const SET_SCAN_DIRECTION: u32 = 0x033c;
pub(crate) const SET_SCAN_DIRECTION_FROM_TOP_LEFT: u32 = 0;
