// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM GEM API
//!
//! C header: [`include/drm/drm_gem.h`](srctree/include/drm/drm_gem.h)

use crate::{
    bindings,
    drm::{
        self,
        device::{
            DeviceContext,
            Normal, //
        },
        driver::{
            AllocImpl,
            AllocOps, //
        },
    },
    error::{
        from_result,
        to_result, //
    },
    page::PAGE_SIZE,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque,
};
use core::{
    marker::PhantomData,
    ops::Deref,
    ptr::NonNull, //
};

mod mapping;
pub use mapping::{
    Fault,
    FaultResult,
    Mmap, //
};

#[cfg(CONFIG_RUST_DRM_GEM_SHMEM_HELPER)]
pub mod shmem;

/// A macro for implementing [`AlwaysRefCounted`] for any GEM object type.
///
/// Since all GEM objects use the same refcounting scheme.
#[macro_export]
macro_rules! impl_aref_for_gem_obj {
    (
        impl $( <$( $tparam_id:ident ),+> )? for $type:ty
        $(
            where
                $( $bind_param:path : $bind_trait:path ),+
        )?
    ) => {
        // SAFETY: All GEM objects are refcounted.
        unsafe impl $( <$( $tparam_id ),+> )? $crate::sync::aref::AlwaysRefCounted for $type
        where
            Self: IntoGEMObject,
            $( $( $bind_param : $bind_trait ),+ )?
        {
            fn inc_ref(&self) {
                // SAFETY: The existence of a shared reference guarantees that the refcount is
                // non-zero.
                unsafe { bindings::drm_gem_object_get(self.as_raw()) };
            }

            unsafe fn dec_ref(obj: core::ptr::NonNull<Self>) {
                // SAFETY: `obj` is a valid pointer to an `Object<T>`.
                let obj = unsafe { obj.as_ref() }.as_raw();

                // SAFETY: The safety requirements guarantee that the refcount is non-zero.
                unsafe { bindings::drm_gem_object_put(obj) };
            }
        }
    };
}
#[cfg_attr(not(CONFIG_RUST_DRM_GEM_SHMEM_HELPER), allow(unused))]
pub(crate) use impl_aref_for_gem_obj;

/// A type alias for retrieving a [`Driver`]s [`DriverFile`] implementation from its
/// [`DriverObject`] implementation.
///
/// [`Driver`]: drm::Driver
/// [`DriverFile`]: drm::file::DriverFile
pub type DriverFile<T> = drm::File<<<T as DriverObject>::Driver as drm::Driver>::File>;

/// A type alias for retrieving the current [`AllocImpl`] for a given [`DriverObject`].
///
/// [`Driver`]: drm::Driver
pub type DriverAllocImpl<T> = <<T as DriverObject>::Driver as drm::Driver>::Object;

/// Arguments for a driver's dumb-buffer allocation callback.
///
/// The callback owns the writable ioctl arguments for this wrapper's lifetime.
pub struct DumbCreate<'a> {
    dev: *mut bindings::drm_device,
    args: &'a mut bindings::drm_mode_create_dumb,
}

impl DumbCreate<'_> {
    /// Calculates pitch and size using byte alignments required by the hardware.
    ///
    /// Zero selects no extra pitch alignment or the default page-size alignment.
    pub fn calculate_size(&mut self, hw_pitch_align: usize, hw_size_align: usize) -> Result {
        // SAFETY: The callback keeps dev alive and gives us exclusive ioctl arguments.
        to_result(unsafe {
            bindings::drm_mode_size_dumb(self.dev, self.args, hw_pitch_align, hw_size_align)
        })
    }

    /// Returns the allocation size in bytes after [`Self::calculate_size`].
    pub fn size(&self) -> u64 {
        self.args.size
    }

    /// Records the handle created for the requesting file.
    pub fn set_handle(&mut self, handle: u32) {
        self.args.handle = handle;
    }
}

