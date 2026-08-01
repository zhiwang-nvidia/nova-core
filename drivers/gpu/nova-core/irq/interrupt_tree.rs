// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Type-state API for walking the GIN PF CPU interrupt tree.
//!
//! Each PCIe function has its own interrupt tree. This module only drives the
//! PF's CPU tree. A [`Leaf`] carries a type state, `Idle` -> `Pending`, so that
//! acknowledging one before reading it fails to compile.
//!
//! The compiler enforces that order on each [`Leaf`] value on its own: the
//! state-changing method consumes the value and returns the next state. It does
//! not coordinate the tree as a whole. Coordinating the whole tree is the
//! caller's responsibility.

use kernel::{
    io::{
        register::Array,
        Io, //
    },
    num::Bounded,
    prelude::*,
};

use crate::{
    driver::Bar0,
    gpu::{
        Architecture,
        Chipset, //
    },
    regs::{
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF as CPU_INTR_LEAF,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_EN_CLEAR as CPU_INTR_LEAF_EN_CLEAR,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_EN_SET as CPU_INTR_LEAF_EN_SET,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_TRIGGER as CPU_INTR_LEAF_TRIGGER,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_CLEAR as CPU_INTR_TOP_EN_CLEAR,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_SET as CPU_INTR_TOP_EN_SET, //
    },
};

/// Index of a leaf register, bounded to the `0..16` range covered by the leaf
/// register arrays.
pub(super) type LeafIndex = Bounded<usize, 4>;

/// Type state of a [`Leaf`] handle.
///
/// A `Leaf` follows `Idle` -> `Pending`. Encoding the stages as types makes
/// acknowledging a leaf before reading it fail to compile.
pub(super) trait State: private::Sealed {}

/// State in which the handle holds no pending bitmask.
pub(super) struct Idle;
impl State for Idle {}

/// State holding a pending bitmask read from hardware.
pub(super) struct Pending {
    mask: u32,
}
impl State for Pending {}

mod private {
    pub(in crate::irq) trait Sealed {}
    impl Sealed for super::Idle {}
    impl Sealed for super::Pending {}
}

/// The GIN PF CPU interrupt tree for a single PCIe function.
#[derive(Clone)]
pub(super) struct Tree {
    /// Number of implemented leaves in this tree, either 8 or 16.
    num_leaves: usize,
    /// Mask of subtree bits the architecture implements.
    subtree_mask: u32,
}

impl Tree {
    /// Creates a `Tree` sized for `chipset`.
    pub(super) fn new(chipset: Chipset) -> Self {
        let num_leaves = match chipset.arch() {
            Architecture::Turing | Architecture::Ampere | Architecture::Ada => 8,
            Architecture::Hopper | Architecture::BlackwellGB10x | Architecture::BlackwellGB20x => {
                16
            }
        };

        Self {
            num_leaves,
            // Each subtree covers two leaves, so one bit per pair of leaves.
            subtree_mask: (1u32 << (num_leaves / 2)) - 1,
        }
    }

    /// Returns a [`Top`] handle for this tree.
    pub(super) fn top(&self) -> Top {
        Top {
            subtree_mask: self.subtree_mask,
        }
    }

    /// Returns a [`Leaf`] handle in the [`Idle`] state for `index`.
    pub(super) fn leaf(&self, index: LeafIndex) -> Leaf<Idle> {
        Leaf::from_index(index)
    }

    /// Injects a software interrupt for `vector` via the trigger register.
    ///
    /// # Errors
    ///
    /// `EOVERFLOW` if `vector` does not fit in the trigger register's vector
    /// field.
    pub(super) fn trigger(&self, bar: Bar0<'_>, vector: u32) -> Result {
        bar.write_reg(CPU_INTR_LEAF_TRIGGER::zeroed().try_with_vector(vector)?);
        Ok(())
    }

    /// Drains any pending interrupts on this tree.
    ///
    /// Unarms the tree, acknowledges every pending bit in every implemented
    /// leaf, and arms again. Used to clear stale interrupt state, for example
    /// state left over from GSP boot.
    ///
    /// Every implemented subtree is walked rather than only those `TOP` reports
    /// as pending. `TOP` summarizes enabled leaf bits, so a bit that latched
    /// while its vector was masked does not appear there, and that is precisely
    /// the state boot leaves behind.
    pub(super) fn drain(&self, bar: Bar0<'_>) {
        self.top().unarm(bar);

        for index in 0..(self.num_leaves / 2) {
            for leaf in (Subtree { index }).iter_pending_leaves(self, bar) {
                leaf.ack(bar);
            }
        }

        self.top().arm(bar);
    }
}

