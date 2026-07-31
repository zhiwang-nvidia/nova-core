// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Interrupt delivery self-test, driven through the CPU doorbell vector.
//!
//! Injects a known vector through the GIN software trigger twice, one delivery at a time, and
//! confirms both reach the handler. Gated behind `CONFIG_NOVA_CORE_SELFTESTS` and run before GSP
//! boot, so it never reads or clears GSP interrupt state.
//!
//! See "Self-test" in `Documentation/gpu/nova/core/interrupts.rst`.

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
    GinVector,
    LeafEnableGuard,
    LeafMask,
    Subtree,
    TopEnableGuard,
    Tree, //
};

use crate::{
    driver::Bar0,
    gpu::Chipset,
    selftest_assert,
    selftest_assert_eq, //
};

/// Fixed CPU doorbell vector, hardwired on every supported chip, so the test can inject it
/// before GSP-RM runs.
const DOORBELL_VECTOR: GinVector = GinVector::new::<129>();

/// Subtree carrying the doorbell vector, and the only subtree this test services.
const DOORBELL_SUBTREE: Subtree = DOORBELL_VECTOR.subtree();

/// Time allowed for each of the two deliveries to arrive.
const DELIVERY_TIMEOUT_MS: time::Msecs = 1000;

/// Interrupt handler installed by the self-test.
///
/// Clears its own leaf bit and rearms PCI interrupt delivery, leaving the rest of the tree
/// untouched, which is what a notification handler does. Records the leaf's pending bits from
/// each of the first two deliveries and signals the matching completion.
#[pin_data]
struct DoorbellTestHandler<'a> {
    /// The interrupt tree, which carries the borrowed BAR0 that register access needs.
    tree: Tree<'a>,
    /// Signaled by the first delivery.
    #[pin]
    first: Completion,
    /// Signaled by the second delivery.
    #[pin]
    second: Completion,
    /// Count of deliveries this handler has serviced.
    irq_count: Atomic<u32>,
    /// Doorbell leaf's pending bits observed on the first delivery.
    first_pending: Atomic<u32>,
    /// Doorbell leaf's pending bits observed on the second delivery.
    second_pending: Atomic<u32>,
}

impl irq::Handler for DoorbellTestHandler<'_> {
    fn handle(&self) -> irq::IrqReturn {
        // Clear only this handler's own bit and leave `TOP_EN` alone, so a missing rearm shows up
        // as a missing delivery.
        let leaf = self.tree.read_pending(DOORBELL_VECTOR.leaf_index());
        let pending = leaf.vectors();
        if !pending.contains(DOORBELL_VECTOR.leaf_mask()) {
            self.tree.rearm_pci_irq(DOORBELL_SUBTREE);
            return irq::IrqReturn::None;
        }
        leaf.clear_vectors(DOORBELL_VECTOR.leaf_mask());

        let count = self.irq_count.fetch_add(1, Relaxed);

        // Rearm before signaling, so delivery is possible again by the time the waiting thread
        // triggers the next vector.
        self.tree.rearm_pci_irq(DOORBELL_SUBTREE);

        match count {
            0 => {
                self.first_pending.store(pending.into_raw(), Relaxed);
                self.first.complete_all();
            }
            1 => {
                self.second_pending.store(pending.into_raw(), Relaxed);
                self.second.complete_all();
            }
            _ => (),
        }

        irq::IrqReturn::Handled
    }
}

/// Everything the running self-test owns.
///
/// Declaration order is the teardown order, for the reason "Enabling the GSP event" gives for the
/// GSP handler.
struct SelftestResources<'a, 'r> {
    _leaf_guard: LeafEnableGuard<'a>,
    reg: Pin<KBox<irq::Registration<'r, DoorbellTestHandler<'a>>>>,
    _top_guard: TopEnableGuard<'a>,
}

impl<'a> SelftestResources<'a, '_> {
    /// Returns the registered handler.
    fn handler(&self) -> &DoorbellTestHandler<'a> {
        self.reg.handler()
    }

    /// Disables the doorbell source and waits for a handler already running on another CPU.
    ///
    /// On return no further delivery can reach the handler, so its counters and the doorbell
    /// leaf hold their final values.
    fn quiesce_source(&self) {
        self.handler()
            .tree
            .disable_leaf(DOORBELL_VECTOR.leaf_index(), DOORBELL_VECTOR.leaf_mask());
        self.reg.synchronize();
    }
}

