// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! CPU doorbell interrupt self-test.
//!
//! Exercises the full MSI path (GPU to PCIe to CPU to handler) with no GSP
//! dependency: it injects a known vector through the GIN software trigger and
//! confirms the handler runs. Gated behind `CONFIG_NOVA_CORE_IRQ_SELFTEST` and
//! run before GSP boot, so it never observes or acknowledges GSP interrupt
//! state.
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
    Leaf,
    LeafIndex,
    Tree, //
};
use crate::{
    driver::Bar0,
    gpu::Chipset, //
};

/// CPU doorbell vector, fixed across architectures.
const DOORBELL_VECTOR: u32 = 129;

/// Leaf holding the doorbell vector: `129 / 32 == 4`.
const DOORBELL_LEAF: usize = 4;

/// Bit of the doorbell vector within its leaf: `129 % 32 == 1`.
const DOORBELL_BIT: u32 = 1 << 1;

/// Handler for the CPU doorbell self-test.
///
/// Runs one drain cycle (unarm, read, ack, rearm), records the doorbell leaf
/// mask it observed, and signals completion.
#[pin_data]
struct DoorbellTestHandler<'a> {
    /// Borrowed BAR0, for register access from interrupt context.
    bar: Bar0<'a>,
    tree: Tree,
    #[pin]
    completion: Completion,
    /// Number of interrupts handled.
    irq_count: Atomic<u32>,
    /// Pending mask observed on the doorbell leaf.
    doorbell_leaf_mask: Atomic<u32>,
}

impl irq::Handler for DoorbellTestHandler<'_> {
    fn handle(&self) -> irq::IrqReturn {
        let bar = self.bar;

        let top = self.tree.top().unarm(bar).read_pending(bar);
        if top.mask() == 0 {
            top.rearm(bar);
            return irq::IrqReturn::None;
        }

        let doorbell_leaf = Leaf::from_index(LeafIndex::new::<DOORBELL_LEAF>());
        for subtree in top.iter_subtrees() {
            for leaf in subtree.iter_pending_leaves(&self.tree, bar) {
                if leaf == doorbell_leaf {
                    self.doorbell_leaf_mask.store(leaf.mask(), Relaxed);
                }
                leaf.ack(bar);
            }
        }
        top.rearm(bar);

        self.irq_count.fetch_add(1, Relaxed);
        self.completion.complete_all();

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
        let _ = self.tree.top().unarm(self.bar);
    }
}

/// Runs the CPU doorbell self-test.
///
/// Quiesces the interrupt tree, registers a temporary handler, injects the
/// doorbell vector through the GIN software trigger, and confirms the handler
/// fires. This validates the MSI to GIN to ISR path without GSP firmware. The
/// handler, its IRQ registration, and all tree state are torn down before this
/// returns.
///
/// # Errors
///
/// `EIO` if the doorbell is already pending before the test, or if the
/// interrupt arrives with an unexpected count or leaf mask. `ETIMEDOUT` if no
/// interrupt arrives within the timeout.
pub(crate) fn run_selftest<'a>(
    pdev: &'a pci::Device<Bound>,
    bar: Bar0<'a>,
    chipset: Chipset,
    vector: pci::IrqVector<'_>,
) -> Result {
    let tree = Tree::new(chipset);
    let doorbell = LeafIndex::new::<DOORBELL_LEAF>();

    // Nothing may be delivered before a handler exists. Disable the doorbell
    // source, clear any stale pending state left in the tree, and leave the top
    // level masked until the handler is registered below. `drain()` rearms the
    // top level as the last step of its cycle, so unarm again afterward.
    tree.leaf(doorbell).block(bar, DOORBELL_BIT);
    tree.drain(bar);
    let _ = tree.top().unarm(bar);

    // The doorbell bit must be clear, otherwise a later pass could not be
    // attributed to the trigger below.
    let pre_mask = tree.leaf(doorbell).read_pending(bar).mask();
    if pre_mask & DOORBELL_BIT != 0 {
        dev_warn!(
            pdev.as_ref(),
            "CPU doorbell self-test: FAIL (doorbell bit already pending, leaf[{}] mask={:#x})\n",
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
        completion <- Completion::new(),
        irq_count: Atomic::new(0),
        doorbell_leaf_mask: Atomic::new(0),
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

    let completed = handler
        .completion
        .wait_for_completion_timeout(time::msecs_to_jiffies(1000));
    let count = handler.irq_count.load(Relaxed);
    let leaf_mask = handler.doorbell_leaf_mask.load(Relaxed);

    if completed && count == 1 && leaf_mask & DOORBELL_BIT != 0 {
        dev_info!(
            pdev.as_ref(),
            "CPU doorbell self-test: PASS (irq_count={}, leaf[{}] mask={:#x})\n",
            count,
            DOORBELL_LEAF,
            leaf_mask,
        );
        Ok(())
    } else {
        dev_warn!(
            pdev.as_ref(),
            "CPU doorbell self-test: FAIL (completed={}, irq_count={}, leaf[{}] mask={:#x})\n",
            completed,
            count,
            DOORBELL_LEAF,
            leaf_mask,
        );
        Err(if completed { EIO } else { ETIMEDOUT })
    }
}
