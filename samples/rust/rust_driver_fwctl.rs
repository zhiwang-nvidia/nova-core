// SPDX-License-Identifier: GPL-2.0

//! Rust fwctl API test (based on QEMU's `pci-testdev`).
//!
//! To make this driver probe, QEMU must be run with `-device pci-testdev`.

use kernel::{
    bindings,
    device,
    device::Core,
    devres::Devres,
    fwctl,
    pci,
    prelude::*,
    sync::aref::ARef,
    types,
};

struct FwctlSampleUserCtx {
    _drvdata: u32,
}

struct FwctlSampleOps;

impl fwctl::Operations for FwctlSampleOps {
    type UserCtx = FwctlSampleUserCtx;

    const DEVICE_TYPE: fwctl::DeviceType = fwctl::DeviceType::RustFwctlTest;

    fn open(
        fwctl_uctx: &types::Opaque<bindings::fwctl_uctx>
    ) -> Result<impl PinInit<Self::UserCtx, Error>, Error> {
        let dev = fwctl::UserCtx::<Self::UserCtx>::parent_device_from_raw(fwctl_uctx);

        dev_info!(dev, "fwctl test driver: open_uctx()");

        // Return an initializer for the user context.
        // The framework will initialize this in-place in the C-allocated memory.
        Ok(try_init!(FwctlSampleUserCtx {
            _drvdata: 0,
        }))
    }

    fn close(uctx: &mut fwctl::UserCtx<FwctlSampleUserCtx>) {
        let dev = uctx.get_parent_device();

        dev_info!(dev, "fwctl test driver: close_uctx()");
    }

    fn info(uctx: &mut fwctl::UserCtx<FwctlSampleUserCtx>) -> Result<KVec<u8>, Error> {
        let dev = uctx.get_parent_device();

        dev_info!(dev, "fwctl test driver: info()");

        let mut infobuf = KVec::<u8>::new();
        infobuf.push(0xef, GFP_KERNEL)?;
        infobuf.push(0xbe, GFP_KERNEL)?;
        infobuf.push(0xad, GFP_KERNEL)?;
        infobuf.push(0xde, GFP_KERNEL)?;

        Ok(infobuf)
    }

    fn fw_rpc(
        uctx: &mut fwctl::UserCtx<FwctlSampleUserCtx>,
        scope: u32,
        rpc_in: &mut [u8],
        _out_len: *mut usize,
    ) -> Result<Option<KVec<u8>>, Error> {
        let dev = uctx.get_parent_device();

        dev_info!(dev, "fwctl test driver: fw_rpc() scope {}", scope);

        if rpc_in.len() != 4 {
            return Err(EINVAL);
        }

        dev_info!(
            dev,
            "fwctl test driver: inbuf len{} bytes[0-3] {:x} {:x} {:x} {:x}",
            rpc_in.len(),
            rpc_in[0],
            rpc_in[1],
            rpc_in[2],
            rpc_in[3]
        );

        let mut outbuf = KVec::<u8>::new();
        outbuf.push(0xef, GFP_KERNEL)?;
        outbuf.push(0xbe, GFP_KERNEL)?;
        outbuf.push(0xad, GFP_KERNEL)?;
        outbuf.push(0xde, GFP_KERNEL)?;

        Ok(Some(outbuf))
    }
}

#[pin_data]
struct FwctlSampleDriver {
    pdev: ARef<pci::Device>,
    #[pin]
    fwctl: Devres<fwctl::Registration<FwctlSampleOps>>,
}

kernel::pci_device_table!(
    PCI_TABLE,
    MODULE_PCI_TABLE,
    <FwctlSampleDriver as pci::Driver>::IdInfo,
    [(pci::DeviceId::from_id(pci::Vendor::REDHAT, 0x5), ())]
);

impl pci::Driver for FwctlSampleDriver {
    type IdInfo = ();
    const ID_TABLE: pci::IdTable<Self::IdInfo> = &PCI_TABLE;

    fn probe(pdev: &pci::Device<Core>, _info: &Self::IdInfo) -> impl PinInit<Self, Error> {
        dev_info!(pdev.as_ref(), "Probe fwctl test driver");

        // `pdev` is `Device<Core>`, which derefs to `Device<Bound>` during probe.
        let pdev_bound: &pci::Device<device::Bound> = pdev;

        try_pin_init!(Self {
            pdev: pdev.into(),
            fwctl <- fwctl::Registration::<FwctlSampleOps>::new(pdev_bound.as_ref()),
        })
    }
}

kernel::module_pci_driver! {
    type: FwctlSampleDriver,
    name: "rust_driver_fwctl",
    authors: ["Zhi Wang"],
    description: "Rust fwctl test",
    license: "GPL v2",
    imports_ns: ["FWCTL"],
}
