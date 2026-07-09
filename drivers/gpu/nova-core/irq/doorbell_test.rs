// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Interrupt delivery self-test, driven through the CPU doorbell vector.
//!
//! Exercises the whole PCI interrupt path (GPU to PCIe to CPU to handler) with
//! no GSP dependency: it injects a known vector through the GIN software
//! trigger and confirms the handler runs. Two interrupts are triggered one at a
//! time, which also covers the rearm that every delivery after the first
//! depends on. Gated behind `CONFIG_NOVA_CORE_IRQ_SELFTEST` and run before GSP
//! boot, so it never observes or acknowledges GSP interrupt state.
//!
//! See `Documentation/gpu/nova/core/interrupts.rst`.

use core::pin::Pin;

use kernel::{
    device::Bound,
    irq,
    pci,
    prelude::*,
    sync::{
        atomic::{
            Atomic,
            Relaxed, //
        },
        Completion, //
    },
    time, //
};

use super::interrupt_tree::{
    subtree_bit,
    vector_leaf_bit,
    LeafIndex,
    Tree, //
};
use crate::{
    driver::Bar0,
    gpu::Chipset, //
};

/// Fixed vector for the CPU doorbell, the same number on every supported part.
const DOORBELL_VECTOR: u32 = 129;

/// Leaf and bit index of the doorbell vector within the interrupt tree.
const DOORBELL_LOC: (usize, u32) = vector_leaf_bit(DOORBELL_VECTOR);

/// Leaf holding the doorbell vector.
const DOORBELL_LEAF: usize = DOORBELL_LOC.0;

/// Bit mask of the doorbell vector within its leaf.
const DOORBELL_BIT: u32 = 1 << DOORBELL_LOC.1;

/// Subtree carrying the doorbell vector, and the only subtree this test arms.
///
/// Derived from the vector so that changing `DOORBELL_VECTOR` moves the arming and the handler
/// together.
const DOORBELL_SUBTREE: u32 = subtree_bit(DOORBELL_VECTOR);

// The self-test reuses the vector probe allocated for the GSP notification, and that vector serves
// the highest armed subtree, so the doorbell vector has to lie in the same subtree as the GSP
// notification.
static_assert!(DOORBELL_SUBTREE == subtree_bit(super::gsp::GSP_INTR_0_VECTOR));

/// Index of the subtree carrying the doorbell vector. Under MSI-X this is also the index of the
/// table entry that subtree raises.
const DOORBELL_SUBTREE_INDEX: u32 = DOORBELL_SUBTREE.trailing_zeros();

/// Time allowed for each of the two deliveries to arrive.
const DELIVERY_TIMEOUT_MS: time::Msecs = 1000;

/// Interrupt handler installed by the self-test.
///
/// Services the doorbell the way a notification source is serviced: it
/// acknowledges its own leaf and rearms PCI interrupt delivery, leaving the
/// rest of the tree untouched. It records the leaf mask seen on each of the
/// first two deliveries and signals the matching completion.
#[pin_data]
struct DoorbellTestHandler<'a> {
    /// Borrowed BAR0, for register access from interrupt context.
    bar: Bar0<'a>,
    tree: Tree,
    /// Signalled by the first delivery.
    #[pin]
    first: Completion,
    /// Signalled by the second delivery.
    #[pin]
    second: Completion,
    /// Count of deliveries this handler has serviced.
    irq_count: Atomic<u32>,
    /// Doorbell leaf mask observed on the first delivery.
    first_mask: Atomic<u32>,
    /// Doorbell leaf mask observed on the second delivery.
    second_mask: Atomic<u32>,
}

