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
    pci::IrqType,
    prelude::*,
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    irq::hal::{
        pf_cpu_interrupt_hal,
        PciIrqRearmMethod, //
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

/// Maps an interrupt `vector` to its position in the tree: the leaf that carries it
/// (`vector / 32`) and the bit index within that leaf (`vector % 32`).
///
/// This is the single definition of the vector encoding, shared by the interrupt handlers and the
/// unit tests. The returned leaf is a raw index; a caller validates it against the architecture's
/// leaf count, for example via [`LeafIndex::try_new`].
pub(super) const fn vector_leaf_bit(vector: u32) -> (usize, u32) {
    (crate::num::u32_as_usize(vector / 32), vector % 32)
}

/// Maps an interrupt `vector` to the `TOP` enable bit of the subtree that carries it.
///
/// A subtree covers two adjacent leaves, so the vector's leaf is in subtree `vector / 64`. The
/// result is a one-bit mask, in the form [`Tree::new`] takes as an armed mask and `TOP_EN_SET`
/// and `TOP_EN_CLEAR` take as a value.
///
/// The bit is not validated against the architecture's subtree count, which the caller checks
/// against [`super::hal::PfCpuInterruptHal::subtree_mask`].
pub(super) const fn subtree_bit(vector: u32) -> u32 {
    1 << (vector / 64)
}

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
    /// Mask of the subtrees this tree arms and services.
    armed_mask: u32,
    /// Method that rearms PCI interrupt delivery, or `None` if the allocated
    /// interrupt type needs no rearm write.
    rearm_method: Option<PciIrqRearmMethod>,
}

impl Tree {
    /// Creates a `Tree` for `chipset`, sized by the interrupt HAL, arming the
    /// subtrees in `armed_mask` and carrying the rearm method for `irq_type`.
    ///
    /// `armed_mask` is the set of subtrees this tree's handler arms and
    /// services. A subtree armed here must have an allocated PCI vector and a
    /// registered handler, which [`super::alloc_vectors`] establishes for the
    /// same mask. Bits outside the subtrees the architecture implements are
    /// dropped, since such a subtree cannot deliver anything.
    ///
    /// The HAL is consulted once here so that an interrupt handler branches on
    /// neither the chipset nor the interrupt type.
    pub(super) fn new(chipset: Chipset, irq_type: IrqType, armed_mask: u32) -> Self {
        let hal = pf_cpu_interrupt_hal(chipset);
        Self {
            num_leaves: hal.num_leaves(),
            armed_mask: armed_mask & hal.subtree_mask(),
            rearm_method: hal.pci_irq_rearm_method(irq_type),
        }
    }

    /// Rearms PCI interrupt delivery to the CPU.
    ///
    /// A handler must call this before it returns, or it receives no further
    /// interrupts.
    ///
    /// Both `TOP` enable cycles take the armed mask: this tree's handler serves
    /// exactly the subtrees it arms, so the mask that MSI cycles for the whole
    /// tree and the mask that MSI-X cycles for one subtree are the same here.
    pub(super) fn rearm_pci_irq(&self, bar: Bar0<'_>) {
        if let Some(method) = self.rearm_method {
            method.rearm(bar, self.armed_mask);
        }
    }

    /// Returns a [`Top`] handle for this tree.
    pub(super) fn top(&self) -> Top {
        Top {
            armed_mask: self.armed_mask,
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

    /// Masks every vector in every implemented leaf (`LEAF_EN_CLEAR`).
    ///
    /// Boot, or a driver that ran before this one, can leave leaf enables set
    /// for vectors nova-core does not service, and such a vector delivers to
    /// nova-core's handler once its subtree is armed. Clearing all of them
    /// first means only the vectors nova-core goes on to allow can deliver.
    pub(super) fn block_all_leaves(&self, bar: Bar0<'_>) {
        for index in 0..self.num_leaves {
            if let Some(index) = LeafIndex::try_new(index) {
                self.leaf(index).block(bar, u32::MAX);
            }
        }
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
    /// the state boot leaves behind. The walk therefore covers subtrees this
    /// tree does not arm, while the arm and unarm around it touch the armed
    /// mask alone.
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
///
/// Both writes cover the armed mask, so nova-core gates only the subtrees it services and leaves
/// the rest of the tree as it found it.
pub(super) struct Top {
    armed_mask: u32,
}

impl Top {
    /// Arms this tree's subtrees: writes `TOP_EN_SET` to enable interrupt
    /// delivery for each of them.
    ///
    /// `TOP_EN_SET` and the per-vector `LEAF_EN_SET` (see [`Leaf::allow`]) are
    /// the same enable-set write at two levels of the tree: arming gates a whole
    /// subtree at the TOP, while allowing gates an individual vector at a leaf.
    pub(super) fn arm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_SET, self.armed_mask.into());
    }

    /// Unarms this tree's subtrees (writes `TOP_EN_CLEAR`), masking interrupt
    /// delivery for each of them while the leaves are read and acknowledged.
    pub(super) fn unarm(self, bar: Bar0<'_>) {
        bar.write(CPU_INTR_TOP_EN_CLEAR, self.armed_mask.into());
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
