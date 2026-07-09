// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GSP event (SWGEN0) interrupt handling.
//!
//! The GSP firmware raises SWGEN0 when it has posted messages for the host in the GSP-to-CPU
//! queue. That signal reaches the host as an ordinary MSI through the GIN tree. This module
//! provides the threaded IRQ handler for it: the hard half services the GIN leaf and the falcon
//! SWGEN0 latch, and the threaded half drains the message queue.
//!
//! See `Documentation/gpu/nova/core/interrupts.rst`.

use kernel::{
    device, irq, pci,
    prelude::*,
    sync::{
        aref::ARef,
        Arc, //
    },
};

use super::interrupt_tree::{
    LeafIndex,
    Tree, //
};
use crate::{
    driver::Bar0,
    falcon::gsp::Gsp as GspFalcon,
    gpu::Chipset,
    gsp::cmdq::Cmdq, //
};

/// Fixed GSP notification vector.
///
/// The resource manager pins the GSP SWGEN0 notification to this vector on every supported chip,
/// so nova-core uses the constant directly instead of discovering it at runtime. The leaf and bit
/// serviced by the handler are derived from it.
pub(crate) const GSP_INTR_0_VECTOR: u32 = 155;

/// Leaf and bit index of the GSP notification vector within the interrupt tree.
const GSP_LOC: (usize, u32) = super::interrupt_tree::vector_leaf_bit(GSP_INTR_0_VECTOR);

/// Leaf holding the GSP notification vector.
const GSP_LEAF: usize = GSP_LOC.0;

/// Bit mask of the GSP notification vector within its leaf.
const GSP_BIT: u32 = 1 << GSP_LOC.1;

/// Enables the GSP notification interrupt.
///
/// Clears any stale GIN state left over from boot, unmasks the GSP vector at its leaf, and leaves
/// the tree armed. Call once, after the handler has been registered.
pub(crate) fn enable(bar: Bar0<'_>, chipset: Chipset) {
    let tree = Tree::new(chipset);
    // This runs after the handler is registered, but the GSP vector is still masked (the `allow`
    // below is the last step) and nova-core has unmasked no other vector, so no interrupt can be
    // delivered yet. The whole-tree drain therefore cannot run concurrently with the handler.
    tree.drain(bar);
    // Unmask the GSP vector. The subtree stays armed from here on, so the handler acknowledges
    // only its own leaf bit and never unarms and rearms the tree.
    tree.leaf(LeafIndex::new::<GSP_LEAF>()).allow(bar, GSP_BIT);
}

/// Threaded IRQ handler for the GSP SWGEN0 event.
///
/// The hard half acknowledges the GIN leaf and reads the falcon SWGEN0 latch; the threaded half
/// drains the GSP-to-CPU message queue under the command-queue lock.
#[pin_data]
pub(crate) struct GspInterrupt<'a> {
    /// Borrowed BAR0, for GIN and falcon register access from interrupt context.
    bar: Bar0<'a>,
    /// The GSP command queue, drained by the threaded half.
    cmdq: Arc<Cmdq>,
    /// The GIN interrupt tree for this chipset.
    tree: Tree,
    /// Device, for logging from interrupt context without taking the command-queue lock.
    dev: ARef<device::Device>,
}

impl<'a> GspInterrupt<'a> {
    /// Creates the handler for `chipset`, borrowing `bar` and sharing `cmdq` with the rest of the
    /// driver.
    pub(crate) fn new(
        bar: Bar0<'a>,
        cmdq: Arc<Cmdq>,
        chipset: Chipset,
        dev: ARef<device::Device>,
    ) -> impl PinInit<Self, Error> + 'a {
        try_pin_init!(Self {
            bar,
            cmdq,
            tree: Tree::new(chipset),
            dev,
        }? Error)
    }
}

