// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Type-state API for walking the GIN PF CPU interrupt tree.
//!
//! Each PCIe function has its own interrupt tree. This module only drives the
//! PF's CPU tree. The type states (`Idle` -> `Unarmed` -> `Pending`) enforce
//! the unarm -> read -> ack -> rearm ordering at compile time, so a handler
//! cannot, for example, acknowledge leaves before it has read the pending
//! snapshot.
//!
//! The compiler enforces this order on each [`Top`] or [`Leaf`] value on its
//! own: every state-changing method consumes the value and returns the next
//! state. It does not coordinate the tree as a whole. Nothing prevents two
//! `Top` values at once, and a `Leaf` can be read and acked without unarming
//! `TOP` first, as the GSP notification path does. Coordinating the whole
//! tree is the caller's responsibility.
//!
//! See `Documentation/gpu/nova/core/interrupts.rst`.

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
    gpu::Chipset,
    irq::hal::gin_hal,
    regs::{
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF as CPU_INTR_LEAF,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_EN_CLEAR as CPU_INTR_LEAF_EN_CLEAR,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_EN_SET as CPU_INTR_LEAF_EN_SET,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_LEAF_TRIGGER as CPU_INTR_LEAF_TRIGGER,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP as CPU_INTR_TOP,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_CLEAR as CPU_INTR_TOP_EN_CLEAR,
        NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_SET as CPU_INTR_TOP_EN_SET, //
    },
};

/// Index of a leaf register, bounded to the `0..16` range covered by the leaf
/// register arrays.
pub(super) type LeafIndex = Bounded<usize, 4>;

/// Maps an interrupt `vector` to its position in the tree: the leaf that carries it
/// (`vector / 32`) and the bit index within that leaf (`vector % 32`).
///
/// This is the single definition of the vector encoding, shared by the GSP interrupt handler and
/// the unit tests. The returned leaf is a raw index; a caller validates it against the
/// architecture's leaf count, for example via [`LeafIndex::try_new`].
pub(super) const fn vector_leaf_bit(vector: u32) -> (usize, u32) {
    (crate::num::u32_as_usize(vector / 32), vector % 32)
}

/// Type state of a [`Top`] or [`Leaf`] handle.
///
/// A `Top` follows `Idle` -> `Unarmed` -> `Pending`, and a `Leaf` follows
/// `Idle` -> `Pending`. Encoding the stages as types makes the illegal
/// orderings (such as acknowledging before reading the pending snapshot)
/// fail to compile.
pub(super) trait State: private::Sealed {}

/// State in which the tree may or may not be armed and no snapshot is held.
pub(super) struct Idle;
impl State for Idle {}

/// State in which this handle has just cleared `TOP_EN`, before reading the
/// pending snapshot.
pub(super) struct Unarmed;
impl State for Unarmed {}

/// State holding a pending bitmask read from hardware.
pub(super) struct Pending {
    mask: u32,
}
impl State for Pending {}

mod private {
    pub(in crate::irq) trait Sealed {}
    impl Sealed for super::Idle {}
    impl Sealed for super::Unarmed {}
    impl Sealed for super::Pending {}
}

/// The GIN PF CPU interrupt tree for a single PCIe function.
#[derive(Clone)]
pub(super) struct Tree {
    /// Number of implemented leaves in this tree, either 8 or 16.
    // Read only by `trigger`, which the self-test alone uses today.
    #[cfg_attr(not(CONFIG_NOVA_CORE_IRQ_SELFTEST), expect(dead_code))]
    num_leaves: usize,
    /// Mask of valid subtree bits in the `TOP` enable registers.
    subtree_mask: u32,
}

impl Tree {
    /// Creates a `Tree` for `chipset`, sized by the interrupt HAL.
    pub(super) fn new(chipset: Chipset) -> Self {
        let hal = gin_hal(chipset);
        Self {
            num_leaves: hal.num_leaves(),
            subtree_mask: hal.subtree_mask(),
        }
    }

