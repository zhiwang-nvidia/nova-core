// SPDX-License-Identifier: GPL-2.0-only

//! Abstractions for the fwctl.
//!
//! This module provides bindings for working with fwctl devices in kernel modules.
//!
//! C header: [`include/linux/fwctl.h`]

use crate::{
    bindings,
    container_of,
    device,
    devres::Devres,
    prelude::*,
    types::{
        ARef,
        Opaque, //
    }, //
};
use core::{
    marker::PhantomData,
    ptr::NonNull,
    slice, //
};

/// Represents a fwctl device type.
///
/// This enum corresponds to the C `enum fwctl_device_type` and is used to identify
/// the specific firmware control interface implemented by a device.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum DeviceType {
    /// Error/invalid device type.
    Error = bindings::fwctl_device_type_FWCTL_DEVICE_TYPE_ERROR,
    /// MLX5 device type.
    Mlx5 = bindings::fwctl_device_type_FWCTL_DEVICE_TYPE_MLX5,
    /// CXL device type.
    Cxl = bindings::fwctl_device_type_FWCTL_DEVICE_TYPE_CXL,
    /// PDS device type.
    Pds = bindings::fwctl_device_type_FWCTL_DEVICE_TYPE_PDS,
    /// Rust fwctl test device type.
    RustFwctlTest = bindings::fwctl_device_type_FWCTL_DEVICE_TYPE_RUST_FWCTL_TEST,
}

impl From<DeviceType> for u32 {
    fn from(device_type: DeviceType) -> Self {
        device_type as u32
    }
}

/// A fwctl device.
///
/// Wraps the C `struct fwctl_device` and manages its reference count.
///
/// # Invariants
///
/// Instances of this type represent a valid `struct fwctl_device` created by the C portion
/// of the kernel.
#[repr(transparent)]
pub struct Device(Opaque<bindings::fwctl_device>);

impl Device {
    /// # Safety
    ///
    /// `ptr` must be a valid pointer to a `struct fwctl_device`.
    unsafe fn from_raw<'a>(ptr: *mut bindings::fwctl_device) -> &'a Self {
        // CAST: `Self` is a transparent wrapper around `bindings::fwctl_device`.
        // SAFETY: By the safety requirement, `ptr` is valid.
        unsafe { &*ptr.cast() }
    }

    fn as_raw(&self) -> *mut bindings::fwctl_device {
        self.0.get()
    }

    /// Returns the parent device.
    pub fn parent(&self) -> &device::Device {
        // SAFETY: By the type invariant, `self.as_raw()` is a valid pointer to a
        // `struct fwctl_device`, which always has a parent device.
        let parent_dev = unsafe { (*self.as_raw()).dev.parent };
        // SAFETY: `parent_dev` points to a valid `struct device`. The parent device
        // is guaranteed to be valid for the lifetime of the fwctl_device.
        unsafe { device::Device::from_raw(parent_dev) }
    }
}

impl AsRef<device::Device> for Device {
    fn as_ref(&self) -> &device::Device {
        // SAFETY: By the type invariant of `Self`, `self.as_raw()` is a pointer to a valid
        // `struct fwctl_device`.
        let dev = unsafe { core::ptr::addr_of_mut!((*self.as_raw()).dev) };

        // SAFETY: `dev` points to a valid `struct device`.
        unsafe { device::Device::from_raw(dev) }
    }
}

// SAFETY: The fwctl_device is reference counted through the embedded `struct device`,
// and inc_ref/dec_ref use fwctl_get/fwctl_put to manage its lifetime.
unsafe impl crate::sync::aref::AlwaysRefCounted for Device {
    fn inc_ref(&self) {
        // SAFETY: The existence of a shared reference guarantees that the refcount is non-zero.
        // `self.as_raw()` is a valid pointer to a `struct fwctl_device`.
        unsafe { bindings::fwctl_get(self.as_raw()) };
    }

    unsafe fn dec_ref(obj: NonNull<Self>) {
        // CAST: `Self` is a transparent wrapper of `bindings::fwctl_device`.
        let fwctl: *mut bindings::fwctl_device = obj.cast().as_ptr();

        // SAFETY: By the type invariant, `fwctl` is a valid pointer to a `struct fwctl_device`.
        unsafe { bindings::fwctl_put(fwctl) };
    }
}

// SAFETY: A `Device` is always reference-counted and can be released from any thread.
unsafe impl Send for Device {}

// SAFETY: `Device` can be shared among threads because all methods are thread-safe.
unsafe impl Sync for Device {}