/// GEM object functions, which must be implemented by drivers.
pub trait DriverObject: Sync + Send + Sized + 'static {
    /// Parent `Driver` for this object.
    type Driver: drm::Driver;

    /// The data type to use for passing arguments to [`DriverObject::new`].
    type Args;

    /// Create a new driver data object for a GEM object of a given size.
    fn new(
        dev: &drm::Device<Self::Driver>,
        size: usize,
        args: Self::Args,
    ) -> impl PinInit<Self, Error>;

    /// Allocates a dumb buffer and records its handle in the ioctl arguments.
    fn dumb_create(
        _dev: &drm::Device<Self::Driver>,
        _file: &DriverFile<Self>,
        _args: &mut DumbCreate<'_>,
    ) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Admits a shared, write-combined I/O mapping without installing any PTEs.
    ///
    /// Device-resource validity must be checked again by [`Self::fault`].
    fn mmap(_obj: &DriverAllocImpl<Self>, _mapping: &Mmap<'_>) -> Result {
        Err(EOPNOTSUPP)
    }

    /// Handles a fault in a previously admitted mapping.
    ///
    /// The GEM reference keeps driver data alive, not the device binding. Keep
    /// the device's unplug guard across any call to [`Fault::insert_pfn`].
    fn fault(_obj: &DriverAllocImpl<Self>, _fault: &Fault<'_>) -> FaultResult {
        FaultResult::SIGBUS
    }

    /// Open a new handle to an existing object, associated with a File.
    fn open(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) -> Result {
        Ok(())
    }

    /// Close a handle to an existing object, associated with a File.
    fn close(_obj: &DriverAllocImpl<Self>, _file: &DriverFile<Self>) {}
}

/// Trait that represents a GEM object subtype
pub trait IntoGEMObject: Sized + super::private::Sealed {
    /// Returns a reference to the raw `drm_gem_object` structure, which must be valid as long as
    /// this owning object is valid.
    fn as_raw(&self) -> *mut bindings::drm_gem_object;

    /// Converts a pointer to a `struct drm_gem_object` into a reference to `Self`.
    ///
    /// # Safety
    ///
    /// - `self_ptr` must be a valid pointer to the `struct drm_gem_object` embedded in a
    ///   valid instance of `Self`.
    /// - The caller promises that holding the immutable reference returned by this function does
    ///   not violate rust's data aliasing rules and remains valid throughout the lifetime of `'a`.
    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self;
}

extern "C" fn open_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) -> core::ffi::c_int {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY:
    // * `open_callback` is specified in the AllocOps structure for `DriverObject`, ensuring that
    //   `raw_obj` is contained within a `DriverAllocImpl<T>`
    // * It is only possible for `open_callback` to be called after device registration, ensuring
    //   that the object's device is in the `Registered` state.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    match T::open(obj, file) {
        Err(e) => e.to_errno(),
        Ok(()) => 0,
    }
}

extern "C" fn close_callback<T: DriverObject>(
    raw_obj: *mut bindings::drm_gem_object,
    raw_file: *mut bindings::drm_file,
) {
    // SAFETY: `open_callback` is only ever called with a valid pointer to a `struct drm_file`.
    let file = unsafe { DriverFile::<T>::from_raw(raw_file) };

    // SAFETY: `close_callback` is specified in the AllocOps structure for `Object<T>`, ensuring
    // that `raw_obj` is indeed contained within a `Object<T>`.
    let obj: &DriverAllocImpl<T> = unsafe { IntoGEMObject::from_raw(raw_obj) };

    T::close(obj, file);
}

impl<T: DriverObject, Ctx: DeviceContext> IntoGEMObject for Object<T, Ctx> {
    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    unsafe fn from_raw<'a>(self_ptr: *mut bindings::drm_gem_object) -> &'a Self {
        // SAFETY: `obj` is guaranteed to be in an `Object<T>` via the safety contract of this
        // function
        unsafe { &*crate::container_of!(Opaque::cast_from(self_ptr), Object<T, Ctx>, obj) }
    }
}