/// Runs the interrupt delivery self-test.
///
/// Quiesces the interrupt tree, registers a temporary handler, and injects the doorbell vector
/// twice, one delivery at a time. The vector allocation, the handler, its IRQ registration, and
/// all tree state are torn down before this returns, so the driver allocates its own vectors
/// afterwards.
///
/// # Errors
///
/// `EINVAL` if the doorbell's subtree is not one this architecture implements. `ETIMEDOUT` if
/// either delivery does not arrive within the timeout. `EIO` if any assertion about the tree's
/// state fails. Otherwise the error from the vector allocation.
pub(crate) fn run_selftest(pdev: &pci::Device<Bound>, bar: Bar0<'_>, chipset: Chipset) -> Result {
    let dev = pdev.as_ref();

    let vectors = super::alloc_vectors(pdev, DOORBELL_SUBTREE.into())?;
    let request = vectors.request_for(DOORBELL_SUBTREE)?;
    let tree = Tree::new(bar, chipset, &vectors)?;
    let doorbell = DOORBELL_VECTOR.leaf_index();
    let doorbell_mask = DOORBELL_VECTOR.leaf_mask();

    dev_info!(
        dev,
        "interrupt self-test: starting on vector {}, subtree {}, with {:?}\n",
        DOORBELL_VECTOR.into_raw(),
        DOORBELL_SUBTREE.index(),
        vectors.msi_type,
    );

    // No delivery may reach the CPU before a handler is registered, and a vector left enabled by
    // boot would fail the pending check below.
    tree.disable_all_leaves();
    tree.drain();

    // A delivery counts as the trigger's only if the vector starts out clear.
    let pre_pending = tree.read_pending(doorbell).vectors();
    selftest_assert!(
        dev,
        !pre_pending.contains(doorbell_mask),
        "vector {} already pending, leaf[{}] is {:#x}",
        DOORBELL_VECTOR.into_raw(),
        doorbell.get(),
        pre_pending.into_raw()
    );

    let handler_init = try_pin_init!(DoorbellTestHandler {
        tree,
        first <- Completion::new(),
        second <- Completion::new(),
        irq_count: Atomic::new(0),
        first_pending: Atomic::new(0),
        second_pending: Atomic::new(0),
    }? Error);

    // Register the handler before any vector is enabled.
    let reg = KBox::pin_init(
        // SAFETY: the registration is owned by `resources` below and dropped before this function
        // returns, so its `Drop` (which calls `free_irq()`) always runs and the registration is
        // never leaked or `mem::forget`-ed.
        unsafe {
            irq::Registration::new(
                request,
                irq::Flags::TRIGGER_NONE,
                c"nova-core-selftest",
                handler_init,
            )
        },
        GFP_KERNEL,
    )?;

    let resources = SelftestResources {
        _leaf_guard: reg
            .handler()
            .tree
            .enable_leaf_guarded(doorbell, doorbell_mask),
        _top_guard: reg.handler().tree.enable_top_guarded(),
        reg,
    };
    let handler = resources.handler();

    handler.tree.trigger(DOORBELL_VECTOR)?;
    let mut completed = handler
        .first
        .wait_for_completion_timeout(time::msecs_to_jiffies(DELIVERY_TIMEOUT_MS))
        .is_some();

    // Trigger the second interrupt only after the first handler has rearmed, so the two cannot
    // coalesce into one delivery.
    if completed {
        handler.tree.trigger(DOORBELL_VECTOR)?;
        completed = handler
            .second
            .wait_for_completion_timeout(time::msecs_to_jiffies(DELIVERY_TIMEOUT_MS))
            .is_some();
    }

    resources.quiesce_source();

    let count = handler.irq_count.load(Relaxed);
    let first_pending = LeafMask::from_raw(handler.first_pending.load(Relaxed));
    let second_pending = LeafMask::from_raw(handler.second_pending.load(Relaxed));
    let residual = handler.tree.read_pending(doorbell).vectors();

    if !completed {
        dev_err!(
            dev,
            "interrupt self-test: only {} of 2 deliveries arrived within {} ms\n",
            count,
            DELIVERY_TIMEOUT_MS,
        );
        return Err(ETIMEDOUT);
    }

    selftest_assert_eq!(dev, count, 2, "delivery count");

    // Nothing else can be pending in this leaf, so require the exact mask.
    selftest_assert_eq!(dev, first_pending, doorbell_mask, "first delivery");
    selftest_assert_eq!(dev, second_pending, doorbell_mask, "second delivery");
    selftest_assert!(
        dev,
        !residual.contains(doorbell_mask),
        "vector {} still pending, leaf[{}] is {:#x}",
        DOORBELL_VECTOR.into_raw(),
        doorbell.get(),
        residual.into_raw()
    );

    dev_info!(
        dev,
        "interrupt self-test: passed, subtree {}, {} deliveries\n",
        DOORBELL_SUBTREE.index(),
        count,
    );

    Ok(())
}
