// SPDX-License-Identifier: GPL-2.0

//! FSP is a hardware unit that runs FMC firmware.

use kernel::{
    device,
    dma::Coherent,
    firmware::Firmware,
    prelude::*, //
};

use crate::gpu::Chipset;

/// FMC firmware loaded for FSP.
///
/// The FMC image is allocated as DMA-coherent memory because the hardware reads it directly.
/// The remaining blobs are accessed only by the driver.
pub(crate) struct FspFirmware {
    pub(crate) fmc_image: Coherent<[u8]>,
    pub(crate) fmc_hash: Firmware,
    pub(crate) fmc_publickey: Firmware,
    pub(crate) fmc_signature: Firmware,
}

impl FspFirmware {
    pub(crate) fn new(dev: &device::Device<device::Bound>, chipset: Chipset) -> Result<Self> {
        let (_, fmc_image_fw) = super::request_firmware(dev, chipset, "fmc-image")?;
        let fmc_image = Coherent::from_slice(dev, fmc_image_fw.data(), GFP_KERNEL)?;

        let (_, fmc_hash) = super::request_firmware(dev, chipset, "fmc-hash")?;
        let (_, fmc_publickey) = super::request_firmware(dev, chipset, "fmc-publickey")?;
        let (_, fmc_signature) = super::request_firmware(dev, chipset, "fmc-signature")?;

        Ok(Self {
            fmc_image,
            fmc_hash,
            fmc_publickey,
            fmc_signature,
        })
    }
}
