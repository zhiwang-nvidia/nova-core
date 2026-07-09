// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Per-architecture properties of the GIN PF CPU interrupt tree.

mod gh100;
mod tu102;

use kernel::{
    io::Io,
    pci::IrqType,
    prelude::*, //
};

use crate::{
    driver::Bar0,
    gpu::{
        Architecture,
        Chipset, //
    },
    regs, //
};

/// Register write that restores PCI interrupt delivery to the CPU.
///
/// A message-signaled interrupt is delivered on an edge, and the edge that delivered one
/// interrupt does not deliver the next. Delivery resumes only once the method named here has been
/// performed, so a handler that skips it receives no further interrupts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PciIrqRearmMethod {
    /// The MSI end-of-interrupt register in the BAR0 PCI configuration-space mirror, used by
    /// MSI on pre-Hopper GPUs.
    ConfigMirrorEoi,

    /// A clear followed by a set of the `TOP` enable bits of every armed subtree, which produces
    /// the enable edge that delivers the next interrupt.
    ///
    /// MSI has a single message that every subtree raises, so one handler serves the whole tree
    /// and its rearm covers every subtree that is armed.
    TopEnableCycleArmed,

    /// The same enable cycle, restricted to the one subtree the handler serves.
    ///
    /// MSI-X raises a separate table entry per subtree, and each armed subtree has its own entry
    /// and its own handler, so a rearm covers the caller's subtree alone.
    TopEnableCycleSubtree,
}

impl PciIrqRearmMethod {
    /// Performs this method's register write.
    ///
    /// `subtrees` is the set of `TOP` bits to cycle, which the caller matches to this method:
    /// every armed subtree for [`Self::TopEnableCycleArmed`], and the bit of the subtree the
    /// caller serves for [`Self::TopEnableCycleSubtree`]. [`Self::ConfigMirrorEoi`] ignores it.
    pub(super) fn rearm(self, bar: Bar0<'_>, subtrees: u32) {
        match self {
            // The written value is ignored, so any write rearms delivery.
            Self::ConfigMirrorEoi => bar.write(regs::tu102::NV_XVE_CYA_2, 0u32.into()),
            Self::TopEnableCycleArmed | Self::TopEnableCycleSubtree => {
                bar.write(
                    regs::NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_CLEAR,
                    subtrees.into(),
                );
                bar.write(
                    regs::NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_TOP_EN_SET,
                    subtrees.into(),
                );
            }
        }
    }
}

/// Per-architecture properties of the GIN PF CPU interrupt tree.
///
/// The tree walk, vector encoding, and acknowledge sequence are identical
/// across GPU architectures and live in generic code. The size of the tree and
/// the method that rearms PCI interrupt delivery differ by family, and are
/// provided here. See `Documentation/gpu/nova/core/interrupts.rst`.
pub(super) trait PfCpuInterruptHal {
    /// Returns the number of implemented interrupt leaves in the PF CPU tree.
    ///
    /// Each leaf is a 32-bit register, so the tree exposes `num_leaves * 32`
    /// vectors. Pre-Hopper parts have 8 leaves, and Hopper and later implement
    /// 16 (of which 12 are currently used).
    fn num_leaves(&self) -> usize;

    /// Returns the mask of subtree bits this architecture implements.
    ///
    /// Each `TOP` bit covers two adjacent leaves, so the tree has
    /// `num_leaves / 2` subtrees, and the returned mask has one bit set for
    /// each. It bounds the bits that are meaningful in `TOP_EN_SET` and
    /// `TOP_EN_CLEAR`.
    fn subtree_mask(&self) -> u32 {
        (1u32 << (self.num_leaves() / 2)) - 1
    }

    /// Returns the method that rearms PCI interrupt delivery for `irq_type`.
    ///
    /// `None` means that `irq_type` needs no rearm write. That is the case for
    /// `INTx`, which is level-triggered, and which nova-core does not allocate.
    fn pci_irq_rearm_method(&self, irq_type: IrqType) -> Option<PciIrqRearmMethod>;
}

/// Returns the [`PfCpuInterruptHal`] for `chipset`.
pub(super) fn pf_cpu_interrupt_hal(chipset: Chipset) -> &'static dyn PfCpuInterruptHal {
    match chipset.arch() {
        Architecture::Turing | Architecture::Ampere | Architecture::Ada => tu102::TU102_HAL,
        Architecture::Hopper | Architecture::BlackwellGB10x | Architecture::BlackwellGB20x => {
            gh100::GH100_HAL
        }
    }
}

#[kunit_tests(nova_core_gin_hal)]
mod tests {
    use super::*;

    use crate::gpu::Chipset;

    /// Pre-Hopper parts have an 8-leaf tree (4 subtrees, mask `0x0f`).
    #[test]
    fn pre_hopper_tree_size() {
        for chipset in [Chipset::TU102, Chipset::GA102, Chipset::AD102] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(hal.num_leaves(), 8);
            assert_eq!(hal.subtree_mask(), 0x0f);
        }
    }

    /// Hopper and later implement a 16-leaf tree (8 subtrees, mask `0xff`).
    #[test]
    fn hopper_plus_tree_size() {
        for chipset in [Chipset::GH100, Chipset::GB100, Chipset::GB202] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(hal.num_leaves(), 16);
            assert_eq!(hal.subtree_mask(), 0xff);
        }
    }

    /// The subtree mask always has exactly `num_leaves / 2` bits set, one per subtree.
    #[test]
    fn subtree_mask_matches_leaf_count() {
        for chipset in [
            Chipset::TU102,
            Chipset::GA102,
            Chipset::AD102,
            Chipset::GH100,
            Chipset::GB100,
            Chipset::GB202,
        ] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(
                hal.subtree_mask().count_ones() as usize,
                hal.num_leaves() / 2
            );
        }
    }

    /// Only pre-Hopper MSI rearms through the configuration-space mirror. MSI on Hopper and later
    /// cycles the `TOP` enables of every armed subtree.
    #[test]
    fn pci_irq_rearm_method_per_arch_and_type() {
        for chipset in [Chipset::TU102, Chipset::GA102, Chipset::AD102] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(
                hal.pci_irq_rearm_method(IrqType::Msi),
                Some(PciIrqRearmMethod::ConfigMirrorEoi)
            );
        }

        for chipset in [Chipset::GH100, Chipset::GB100, Chipset::GB202] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(
                hal.pci_irq_rearm_method(IrqType::Msi),
                Some(PciIrqRearmMethod::TopEnableCycleArmed)
            );
        }
    }

    /// MSI-X gives each subtree its own table entry, so on every architecture its rearm cycles
    /// only the subtree the handler serves.
    #[test]
    fn msix_rearms_one_subtree_on_every_arch() {
        for chipset in [
            Chipset::TU102,
            Chipset::GA102,
            Chipset::AD102,
            Chipset::GH100,
            Chipset::GB100,
            Chipset::GB202,
        ] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(
                hal.pci_irq_rearm_method(IrqType::MsiX),
                Some(PciIrqRearmMethod::TopEnableCycleSubtree)
            );
        }
    }

    /// `INTx` is level-triggered and needs no rearm write on any architecture.
    #[test]
    fn intx_needs_no_rearm() {
        for chipset in [
            Chipset::TU102,
            Chipset::GA102,
            Chipset::AD102,
            Chipset::GH100,
            Chipset::GB100,
            Chipset::GB202,
        ] {
            let hal = pf_cpu_interrupt_hal(chipset);
            assert_eq!(hal.pci_irq_rearm_method(IrqType::Intx), None);
        }
    }
}
