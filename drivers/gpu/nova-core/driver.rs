// SPDX-License-Identifier: GPL-2.0

use kernel::{
    auxiliary,
    device::{
        Bound,
        Core, //
    },
    io::resource,
    pci,
    pci::{
        Class,
        ClassMask,
        Vendor, //
    },
    prelude::*,
    sizes::SZ_16M,
    sync::atomic::{
        Atomic,
        Relaxed, //
    },
    types::CovariantForLt,
};

use crate::{
    gpu,
    gpu::{
        Gpu,
        Spec, //
    }, //
};

#[cfg(CONFIG_PCI_IOV)]
use kernel::types::ForLt;

#[cfg(CONFIG_PCI_IOV)]
use crate::vgpu::vgpu_api::NovaCoreVfApi;

/// Counter for generating unique auxiliary device IDs.
static AUXILIARY_ID_COUNTER: Atomic<u32> = Atomic::new(0);

#[pin_data]
pub(crate) struct NovaCore<'bound> {
    #[cfg(CONFIG_PCI_IOV)]
    #[allow(clippy::type_complexity)]
    #[pin]
    _vf_registration: pci::VfRegistration<'bound, ForLt!(NovaCoreVfApi<'_>)>,
    #[pin]
    pub(crate) gpu: Gpu<'bound>,
    bar: pci::Bar<'bound, BAR0_SIZE>,
    bar1: Bar1<'bound>,
    #[allow(clippy::type_complexity)]
    _reg: auxiliary::Registration<'bound, CovariantForLt!(())>,
}

pub(crate) struct NovaCoreDriver;

const BAR0_SIZE: usize = SZ_16M;

pub(crate) type Bar0<'a> = &'a pci::Bar<'a, BAR0_SIZE>;
pub(crate) type NovaRegisters = kernel::io::Region<BAR0_SIZE>;
pub(crate) type Bar1<'a> = pci::Bar<'a>;

/// Returns the Linux PCI resource index that holds BAR1 for an NVIDIA GPU.
///
/// On Maxwell through Ada, BAR0 is a 32-bit memory BAR occupying a single
/// Linux PCI resource slot, so BAR1 lives at index 1. Starting with Blackwell
/// (and on some Ampere GA100 / Hopper SKUs) BAR0 is a 64-bit memory BAR that
/// consumes two consecutive resource slots: index 0 holds the low 32 bits and
/// index 1 holds the high 32 bits (with no `flags` / or size of its own),
/// shifting BAR1 to index 2.
pub(crate) fn bar1_resource_index(pdev: &pci::Device<Bound>) -> Result<u32> {
    // Probe the `IORESOURCE_MEM_64` flag of BAR0 as a robust way of exposing
    // if BAR0 and hence BAR1 is 64-bit.
    let flags0 = pdev.resource_flags(0)?;
    if flags0.contains(resource::Flags::IORESOURCE_MEM_64) {
        Ok(2)
    } else {
        Ok(1)
    }
}

kernel::pci_device_table!(
    PCI_TABLE,
    <NovaCoreDriver as pci::Driver>::IdInfo,
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

#[vtable]
impl pci::Driver for NovaCoreDriver {
    type IdInfo = ();
    type Data<'bound> = NovaCore<'bound>;
    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;

    fn probe<'bound>(
        pdev: &'bound pci::Device<Core<'_>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            dev_dbg!(pdev, "Probe Nova Core GPU driver.\n");

            #[cfg(CONFIG_PCI_IOV)]
            if pdev.is_virtfn() {
                return Err(ENODEV);
            }

            pdev.enable_device_mem()?;
            pdev.set_master();

            Ok(try_pin_init!(&this in NovaCore {
                bar: pdev.iomap_region_sized::<BAR0_SIZE>(0, c"nova-core/bar0")?,
                bar1: {
                    let bar1_idx = bar1_resource_index(pdev)?;
                    pdev.iomap_region(bar1_idx, c"nova-core/bar1")?
                },
                _: {
                    let spec = Spec::new(pdev.as_ref(), bar)?;

                    gpu::wait_gfw_boot_completion(pdev.as_ref(), bar, spec.chipset)?;

                    // Runs before the GSP boots, so the self-test never reads or clears GSP
                    // interrupt state.
                    #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
                    crate::irq::doorbell_test::run_selftest(pdev, bar, spec.chipset)?;
                },
                // TODO: Use self-referential pin-init syntax once available.
                gpu <- Gpu::new(
                    pdev,
                    // SAFETY: `bar` is initialized above, pinned, and outlives `gpu`.
                    unsafe { &*core::ptr::from_ref(bar) },
                    // SAFETY: `bar1` is initialized above, pinned, and outlives `gpu`.
                    unsafe { &*core::ptr::from_ref(bar1) },
                ),
                // Run optional GPU selftests.
                #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
                _: { gpu.run_selftests(pdev) },
                #[cfg(CONFIG_PCI_IOV)]
                _vf_registration <- {
                    // SAFETY: `gpu` is initialized at its pinned address and
                    // outlives the registration and its borrowed API data.
                    let gpu = unsafe { &(*this.as_ptr()).gpu };
                    let api = NovaCoreVfApi::new(gpu, pdev);
                    // SAFETY: Probe has exclusive access to the registration
                    // slot and no VFs are enabled before successful probe. The
                    // PCI adapter uses managed SR-IOV, and the pinned driver
                    // data drops this registration before its borrowed GPU resources.
                    unsafe { pci::VfRegistration::new(pdev, Ok(api)) }
                },
                _reg: auxiliary::Registration::new(
                    pdev.as_ref(),
                    c"nova-drm",
                    // TODO[XARR]: Use XArray or perhaps IDA for proper ID allocation/recycling. For
                    // now, use a simple atomic counter that never recycles IDs.
                    AUXILIARY_ID_COUNTER.fetch_add(1, Relaxed),
                    crate::MODULE_NAME,
                    (),
                )?,
            }))
        })
    }

    #[cfg(CONFIG_PCI_IOV)]
    fn sriov_configure<'bound>(
        dev: &'bound pci::sriov::Device<Core<'_>>,
        _this: Pin<&Self::Data<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32> {
        if nr_virtfn == 0 {
            dev.disable_sriov();
        } else {
            dev.enable_sriov(nr_virtfn)?;
        }

        Ok(nr_virtfn)
    }
}