impl irq::ThreadedHandler for GspInterrupt<'_> {
    /// Hard IRQ half: acknowledges the GIN leaf and reads the falcon SWGEN0 latch.
    fn handle(&self) -> irq::ThreadedIrqReturn {
        let bar = self.bar;

        // Only service our own vector: require the GSP bit in the leaf and acknowledge just that
        // bit, so a co-pending vector in the same leaf stays pending for its owner. The subtree
        // stays armed (this is a notification), so there is no unarm and rearm.
        let leaf = self
            .tree
            .leaf(LeafIndex::new::<GSP_LEAF>())
            .read_pending(bar);
        if leaf.mask() & GSP_BIT == 0 {
            return irq::ThreadedIrqReturn::None;
        }
        leaf.ack_bits(bar, GSP_BIT);

        // SWGEN0 is the message-queue notification; wake the threaded half to drain it.
        if GspFalcon::take_swgen0_intr(bar) {
            irq::ThreadedIrqReturn::WakeThread
        } else {
            // Our vector fired without SWGEN0, so a non-message falcon cause (for example a HALT
            // from a GSP crash, or a fatal error) reached the host. nova-core does not implement
            // GSP crash, ECC, or fatal-error recovery, so report it rather than dropping it
            // silently. The falcon is deliberately not retriggered: the threaded half drains the
            // whole queue, so no message is lost, and retriggering a cause that cannot be cleared
            // would storm.
            dev_err!(
                &self.dev,
                "GSP interrupt without SWGEN0: unhandled GSP fault (crash/ECC/fatal, out of scope)\n"
            );
            irq::ThreadedIrqReturn::Handled
        }
    }

    /// Threaded half: drains and dispatches the GSP-to-CPU message queue.
    fn handle_threaded(&self) -> irq::IrqReturn {
        if let Err(e) = self.cmdq.drain(self.bar) {
            dev_err!(
                &self.dev,
                "GSP event drain failed ({:?}); the message queue is no longer serviced\n",
                e
            );
        }
        irq::IrqReturn::Handled
    }
}

/// The registered GSP event interrupt.
///
/// Wraps the threaded IRQ registration so that teardown disables the GSP source at the interrupt
/// tree before `free_irq` runs. This closes the window, including a probe partial-unwind, in which
/// an interrupt could be delivered to a handler that is being freed.
#[pin_data(PinnedDrop)]
pub(crate) struct GspIrq<'a> {
    #[pin]
    reg: irq::ThreadedRegistration<'a, GspInterrupt<'a>>,
    /// Borrowed BAR0 and the interrupt tree, used by the teardown to block the GSP source.
    bar: Bar0<'a>,
    tree: Tree,
}

impl<'a> GspIrq<'a> {
    /// Registers the GSP SWGEN0 threaded handler on `vector`.
    ///
    /// # Safety
    ///
    /// The caller must not leak the returned value: its [`Drop`] runs `free_irq`.
    pub(crate) unsafe fn new(
        pdev: &'a pci::Device<device::Bound>,
        vector: pci::IrqVector<'a>,
        bar: Bar0<'a>,
        cmdq: Arc<Cmdq>,
        chipset: Chipset,
    ) -> impl PinInit<Self, Error> + 'a {
        let dev: ARef<device::Device> = pdev.as_ref().into();
        try_pin_init!(Self {
            // SAFETY: the caller guarantees the returned `GspIrq` is not leaked, so this
            // registration's `Drop` (`free_irq`) always runs.
            reg <- unsafe {
                pdev.request_threaded_irq(
                    vector,
                    irq::Flags::TRIGGER_NONE,
                    c"nova-core",
                    GspInterrupt::new(bar, cmdq, chipset, dev),
                )
            },
            bar,
            tree: Tree::new(chipset),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for GspIrq<'_> {
    fn drop(self: Pin<&mut Self>) {
        // Disable the GSP source before `reg` drops and runs `free_irq`, so no interrupt reaches a
        // handler being torn down. This `PinnedDrop` runs before any field drops, so the order is
        // block-then-free_irq.
        let this = self.project();
        this.tree
            .leaf(LeafIndex::new::<GSP_LEAF>())
            .block(this.bar, GSP_BIT);
    }
}
