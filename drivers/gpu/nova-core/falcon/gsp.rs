// SPDX-License-Identifier: GPL-2.0

use kernel::{
    io::{
        io_project,
        poll::read_poll_timeout,
        register,
        register::Array,
        Io,
        Mmio, //
    },
    prelude::*,
    time::Delta, //
};

use crate::{
    driver::{
        Bar0,
        NovaRegisters, //
    },
    falcon::{
        hal,
        Falcon,
        FalconEngine, //
    },
    gpu::Chipset,
    regs,
};

/// Type specifying the `Gsp` falcon engine. Cannot be instantiated.
pub(crate) struct Gsp(());

register! {
    base: NovaRegisters;

    PFALCON: super::PFalconRegisters @ 0x00110000;
    PFALCON2: super::PFalcon2Registers @ 0x00111000;
}

impl FalconEngine for Gsp {
    #[inline]
    fn pfalcon<'a>(io: Bar0<'a>) -> Mmio<'a, super::PFalconRegisters> {
        io_project!(io, build: PFALCON)
    }

    #[inline]
    fn pfalcon2<'a>(io: Bar0<'a>) -> Mmio<'a, super::PFalcon2Registers> {
        io_project!(io, build: PFALCON2)
    }
}

impl Gsp {
    /// Clears the GSP falcon SWGEN0 interrupt latch.
    ///
    /// The latch holds until it is cleared, and the GSP drives no new edge into the interrupt
    /// tree while it is set, so a caller that consumed a notification by any means other than the
    /// interrupt handler must clear it or no further notification is delivered.
    pub(crate) fn clear_swgen0_intr(bar: Bar0<'_>) {
        Self::pfalcon(bar).write_reg(regs::NV_PFALCON_FALCON_IRQSCLR::zeroed().with_swgen0(true));
    }

    /// Reads the GSP falcon interrupt status, clearing the SWGEN0 latch if it was set.
    ///
    /// Returns the status as it was read, before the clear. The GSP raises SWGEN0 when it has
    /// posted messages in the GSP-to-CPU queue. The interrupt tree routes every falcon cause to
    /// a single vector, so the rest of the status identifies a cause other than a posted message.
    pub(crate) fn take_swgen0_intr(bar: Bar0<'_>) -> regs::NV_PFALCON_FALCON_IRQSTAT {
        let status = Self::pfalcon(bar).read(regs::NV_PFALCON_FALCON_IRQSTAT);

        if status.swgen0() {
            Self::clear_swgen0_intr(bar);
        }

        status
    }

    /// Masks and clears every interrupt cause set in `status`.
    ///
    /// A masked cause leaves the falcon's enabled set, so it neither raises the tree again nor
    /// holds that set non-empty.
    pub(crate) fn mask_and_clear_intr(bar: Bar0<'_>, status: regs::NV_PFALCON_FALCON_IRQSTAT) {
        let causes = status.into_raw();
        let pfalcon = Self::pfalcon(bar);

        pfalcon.write_reg(regs::NV_PFALCON_FALCON_IRQMCLR::zeroed().with_value(causes));
        pfalcon.write_reg(regs::NV_PFALCON_FALCON_IRQSCLR::from(causes));
    }

    /// Re-emits the falcon's enabled interrupt causes into the interrupt tree.
    ///
    /// The falcon signals the tree on a transition of its enabled causes, so clearing the tree
    /// leaf while a cause is still latched leaves no transition and no further vector.
    ///
    /// Does nothing on Turing, whose falcons do not implement the register.
    pub(crate) fn retrigger_intr(bar: Bar0<'_>, chipset: Chipset) {
        if !hal::has_intr_retrigger(chipset) {
            return;
        }

        Self::pfalcon(bar).write(
            regs::NV_PFALCON_FALCON_INTR_RETRIGGER::at(0),
            regs::NV_PFALCON_FALCON_INTR_RETRIGGER::zeroed().with_trigger(true),
        );
    }
}

impl<'a> Falcon<'a, Gsp> {
    /// Clears the SWGEN0 bit in the Falcon's IRQ status clear register to
    /// allow GSP to signal CPU for processing new messages in message queue.
    pub(crate) fn clear_swgen0_intr(&self) {
        Gsp::clear_swgen0_intr(self.bar);
    }

    /// Checks if GSP reload/resume has completed during the boot process.
    pub(crate) fn check_reload_completed(&self, timeout: Delta) -> Result<bool> {
        read_poll_timeout(
            || Ok(self.bar.read(regs::NV_PGC6_BSI_SECURE_SCRATCH_14)),
            |val| val.boot_stage_3_handoff(),
            Delta::ZERO,
            timeout,
        )
        .map(|_| true)
    }

    /// Returns whether the RISC-V branch privilege lockdown bit is set.
    pub(crate) fn riscv_branch_privilege_lockdown(&self) -> bool {
        self.pfalcon
            .read(regs::NV_PFALCON_FALCON_HWCFG2)
            .riscv_br_priv_lockdown()
    }

    /// Returns whether GSP registers can be read by the CPU.
    pub(crate) fn priv_target_mask_released(&self) -> bool {
        /// Pattern returned by GSP register reads while the PRIV target mask still blocks CPU
        /// access. The low byte varies; the upper 24 bits are fixed.
        const LOCKED_PATTERN: u32 = 0xbadf_4100;
        const LOCKED_MASK: u32 = 0xffff_ff00;

        let hwcfg2 = self.pfalcon.read(regs::NV_PFALCON_FALCON_HWCFG2).into_raw();

        hwcfg2 != 0 && (hwcfg2 & LOCKED_MASK) != LOCKED_PATTERN
    }
}
