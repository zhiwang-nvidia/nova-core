// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GSP event (SWGEN0) interrupt handling.
//!
//! The GSP firmware raises SWGEN0 when it has posted messages in the GSP-to-CPU queue. That
//! signal reaches the CPU as a PCI interrupt through the GIN tree. This module provides the
//! threaded IRQ handler for it: the hard half services the GIN leaf and the falcon SWGEN0
//! latch, and the threaded half drains the message queue.
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

/// Subtree carrying the GSP notification vector, and the only subtree nova-core arms.
///
/// This is the mask probe allocates PCI vectors for, and the mask every handler on this tree arms
/// and rearms.
pub(crate) const GSP_SUBTREE: u32 = super::interrupt_tree::subtree_bit(GSP_INTR_0_VECTOR);

/// Enables the GSP notification interrupt.
///
/// Clears the interrupt state left over from GSP boot, first every leaf's enables, then the
/// falcon's SWGEN0 latch, then the tree, and unmasks the GSP vector, leaving the owned subtree
/// armed. Call once, after the handler has been registered and before draining the messages the
/// GSP posted during boot.
pub(crate) fn enable(bar: Bar0<'_>, chipset: Chipset, irq_type: pci::IrqType) {
    let tree = Tree::new(chipset, irq_type, GSP_SUBTREE);
    // Boot, or a previous driver, can leave leaf enables set for vectors nova-core does not
    // service, and an enabled vector in an armed subtree reaches nova-core's handler. Mask every
    // implemented leaf before enabling the one vector this driver owns.
    tree.block_all_leaves(bar);
    // GSP boot consumes its notifications by polling the queue, which leaves SWGEN0 latched. The
    // GSP drives no new edge while the latch is set, so clearing it is what makes the first
    // interrupt possible. Messages posted before this point produce no interrupt and are covered
    // by the caller's drain. Clear it first, so the drain below acknowledges whatever tree state
    // the clear leaves behind.
    GspFalcon::clear_swgen0_intr(bar);
    // Clear the stale tree state left by boot. This runs after the handler is registered, but the
    // clear above leaves every leaf masked and the `allow` below is the last step, so no interrupt
    // can be delivered yet and the whole-tree drain cannot run concurrently with the handler.
    tree.drain(bar);
    // The drain acknowledged the tree while GSP causes may still be latched at the falcon, which
    // leaves the falcon with no transition to signal. Re-emit so a cause that survived the drain
    // still reaches the CPU once the vector is unmasked below.
    GspFalcon::retrigger_intr(bar, chipset);
    // Unmask the GSP vector. The subtree stays armed from here on, so the handler acknowledges
    // only its own leaf bit rather than walking the whole tree.
    //
    // A message posted between the drain and this point sets both the tree bit and the latch, so
    // the first interrupt is a real one and the handler drains normally.
    tree.leaf(LeafIndex::new::<GSP_LEAF>()).allow(bar, GSP_BIT);
}

/// Threaded IRQ handler for the GSP SWGEN0 event.
///
/// The hard half acknowledges the GIN leaf and reads the falcon SWGEN0 latch. The threaded half
/// drains the GSP-to-CPU message queue under the command-queue lock.
#[pin_data]
pub(crate) struct GspInterrupt<'a> {
    /// Borrowed BAR0, for GIN and falcon register access from interrupt context.
    bar: Bar0<'a>,
    /// The GSP command queue, drained by the threaded half.
    cmdq: Arc<Cmdq>,
    /// The GIN interrupt tree for this chipset.
    tree: Tree,
    /// Chipset, for the falcon retrigger, which Turing does not implement.
    chipset: Chipset,
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
        irq_type: pci::IrqType,
        dev: ARef<device::Device>,
    ) -> impl PinInit<Self, Error> + 'a {
        try_pin_init!(Self {
            bar,
            cmdq,
            tree: Tree::new(chipset, irq_type, GSP_SUBTREE),
            chipset,
            dev,
        }? Error)
    }
}

impl irq::ThreadedHandler for GspInterrupt<'_> {
    /// Hard IRQ half: acknowledges the GIN leaf, takes the falcon SWGEN0 latch, and rearms PCI
    /// interrupt delivery.
    fn handle(&self) -> irq::ThreadedIrqReturn {
        let bar = self.bar;

        // Only service our own vector: require the GSP bit in the leaf and acknowledge just that
        // bit, so a co-pending vector in the same leaf stays pending for its owner. The subtree
        // stays armed, so there is no whole-tree unarm and rearm.
        let leaf = self
            .tree
            .leaf(LeafIndex::new::<GSP_LEAF>())
            .read_pending(bar);
        if leaf.mask() & GSP_BIT == 0 {
            // Nothing to service, but nova-core owns the whole PCI interrupt, so skipping the
            // rearm here would silence every later interrupt as well.
            self.tree.rearm_pci_irq(bar);
            return irq::ThreadedIrqReturn::None;
        }
        leaf.ack_bits(bar, GSP_BIT);

        // SWGEN0 is the message-queue notification, so wake the threaded half to drain it.
        let status = GspFalcon::take_swgen0_intr(bar);
        let ret = if status.swgen0() {
            irq::ThreadedIrqReturn::WakeThread
        } else {
            // The tree routes every falcon cause to this vector, so something other than a posted
            // message fired it, for example a HALT from a GSP crash. There is no recovery path for
            // those causes, so report the status rather than discarding it, then take the cause
            // out of the falcon's enabled set. The retrigger below re-emits whatever remains
            // enabled, and a cause nothing clears would keep re-emitting.
            dev_err!(
                &self.dev,
                "GSP interrupt with no SWGEN0, falcon IRQSTAT {:#x}\n",
                status.into_raw()
            );
            GspFalcon::mask_and_clear_intr(bar, status);
            irq::ThreadedIrqReturn::Handled
        };

        // The leaf acknowledge above consumed the tree's record of this interrupt, and the falcon
        // signals the tree only on a transition of its enabled causes, so a cause that arrived
        // while this handler ran would never reach the CPU. Re-emit to supply that transition.
        GspFalcon::retrigger_intr(bar, self.chipset);

        // Delivery resumes only after this, so it must happen on every path that services the
        // vector, including the fault path above.
        self.tree.rearm_pci_irq(bar);

        ret
    }

    /// Threaded half: drains and dispatches the GSP-to-CPU message queue.
    fn handle_threaded(&self) -> irq::IrqReturn {
        if let Err(e) = self.cmdq.drain(self.bar) {
            // A queue that fails to drain cannot advance past the message that failed, so every
            // later notification would repeat this failure. Mask the source instead.
            self.tree
                .leaf(LeafIndex::new::<GSP_LEAF>())
                .block(self.bar, GSP_BIT);
            dev_err!(
                &self.dev,
                "GSP event drain failed ({:?}), the message queue is no longer serviced\n",
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
        let irq_type = vector.irq_type();
        try_pin_init!(Self {
            // SAFETY: the caller guarantees the returned `GspIrq` is not leaked, so this
            // registration's `Drop` (`free_irq`) always runs.
            reg <- unsafe {
                pdev.request_threaded_irq(
                    vector,
                    irq::Flags::TRIGGER_NONE,
                    c"nova-core",
                    GspInterrupt::new(bar, cmdq, chipset, irq_type, dev),
                )
            },
            bar,
            tree: Tree::new(chipset, irq_type, GSP_SUBTREE),
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