    /// Returns a [`Top`] handle in the [`Idle`] state.
    pub(super) fn top(&self) -> Top<Idle> {
        Top {
            subtree_mask: self.subtree_mask,
            state: Idle,
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
    /// `EINVAL` if `vector` lies outside this tree (`vector >= num_leaves *
    /// 32`). `EOVERFLOW` if `vector` does not fit in the trigger register's
    /// vector field.
    // Only the interrupt self-test injects a software interrupt.
    #[cfg_attr(not(CONFIG_NOVA_CORE_IRQ_SELFTEST), expect(dead_code))]
    pub(super) fn trigger(&self, bar: Bar0<'_>, vector: u32) -> Result {
        if crate::num::u32_as_usize(vector) >= self.num_leaves * 32 {
            return Err(EINVAL);
        }
        bar.write_reg(CPU_INTR_LEAF_TRIGGER::zeroed().try_with_vector(vector)?);
        Ok(())
    }

    /// Drains any pending interrupts on this tree.
    ///
    /// Unarms the tree, reads the pending subtrees, acknowledges every pending
    /// leaf, and rearms. Used to clear stale interrupt state, for example state
    /// left over from GSP boot.
    pub(super) fn drain(&self, bar: Bar0<'_>) {
        let top = self.top().unarm(bar).read_pending(bar);

        for subtree in top.iter_subtrees() {
            for leaf in subtree.iter_pending_leaves(self, bar) {
                leaf.ack(bar);
            }
        }

        top.rearm(bar);
    }
}

/// Top-level view of the interrupt tree.
pub(super) struct Top<S: State = Idle> {
    subtree_mask: u32,
    state: S,
}

impl Top<Idle> {
    /// Arms the tree: writes `TOP_EN_SET` to enable MSI delivery for whole
    /// subtrees.
    ///
    /// `TOP_EN_SET` and the per-vector `LEAF_EN_SET` (see [`Leaf::allow`]) are
    /// the same enable-set write at two levels of the tree: arming gates a whole
    /// subtree's MSI at the TOP, while allowing gates an individual vector at a
    /// leaf.
    ///
    /// Used for one-shot initial setup before any interrupts are expected. The
    /// handler's normal rearm path goes through [`Top::unarm`] ->
    /// [`Top::read_pending`] -> [`Top::rearm`] instead.
    // Only the interrupt self-test arms the tree directly; the GSP path arms via `drain`.
    #[cfg_attr(not(CONFIG_NOVA_CORE_IRQ_SELFTEST), expect(dead_code))]
    pub(super) fn arm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_SET, self.subtree_mask.into());
    }

    /// Unarms the tree (writes `TOP_EN_CLEAR`), masking MSI delivery for all
    /// subtrees while the host reads and acks the leaves, and transitions to
    /// [`Unarmed`].
    ///
    /// This is the full-tree drain path. It is not required for a steady-state
    /// notification source, which can stay armed and just ack its leaf.
    pub(super) fn unarm(self, bar: Bar0<'_>) -> Top<Unarmed> {
        bar.write(CPU_INTR_TOP_EN_CLEAR, self.subtree_mask.into());
        Top {
            subtree_mask: self.subtree_mask,
            state: Unarmed,
        }
    }
}

impl Top<Unarmed> {
    /// Reads the `TOP` pending bitmask and transitions to [`Pending`].
    ///
    /// The snapshot is masked to this tree's valid subtree bits, so a spurious
    /// or unimplemented high bit cannot make iteration descend into a subtree
    /// the architecture does not have.
    pub(super) fn read_pending(self, bar: Bar0<'_>) -> Top<Pending> {
        let mask = bar.read(CPU_INTR_TOP).into_raw() & self.subtree_mask;
        Top {
            subtree_mask: self.subtree_mask,
            state: Pending { mask },
        }
    }
}

/// One subtree of the `TOP` pending mask, covering two adjacent leaves.
#[derive(Clone, Copy)]
pub(super) struct Subtree {
    index: usize,
}

impl Subtree {
    /// Yields the two [`Leaf`] handles covered by this subtree.
    fn iter_leaves<'a>(self, tree: &'a Tree) -> impl Iterator<Item = Leaf<Idle>> + 'a {
        // Each subtree covers two adjacent leaves on all architectures. The
        // leaf-register arrays have 16 entries on every arch, out-of-range
        // indices are dropped by `LeafIndex`, and any implemented-but-unused
        // leaf reads back zero, so materializing both leaves of a subtree is
        // harmless.
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

impl Top<Pending> {
    /// Returns the `TOP` pending bitmask snapshot.
    pub(super) fn mask(&self) -> u32 {
        self.state.mask
    }

