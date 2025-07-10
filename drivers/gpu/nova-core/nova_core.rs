// SPDX-License-Identifier: GPL-2.0

//! Nova Core GPU Driver

use kernel::prelude::*;

mod dma;
mod driver;
mod falcon;
mod fb;
mod firmware;
mod gfw;
mod gpu;
mod regs;
mod util;
mod vbios;

pub(crate) const MODULE_NAME: &kernel::str::CStr = <LocalModule as kernel::ModuleMetadata>::NAME;

kernel::module_pci_driver! {
    type: driver::NovaCore,
    name: "NovaCore",
    author: "Danilo Krummrich",
    description: "Nova Core GPU driver",
    license: "GPL v2",
    firmware: [],
}

kernel::module_firmware!(firmware::ModInfoBuilder);

#[kunit_tests(nova_core_tests)]
mod tests {
    use kernel::prelude::*;

    #[test]
    fn test_hello() {
        pr_info!("hello there!\n");
    }
}