/// Base operations shared by all GEM object classes
pub trait BaseObject: IntoGEMObject {
    /// Returns the size of the object in bytes.
    fn size(&self) -> usize {
        // SAFETY: `self.as_raw()` is guaranteed to be a pointer to a valid `struct drm_gem_object`.
        unsafe { (*self.as_raw()).size }
    }

    /// Creates a new handle for the object associated with a given `File`
    /// (or returns an existing one).
    fn create_handle<D, F>(&self, file: &drm::File<F>) -> Result<u32>
    where
        Self: AllocImpl<Driver = D>,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        let mut handle: u32 = 0;
        // SAFETY: The arguments are all valid per the type invariants.
        to_result(unsafe {
            bindings::drm_gem_handle_create(file.as_raw().cast(), self.as_raw(), &mut handle)
        })?;
        Ok(handle)
    }

    /// Looks up an object by its handle for a given `File`.
    fn lookup_handle<D, F>(file: &drm::File<F>, handle: u32) -> Result<ARef<Self>>
    where
        Self: AllocImpl<Driver = D> + AlwaysRefCounted,
        D: drm::Driver<Object = Self, File = F>,
        F: drm::file::DriverFile<Driver = D>,
    {
        // SAFETY: The arguments are all valid per the type invariants.
        let ptr = unsafe { bindings::drm_gem_object_lookup(file.as_raw().cast(), handle) };
        if ptr.is_null() {
            return Err(ENOENT);
        }

        // SAFETY:
        // - A `drm::Driver` can only have a single `File` implementation.
        // - `file` uses the same `drm::Driver` as `Self`.
        // - Therefore, we're guaranteed that `ptr` must be a gem object embedded within `Self`.
        // - And we check if the pointer is null befoe calling from_raw(), ensuring that `ptr` is a
        //   valid pointer to an initialized `Self`.
        let obj = unsafe { Self::from_raw(ptr) };

        // SAFETY:
        // - We take ownership of the reference of `drm_gem_object_lookup()`.
        // - Our `NonNull` comes from an immutable reference, thus ensuring it is a valid pointer to
        //   `Self`.
        Ok(unsafe { ARef::from_raw(obj.into()) })
    }

    /// Creates an mmap offset to map the object from userspace.
    fn create_mmap_offset(&self) -> Result<u64> {
        // SAFETY: The arguments are valid per the type invariant.
        to_result(unsafe { bindings::drm_gem_create_mmap_offset(self.as_raw()) })?;

        // SAFETY: The arguments are valid per the type invariant.
        Ok(unsafe { bindings::drm_vma_node_offset_addr(&raw mut (*self.as_raw()).vma_node) })
    }
}

impl<T: IntoGEMObject> BaseObject for T {}

/// Crate-private base operations shared by all GEM object classes.
#[cfg_attr(not(CONFIG_RUST_DRM_GEM_SHMEM_HELPER), expect(unused))]
pub(crate) trait BaseObjectPrivate: IntoGEMObject {
    /// Return a pointer to this object's dma_resv.
    fn raw_dma_resv(&self) -> *mut bindings::dma_resv {
        // SAFETY: `self.as_raw()` always returns a valid pointer to the base DRM GEM object.
        unsafe { (*self.as_raw()).resv }
    }
}

impl<T: IntoGEMObject> BaseObjectPrivate for T {}

/// A base GEM object.
///
/// # Invariants
///
/// * `self.obj` is a valid instance of a `struct drm_gem_object`.
/// * Any type invariants of `Ctx` apply to the parent DRM device for this GEM object.
#[repr(C)]
#[pin_data]
pub struct Object<T: DriverObject + Send + Sync, Ctx: DeviceContext = Normal> {
    obj: Opaque<bindings::drm_gem_object>,
    #[pin]
    data: T,
    // Keep the device alive through GEM release and driver-data destruction.
    dev: ARef<drm::Device<T::Driver>>,
    _ctx: PhantomData<Ctx>,
}

// SAFETY: GEM reference counting and mutable C state have their own synchronization.
// DriverObject and DeviceContext require Send; no binding capability is added.
unsafe impl<T: DriverObject, Ctx: DeviceContext> Send for Object<T, Ctx> {}