    /// Iterates over the subtrees with a pending bit set in the snapshot.
    pub(super) fn iter_subtrees(&self) -> impl Iterator<Item = Subtree> + '_ {
        (0..32usize)
            .filter(move |&bit| self.state.mask & (1u32 << bit) != 0)
            .map(|index| Subtree { index })
    }

    /// Rearms the tree (writes `TOP_EN_SET`), consuming the snapshot so it
    /// cannot be acted on again.
    pub(super) fn rearm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_SET, self.subtree_mask.into());
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

    /// Acknowledges only the bits in `mask` (write-1-to-clear), leaving any other pending bits
    /// set for their owners.
    ///
    /// A handler that owns a single vector uses this instead of [`Self::ack`] so it does not clear
    /// a co-pending vector in the same leaf.
    pub(super) fn ack_bits(&self, bar: Bar0<'_>, mask: u32) {
        if mask != 0 {
            if let Some(loc) = CPU_INTR_LEAF::try_at(self.index.get()) {
                bar.write(loc, mask.into());
            }
        }
    }
}

#[kunit_tests(nova_core_gin_tree)]
mod tests {
    use super::*;

    /// A leaf index is a `Bounded<usize, 4>`, so it accepts 0..=15 and rejects 16.
    #[test]
    fn leaf_index_bounds() {
        assert!(LeafIndex::try_new(0).is_some());
        assert!(LeafIndex::try_new(15).is_some());
        assert!(LeafIndex::try_new(16).is_none());
    }

    /// Subtree `N` covers the two adjacent leaves `2N` and `2N + 1`.
    #[test]
    fn subtree_covers_two_adjacent_leaves() {
        let tree = Tree {
            num_leaves: 16,
            subtree_mask: 0xff,
        };

        for index in 0..8usize {
            let mut leaves = Subtree { index }.iter_leaves(&tree);
            assert_eq!(leaves.next().map(|leaf| leaf.index.get()), Some(index * 2));
            assert_eq!(
                leaves.next().map(|leaf| leaf.index.get()),
                Some(index * 2 + 1)
            );
            assert!(leaves.next().is_none());
        }
    }

    /// Leaves that fall outside the addressable range are filtered out, never panicking.
    #[test]
    fn subtree_leaves_out_of_range_are_filtered() {
        let tree = Tree {
            num_leaves: 0,
            subtree_mask: 0,
        };

        // Subtree 8 would cover leaves 16 and 17, both beyond the leaf index range.
        assert!(Subtree { index: 8 }.iter_leaves(&tree).next().is_none());
    }

    /// Iterating the pending subtrees of a `TOP` snapshot yields exactly the set bits.
    #[test]
    fn iter_subtrees_yields_set_bits() {
        let top = Top {
            subtree_mask: 0xff,
            state: Pending { mask: 0b10_0101 },
        };

        let mut subtrees = top.iter_subtrees();
        assert_eq!(subtrees.next().map(|s| s.index), Some(0));
        assert_eq!(subtrees.next().map(|s| s.index), Some(2));
        assert_eq!(subtrees.next().map(|s| s.index), Some(5));
        assert!(subtrees.next().is_none());
    }

    /// The production [`vector_leaf_bit`] maps every vector to a `(leaf, bit)` pair, valid leaves
    /// stay within [`LeafIndex`], and the fixed doorbell (129) and GSP (155) vectors land where
    /// the handlers expect.
    #[test]
    fn vector_maps_to_leaf_and_bit() {
        // Every vector of a 16-leaf tree maps to an addressable leaf and a bit in 0..32.
        for vector in 0u32..(16 * 32) {
            let (leaf, bit) = vector_leaf_bit(vector);

            assert!(LeafIndex::try_new(leaf).is_some());
            assert!(bit < 32);
            assert_eq!(leaf as u32 * 32 + bit, vector);
        }

        // The fixed vectors the handlers rely on: CPU doorbell 129 and GSP notification 155, both
        // in leaf 4, which is present on both the 8-leaf (pre-Hopper) and 16-leaf trees.
        assert_eq!(vector_leaf_bit(129), (4, 1));
        assert_eq!(vector_leaf_bit(155), (4, 27));
        assert!(LeafIndex::try_new(vector_leaf_bit(155).0).is_some());

        // The first vector beyond the 16-leaf tree lands in leaf 16, which is out of range.
        assert!(LeafIndex::try_new(vector_leaf_bit(16 * 32).0).is_none());
    }
}
