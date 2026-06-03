// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::{
    bits,
    device,
    dma::Coherent,
    io::{
        poll::read_poll_timeout,
        register::WithBase,
        Io, //
    },
    prelude::*,
    time::Delta,
    transmute::FromBytes,
    types::ScopeGuard, //
};

use crate::{
    driver::Bar0,
    falcon::{
        gsp::Gsp,
        sec2::Sec2,
        Falcon,
        FalconFbifMemType,
        FalconFbifTarget,
        FalconMem,
        FalconModSelAlgo, //
    },
    fb::FbLayout,
    firmware::{
        bindata::request_ucodes_firmware,
        gsp::GspFirmware,
        radix3::Radix3, //
    },
    gpu::TOTAL_CHANNELS,
    gsp::{
        cmdq::Cmdq,
        commands,
        fw::{
            BindataArgs,
            GspArgumentsPadded, //
        },
        GspFwWprMeta, //
    },
    regs,
    vgpu::VgpuManager, //
};

fn read_gsp_fbif_transcfg(bar: Bar0<'_>, ctx_dma: u8) -> Result<regs::NV_PFALCON_FBIF_TRANSCFG> {
    if ctx_dma >= 8 {
        return Err(EINVAL);
    }
    Ok(bar.read(regs::NV_PFALCON_FBIF_TRANSCFG::of::<Gsp>().at(usize::from(ctx_dma))))
}

fn write_gsp_fbif_transcfg(
    bar: Bar0<'_>,
    ctx_dma: u8,
    reg: regs::NV_PFALCON_FBIF_TRANSCFG,
) -> Result {
    if ctx_dma >= 8 {
        return Err(EINVAL);
    }
    bar.write(WithBase::of::<Gsp>().at(usize::from(ctx_dma)), reg);
    Ok(())
}

fn update_gsp_fbif_transcfg(
    bar: Bar0<'_>,
    ctx_dma: u8,
    f: impl FnOnce(regs::NV_PFALCON_FBIF_TRANSCFG) -> regs::NV_PFALCON_FBIF_TRANSCFG,
) -> Result {
    if ctx_dma >= 8 {
        return Err(EINVAL);
    }
    bar.update(
        regs::NV_PFALCON_FBIF_TRANSCFG::of::<Gsp>().at(usize::from(ctx_dma)),
        f,
    );
    Ok(())
}

/// GMC command id for the `LOAD_EXEC_GENERIC_BOOTLOADER` boot event.
///
/// Matches `GMCAPI_COMMANDS_GMCAPI_CMD_EXEC_GENERIC_BOOTLOADER` in the
/// r000 bindings. Category GSP_MGMT (0x01), index 0x0002.
const CMD_EXEC_GENERIC_BOOTLOADER: u32 = 0x0001_0002;

/// GMC command id for the `LOAD_EXEC_HS_BINARY` boot event.
///
/// Matches `GMCAPI_COMMANDS_GMCAPI_CMD_EXEC_HS_BINARY` in the r000 bindings.
/// Category GSP_MGMT (0x01), index 0x0003.
const CMD_EXEC_HS_BINARY: u32 = 0x0001_0003;

