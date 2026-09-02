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
    fn pfalcon(io: Bar0<'_>) -> Mmio<'_, super::PFalconRegisters> {
        io_project!(io, build: PFALCON)
    }

    #[inline]
    fn pfalcon2(io: Bar0<'_>) -> Mmio<'_, super::PFalcon2Registers> {
        io_project!(io, build: PFALCON2)
    }
}

impl Gsp {
    /// Clears the GSP falcon SWGEN0 interrupt latch.
    ///
    /// The GSP signals the interrupt tree no further while the latch stays set, so a caller that
    /// consumed a notification without the interrupt handler must clear it.
    pub(crate) fn clear_swgen0_intr(bar: Bar0<'_>) {
        Self::pfalcon(bar).write_reg(regs::NV_PFALCON_FALCON_IRQSCLR::zeroed().with_swgen0(true));
    }

    /// Reads the GSP falcon interrupt causes pending for the host, without clearing any latch.
    ///
    /// Causes the falcon routes to its own RISC-V core belong to the firmware and are excluded,
    /// so every cause here other than SWGEN0 is a GSP fault.
    pub(crate) fn read_host_intr(
        bar: Bar0<'_>,
        chipset: Chipset,
    ) -> regs::NV_PFALCON_FALCON_IRQSTAT {
        let latched = Self::pfalcon(bar).read(regs::NV_PFALCON_FALCON_IRQSTAT);

        hal::falcon_intr_hal(chipset)
            .riscv_routing()
            .host_routed_causes(Self::pfalcon2(bar), latched)
    }

    /// Reads the GSP falcon interrupt causes pending for the host, clearing the SWGEN0 latch if
    /// it was set.
    ///
    /// Returns the causes as they were read, before the clear, and changes no other latch.
    /// [`Self::read_host_intr`] is the same read without the clear.
    pub(crate) fn take_host_intr(
        bar: Bar0<'_>,
        chipset: Chipset,
    ) -> regs::NV_PFALCON_FALCON_IRQSTAT {
        let status = Self::read_host_intr(bar, chipset);

        if status.swgen0() {
            Self::clear_swgen0_intr(bar);
        }

        status
    }

    /// Clears the latch of every interrupt cause set in `status`.
    ///
    /// A cause driven from outside the falcon stays set. [`Self::read_host_intr`] reports which
    /// causes are still set after the write.
    pub(crate) fn clear_intr(bar: Bar0<'_>, status: regs::NV_PFALCON_FALCON_IRQSTAT) {
        Self::pfalcon(bar).write_reg(regs::NV_PFALCON_FALCON_IRQSCLR::from(status.into_raw()));
    }

    /// Re-emits the falcon's host-routed interrupt causes into the interrupt tree.
    ///
    /// The caller must have cleared every host cause first. A cause still set is re-emitted at
    /// once, and its vector arrives again as soon as the caller rearms delivery.
    ///
    /// Does nothing on Turing, whose falcons do not implement the register.
    pub(crate) fn retrigger_intr(bar: Bar0<'_>, chipset: Chipset) {
        if !hal::falcon_intr_hal(chipset).has_intr_retrigger() {
            return;
        }

        Self::pfalcon(bar).write(
            Array::at(0),
            regs::NV_PFALCON_FALCON_INTR_RETRIGGER::zeroed().with_trigger(true),
        );
    }
}

impl<'a> Falcon<'a, Gsp> {
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