/// The registration of a fwctl device.
///
/// This type represents the registration of a [`struct fwctl_device`]. It should always be
/// used within a [`Devres`] wrapper to ensure proper lifetime management. When dropped,
/// the fwctl device will be unregistered and freed.
///
/// [`Devres`] guarantees that the device is unregistered before the parent device is unbound.
///
/// [`struct fwctl_device`]: srctree/include/linux/device/fwctl.h
pub struct Registration<T: Operations> {
    device: ARef<Device>,
    _marker: PhantomData<T>,
}

impl<T: Operations> Registration<T> {
    /// Allocate and register a new fwctl device under the given parent device.
    ///
    /// The returned [`Devres`] wrapper ensures that the fwctl device is unregistered
    /// before the parent device is unbound.
    pub fn new<'a>(
        parent: &'a device::Device<device::Bound>,
    ) -> impl PinInit<Devres<Self>, Error> + 'a
    where
        T: 'a,
    {
        pin_init::pin_init_scope(move || {
            let ops = core::ptr::from_ref::<bindings::fwctl_ops>(&VTable::<T>::VTABLE).cast_mut();

            // SAFETY: `_fwctl_alloc_device()` allocates a new `fwctl_device`
            // and initializes its embedded `struct device`. The `ops` pointer
            // points to a static VTABLE that outlives the device. The parent
            // device is guaranteed to be bound to a driver (Device<Bound>),
            // ensuring it remains valid during allocation.
            let dev = unsafe {
                bindings::_fwctl_alloc_device(
                    parent.as_raw(),
                    ops,
                    core::mem::size_of::<bindings::fwctl_device>(),
                )
            };

            if dev.is_null() {
                return Err(ENOMEM);
            }

            // SAFETY: dev is guaranteed to be a valid pointer from `_fwctl_alloc_device()`.
            let ret = unsafe { bindings::fwctl_register(dev) };
            if ret != 0 {
                // SAFETY: dev is guaranteed to be a valid pointer from `_fwctl_alloc_device()`.
                unsafe {
                    bindings::fwctl_put(dev);
                }
                return Err(Error::from_errno(ret));
            }

            // SAFETY: dev is guaranteed to be a valid pointer from `_fwctl_alloc_device()`.
            let device = unsafe {
                let dev_ref = Device::from_raw(dev);
                // SAFETY: We just verified dev is non-null above, and Device::from_raw
                // returns a reference, so NonNull::new_unchecked is safe.
                ARef::from_raw(NonNull::new_unchecked(
                    core::ptr::from_ref(dev_ref).cast_mut(),
                ))
            };

            Ok(Devres::new(
                parent,
                Self {
                    device,
                    _marker: PhantomData,
                },
            ))
        })
    }

    fn as_raw(&self) -> *mut bindings::fwctl_device {
        self.device.as_raw()
    }
}

impl<T: Operations> Drop for Registration<T> {
    fn drop(&mut self) {
        // SAFETY: `fwctl_unregister()` expects a valid registered device.
        // By the type invariant, `self.device` holds a valid fwctl_device.
        unsafe {
            bindings::fwctl_unregister(self.as_raw());
        }
        // The ARef<Device> will automatically call fwctl_put() when dropped.
    }
}

// SAFETY: `Registration` can be sent to other threads because:
// - It only contains a `NonNull<fwctl_device>` pointer and a PhantomData marker
// - The underlying C fwctl_device is thread-safe with internal locking
// - `Drop` calls `fwctl_unregister()/fwctl_put()` which are safe from any sleepable context
unsafe impl<T: Operations> Send for Registration<T> {}

// SAFETY: `Registration` can be shared between threads because:
// - It provides no methods for mutation (except Drop, which takes &mut self)
// - The underlying C fwctl_device is protected by internal locking (registration_lock)
// - Multiple threads can safely hold immutable references to the same Registration
unsafe impl<T: Operations> Sync for Registration<T> {}

/// Trait implemented by each Rust driver that integrates with the fwctl subsystem.
///
/// Each implementation corresponds to a specific device type and provides
/// the vtable used by the core `fwctl` layer to manage per-FD user contexts
/// and handle RPC requests.
pub trait Operations: Sized {
    /// Driver user context type.
    type UserCtx;

    /// fwctl device type.
    const DEVICE_TYPE: DeviceType;

    /// Called when a new user context is opened by userspace.
    fn open(
        fwctl_uctx: &Opaque<bindings::fwctl_uctx>,
    ) -> Result<impl PinInit<Self::UserCtx, Error>, Error>;

    /// Called when the user context is being closed.
    fn close(uctx: &mut UserCtx<Self::UserCtx>);