impl super::Gsp {
    /// Attempt to boot the GSP.
    ///
    /// This is a GPU-dependent and complex procedure that involves loading firmware files from
    /// user-space, patching them with signatures, and building firmware-specific intricate data
    /// structures that the GSP will use at runtime.
    ///
    /// Upon return, the GSP is up and running, and its unload bundle and static GPU information
    /// are returned.
    pub(crate) fn boot(
        self: Pin<&mut Self>,
        mut ctx: super::GspBootContext<'_, '_>,
        mut vgpu: Pin<&mut VgpuManager<'_>>,
    ) -> Result<super::BootResult> {
        let pdev = ctx.pdev;
        let bar = ctx.bar;
        let chipset = ctx.chipset;
        let gsp_falcon = ctx.gsp_falcon;
        let dev = pdev.as_ref();
        let hal = super::hal::gsp_hal(chipset);

        let gsp_fw = KBox::pin_init(GspFirmware::new(dev, chipset, &self.gsp_tlv), GFP_KERNEL)?;

        dev_info!(
            dev,
            "GSP firmware: {} (internal version: {})\n",
            gsp_fw.fw_path.to_str().unwrap_or("unknown"),
            gsp_fw.fw_version.to_str().unwrap_or("unknown")
        );

        // Load the optional ucodes firmware and map it through the radix3 page-table format
        // expected by GSP-RM. The mapping must stay alive until initialization has completed.
        let ucodes_radix3 = match request_ucodes_firmware(dev, chipset)? {
            Some(ucodes) => Some(KBox::pin_init(
                Radix3::new(dev, ucodes.as_slice()),
                GFP_KERNEL,
            )?),
            None => {
                dev_dbg!(
                    dev,
                    "ucodes firmware not found; bindata arguments remain empty\n"
                );
                None
            }
        };

        let bindata = if let Some(radix3) = ucodes_radix3.as_ref() {
            Some(BindataArgs {
                radix3: radix3.dma_handle(),
                size: radix3.size.try_into().map_err(|_| EOVERFLOW)?,
            })
        } else {
            None
        };
        GspArgumentsPadded::set_bindata(&self.rmargs, bindata.as_ref());

        let fb_layout = FbLayout::new(chipset, bar, &gsp_fw, vgpu.as_ref().state())?;
        dev_dbg!(dev, "{:#x?}\n", fb_layout);

        let wpr_meta = Coherent::init(dev, GFP_KERNEL, GspFwWprMeta::new(&gsp_fw, &fb_layout))?;

        // Perform the chipset-specific boot sequence, and retrieve the unload bundle.
        let unload_bundle = hal
            .boot(&self, &mut ctx, &fb_layout, &wpr_meta)?
            .or_else(|| {
                dev_warn!(dev, "The GSP won't be able to unload properly on unbind.\n");
                dev_warn!(
                    dev,
                    "The GPU will need to be reset before the driver can bind again.\n"
                );

                None
            });

        let mut unload_guard =
            ScopeGuard::new_with_data((ctx, unload_bundle), |(ctx, unload_bundle)| {
                let _ = self.unload(ctx, unload_bundle);
            });
        let ctx = &mut unload_guard.0;

        gsp_falcon.write_os_version(gsp_fw.bootloader.app_version);

        // Poll for RISC-V to become active before continuing.
        read_poll_timeout(
            || Ok(gsp_falcon.is_riscv_active()),
            |val: &bool| *val,
            Delta::from_millis(10),
            Delta::from_secs(5),
        )?;

        dev_dbg!(pdev, "RISC-V active? {}\n", gsp_falcon.is_riscv_active(),);

        // GSP-RM discards any RPC seen before GSP_INIT, so the system-info
        // and registry data ride inline in the GSP_INIT NVKV payload. The
        // synchronous GSP_INIT reply arrives only after GSP-RM is fully up,
        // and any LOAD_EXEC events GSP-RM raises in the meantime are
        // dispatched inline by the GMC boot-event handler.
        let init_payload = commands::build_gsp_init_payload(pdev, chipset, vgpu.as_ref().state())?;
        let bootloader_app_version = gsp_fw.bootloader.app_version;
        let libos_dma_handle = self.libos.dma_handle();
        let static_info = commands::gsp_init(&self.cmdq, bar, &init_payload, |id, payload| {
            Self::dispatch_gmc_boot_event(
                id,
                payload,
                gsp_falcon,
                ctx.sec2_falcon,
                bar,
                dev,
                bootloader_app_version,
                libos_dma_handle,
            )
        })?;

        vgpu.as_mut().init(
            &static_info.gmc_engine_masks,
            static_info.vmmu_segment_size,
            TOTAL_CHANNELS,
        );

        let (_, unload_bundle) = unload_guard.dismiss();

        Ok(super::BootResult::new(unload_bundle, static_info))
    }

