// SPDX-License-Identifier: GPL-2.0

#include <linux/err.h>
#include <linux/module.h>
#include <linux/pci.h>

#include "rust_driver_sriov.h"

#define E1000_DEV_ID_82576_VF	0x10ca

static const struct rust_ffi_token ffi_token = {
	.high = RUST_DRIVER_SRIOV_FFI_TOKEN_HIGH,
	.low = RUST_DRIVER_SRIOV_FFI_TOKEN_LOW,
};

static int rust_driver_sriov_c_vf_probe(struct pci_dev *pdev,
					const struct pci_device_id *id)
{
	const struct rust_driver_sriov_ops *ops;
	const struct rust_ffi *ffi;
	int ret;

	ffi = pci_iov_borrow_rust_pf_data(pdev, &ffi_token,
					  RUST_DRIVER_SRIOV_FFI_ABI_MAJOR,
					  RUST_DRIVER_SRIOV_FFI_ABI_MINOR,
					  sizeof(*ops));
	if (IS_ERR(ffi))
		return dev_err_probe(&pdev->dev, PTR_ERR(ffi),
				     "failed to borrow PF FFI\n");

	ops = ffi->ops;
	if (!ops->submit)
		return dev_err_probe(&pdev->dev, -EOPNOTSUPP,
				     "PF FFI does not implement submit\n");

	ret = ops->submit(ffi->context, pci_dev_id(pdev));
	if (ret)
		return dev_err_probe(&pdev->dev, ret,
				     "failed to submit through PF FFI\n");

	pci_info(pdev, "submitted request through Rust PF FFI\n");

	return 0;
}

static const struct pci_device_id rust_driver_sriov_c_vf_id_table[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_INTEL, E1000_DEV_ID_82576_VF) },
	{ }
};
MODULE_DEVICE_TABLE(pci, rust_driver_sriov_c_vf_id_table);

static struct pci_driver rust_driver_sriov_c_vf_driver = {
	.name = "rust_driver_sriov_c_vf",
	.id_table = rust_driver_sriov_c_vf_id_table,
	.probe = rust_driver_sriov_c_vf_probe,
};
module_pci_driver(rust_driver_sriov_c_vf_driver);

MODULE_AUTHOR("Rust for Linux Contributors");
MODULE_DESCRIPTION("C VF consumer for the Rust SR-IOV driver sample");
MODULE_LICENSE("GPL");
