// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GPU interrupt support: the GIN controller (GPU Interrupt and Notification
//! unit).
//!
//! See `Documentation/gpu/nova/core/interrupts.rst`.

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

/// Allocates a single MSI or MSI-X interrupt vector for `pdev`.
///
/// The GIN interrupt tree delivers each source as its own message-signaled
/// vector, so nova-core requires MSI or MSI-X and does not fall back to a
/// shared INTx line. Allocation fails if neither is available.
pub(crate) fn alloc_vector(pdev: &pci::Device<Bound>) -> Result<pci::IrqVector<'_>> {
    let msi_types = IrqTypes::default().with(IrqType::Msi).with(IrqType::MsiX);
    let irq_vectors = pdev.alloc_irq_vectors(1, 1, msi_types)?;

    Ok(*irq_vectors.start())
}
