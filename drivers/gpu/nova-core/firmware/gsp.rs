// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    dma::{
        Coherent,
        DmaAddress, //
    },
    prelude::*,
    str::{
        CStr,
        CString, //
    },
};

use crate::{
    firmware::{
        radix3::Radix3,
        riscv::RiscvFirmware,
        BuildId, //
    },
    gpu::Chipset,
};

/// GSP firmware with 3-level radix page tables for the GSP bootloader.
///
/// Also known as "Radix3" firmware.
#[pin_data]
pub(crate) struct GspFirmware {
    /// The GSP firmware image mapped via a 3-level radix page table.
    #[pin]
    radix3: Radix3,
    /// Firmware file path as requested from userspace (e.g. `nvidia/gb202/gsp/gsp.bin`).
    pub(crate) fw_path: CString,
    /// Firmware version string extracted from the firmware.
    pub(crate) fw_version: CString,
    /// Build ID extracted from firmware.
    pub(crate) build_id: Option<BuildId>,
    /// Device-mapped GSP signatures matching the GPU's [`Chipset`].
    pub(crate) signatures: Coherent<[u8]>,
    /// GSP bootloader, verifies the GSP firmware before loading and running it.
    pub(crate) bootloader: RiscvFirmware,
}

impl GspFirmware {
    /// Loads GSP firmware files and creates the page tables expected by the GSP
    /// bootloader.
    pub(crate) fn new<'a>(
        dev: &'a device::Device<device::Bound>,
        chipset: Chipset,
    ) -> impl PinInit<Self, Error> + 'a {
        pin_init::pin_init_scope(move || {
            let (fw_path, gsp_fw) = super::request_firmware(dev, chipset, "gsp")?;
            let fw_data = VVec::with_capacity(gsp_fw.data().len(), GFP_KERNEL)
                .and_then(|mut v| {
                    v.extend_from_slice(gsp_fw.data(), GFP_KERNEL)?;
                    Ok(v)
                })
                .map_err(|_| ENOMEM)?;

            let (_, sig_fw) = super::request_firmware(dev, chipset, "gsp-fwsig")?;
            let signatures = Coherent::from_slice(dev, sig_fw.data(), GFP_KERNEL)?;

            let fw_version = match super::request_firmware(dev, chipset, "gsp-version") {
                Ok((_, fw)) => {
                    let version_str = CStr::from_bytes_until_nul(fw.data())
                        .ok()
                        .and_then(|cstr| cstr.to_str().ok())
                        .unwrap_or("unknown");
                    CString::try_from_fmt(fmt!("{version_str}"))?
                }
                Err(_) => CString::try_from_fmt(fmt!("unknown"))?,
            };

            let build_id = super::request_firmware(dev, chipset, "gsp-buildid")
                .ok()
                .and_then(|(_, fw)| BuildId::from_raw(fw.data()));

            let (_, bl) = super::request_firmware(dev, chipset, "bootloader")?;
            let bootloader = RiscvFirmware::new(dev, &bl)?;

            Ok(try_pin_init!(Self {
                radix3 <- Radix3::new(dev, &fw_data),
                fw_path,
                fw_version,
                build_id,
                signatures,
                bootloader,
            }))
        })
    }

    /// Returns the size of the GSP firmware image, in bytes.
    pub(crate) fn size(&self) -> usize {
        self.radix3.size
    }

    /// Returns the DMA handle of the radix3 level 0 page table.
    pub(crate) fn radix3_dma_handle(&self) -> DmaAddress {
        self.radix3.dma_handle()
    }
}
