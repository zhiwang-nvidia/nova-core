// SPDX-License-Identifier: GPL-2.0

//! Rust SR-IOV driver sample based on QEMU's 82576 ([igb]) emulation.
//!
//! To make this driver probe, QEMU must be run with `-device igb`.
//!
//! Further, enable [vIOMMU] with interrupt remapping using, e.g.,
//!
//! `-M q35,accel=kvm,kernel-irqchip=split -device intel-iommu,intremap=on,caching-mode=on`
//!
//! and append `intel_iommu=on` to the guest kernel arguments.
//!
//! [igb]: https://www.qemu.org/docs/master/system/devices/igb.html
//! [vIOMMU]: https://wiki.qemu.org/Features/VT-d

use kernel::{
    device::{
        Bound,
        Core, //
    },
    driver,
    new_mutex,
    pci,
    prelude::*,
    sync::{
        aref::ARef,
        Mutex, //
    },
    types::CovariantForLt,
    InPlaceModule, //
};

const PF_DRIVER_NAME: &CStr = c"rust_driver_sriov_pf";
const VF_DRIVER_NAME: &CStr = c"rust_driver_sriov_vf";

struct SamplePfDriver;
struct SampleVfDriver;

#[pin_data]
struct PfApi<'bound> {
    pdev: &'bound pci::Device<Bound>,
    #[pin]
    requests: Mutex<u64>,
}

type PfApiForLt = CovariantForLt!(PfApi<'_>);

impl PfApi<'_> {
    fn submit(self: Pin<&Self>, vf: &pci::Device<Bound>) -> Result<u64> {
        let mut requests = self.requests.lock();
        let request = (*requests).checked_add(1).ok_or(EOVERFLOW)?;
        *requests = request;
        drop(requests);

        dev_info!(
            self.pdev,
            "Handle PF request {} from VF devfn {:#x}.\n",
            request,
            vf.dev_id()
        );

        Ok(request)
    }
}

#[pin_data(PinnedDrop)]
struct PfDriverData<'bound> {
    // Keep the device alive until the registration stops exposing `PfApi::pdev`.
    _registration: pci::SriovPfRegistration<'bound, PfApiForLt>,
    pdev: ARef<pci::Device>,
}

#[pin_data(PinnedDrop)]
struct VfDriverData {
    pdev: ARef<pci::Device>,
}

kernel::pci_device_table!(
    PF_TABLE,
    <SamplePfDriver as pci::SriovPfDriver>::IdInfo,
    [(
        // E1000_DEV_ID_82576
        pci::DeviceId::from_id(pci::Vendor::INTEL, 0x10c9),
        ()
    )]
);

kernel::pci_device_table!(
    VF_TABLE,
    <SampleVfDriver as pci::SriovVfDriver>::IdInfo,
    [(
        // E1000_DEV_ID_82576_VF
        pci::DeviceId::from_id(pci::Vendor::INTEL, 0x10ca),
        ()
    )]
);

impl pci::SriovPfDriver for SamplePfDriver {
    type IdInfo = ();
    type Data<'bound> = PfDriverData<'bound>;

    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PF_TABLE;

    fn probe<'bound>(
        pdev: &'bound pci::Device<Core<'_>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            dev_info!(
                pdev,
                "Probe Rust SR-IOV PF sample (PCI ID: {}, 0x{:x}).\n",
                pdev.vendor_id(),
                pdev.device_id()
            );

            pdev.enable_device_mem()?;
            pdev.set_master();

            // SAFETY:
            // - this is the PF probe callback and SR-IOV has not been enabled;
            // - the registration is stored in immutable PF driver data and is neither replaced
            //   nor forgotten;
            // - this is the only registration created for the PF; and
            // - VFs are enabled only by `sriov_configure`, after probe has completed.
            let registration = unsafe {
                pci::SriovPfRegistration::new_with_lt(
                    pdev,
                    pin_init!(PfApi {
                        pdev,
                        requests <- new_mutex!(0),
                    }),
                )?
            };

            Ok(try_pin_init!(PfDriverData {
                _registration: registration,
                pdev: pdev.into(),
            }))
        })
    }

    fn sriov_configure<'bound>(
        dev: &'bound pci::sriov::Device<Core<'_>>,
        this: Pin<&Self::Data<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32> {
        if nr_virtfn == 0 {
            dev_info!(
                this.pdev,
                "Disable SR-IOV (PCI ID: {}, 0x{:x}).\n",
                this.pdev.vendor_id(),
                this.pdev.device_id()
            );
            dev.disable_sriov();
        } else {
            dev_info!(
                this.pdev,
                "Enable SR-IOV (PCI ID: {}, 0x{:x}).\n",
                this.pdev.vendor_id(),
                this.pdev.device_id()
            );
            dev.enable_sriov(nr_virtfn)?;
        }

        assert_eq!(dev.num_vfs(), nr_virtfn);
        Ok(nr_virtfn)
    }
}

impl pci::SriovVfDriver for SampleVfDriver {
    type IdInfo = ();
    type Data<'bound> = VfDriverData;
    type PfData = PfApiForLt;

    const ID_TABLE: pci::IdTable<Self::IdInfo> = &VF_TABLE;

    fn probe<'bound>(
        pdev: &'bound pci::Device<Core<'_>>,
        pf_api: Pin<&'bound PfApi<'bound>>,
        _info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            dev_info!(
                pdev,
                "Probe Rust SR-IOV VF sample (PCI ID: {}, 0x{:x}).\n",
                pdev.vendor_id(),
                pdev.device_id()
            );

            pdev.enable_device_mem()?;
            pdev.set_master();

            let request = pf_api.submit(pdev)?;
            dev_info!(pdev, "Submitted request {} through PF data.\n", request);

            Ok(try_pin_init!(VfDriverData { pdev: pdev.into() }))
        })
    }
}

#[pinned_drop]
impl PinnedDrop for PfDriverData<'_> {
    fn drop(self: Pin<&mut Self>) {
        dev_info!(self.pdev, "Remove Rust SR-IOV PF sample.\n");
    }
}

#[pinned_drop]
impl PinnedDrop for VfDriverData {
    fn drop(self: Pin<&mut Self>) {
        dev_info!(self.pdev, "Remove Rust SR-IOV VF sample.\n");
    }
}

#[pin_data]
struct SampleModule {
    // Keep the VF driver registered while PF removal tears down its VFs.
    #[pin]
    _pf: driver::Registration<pci::Adapter<pci::sriov::PfAdapter<SamplePfDriver, SampleModule>>>,
    #[pin]
    _vf: driver::Registration<pci::Adapter<pci::sriov::VfAdapter<SampleVfDriver, SampleModule>>>,
}

impl InPlaceModule for SampleModule {
    fn init(module: &'static ThisModule) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            // The VF driver must be ready before the PF can enable VFs.
            _vf <- driver::Registration::new(VF_DRIVER_NAME, module),
            _pf <- driver::Registration::new(PF_DRIVER_NAME, module),
        })
    }
}

module! {
    type: SampleModule,
    name: "rust_driver_sriov",
    authors: ["Peter Colberg"],
    description: "Rust SR-IOV driver",
    license: "GPL v2",
}