    /// Dispatch a single GMC boot event to the matching load-and-execute handler.
    ///
    /// The r000 boot path delivers load-and-execute steps as GMC events keyed by
    /// command id. The payload after the GMC header is the same as in the prior
    /// VGPU-style framing, so the existing handlers are reused as-is.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_gmc_boot_event(
        command_id: u32,
        payload: &[u8],
        gsp_falcon: &Falcon<'_, Gsp>,
        sec2_falcon: &Falcon<'_, Sec2>,
        bar: Bar0<'_>,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        match command_id {
            CMD_EXEC_GENERIC_BOOTLOADER => Self::handle_load_exec_bootloader(
                payload,
                gsp_falcon,
                sec2_falcon,
                bar,
                dev,
                bootloader_app_version,
                libos_dma_handle,
            ),
            CMD_EXEC_HS_BINARY => Self::handle_load_exec_hs_binary(
                payload,
                gsp_falcon,
                sec2_falcon,
                bar,
                dev,
                bootloader_app_version,
                libos_dma_handle,
            ),
            _ => {
                dev_err!(
                    dev,
                    "Unexpected GMC boot event: command_id={:#010x}\n",
                    command_id
                );
                Err(EINVAL)
            }
        }
    }

    /// Handle a `GSP_LOAD_EXEC_GENERIC_BOOTLOADER` event.
    ///
    /// The GSP firmware sends this event with a bootloader descriptor and the
    /// aperture metadata needed to DMA the firmware from CPU-controlled memory
    /// into GSP IMEM and DMEM. Once the image has run to completion, the driver
    /// performs a core resume to restart GSP-RM.
    #[allow(clippy::too_many_arguments)]
    fn handle_load_exec_bootloader(
        payload: &[u8],
        gsp_falcon: &Falcon<'_, Gsp>,
        sec2_falcon: &Falcon<'_, Sec2>,
        bar: Bar0<'_>,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        let params = LoadExecGenericBootloaderParams::from_bytes_prefix(payload)
            .ok_or(EINVAL)?
            .0;
        let dmem_desc_size =
            u32::try_from(core::mem::size_of::<FalconBlDmemDesc>()).map_err(|_| EOVERFLOW)?;
        if params.dmem_desc_size != dmem_desc_size {
            dev_err!(
                dev,
                "Unexpected load-exec descriptor size: {} (expected {})\n",
                params.dmem_desc_size,
                dmem_desc_size
            );
            return Err(EINVAL);
        }

        let ctx_dma = params.ctx_dma().inspect_err(|_| {
            dev_err!(
                dev,
                "Unsupported load-exec DMA context: {}\n",
                params.dmem_desc.ctx_dma
            );
        })?;
        let fbif_target = params.fbif_target().inspect_err(|_| {
            dev_err!(
                dev,
                "Unsupported load-exec aperture: addr_space={}, cpu_cache_attrib={}\n",
                params.addr_space,
                params.cpu_cache_attrib
            );
        })?;
        let dmem_desc = params.dmem_desc;

        let code_dma_base =
            u64::from(dmem_desc.code_dma_base_lo) | (u64::from(dmem_desc.code_dma_base_hi) << 32);
        let data_dma_base =
            u64::from(dmem_desc.data_dma_base_lo) | (u64::from(dmem_desc.data_dma_base_hi) << 32);
        dev_dbg!(
            dev,
            "Load-exec bootloader: ctx_dma={}, addr_space={}, cpu_cache_attrib={}, code_dma_base={:#x}, data_dma_base={:#x}\n",
            ctx_dma,
            params.addr_space,
            params.cpu_cache_attrib,
            code_dma_base,
            data_dma_base
        );

        gsp_falcon.wait_for_processor_suspend().inspect_err(|_| {
            dev_err!(
                dev,
                "Timeout waiting for GSP suspend (mbox0={:#x})\n",
                gsp_falcon.read_mailbox0()
            );
        })?;

        gsp_falcon.reset()?;

        gsp_falcon.dma_reset();
        let saved_fbif_transcfg = read_gsp_fbif_transcfg(bar, ctx_dma)?;
        update_gsp_fbif_transcfg(bar, ctx_dma, |v| {
            v.with_target(fbif_target)
                .with_mem_type(FalconFbifMemType::Physical)
        })?;

        let load_result = (|| -> Result {
            if dmem_desc.non_secure_code_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    ctx_dma,
                    code_dma_base,
                    FalconMem::ImemNonSecure,
                    dmem_desc.non_secure_code_off,
                    dmem_desc.non_secure_code_off,
                    dmem_desc.non_secure_code_size,
                )?;
            }

            if dmem_desc.secure_code_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    ctx_dma,
                    code_dma_base,
                    FalconMem::ImemSecure,
                    dmem_desc.secure_code_off,
                    dmem_desc.secure_code_off,
                    dmem_desc.secure_code_size,
                )?;
            }

            if dmem_desc.data_size > 0 {
                gsp_falcon.raw_dma_transfer(
                    ctx_dma,
                    data_dma_base,
                    FalconMem::Dmem,
                    0,
                    0,
                    dmem_desc.data_size,
                )?;
            }

            bar.write(
                WithBase::of::<Gsp>(),
                regs::NV_PFALCON_FALCON_BOOTVEC::zeroed().with_value(dmem_desc.code_entry_point),
            );

            gsp_falcon.start()?;

            gsp_falcon.wait_till_halted().inspect_err(|_| {
                dev_err!(
                    dev,
                    "Timeout waiting for firmware to halt (mbox0={:#x})\n",
                    gsp_falcon.read_mailbox0()
                );
            })?;

            Ok(())
        })();
        write_gsp_fbif_transcfg(bar, ctx_dma, saved_fbif_transcfg)?;
        load_result?;

        // Core resume: restart GSP-RM via SEC2.
        gsp_falcon.reset()?;

        gsp_falcon.write_mailboxes(
            Some(libos_dma_handle as u32),
            Some((libos_dma_handle >> 32) as u32),
        );

        sec2_falcon.start()?;

        gsp_falcon
            .check_reload_completed(Delta::from_secs(2))
            .inspect_err(|_| {
                let mbox0 = sec2_falcon.read_mailbox0();
                dev_err!(
                    dev,
                    "Timeout waiting for SEC2 to resume GSP-RM (SEC2 mbox0={:#x})\n",
                    mbox0
                );
            })?;

        let sec2_mbox0 = sec2_falcon.read_mailbox0();
        if sec2_mbox0 != 0 {
            dev_err!(
                dev,
                "SEC2 reported error during core resume: {:#x}\n",
                sec2_mbox0
            );
            return Err(EIO);
        }

        gsp_falcon.write_os_version(bootloader_app_version);

        if !gsp_falcon.is_riscv_active() {
            dev_err!(dev, "GSP RISC-V not active after core resume\n");
            return Err(EIO);
        }

        Ok(())
    }

    /// Handle a `GSP_LOAD_EXEC_HS_BINARY` event.
    ///
    /// Similar to the generic bootloader handler, but loads a high-security (HS)
    /// binary from framebuffer memory. The HS binary requires PKC signature
    /// validation via BROM registers before execution.
    #[allow(clippy::too_many_arguments)]
    fn handle_load_exec_hs_binary(
        payload: &[u8],
        gsp_falcon: &Falcon<'_, Gsp>,
        sec2_falcon: &Falcon<'_, Sec2>,
        bar: Bar0<'_>,
        dev: &device::Device,
        bootloader_app_version: u32,
        libos_dma_handle: u64,
    ) -> Result {
        let params = HsBinaryParams::from_bytes_prefix(payload).ok_or(EINVAL)?.0;

        gsp_falcon.wait_for_processor_suspend().inspect_err(|_| {
            dev_err!(
                dev,
                "Timeout waiting for GSP suspend (mbox0={:#x})\n",
                gsp_falcon.read_mailbox0()
            );
        })?;

        gsp_falcon.reset()?;

        gsp_falcon.dma_reset();
        bar.update(regs::NV_PFALCON_FBIF_TRANSCFG::of::<Gsp>().at(0), |v| {
            v.with_target(FalconFbifTarget::LocalFb)
                .with_mem_type(FalconFbifMemType::Physical)
                .with_engine_id_flag(true)
        });

        if params.ucode_imem_size > 0 {
            gsp_falcon.raw_dma_transfer(
                0,
                params.imem_phys_addr,
                FalconMem::ImemSecure,
                params.ucode_imem_va,
                params.ucode_imem_pa,
                params.ucode_imem_size,
            )?;
        }

        if params.ucode_dmem_size > 0 {
            let dmem_mem_off = if params.ucode_dmem_va == 0xFFFFFFFF {
                0
            } else {
                params.ucode_dmem_va
            };
            gsp_falcon.raw_dma_transfer(
                0,
                params.dmem_phys_addr,
                FalconMem::Dmem,
                dmem_mem_off,
                params.ucode_dmem_pa,
                params.ucode_dmem_size,
            )?;
        }

        bar.write(
            WithBase::of::<Gsp>().at(0),
            regs::NV_PFALCON2_FALCON_BROM_PARAADDR::zeroed().with_value(params.hs_sig_dmem_addr),
        );
        bar.write(
            WithBase::of::<Gsp>(),
            regs::NV_PFALCON2_FALCON_BROM_ENGIDMASK::zeroed().with_value(params.engine_id_mask),
        );
        bar.write(
            WithBase::of::<Gsp>(),
            regs::NV_PFALCON2_FALCON_BROM_CURR_UCODE_ID::zeroed()
                .with_ucode_id(params.ucode_id as u8),
        );
        bar.write(
            WithBase::of::<Gsp>(),
            regs::NV_PFALCON2_FALCON_MOD_SEL::zeroed().with_algo(FalconModSelAlgo::Rsa3k),
        );

        gsp_falcon.write_mailboxes(Some(0xdead), None);

        bar.write(
            WithBase::of::<Gsp>(),
            regs::NV_PFALCON_FALCON_BOOTVEC::zeroed().with_value(params.ucode_imem_va),
        );

        gsp_falcon.start()?;
        gsp_falcon.wait_till_halted().inspect_err(|_| {
            dev_err!(
                dev,
                "Timeout waiting for HS binary to halt (mbox0={:#x})\n",
                gsp_falcon.read_mailbox0()
            );
        })?;

        // Core resume: restart GSP-RM via SEC2.
        gsp_falcon.reset()?;
        gsp_falcon.write_mailboxes(
            Some(libos_dma_handle as u32),
            Some((libos_dma_handle >> 32) as u32),
        );
        sec2_falcon.start()?;

        gsp_falcon
            .check_reload_completed(Delta::from_secs(2))
            .inspect_err(|_| {
                let mbox0 = sec2_falcon.read_mailbox0();
                dev_err!(
                    dev,
                    "Timeout waiting for SEC2 to resume GSP-RM (SEC2 mbox0={:#x})\n",
                    mbox0
                );
            })?;

        let sec2_mbox0 = sec2_falcon.read_mailbox0();
        if sec2_mbox0 != 0 {
            dev_err!(
                dev,
                "SEC2 reported error during core resume: {:#x}\n",
                sec2_mbox0
            );
            return Err(EIO);
        }

        gsp_falcon.write_os_version(bootloader_app_version);

        if !gsp_falcon.is_riscv_active() {
            dev_err!(dev, "GSP RISC-V not active after core resume\n");
            return Err(EIO);
        }

        Ok(())
    }

    /// Shut down the GSP and wait until it is offline.
    fn shutdown_gsp(
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        gsp_falcon: &Falcon<'_, Gsp>,
        mode: commands::PowerStateLevel,
    ) -> Result {
        // Command to shut the GSP down.
        cmdq.send_command(bar, commands::UnloadingGuestDriver::new(mode))?;

        // Wait until GSP signals it is suspended.
        const LIBOS_INTERRUPT_PROCESSOR_SUSPENDED: u32 = bits::bit_u32(31);
        read_poll_timeout(
            || Ok(gsp_falcon.read_mailbox0()),
            |&mb0| mb0 & LIBOS_INTERRUPT_PROCESSOR_SUSPENDED != 0,
            Delta::from_millis(10),
            Delta::from_secs(5),
        )
        .map(|_| ())
    }

    /// Attempts to unload the GSP firmware.
    ///
    /// This stops all activity on the GSP.
    pub(crate) fn unload(
        &self,
        mut ctx: super::GspBootContext<'_, '_>,
        unload_bundle: Option<super::UnloadBundle>,
    ) -> Result {
        let dev = ctx.dev();

        // Shut down the GSP. Keep going even in case of error.
        let mut res = Self::shutdown_gsp(
            &self.cmdq,
            ctx.bar,
            ctx.gsp_falcon,
            commands::PowerStateLevel::Level0,
        )
        .inspect_err(|e| dev_err!(dev, "GSP shutdown failed: {:?}\n", e));

        // Run the unload bundle to reset the GSP so it can be booted again.
        if let Some(unload_bundle) = unload_bundle {
            res = res.and(
                unload_bundle
                    .0
                    .run(&mut ctx)
                    .inspect_err(|e| dev_err!(dev, "Unload bundle failed: {:?}\n", e)),
            );
        } else {
            dev_warn!(
                dev,
                "Unload bundle is missing, GSP won't be properly reset.\n"
            );

            res = Err(EAGAIN);
        }

        res.inspect(|()| dev_info!(dev, "GSP successfully unloaded\n"))
    }
}

