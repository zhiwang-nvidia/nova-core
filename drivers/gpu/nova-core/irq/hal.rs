// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Per-architecture parameters of the GIN PF CPU interrupt tree.

mod gh100;
mod tu102;

use crate::gpu::{
    Architecture,
    Chipset, //
};

/// Per-architecture parameters of the GIN PF CPU interrupt tree.
///
/// The tree walk, vector encoding, and acknowledge sequence are identical
/// across GPU architectures and live in generic code. Only the size of the
/// tree differs by family, and that is provided here. See
/// `Documentation/gpu/nova/core/interrupts.rst`.
pub(crate) trait GinHal {
    /// Returns the number of implemented interrupt leaves in the PF CPU tree.
    ///
    /// Each leaf is a 32-bit register, so the tree exposes `num_leaves * 32`
    /// vectors. Pre-Hopper parts have 8 leaves, and Hopper and later implement
    /// 16 (of which 12 are currently used).
    fn num_leaves(&self) -> usize;

    /// Returns the mask of valid subtree bits for the `TOP` enable registers.
    ///
    /// Each `TOP` bit covers two adjacent leaves, so the tree has
    /// `num_leaves / 2` subtrees. The returned mask has one bit set per
    /// subtree and is written to `TOP_EN_SET` or `TOP_EN_CLEAR` to arm or
    /// unarm the whole tree.
    fn subtree_mask(&self) -> u32 {
        (1u32 << (self.num_leaves() / 2)) - 1
    }
}

/// Returns the [`GinHal`] for `chipset`.
pub(super) fn gin_hal(chipset: Chipset) -> &'static dyn GinHal {
    match chipset.arch() {
        Architecture::Turing | Architecture::Ampere | Architecture::Ada => tu102::TU102_HAL,
        Architecture::Hopper | Architecture::BlackwellGB10x | Architecture::BlackwellGB20x => {
            gh100::GH100_HAL
        }
    }
}
