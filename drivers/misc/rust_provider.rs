use kernel::prelude::*;
use core::ffi::c_int;

#[repr(C)]
pub struct AddArgs {
    a: c_int,
    b: c_int,
}

module! {
    type: RustProvider,
    name: "rust_provider",
    authors: ["Test"],
    description: "Rust to C export",
    license: "GPL",
}

struct RustProvider;

impl kernel::Module for RustProvider {
    fn init(_module: &'static ThisModule) -> Result<Self> {
        pr_info!("Rust provider loaded\n");
        Ok(RustProvider)
    }
}

#[no_mangle]
pub extern "C" fn rust_add_struct(args: *const AddArgs) -> c_int {
    if args.is_null() {
        return -1;
    }
    unsafe {
        (*args).a + (*args).b
    }
}