// SAFETY: Shared GEM operations use DRM's synchronization, and driver data is Sync.
unsafe impl<T: DriverObject, Ctx: DeviceContext> Sync for Object<T, Ctx> {}

impl<T: DriverObject, Ctx: DeviceContext> Object<T, Ctx> {
    const OBJECT_FUNCS: bindings::drm_gem_object_funcs = bindings::drm_gem_object_funcs {
        free: Some(Self::free_callback),
        open: Some(open_callback::<T>),
        close: Some(close_callback::<T>),
        mmap: Some(mapping::mmap_callback::<T>),
        vm_ops: &mapping::VmaOps::<T>::OPS,
        ..pin_init::zeroed()
    };

    /// Returns the `Device` that owns this GEM object.
    pub fn dev(&self) -> &drm::Device<T::Driver, Ctx> {
        // SAFETY: The retained reference keeps this constructor's device alive.
        // Object's Ctx invariant supplies the context guarantee; the retained
        // ordinary reference itself does not establish a device binding.
        unsafe { self.dev.assume_ctx() }
    }

    fn as_raw(&self) -> *mut bindings::drm_gem_object {
        self.obj.get()
    }

    extern "C" fn free_callback(obj: *mut bindings::drm_gem_object) {
        let ptr: *mut Opaque<bindings::drm_gem_object> = obj.cast();

        // SAFETY: All of our objects are of type `Object<T>`.
        let this = unsafe { crate::container_of!(ptr, Self, obj) };

        // SAFETY: The C code only ever calls this callback with a valid pointer to a `struct
        // drm_gem_object`.
        unsafe { bindings::drm_gem_object_release(obj) };

        // SAFETY: All of our objects are allocated via `KBox`, and we're in the
        // free callback which guarantees this object has zero remaining references,
        // so we can drop it.
        let _ = unsafe { KBox::from_raw(this) };
    }
}

impl<T: DriverObject> Object<T>
where
    T::Driver: drm::Driver<Object = Self>,
{
    /// Create a new GEM object.
    pub fn new(dev: &drm::Device<T::Driver>, size: usize, args: T::Args) -> Result<ARef<Self>> {
        Self::new_inner(dev, size, args, false)
    }

    /// Creates a GEM object whose backing storage is supplied by the driver.
    pub fn new_private(
        dev: &drm::Device<T::Driver>,
        size: usize,
        args: T::Args,
    ) -> Result<ARef<Self>> {
        Self::new_inner(dev, size, args, true)
    }

    fn new_inner(
        dev: &drm::Device<T::Driver>,
        size: usize,
        args: T::Args,
        private: bool,
    ) -> Result<ARef<Self>> {
        if size == 0 || !size.is_multiple_of(PAGE_SIZE) {
            return Err(EINVAL);
        }
        let obj: Pin<KBox<Self>> = KBox::pin_init(
            try_pin_init!(Self {
                obj: Opaque::new(bindings::drm_gem_object::default()),
                data <- T::new(dev, size, args),
                dev: dev.into(),
                _ctx: PhantomData,
            }),
            GFP_KERNEL,
        )?;

        // SAFETY: `obj.as_raw()` is guaranteed to be valid by the initialization above.
        unsafe { (*obj.as_raw()).funcs = &Self::OBJECT_FUNCS };

        // INVARIANT: `dev` and the GEM object are in the same state at the moment, and upgrading
        // the typestate in `dev` will not carry over to the GEM object.
        let result = if private {
            // SAFETY: The allocation is pinned, dev is live, and size is page-aligned.
            unsafe { bindings::drm_gem_private_object_init(dev.as_raw(), obj.obj.get(), size) };
            Ok(())
        } else {
            // SAFETY: The same initialized allocation and page-aligned size apply here.
            to_result(unsafe { bindings::drm_gem_object_init(dev.as_raw(), obj.obj.get(), size) })
        };
        if let Err(err) = result {
            // SAFETY: `drm_gem_object_init()` initializes the private GEM object state before
            // failing, so `drm_gem_private_object_fini()` is the matching cleanup.
            unsafe { bindings::drm_gem_private_object_fini(obj.obj.get()) };
            return Err(err);
        }

        // SAFETY: We will never move out of `Self` as `ARef<Self>` is always treated as pinned.
        let ptr = KBox::into_raw(unsafe { Pin::into_inner_unchecked(obj) });

        // SAFETY: `ptr` comes from `KBox::into_raw` and hence can't be NULL.
        let ptr = unsafe { NonNull::new_unchecked(ptr) };

        // SAFETY: We take over the initial reference count from the GEM initializer.
        Ok(unsafe { ARef::from_raw(ptr) })
    }
}

