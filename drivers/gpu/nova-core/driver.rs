// SPDX-License-Identifier: GPL-2.0

use kernel::{
    auxiliary,
    device::Core,
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

use crate::gpu::Gpu;

/// Counter for generating unique auxiliary device IDs.
static AUXILIARY_ID_COUNTER: Atomic<u32> = Atomic::new(0);

#[pin_data]
pub(crate) struct NovaCore<'bound> {
    #[cfg(CONFIG_PCI_IOV)]
    #[allow(clippy::type_complexity)]
    _pf_registration: Option<pci::SriovPfRegistration<'bound, CovariantForLt!(())>>,
    #[pin]
    pub(crate) gpu: Gpu<'bound>,
    bar: pci::Bar<'bound, BAR0_SIZE>,
    #[allow(clippy::type_complexity)]
    _reg: auxiliary::Registration<'bound, CovariantForLt!(())>,
}

pub(crate) struct NovaCoreDriver;

const BAR0_SIZE: usize = SZ_16M;

pub(crate) type Bar0<'a> = &'a pci::Bar<'a, BAR0_SIZE>;
pub(crate) type NovaRegisters = kernel::io::Region<BAR0_SIZE>;

kernel::pci_device_table!(
    PCI_TABLE,
    (),
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

impl NovaCoreDriver {
    fn probe<'bound, 'ctx>(
        pdev: &'bound pci::Device<Core<'ctx>>,
    ) -> impl PinInit<NovaCore<'bound>, Error> + 'bound + use<'bound, 'ctx> {
        pin_init::pin_init_scope(move || {
            dev_dbg!(pdev, "Probe Nova Core GPU driver.\n");

            pdev.enable_device_mem()?;
            pdev.set_master();

            Ok(try_pin_init!(NovaCore {
                bar: pdev.iomap_region_sized::<BAR0_SIZE>(0, c"nova-core/bar0")?,
                // TODO: Use `&bar` self-referential pin-init syntax once available.
                //
                // SAFETY: `bar` is initialized before this expression is evaluated
                // (`try_pin_init!()` initializes fields in declaration order), lives at a pinned
                // stable address, and is dropped after `gpu` (struct field drop order).
                gpu <- Gpu::new(pdev, unsafe { &*core::ptr::from_ref(bar) }),
                // Run optional GPU selftests.
                #[cfg(CONFIG_NOVA_CORE_SELFTESTS)]
                _: { gpu.run_selftests(pdev) },
                _reg: auxiliary::Registration::new(
                    pdev.as_ref(),
                    c"nova-drm",
                    // TODO[XARR]: Use XArray or perhaps IDA for proper ID allocation/recycling. For
                    // now, use a simple atomic counter that never recycles IDs.
                    AUXILIARY_ID_COUNTER.fetch_add(1, Relaxed),
                    crate::MODULE_NAME,
                    (),
                )?,
                #[cfg(CONFIG_PCI_IOV)]
                _pf_registration: if pdev.sriov_get_totalvfs().is_some() {
                    // SAFETY:
                    // - the PCI core serializes this probe, providing exclusive access to the
                    //   registration slot;
                    // - VFs are enabled only by `sriov_configure`, after probe has completed;
                    // - the registration is stored in immutable driver data and is neither
                    //   replaced nor forgotten; and
                    // - the PCI adapter uses managed SR-IOV, which removes every VF before the
                    //   driver data and its registration are dropped.
                    Some(unsafe { pci::SriovPfRegistration::new_with_lt(pdev, ())? })
                } else {
                    None
                },
            }))
        })
    }

    #[cfg(CONFIG_PCI_IOV)]
    fn sriov_configure<'bound>(
        dev: &'bound pci::sriov::Device<Core<'_>>,
        this: Pin<&NovaCore<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32> {
        if this._pf_registration.is_none() {
            return Err(ENODEV);
        }

        if nr_virtfn == 0 {
            dev.disable_sriov();
        } else {
            dev.enable_sriov(nr_virtfn)?;
        }

        Ok(nr_virtfn)
    }
}

#[cfg(CONFIG_PCI_IOV)]
impl pci::SriovPfDriver for NovaCoreDriver {
    type IdInfo = ();
    type Data<'bound> = NovaCore<'bound>;
    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;

    fn probe<'bound>(
        pdev: &'bound pci::Device<Core<'_>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        Self::probe(pdev)
    }

    fn sriov_configure<'bound>(
        dev: &'bound pci::sriov::Device<Core<'_>>,
        this: Pin<&Self::Data<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32> {
        Self::sriov_configure(dev, this, nr_virtfn)
    }
}

#[cfg(not(CONFIG_PCI_IOV))]
#[vtable]
impl pci::Driver for NovaCoreDriver {
    type IdInfo = ();
    type Data<'bound> = NovaCore<'bound>;
    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;

    fn probe<'bound>(
        pdev: &'bound pci::Device<Core<'_>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        Self::probe(pdev)
    }
}
