// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GPU interrupt support: the GIN controller (GPU Interrupt and Notification
//! unit).
//!
//! See `Documentation/gpu/nova/core/interrupts.rst`.

#[cfg(CONFIG_NOVA_CORE_IRQ_SELFTEST)]
pub(crate) mod doorbell_test;
pub(crate) mod gsp;
mod hal;
mod interrupt_tree;

use kernel::{
    device::Bound,
    pci::{
        self,
        IrqType,
        IrqTypes, //
    },
    prelude::*,
};

/// Allocates the interrupt vectors that the subtrees in `armed_mask` require, and returns the
/// vector that delivers the highest of them.
///
/// Every subtree armed at `TOP` must have an allocated vector with a registered handler, or the
/// interrupts it raises are lost. How many vectors that takes depends on the type Linux grants.
/// MSI has a single message that every subtree raises, so one vector serves the whole tree. MSI-X
/// raises a separate table entry per subtree, and Linux masks every entry a driver did not
/// allocate, so the allocation has to reach the highest armed subtree. Entries below it that
/// nova-core does not service cost nothing, because Linux unmasks an entry only when its interrupt
/// is requested.
///
/// MSI-X is preferred and requested first, for the exact count the armed mask needs. A part whose
/// MSI-X table is smaller than that fails the minimum, and the MSI request that follows serves the
/// whole tree from one vector. nova-core requires one of the two and does not fall back to a
/// shared INTx line.
///
/// The PCI abstraction exposes only the first and last vector of an allocation, so the handler the
/// caller registers on the returned vector must be the one serving the highest armed subtree.
///
/// # Errors
///
/// `EINVAL` if `armed_mask` is empty. The error from the MSI request if neither type can be
/// allocated.
pub(crate) fn alloc_vectors(
    pdev: &pci::Device<Bound>,
    armed_mask: u32,
) -> Result<pci::IrqVector<'_>> {
    // One entry per subtree up to and including the highest armed one.
    let msix_count = u32::BITS - armed_mask.leading_zeros();
    if msix_count == 0 {
        return Err(EINVAL);
    }

    let msix = IrqTypes::default().with(IrqType::MsiX);
    if let Ok(vectors) = pdev.alloc_irq_vectors(msix_count, msix_count, msix) {
        // The last entry is the one the highest armed subtree raises.
        return Ok(*vectors.end());
    }

    let msi = IrqTypes::default().with(IrqType::Msi);
    let vectors = pdev.alloc_irq_vectors(1, 1, msi)?;

    Ok(*vectors.start())
}
