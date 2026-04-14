// SPDX-License-Identifier: GPL-2.0

use kernel::{
    auxiliary,
    device::Core,
    devres::Devres,
    pci,
    pci::{
        Class,
        ClassMask,
        Vendor, //
    },
    prelude::*,
    sizes::SZ_16M,
    sync::{
        atomic::{
            Atomic,
            Relaxed, //
        },
        Arc,
    },
};

use crate::gpu::Gpu;

/// Counter for generating unique auxiliary device IDs.
static AUXILIARY_ID_COUNTER: Atomic<u32> = Atomic::new(0);

/// Find the PCI resource index for the GPU's second BAR (BAR1 aperture).
///
/// 64-bit BAR0 consumes two resource slots (0 + 1), pushing BAR1 to index 2.
/// 32-bit BAR0 uses only slot 0, so BAR1 is at index 1.
pub(crate) fn bar1_region(pdev: &pci::Device<Core>) -> Result<u32> {
    for idx in 1u32..6 {
        if pdev.resource_len(idx)? > 0 {
            return Ok(idx);
        }
    }
    Err(ENODEV)
}

#[pin_data]
pub(crate) struct NovaCore {
    #[pin]
    pub(crate) gpu: Gpu,
    #[pin]
    _reg: Devres<auxiliary::Registration>,
}

const BAR0_SIZE: usize = SZ_16M;
pub(crate) type Bar0 = pci::Bar<BAR0_SIZE>;
pub(crate) type Bar1 = pci::Bar;

kernel::pci_device_table!(
    PCI_TABLE,
    MODULE_PCI_TABLE,
    <NovaCore as pci::Driver>::IdInfo,
    [
        // Modern NVIDIA GPUs will show up as either VGA or 3D controllers.
        (
            pci::DeviceId::from_class_and_vendor(
                Class::DISPLAY_VGA,
                ClassMask::ClassSubclass,
                Vendor::NVIDIA
            ),
            ()
        ),
        (
            pci::DeviceId::from_class_and_vendor(
                Class::DISPLAY_3D,
                ClassMask::ClassSubclass,
                Vendor::NVIDIA
            ),
            ()
        ),
    ]
);

impl pci::Driver for NovaCore {
    type IdInfo = ();
    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;

    fn probe(pdev: &pci::Device<Core>, _info: &Self::IdInfo) -> impl PinInit<Self, Error> {
        pin_init::pin_init_scope(move || {
            dev_dbg!(pdev, "Probe Nova Core GPU driver.\n");

            pdev.enable_device_mem()?;
            pdev.set_master();

            let bar = Arc::pin_init(
                pdev.iomap_region_sized::<BAR0_SIZE>(0, c"nova-core/bar0"),
                GFP_KERNEL,
            )?;

            let bar1_idx = bar1_region(pdev)?;
            let bar1 = Arc::pin_init(
                pdev.iomap_region(bar1_idx, c"nova-core/bar1"),
                GFP_KERNEL,
            )?;

            Ok(try_pin_init!(Self {
                gpu <- Gpu::new(pdev, bar.clone(), bar1, bar.access(pdev.as_ref())?),
                // Run optional GPU selftests.
                _: {
                    let mut gpu = gpu;
                    gpu.as_mut().run_selftests(pdev)?;
                    gpu.mock_bootload(pdev)?;
                },
                _reg <- auxiliary::Registration::new(
                    pdev.as_ref(),
                    c"nova-drm",
                    // TODO[XARR]: Use XArray or perhaps IDA for proper ID allocation/recycling. For
                    // now, use a simple atomic counter that never recycles IDs.
                    AUXILIARY_ID_COUNTER.fetch_add(1, Relaxed),
                    crate::MODULE_NAME
                ),
            }))
        })
    }

    fn unbind(pdev: &pci::Device<Core>, this: Pin<&Self>) {
        this.gpu.unbind(pdev.as_ref());
    }
}
