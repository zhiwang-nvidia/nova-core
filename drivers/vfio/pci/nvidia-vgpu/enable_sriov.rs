// SPDX-License-Identifier: GPL-2.0

//! Runtime switch for PF SR-IOV configuration.

use core::sync::atomic::{
    AtomicBool,
    Ordering, //
};
use kernel::{
    bindings,
    ffi::{
        c_char,
        c_int, //
    },
    module_param::KernelParam, //
};

static ENABLED: AtomicBool = AtomicBool::new(false);

pub(super) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// # Safety
///
/// A non-null `value` must point to a readable NUL-terminated string.
unsafe extern "C" fn set(value: *const c_char, _param: *const bindings::kernel_param) -> c_int {
    let mut enabled = true;
    if !value.is_null() {
        // SAFETY: `value` is a readable NUL-terminated string, and the local boolean is writable.
        let ret = unsafe { bindings::kstrtobool(value, &mut enabled) };
        if ret != 0 {
            return ret;
        }
    }

    ENABLED.store(enabled, Ordering::Relaxed);
    0
}

/// # Safety
///
/// `buffer` must point to the writable buffer supplied by the parameter subsystem.
unsafe extern "C" fn get(buffer: *mut c_char, _param: *const bindings::kernel_param) -> c_int {
    let mut enabled = enabled();
    let param = bindings::kernel_param {
        __bindgen_anon_1: bindings::kernel_param__bindgen_ty_1 {
            arg: (&raw mut enabled).cast(),
        },
        ..Default::default()
    };

    // SAFETY: The parameter subsystem supplies a 4 KiB writable buffer. `param_get_bool` only
    // reads `param.arg`, which points to the local boolean for the duration of this call.
    unsafe { bindings::param_get_bool(buffer, &param) }
}

static OPS: bindings::kernel_param_ops = bindings::kernel_param_ops {
    flags: bindings::KERNEL_PARAM_OPS_FL_NOARG,
    set: Some(set),
    get: Some(get),
    free: None,
};

#[used]
#[unsafe(link_section = "__param")]
static PARAM: KernelParam = KernelParam::new(bindings::kernel_param {
    name: kernel::str::as_char_ptr_in_const_context(if cfg!(MODULE) {
        c"enable_sriov"
    } else {
        c"nvidia_vgpu_vfio_pci.enable_sriov"
    }),
    mod_: kernel::module::this_module::<crate::LocalModule>().as_ptr(),
    ops: &OPS,
    perm: 0o644,
    level: -1,
    flags: 0,
    __bindgen_anon_1: bindings::kernel_param__bindgen_ty_1 {
        arg: core::ptr::null_mut(),
    },
});

#[cfg(MODULE)]
#[used]
#[unsafe(link_section = ".modinfo")]
static PARAM_TYPE: [u8; 27] = *b"parmtype=enable_sriov:bool\0";

#[cfg(not(MODULE))]
#[used]
#[unsafe(link_section = ".modinfo")]
static PARAM_TYPE: [u8; 48] = *b"nvidia_vgpu_vfio_pci.parmtype=enable_sriov:bool\0";

#[cfg(MODULE)]
#[used]
#[unsafe(link_section = ".modinfo")]
static PARAM_DESCRIPTION: [u8; 61] =
    *b"parm=enable_sriov:Enable SR-IOV for PFs bound to this driver\0";

#[cfg(not(MODULE))]
#[used]
#[unsafe(link_section = ".modinfo")]
static PARAM_DESCRIPTION: [u8; 82] =
    *b"nvidia_vgpu_vfio_pci.parm=enable_sriov:Enable SR-IOV for PFs bound to this driver\0";