/// Falcon bootloader DMEM descriptor (RM_FLCN_BL_DMEM_DESC).
///
/// This is nested within the `GSP_LOAD_EXEC_GENERIC_BOOTLOADER` payload and
/// describes the firmware image to be loaded by the host into the GSP falcon.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct FalconBlDmemDesc {
    _reserved: [u32; 4],
    _signature: [u32; 4],
    ctx_dma: u32,
    code_dma_base_lo: u32,
    code_dma_base_hi: u32,
    non_secure_code_off: u32,
    non_secure_code_size: u32,
    secure_code_off: u32,
    secure_code_size: u32,
    code_entry_point: u32,
    data_dma_base_lo: u32,
    data_dma_base_hi: u32,
    data_size: u32,
    _argc: u32,
    _argv: u32,
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for FalconBlDmemDesc {}

/// Parameters for loading and executing the generic bootloader.
///
/// Sent by GSP-RM as the payload of `GSP_LOAD_EXEC_GENERIC_BOOTLOADER`.
/// The descriptor carries the code and data addresses, while `addr_space` and
/// `cpu_cache_attrib` tell the CPU side how to configure the selected FBIF
/// aperture.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct LoadExecGenericBootloaderParams {
    dmem_desc: FalconBlDmemDesc,
    dmem_desc_size: u32,
    addr_space: u32,
    cpu_cache_attrib: u32,
    _reserved: [u32; 4],
}