impl irq::Handler for DoorbellTestHandler<'_> {
    fn handle(&self) -> irq::IrqReturn {
        let bar = self.bar;

        // Acknowledge only this handler's own leaf and leave `TOP_EN` alone. A
        // full walk unarms and rearms the tree, which produces a delivery edge
        // by itself and would hide a missing PCI interrupt rearm.
        let leaf = self
            .tree
            .leaf(LeafIndex::new::<DOORBELL_LEAF>())
            .read_pending(bar);
        let mask = leaf.mask();
        if mask & DOORBELL_BIT == 0 {
            self.tree.rearm_pci_irq(bar);
            return irq::IrqReturn::None;
        }
        leaf.ack(bar);

        let count = self.irq_count.fetch_add(1, Relaxed);

        // Rearm before signalling, so delivery is possible again by the time
        // the waiting thread triggers the next vector.
        self.tree.rearm_pci_irq(bar);

        match count {
            0 => {
                self.first_mask.store(mask, Relaxed);
                self.first.complete_all();
            }
            1 => {
                self.second_mask.store(mask, Relaxed);
                self.second.complete_all();
            }
            _ => (),
        }

        irq::IrqReturn::Handled
    }
}

/// Teardown guard for the self-test.
///
/// Owns the IRQ registration so that, on every exit path (including an early
/// error), the interrupt is torn down in a race-free order: blocking the leaf
/// stops new deliveries, dropping the registration runs `free_irq()` (which
/// waits for any handler still in flight), and only then is the tree unarmed,
/// so a late handler cannot rearm it behind us.
struct SelftestGuard<'a, 'r> {
    bar: Bar0<'a>,
    tree: Tree,
    doorbell: LeafIndex,
    reg: Option<Pin<KBox<irq::Registration<'r, DoorbellTestHandler<'a>>>>>,
}

impl<'a, 'r> SelftestGuard<'a, 'r> {
    /// Returns the registered handler.
    fn handler(&self) -> &DoorbellTestHandler<'a> {
        // `reg` is `Some` for the whole lifetime of the guard; only `drop`
        // clears it.
        self.reg.as_ref().unwrap().handler()
    }
}

impl Drop for SelftestGuard<'_, '_> {
    fn drop(&mut self) {
        self.tree.leaf(self.doorbell).block(self.bar, DOORBELL_BIT);
        self.reg = None;
        self.tree.top().unarm(self.bar);
    }
}