/// Top-level view of the interrupt tree, gating delivery for whole subtrees.
pub(super) struct Top {
    subtree_mask: u32,
}

impl Top {
    /// Arms the tree: writes `TOP_EN_SET` to enable interrupt delivery for
    /// whole subtrees.
    ///
    /// `TOP_EN_SET` and the per-vector `LEAF_EN_SET` (see [`Leaf::allow`]) are
    /// the same enable-set write at two levels of the tree: arming gates a whole
    /// subtree at the TOP, while allowing gates an individual vector at a leaf.
    pub(super) fn arm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_SET, self.subtree_mask.into());
    }

    /// Unarms the tree (writes `TOP_EN_CLEAR`), masking interrupt delivery for
    /// all subtrees while the leaves are read and acknowledged.
    pub(super) fn unarm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_CLEAR, self.subtree_mask.into());
    }
}

/// One subtree of the interrupt tree, covering two adjacent leaves.
#[derive(Clone, Copy)]
pub(super) struct Subtree {
    index: usize,
}

impl Subtree {
    /// Yields the two [`Leaf`] handles covered by this subtree.
    fn iter_leaves<'a>(self, tree: &'a Tree) -> impl Iterator<Item = Leaf<Idle>> + 'a {
        // Each subtree covers two adjacent leaves. Callers only construct a
        // `Subtree` for an implemented index, and `LeafIndex` drops any index
        // beyond the leaf register arrays, so neither leaf can fall outside
        // this tree.
        (0..2usize).filter_map(move |offset| {
            let idx = self.index * 2 + offset;
            LeafIndex::try_new(idx).map(|idx| tree.leaf(idx))
        })
    }

    /// Like [`Self::iter_leaves`], but keeps only leaves with a non-zero
    /// pending mask.
    pub(super) fn iter_pending_leaves<'a>(
        self,
        tree: &'a Tree,
        bar: Bar0<'a>,
    ) -> impl Iterator<Item = Leaf<Pending>> + 'a {
        self.iter_leaves(tree).filter_map(move |idle| {
            let pending = idle.read_pending(bar);
            (pending.mask() != 0).then_some(pending)
        })
    }
}

/// View of a single interrupt leaf.
pub(super) struct Leaf<S: State = Idle> {
    index: LeafIndex,
    state: S,
}

impl<Left: State, Right: State> PartialEq<Leaf<Right>> for Leaf<Left> {
    fn eq(&self, other: &Leaf<Right>) -> bool {
        self.index == other.index
    }
}

impl<S: State> Eq for Leaf<S> {}

// The `try_at(...)` calls below cannot fail: `LeafIndex` is `Bounded<usize, 4>`,
// so its value is in 0..16, and every leaf register array has 16 elements.
impl Leaf<Idle> {
    /// Creates a [`Leaf`] handle for `index`.
    pub(super) fn from_index(index: LeafIndex) -> Self {
        Leaf { index, state: Idle }
    }

    /// Enables (allows) the bits in `mask` for this leaf (`LEAF_EN_SET`).
    ///
    /// This is the per-vector enable. [`Top::arm`] is the same enable-set write
    /// for a whole subtree at the TOP.
    pub(super) fn allow(&self, bar: Bar0<'_>, mask: u32) {
        if let Some(loc) = CPU_INTR_LEAF_EN_SET::try_at(self.index.get()) {
            bar.write(loc, mask.into());
        }
    }

    /// Disables (blocks) the bits in `mask` for this leaf (`LEAF_EN_CLEAR`).
    pub(super) fn block(&self, bar: Bar0<'_>, mask: u32) {
        if let Some(loc) = CPU_INTR_LEAF_EN_CLEAR::try_at(self.index.get()) {
            bar.write(loc, mask.into());
        }
    }

    /// Reads this leaf's pending mask and transitions to [`Pending`].
    pub(super) fn read_pending(self, bar: Bar0<'_>) -> Leaf<Pending> {
        let mask = CPU_INTR_LEAF::try_at(self.index.get())
            .map(|loc| bar.read(loc).into_raw())
            .unwrap_or(0);
        Leaf {
            index: self.index,
            state: Pending { mask },
        }
    }
}

impl Leaf<Pending> {
    /// Returns the pending bitmask read from hardware.
    pub(super) fn mask(&self) -> u32 {
        self.state.mask
    }

    /// Acknowledges all pending bits by writing the mask back (write-1-to-clear).
    pub(super) fn ack(&self, bar: Bar0<'_>) {
        if self.state.mask != 0 {
            if let Some(loc) = CPU_INTR_LEAF::try_at(self.index.get()) {
                bar.write(loc, self.state.mask.into());
            }
        }
    }
}
