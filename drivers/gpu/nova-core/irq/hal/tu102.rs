// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::pci::IrqType;

use super::{
    PciIrqRearmMethod,
    PfCpuInterruptHal, //
};

/// GIN parameters for Turing, Ampere, and Ada, which have an 8-leaf PF CPU tree.
struct Tu102;

impl PfCpuInterruptHal for Tu102 {
    fn num_leaves(&self) -> usize {
        8
    }

    fn pci_irq_rearm_method(&self, irq_type: IrqType) -> Option<PciIrqRearmMethod> {
        match irq_type {
            IrqType::Intx => None,
            IrqType::Msi => Some(PciIrqRearmMethod::ConfigMirrorEoi),
            IrqType::MsiX => Some(PciIrqRearmMethod::TopEnableCycleSubtree),
        }
    }
}

const TU102: Tu102 = Tu102;
pub(super) const TU102_HAL: &dyn PfCpuInterruptHal = &TU102;