/// Runs the interrupt delivery self-test.
///
/// Quiesces the interrupt tree, registers a temporary handler, and injects the
/// doorbell vector through the GIN software trigger twice, one delivery at a
/// time. This validates the PCI interrupt path from GIN to the ISR without GSP
/// firmware, including the rearm without which only the first interrupt would
/// arrive. The handler, its IRQ registration, and all tree state are torn down
/// before this returns.
///
/// # Errors
///
/// `EIO` if the doorbell is already pending before the test, or if a delivery
/// arrives with an unexpected count or leaf mask. `ETIMEDOUT` if either
/// delivery does not arrive within the timeout.
pub(crate) fn run_selftest<'a>(
    pdev: &'a pci::Device<Bound>,
    bar: Bar0<'a>,
    chipset: Chipset,
    vector: pci::IrqVector<'_>,
) -> Result {
    // The granted interrupt type decides how the handler rearms delivery.
    let irq_type = vector.irq_type();
    let tree = Tree::new(chipset, irq_type, DOORBELL_SUBTREE);
    let doorbell = LeafIndex::new::<DOORBELL_LEAF>();

    // A result means something different under MSI than under MSI-X. Under MSI-X the subtree
    // index is also the table entry the delivery came through, which is what shows that the
    // per-subtree routing works.
    dev_info!(
        pdev.as_ref(),
        "interrupt self-test: starting on vector {}, subtree {}, with {:?}\n",
        DOORBELL_VECTOR,
        DOORBELL_SUBTREE_INDEX,
        irq_type,
    );

    // Nothing may be delivered before a handler exists. Disable the doorbell
    // source, clear any stale pending state left in the tree, and leave the top
    // level masked until the handler is registered below. `drain()` rearms the
    // top level as the last step of its cycle, so unarm again afterward.
    tree.leaf(doorbell).block(bar, DOORBELL_BIT);
    tree.drain(bar);
    tree.top().unarm(bar);

    // A delivery can be credited to the trigger below only if the vector starts
    // out clear, so refuse to run otherwise.
    let pre_mask = tree.leaf(doorbell).read_pending(bar).mask();
    if pre_mask & DOORBELL_BIT != 0 {
        dev_warn!(
            pdev.as_ref(),
            "interrupt self-test: failed, vector {} already pending (leaf[{}] mask {:#x})\n",
            DOORBELL_VECTOR,
            DOORBELL_LEAF,
            pre_mask,
        );
        return Err(EIO);
    }

    // `try_pin_init!` moves its captures into a closure, so clone `tree` into a
    // local up front rather than inline, which would move `tree` and leave none
    // for the guard below.
    let handler_tree = tree.clone();
    let handler_init = try_pin_init!(DoorbellTestHandler {
        bar,
        tree: handler_tree,
        first <- Completion::new(),
        second <- Completion::new(),
        irq_count: Atomic::new(0),
        first_mask: Atomic::new(0),
        second_mask: Atomic::new(0),
    }? Error);

    // Register the handler before allowing any source to fire.
    let reg = KBox::pin_init(
        // SAFETY: the registration is owned by `guard` below and dropped before
        // this function returns, so its `Drop` (which calls `free_irq()`)
        // always runs and the registration is never leaked or `mem::forget`-ed.
        unsafe { pdev.request_irq(vector, irq::Flags::TRIGGER_NONE, c"nova-core", handler_init) },
        GFP_KERNEL,
    )?;

    // From here every exit must tear down the source, the registration, and the
    // tree, so hand the registration to a guard that does so on drop.
    let guard = SelftestGuard {
        bar,
        tree: tree.clone(),
        doorbell,
        reg: Some(reg),
    };
    let handler = guard.handler();

    // A handler is installed now: allow the doorbell vector and arm the tree.
    handler.tree.leaf(doorbell).allow(bar, DOORBELL_BIT);
    handler.tree.top().arm(bar);

    // Inject the doorbell vector through the software trigger.
    handler.tree.trigger(bar, DOORBELL_VECTOR)?;
    let mut completed = handler
        .first
        .wait_for_completion_timeout(time::msecs_to_jiffies(DELIVERY_TIMEOUT_MS));

    // Trigger the second interrupt only once the first handler has
    // acknowledged and rearmed, so the two cannot coalesce into one delivery
    // and a handler that never rearms cannot pass.
    if completed {
        handler.tree.trigger(bar, DOORBELL_VECTOR)?;
        completed = handler
            .second
            .wait_for_completion_timeout(time::msecs_to_jiffies(DELIVERY_TIMEOUT_MS));
    }

    let count = handler.irq_count.load(Relaxed);
    let first_mask = handler.first_mask.load(Relaxed);
    let second_mask = handler.second_mask.load(Relaxed);
    let masks_seen = (first_mask & DOORBELL_BIT != 0) && (second_mask & DOORBELL_BIT != 0);

    if completed && count == 2 && masks_seen {
        dev_info!(
            pdev.as_ref(),
            "interrupt self-test: passed, subtree {}, {} deliveries, leaf[{}] masks {:#x} {:#x}\n",
            DOORBELL_SUBTREE_INDEX,
            count,
            DOORBELL_LEAF,
            first_mask,
            second_mask,
        );
        Ok(())
    } else {
        dev_warn!(
            pdev.as_ref(),
            "interrupt self-test: failed, {} of 2 deliveries, leaf[{}] masks {:#x} and {:#x}\n",
            count,
            DOORBELL_LEAF,
            first_mask,
            second_mask,
        );
        Err(if completed { EIO } else { ETIMEDOUT })
    }
}