impl_aref_for_gem_obj! {
    impl<T> for Object<T>
    where
        T: DriverObject
}

impl<T: DriverObject, Ctx: DeviceContext> super::private::Sealed for Object<T, Ctx> {}

impl<T: DriverObject, Ctx: DeviceContext> Deref for Object<T, Ctx> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

// Keep PRIME disabled: a missing driver callback invokes DRM's GEM fallback.
extern "C" fn prime_handle_to_fd_callback(
    _dev: *mut bindings::drm_device,
    _file: *mut bindings::drm_file,
    _handle: u32,
    _flags: u32,
    _prime_fd: *mut c_int,
) -> c_int {
    EOPNOTSUPP.to_errno()
}

extern "C" fn prime_fd_to_handle_callback(
    _dev: *mut bindings::drm_device,
    _file: *mut bindings::drm_file,
    _prime_fd: c_int,
    _handle: *mut u32,
) -> c_int {
    EOPNOTSUPP.to_errno()
}

extern "C" fn dumb_create_callback<T: DriverObject>(
    raw_file: *mut bindings::drm_file,
    raw_dev: *mut bindings::drm_device,
    raw_args: *mut bindings::drm_mode_create_dumb,
) -> c_int {
    from_result(|| {
        // SAFETY: DRM calls this driver's callback with a live device and file.
        let dev = unsafe { drm::Device::<T::Driver>::from_raw(raw_dev) };
        // SAFETY: The file belongs to the same driver and lives through this callback.
        let file = unsafe { DriverFile::<T>::from_raw(raw_file) };
        let mut args = DumbCreate {
            dev: raw_dev,
            // SAFETY: These kernel ioctl arguments are exclusive to this invocation.
            args: unsafe { &mut *raw_args },
        };
        T::dumb_create(dev, file, &mut args)?;
        Ok(0)
    })
}

impl<T: DriverObject, Ctx: DeviceContext> AllocImpl for Object<T, Ctx> {
    type Driver = T::Driver;

    const ALLOC_OPS: AllocOps = AllocOps {
        gem_create_object: None,
        prime_handle_to_fd: Some(prime_handle_to_fd_callback),
        prime_fd_to_handle: Some(prime_fd_to_handle_callback),
        gem_prime_import: None,
        gem_prime_import_sg_table: None,
        dumb_create: Some(dumb_create_callback::<T>),
        dumb_map_offset: None,
        #[cfg(CONFIG_RUST_DRM_KMS)]
        fbdev_probe: None,
    };
}

pub(super) const fn create_fops(owner: *mut bindings::module) -> bindings::file_operations {
    let mut fops: bindings::file_operations = pin_init::zeroed();

    fops.owner = owner;
    fops.open = Some(bindings::drm_open);
    fops.release = Some(bindings::drm_release);
    fops.unlocked_ioctl = Some(bindings::drm_ioctl);
    #[cfg(CONFIG_COMPAT)]
    {
        fops.compat_ioctl = Some(bindings::drm_compat_ioctl);
    }
    fops.poll = Some(bindings::drm_poll);
    fops.read = Some(bindings::drm_read);
    fops.llseek = Some(bindings::noop_llseek);
    fops.mmap = Some(bindings::drm_gem_mmap);
    fops.fop_flags = bindings::FOP_UNSIGNED_OFFSET;

    fops
}