    /// Return device or context information to userspace.
    fn info(uctx: &mut UserCtx<Self::UserCtx>) -> Result<KVec<u8>, Error>;

    /// Called when a userspace RPC request is received.
    fn fw_rpc(
        uctx: &mut UserCtx<Self::UserCtx>,
        scope: u32,
        rpc_in: &mut [u8],
        out_len: *mut usize,
    ) -> Result<Option<KVec<u8>>, Error>;
}

/// Represents a per-FD user context (`struct fwctl_uctx`).
#[repr(C)]
#[pin_data]
pub struct UserCtx<T> {
    /// The core fwctl user context shared with the C implementation.
    #[pin]
    fwctl_uctx: Opaque<bindings::fwctl_uctx>,

    /// Driver-specific data associated with this user context.
    #[pin]
    uctx: T,
}

impl<T> UserCtx<T> {
    /// Converts a raw C pointer to `struct fwctl_uctx` into a reference to the
    /// enclosing `UserCtx<T>`.
    ///
    /// # Safety
    /// * `ptr` must be a valid pointer to a `fwctl_uctx` that is embedded
    ///   inside an existing `UserCtx<T>` instance.
    /// * The caller must ensure that the lifetime of the returned reference
    ///   does not outlive the underlying object managed on the C side.
    unsafe fn from_raw<'a>(ptr: *mut bindings::fwctl_uctx) -> &'a mut Self {
        // SAFETY: `ptr` was originally created from a valid `UserCtx<T>`.
        // We cast through `Opaque` since `fwctl_uctx` is wrapped in `Opaque`.
        unsafe { &mut *container_of!(Opaque::cast_from(ptr), UserCtx<T>, fwctl_uctx).cast_mut() }
    }

    /// Returns a reference to the parent device from a raw `fwctl_uctx` pointer.
    pub fn parent_device_from_raw(
        fwctl_uctx: &Opaque<bindings::fwctl_uctx>,
    ) -> &device::Device<device::Bound> {
        // SAFETY: `fwctl_uctx` is initialized by the fwctl subsystem
        // and guaranteed to remain valid.
        let raw_fwctl = unsafe { (*fwctl_uctx.get()).fwctl };
        // SAFETY: `raw_fwctl` is a valid pointer to a `fwctl_device`, and its `dev.parent`
        // field points to a valid parent device.
        let raw_dev = unsafe { (*raw_fwctl).dev.parent };

        // SAFETY: `raw_dev` points to a live device object.
        let dev: &device::Device = unsafe { device::Device::from_raw(raw_dev) };

        // SAFETY: The device is guaranteed to be bound.
        unsafe { dev.as_bound() }
    }

    /// Returns a reference to the parent device of this user context.
    pub fn get_parent_device(&self) -> &device::Device<device::Bound> {
        Self::parent_device_from_raw(&self.fwctl_uctx)
    }
}

impl<T> core::ops::Deref for UserCtx<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.uctx
    }
}

impl<T> core::ops::DerefMut for UserCtx<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.uctx
    }
}

/// Static vtable mapping Rust trait methods to C callbacks.
pub struct VTable<T: Operations>(PhantomData<T>);

impl<T: Operations> VTable<T> {
    /// Static instance of `fwctl_ops` used by the C core to call into Rust.
    pub const VTABLE: bindings::fwctl_ops = bindings::fwctl_ops {
        device_type: T::DEVICE_TYPE as u32,
        uctx_size: core::mem::size_of::<UserCtx<T::UserCtx>>(),
        open_uctx: Some(Self::open_uctx_callback),
        close_uctx: Some(Self::close_uctx_callback),
        info: Some(Self::info_callback),
        fw_rpc: Some(Self::fw_rpc_callback),
    };

    /// Called when a new user context is opened by userspace.
    /// # Safety
    ///
    /// `uctx` must be a valid pointer to an initialized `fwctl_uctx` structure,
    /// embedded within a C-allocated `UserCtx<T::UserCtx>` with sufficient space.
    unsafe extern "C" fn open_uctx_callback(uctx: *mut bindings::fwctl_uctx) -> ffi::c_int {
        // SAFETY: `uctx` points to valid, initialized `fwctl_uctx` structure.
        let fwctl_uctx_ref = unsafe { &*Opaque::cast_from(uctx) };

        let initializer = match T::open(fwctl_uctx_ref) {
            Ok(init) => init,
            Err(e) => return e.to_errno(),
        };

        let uctx_offset = core::mem::offset_of!(UserCtx<T::UserCtx>, uctx);

        // SAFETY: The C side allocated enough space for the entire UserCtx.
        let uctx_ptr: *mut T::UserCtx = unsafe { uctx.cast::<u8>().add(uctx_offset).cast() };

        // Initialize the uctx field in-place using the pin initializer.
        // SAFETY:
        // - uctx_ptr points to valid allocated memory
        // - The memory is properly aligned (guaranteed by #[repr(C)] and our compile-time check)
        // - The memory is uninitialized, which is what PinInit expects
        // - After this call, the memory will be properly initialized
        match unsafe { initializer.__pinned_init(uctx_ptr.cast()) } {
            Ok(()) => 0,
            Err(e) => e.to_errno(),
        }
    }

