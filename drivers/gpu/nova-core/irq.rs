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

/// MSI-X table index that receives the GSP interrupt subtree.
const GSP_MSIX_INDEX: u32 = 2;

/// Allocates the MSI-X vectors needed to reach the GSP interrupt subtree.
///
/// GSP vector 155 belongs to GIN subtree 2, which is delivered through MSI-X
/// table entry 2. Allocate entries 0 through 2 and return entry 2 for the GSP
/// handler.
pub(crate) fn alloc_vector(pdev: &pci::Device<Bound>) -> Result<pci::IrqVector<'_>> {
    let vector_count = GSP_MSIX_INDEX + 1;
    let msix_only = IrqTypes::default().with(IrqType::MsiX);
    let irq_vectors = pdev.alloc_irq_vectors(vector_count, vector_count, msix_only)?;

    Ok(*irq_vectors.end())
}