impl LoadExecGenericBootloaderParams {
    const ADDR_SYSMEM: u32 = 1;
    const ADDR_FBMEM: u32 = 2;
    const NV_MEMORY_CACHED: u32 = 0;
    const NV_MEMORY_UNCACHED: u32 = 1;
    const NUM_CTX_DMA: usize = 8;

    fn ctx_dma(&self) -> Result<u8> {
        let ctx_dma = u8::try_from(self.dmem_desc.ctx_dma).map_err(|_| EINVAL)?;

        if usize::from(ctx_dma) >= Self::NUM_CTX_DMA {
            return Err(EINVAL);
        }

        Ok(ctx_dma)
    }

    fn fbif_target(&self) -> Result<FalconFbifTarget> {
        match (self.addr_space, self.cpu_cache_attrib) {
            (Self::ADDR_FBMEM, _) => Ok(FalconFbifTarget::LocalFb),
            (Self::ADDR_SYSMEM, Self::NV_MEMORY_CACHED) => Ok(FalconFbifTarget::CoherentSysmem),
            (Self::ADDR_SYSMEM, Self::NV_MEMORY_UNCACHED) => {
                Ok(FalconFbifTarget::NoncoherentSysmem)
            }
            _ => Err(EINVAL),
        }
    }
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for LoadExecGenericBootloaderParams {}

/// Parameters for loading and executing an HS (High-Security) binary.
///
/// Sent by GSP-RM as the payload of `GSP_LOAD_EXEC_HS_BINARY`. The firmware
/// code and data are located in framebuffer memory at the given physical addresses.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
struct HsBinaryParams {
    imem_phys_addr: u64,
    dmem_phys_addr: u64,
    _reserved64: [u64; 2],
    ucode_imem_va: u32,
    ucode_imem_pa: u32,
    ucode_imem_size: u32,
    ucode_dmem_va: u32,
    ucode_dmem_pa: u32,
    ucode_dmem_size: u32,
    hs_sig_dmem_addr: u32,
    engine_id_mask: u32,
    ucode_id: u32,
    _reserved32: [u32; 3],
}

// SAFETY: This struct only contains integer types for which all bit patterns are valid.
unsafe impl FromBytes for HsBinaryParams {}
