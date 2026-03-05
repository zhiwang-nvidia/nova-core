// SPDX-License-Identifier: GPL-2.0

use kernel::{
    auxiliary,
    device::Bound,
    device::Core,
    fwctl,
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
    types::ForLt,
};

use crate::{
    fwctl::{
        NovaCoreFwCtl,
        NovaCoreFwCtlData, //
    },
    gpu::Gpu,
    irq::gsp::GspIrq, //
};

/// Counter for generating unique auxiliary device IDs.
static AUXILIARY_ID_COUNTER: Atomic<u32> = Atomic::new(0);

#[pin_data]
pub(crate) struct NovaCore<'bound> {
    #[allow(clippy::type_complexity)]
    _reg: auxiliary::Registration<'bound, ForLt!(())>,
    // Declared before the IRQ and GPU so fwctl is unregistered and all callbacks have drained
    // while interrupt-driven command-queue service is still live.
    _fwctl: fwctl::Registration<'bound, NovaCoreFwCtl>,
    /// GSP event interrupt registration.
    ///
    /// Declared before the GPU and BAR so `free_irq` runs (waiting out any in-flight handler)
    /// before the GSP is unloaded (`gpu`) or the BAR mapping is released (`bar`).
    #[pin]
    _gsp_irq: GspIrq<'bound>,
    #[pin]
    pub(crate) gpu: Gpu<'bound>,
    bar: pci::Bar<'bound, BAR0_SIZE>,
}

pub(crate) struct NovaCoreDriver;

const BAR0_SIZE: usize = SZ_16M;

pub(crate) type Bar0<'a> = &'a pci::Bar<'a, BAR0_SIZE>;
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
    MODULE_PCI_TABLE,
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
        _info: &'bound Self::IdInfo,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            dev_dbg!(pdev, "Probe Nova Core GPU driver.\n");

            pdev.enable_device_mem()?;
            pdev.set_master();

            // Allocate the single MSI vector once. It is shared by the self-test (if enabled),
            // whose handler is freed before it returns, and the permanent GSP handler below.
            let vector = crate::irq::alloc_vector(pdev)?;

            Ok(try_pin_init!(&this in NovaCore {
                bar: pdev.iomap_region_sized::<BAR0_SIZE>(0, c"nova-core/bar0")?,
                // TODO: Use `&bar` self-referential pin-init syntax once available.
                //
                // SAFETY: `bar` is initialized before this expression is evaluated
                // (`try_pin_init!()` initializes fields in declaration order), lives at a pinned
                // stable address, and is dropped after `gpu` (struct field drop order).
                gpu <- Gpu::new(pdev, unsafe { &*core::ptr::from_ref(bar) }, vector),
                // Register the permanent GSP SWGEN0 handler before enabling the interrupt.
                //
                // SAFETY: `bar` is initialized before this expression is evaluated, lives at a
                // pinned stable address, and is dropped after `_gsp_irq` (declared before the GPU
                // and BAR), so the handler's borrow stays valid for its whole lifetime.
                // `_gsp_irq` is stored in `NovaCore`, whose `Drop` runs `free_irq`, so the
                // registration is never leaked.
                _gsp_irq <- unsafe {
                    GspIrq::new(
                        pdev,
                        vector,
                        &*core::ptr::from_ref(bar),
                        gpu.cmdq(),
                        gpu.chipset(),
                    )
                },
                // Enable the GSP notification now that the handler is registered, then drain any
                // messages the GSP posted during boot before relying on the interrupt.
                _: {
                    // SAFETY: as for the `bar` borrow above.
                    let bar = unsafe { &*core::ptr::from_ref(bar) };
                    crate::irq::gsp::enable(bar, gpu.chipset());
                    gpu.cmdq().drain(bar)?;
                },
                _fwctl: {
                    // SAFETY: `gpu` was initialized above in this pinned `NovaCore`. The fwctl
                    // registration is declared before `_gsp_irq` and `gpu`, so its Drop
                    // implementation unregisters the device and drains callbacks before IRQ
                    // teardown or command-queue release. The same ordering also holds during
                    // initializer cleanup.
                    let gpu = unsafe { &*core::ptr::addr_of!((*this.as_ptr()).gpu) };
                    let fwctl_dev = fwctl::Device::<NovaCoreFwCtl>::new(pdev.as_ref())?;

                    // SAFETY: `fwctl_dev` is newly allocated and unregistered. The returned
                    // registration is stored in `NovaCore` and cannot outlive the bound parent
                    // device. Its registration data borrows pinned BAR0 and owns a shared
                    // command-queue handle obtained from the pinned `gpu` above.
                    unsafe {
                        fwctl::Registration::new(
                            pdev.as_ref(),
                            &fwctl_dev,
                            NovaCoreFwCtlData::new(gpu.bar0(), gpu.cmdq()),
                        )?
                    }
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
    fn sriov_configure(dev: &pci::Device<Core<'_>>, nr_virtfn: i32) -> Result<i32> {
        if nr_virtfn == 0 {
            dev.disable_sriov();
        } else {
            dev.enable_sriov(nr_virtfn)?;
        }
        Ok(nr_virtfn)
    }
}