    /// Called when the user context is being closed.
    /// # Safety
    ///
    /// `uctx` must be a valid pointer to an initialized `fwctl_uctx` structure,
    /// embedded within a fully initialized `UserCtx<T::UserCtx>`.
    unsafe extern "C" fn close_uctx_callback(uctx: *mut bindings::fwctl_uctx) {
        // SAFETY: `uctx` is guaranteed by the fwctl subsystem to be a valid pointer.
        let ctx = unsafe { UserCtx::<T::UserCtx>::from_raw(uctx) };
        T::close(ctx);
    }

    /// Returns device or context information.
    /// # Safety
    ///
    /// - `uctx` must be a valid pointer to an initialized `fwctl_uctx` structure,
    ///   embedded within a fully initialized `UserCtx<T::UserCtx>`.
    /// - `length` must be a valid pointer to write the output length.
    unsafe extern "C" fn info_callback(
        uctx: *mut bindings::fwctl_uctx,
        length: *mut usize,
    ) -> *mut ffi::c_void {
        // SAFETY: `uctx` is guaranteed by the fwctl subsystem to be a valid pointer.
        let ctx = unsafe { UserCtx::<T::UserCtx>::from_raw(uctx) };

        match T::info(ctx) {
            Ok(kvec) => {
                // The ownership of the buffer is now transferred to the foreign
                // caller. It must eventually be released by fwctl framework.
                let (ptr, len, _cap) = kvec.into_raw_parts();

                // SAFETY: `length` is a valid out-parameter provided by the C
                // caller. Write the number of bytes in the returned buffer.
                unsafe {
                    *length = len;
                }

                ptr.cast::<ffi::c_void>()
            }

            Err(e) => Error::to_ptr(e),
        }
    }

    /// Called when a user-space RPC request is received.
    /// # Safety
    ///
    /// - `uctx` must be a valid pointer to an initialized `fwctl_uctx` structure,
    ///   embedded within a fully initialized `UserCtx<T::UserCtx>`.
    /// - `rpc_in` must be a valid pointer to `in_len` bytes of readable/writable memory.
    /// - `out_len` must be a valid pointer to write the output length.
    unsafe extern "C" fn fw_rpc_callback(
        uctx: *mut bindings::fwctl_uctx,
        scope: u32,
        rpc_in: *mut ffi::c_void,
        in_len: usize,
        out_len: *mut usize,
    ) -> *mut ffi::c_void {
        // SAFETY: `uctx` is guaranteed by the fwctl framework to be a valid pointer.
        let ctx = unsafe { UserCtx::<T::UserCtx>::from_raw(uctx) };

        // SAFETY: Creating a mutable slice from `rpc_in`:
        // - `rpc_in` is non-null and properly aligned: guaranteed by the fwctl subsystem
        // - `rpc_in` points to `in_len` consecutive properly initialized bytes
        // - The memory is valid for reads and writes for the lifetime of the returned slice
        // - The total size `in_len` does not exceed `isize::MAX`: checked by the fwctl subsystem
        // - No other references to this memory exist during this callback
        let rpc_in_slice: &mut [u8] =
            unsafe { slice::from_raw_parts_mut(rpc_in.cast::<u8>(), in_len) };

        match T::fw_rpc(ctx, scope, rpc_in_slice, out_len) {
            // Driver allocates a new output buffer.
            Ok(Some(kvec)) => {
                // The ownership of the buffer is now transferred to the foreign
                // caller. It must eventually be released by fwctl subsystem.
                let (ptr, len, _cap) = kvec.into_raw_parts();

                // SAFETY: `out_len` is a valid writable pointer provided by the C caller.
                unsafe { *out_len = len };

                ptr.cast::<ffi::c_void>()
            }

            // Driver re-uses the existing input buffer and writes the out_len.
            Ok(None) => rpc_in,

            Err(e) => Error::to_ptr(e),
        }
    }
}
