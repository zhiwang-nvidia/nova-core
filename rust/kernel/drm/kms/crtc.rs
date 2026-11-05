// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM CRTCs.
//!
//! C header: [`include/drm/drm_crtc.h`](srctree/include/drm/drm_crtc.h)

use super::{
    atomic::*, plane::*, vblank::*, KmsDriver, ModeObject, ModeObjectVtable, StaticModeObject,
    UnregisteredKmsDevice,
};
use crate::{
    alloc::KBox,
    bindings,
    drm::device::Device,
    drm::private::Sealed,
    error::{from_result, to_result},
    prelude::*,
    types::{NotThreadSafe, Opaque},
};
use core::{
    cell::{Cell, UnsafeCell},
    marker::*,
    mem::{self, ManuallyDrop},
    ops::{Deref, DerefMut},
    ptr::{addr_of_mut, null, null_mut, NonNull},
};
use macros::vtable;

/// The main trait for implementing the [`struct drm_crtc`] API for [`Crtc`].
///
/// Any KMS driver should have at least one implementation of this type, which allows them to create
/// [`Crtc`] objects. Additionally, a driver may store driver-private data within the type that
/// implements [`DriverCrtc`] - and it will be made available when using a fully typed [`Crtc`]
/// object.
///
/// # Invariants
///
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_crtc`] pointers are contained within a [`Crtc<Self>`].
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_crtc_state`] pointers are contained within a [`CrtcState<Self::State>`].
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
#[vtable]
pub trait DriverCrtc: Send + Sync + Sized {
    /// The generated C vtable for this [`DriverCrtc`] implementation.
    #[unique]
    const OPS: &'static DriverCrtcOps = &DriverCrtcOps {
        funcs: bindings::drm_crtc_funcs {
            atomic_destroy_state: Some(atomic_destroy_state_callback::<Self::State>),
            atomic_duplicate_state: Some(atomic_duplicate_state_callback::<Self::State>),
            atomic_get_property: None,
            atomic_print_state: None,
            atomic_create_state: Some(crtc_create_state_callback::<Self::State>),
            atomic_set_property: None,
            cursor_move: None,
            cursor_set2: None,
            cursor_set: None,
            destroy: Some(crtc_destroy_callback::<Self>),
            disable_vblank: <Self::VblankImpl as VblankImpl>::VBLANK_OPS.disable_vblank,
            early_unregister: None,
            enable_vblank: <Self::VblankImpl as VblankImpl>::VBLANK_OPS.enable_vblank,
            gamma_set: None,
            get_crc_sources: None,
            get_vblank_counter: None,
            get_vblank_timestamp: <Self::VblankImpl as VblankImpl>::VBLANK_OPS.get_vblank_timestamp,
            late_register: None,
            page_flip: Some(bindings::drm_atomic_helper_page_flip),
            page_flip_target: None,
            set_config: Some(bindings::drm_atomic_helper_set_config),
            set_crc_source: None,
            set_property: None,
            verify_crc_source: None,
        },

        helper_funcs: bindings::drm_crtc_helper_funcs {
            atomic_disable: if Self::HAS_ATOMIC_DISABLE {
                Some(atomic_disable_callback::<Self>)
            } else {
                None
            },
            atomic_enable: if Self::HAS_ATOMIC_ENABLE {
                Some(atomic_enable_callback::<Self>)
            } else {
                None
            },
            atomic_check: if Self::HAS_ATOMIC_CHECK {
                Some(atomic_check_callback::<Self>)
            } else {
                None
            },
            hw_reset: None,
            dpms: None,
            commit: None,
            prepare: None,
            disable: None,
            mode_set: None,
            mode_valid: None,
            mode_fixup: None,
            atomic_begin: if Self::HAS_ATOMIC_BEGIN {
                Some(atomic_begin_callback::<Self>)
            } else {
                None
            },
            atomic_flush: if Self::HAS_ATOMIC_FLUSH {
                Some(atomic_flush_callback::<Self>)
            } else {
                None
            },
            mode_set_nofb: None,
            mode_set_base: None,
            handle_vblank_timeout: None,
            get_scanout_position: None,
        },
    };

    /// The type to pass to the `args` field of [`UnregisteredCrtc::new`].
    ///
    /// This type will be made available in the `args` argument of [`Self::new`]. Drivers which
    /// don't need this can simply pass `()` here.
    type Args;

    /// The parent [`KmsDriver`] implementation.
    type Driver: KmsDriver;

    /// The [`DriverCrtcState`] implementation for this [`DriverCrtc`].
    ///
    /// See [`DriverCrtcState`] for more info.
    type State: DriverCrtcState;

    /// The driver's optional hardware vblank implementation
    ///
    /// See [`VblankSupport`] for more info. Drivers that don't care about this can just pass
    /// [`PhantomData<Self>`].
    type VblankImpl: VblankImpl<Crtc = Self>;

    /// The constructor for creating a [`Crtc`] using this [`DriverCrtc`] implementation.
    ///
    /// Drivers may use this to instantiate their [`DriverCrtc`] object.
    fn new(device: &Device<Self::Driver>, args: &Self::Args) -> impl PinInit<Self, Error>;

    /// The optional [`drm_crtc_helper_funcs.atomic_check`] hook for this crtc.
    ///
    /// Drivers may use this to customize the atomic check phase of their [`Crtc`] objects. The
    /// result of this function determines whether the atomic check passed or failed.
    ///
    /// [`drm_crtc_helper_funcs.atomic_check`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_check(
        _crtc: &Crtc<Self>,
        _old_state: &CrtcState<Self::State>,
        _new_state: CrtcStateMutator<'_, CrtcState<Self::State>>,
        _state: &AtomicStateComposer<Self::Driver>,
    ) -> Result {
        build_error::build_error("This should not be reachable")
    }

    /// The optional [`drm_crtc_helper_funcs.atomic_begin`] hook.
    ///
    /// This hook will be called before a set of [`Plane`] updates are performed for the given
    /// [`Crtc`].
    ///
    /// [`drm_crtc_helper_funcs.atomic_begin`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_begin(
        _crtc: &Crtc<Self>,
        _old_state: &CrtcState<Self::State>,
        _new_state: CrtcStateMutator<'_, CrtcState<Self::State>>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        build_error::build_error("This should not be reachable")
    }

    /// The optional [`drm_crtc_helper_funcs.atomic_flush`] hook.
    ///
    /// This hook will be called after a set of [`Plane`] updates are performed for the given
    /// [`Crtc`].
    ///
    /// [`drm_crtc_helper_funcs.atomic_flush`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_flush(
        _crtc: &Crtc<Self>,
        _old_state: &CrtcState<Self::State>,
        _new_state: CrtcStateMutator<'_, CrtcState<Self::State>>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        build_error::build_error("This should never be reachable")
    }

    /// The optional [`drm_crtc_helper_funcs.atomic_enable`] hook.
    ///
    /// This hook will be called before enabling a [`Crtc`] in an atomic commit.
    ///
    /// [`drm_crtc_helper_funcs.atomic_enable`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_enable(
        _crtc: &Crtc<Self>,
        _old_state: &CrtcState<Self::State>,
        _new_state: CrtcStateMutator<'_, CrtcState<Self::State>>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        build_error::build_error("This should never be reachable")
    }

    /// The optional [`drm_crtc_helper_funcs.atomic_disable`] hook.
    ///
    /// This hook will be called before disabling a [`Crtc`] in an atomic commit.
    ///
    /// [`drm_crtc_helper_funcs.atomic_disable`]: srctree/include/drm/drm_modeset_helper_vtables.h
    fn atomic_disable(
        _crtc: &Crtc<Self>,
        _old_state: &CrtcState<Self::State>,
        _new_state: CrtcStateMutator<'_, CrtcState<Self::State>>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        build_error::build_error("This should never be reachable")
    }
}

/// The generated C vtable for a [`DriverCrtc`].
///
/// This type is created internally by DRM.
pub struct DriverCrtcOps {
    funcs: bindings::drm_crtc_funcs,
    helper_funcs: bindings::drm_crtc_helper_funcs,
}

/// The main interface for a [`struct drm_crtc`].
///
/// This type is the main interface for dealing with DRM CRTCs. In addition, it also allows
/// immutable access to whatever private data is contained within an implementor's [`DriverCrtc`]
/// type.
///
/// # Invariants
///
/// - `crtc` and `inner` are initialized for as long as this object is made available to users.
/// - The data layout of this structure begins with [`struct drm_crtc`].
/// - The atomic state for this type can always be assumed to be of type [`CrtcState<T::State>`].
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
#[repr(C)]
#[pin_data]
pub struct Crtc<T: DriverCrtc> {
    // The FFI drm_crtc object
    crtc: Opaque<bindings::drm_crtc>,
    /// The driver's private inner data
    #[pin]
    inner: T,
    #[pin]
    _p: PhantomPinned,
}

impl<T: DriverCrtc> Sealed for Crtc<T> {}

// SAFETY: Our CRTC interfaces are guaranteed to be thread-safe
unsafe impl<T: DriverCrtc> Send for Crtc<T> {}

// SAFETY: Our CRTC interfaces are guaranteed to be thread-safe
unsafe impl<T: DriverCrtc> Sync for Crtc<T> {}

impl<T: DriverCrtc> Deref for Crtc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

// SAFETY: We don't expose Crtc<T> to users before `base` is initialized in ::new(), so
// `raw_mode_obj` always returns a valid pointer to a bindings::drm_mode_object.
unsafe impl<T: DriverCrtc> ModeObject for Crtc<T> {
    type Driver = T::Driver;

    fn drm_dev(&self) -> &Device<Self::Driver> {
        // SAFETY: DRM connectors exist for as long as the device does, so this pointer is always
        // valid
        unsafe { Device::from_raw((*self.as_raw()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose Crtc<T> to users before it's initialized, so `base` is always
        // initialized
        unsafe { addr_of_mut!((*self.as_raw()).base) }
    }
}

// SAFETY: CRTCs are non-refcounted modesetting objects
unsafe impl<T: DriverCrtc> StaticModeObject for Crtc<T> {}

// SAFETY: `funcs` is initialized when the crtc is allocated
unsafe impl<T: DriverCrtc> ModeObjectVtable for Crtc<T> {
    type Vtable = bindings::drm_crtc_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid pointer to a CRTC
        unsafe { *self.as_raw() }.funcs
    }
}

impl<T: DriverCrtc> Crtc<T> {
    /// Return the primary plane assigned when this CRTC was created.
    pub fn primary_plane(&self) -> &OpaquePlane<T::Driver> {
        // SAFETY: UnregisteredCrtc::new requires a same-device primary plane.
        // Its pointer is immutable, non-null, and the device retains both objects.
        unsafe { OpaquePlane::from_raw((*self.as_raw()).primary) }
    }

    super::impl_from_opaque_mode_obj! {
        fn <'a, D>(&'a OpaqueCrtc<D>) -> &'a Self;
        use
            T as DriverCrtc,
            D as KmsDriver
    }

    pub(crate) fn get_vblank_ptr(&self) -> *mut bindings::drm_vblank_crtc {
        // SAFETY: FFI Call with no special requirements
        unsafe { bindings::drm_crtc_vblank_crtc(self.as_raw()) }
    }

    pub(crate) const fn has_vblank() -> bool {
        T::OPS.funcs.enable_vblank.is_some()
    }
}

/// A [`Crtc`] that has not yet been registered with userspace.
///
/// KMS registration is single-threaded, so this object is not thread-safe.
///
/// # Invariants
///
/// - This object can only exist before its respective KMS device has been registered.
/// - Otherwise, it inherits all invariants of [`Crtc`] and has an identical data layout.
pub struct UnregisteredCrtc<T: DriverCrtc>(Crtc<T>, NotThreadSafe);

impl<T: DriverCrtc> UnregisteredCrtc<T> {
    /// Construct a new [`UnregisteredCrtc`].
    ///
    /// A driver may use this from their [`KmsDriver::create_objects`] callback in order to
    /// construct new [`UnregisteredCrtc`] objects.
    ///
    /// [`KmsDriver::create_objects`]: kernel::drm::kms::KmsDriver::create_objects
    pub fn new<'a, 'b: 'a, PrimaryData, CursorData>(
        dev: &'a UnregisteredKmsDevice<'a, T::Driver>,
        primary: &'a UnregisteredPlane<PrimaryData>,
        cursor: Option<&'a UnregisteredPlane<CursorData>>,
        name: Option<&CStr>,
        args: T::Args,
    ) -> Result<&'a Self>
    where
        PrimaryData: DriverPlane<Driver = T::Driver>,
        CursorData: DriverPlane<Driver = T::Driver>,
    {
        // SAFETY: Both planes are initialized and their device pointers are
        // immutable. The owning device must retain them for the CRTC's lifetime.
        if unsafe { (*primary.as_raw()).dev != dev.as_raw() }
            || cursor.is_some_and(|cursor| {
                // SAFETY: The optional cursor has the same initialized-plane invariant.
                unsafe { (*cursor.as_raw()).dev != dev.as_raw() }
            })
        {
            return Err(EINVAL);
        }
        if Crtc::<T>::has_vblank() {
            dev.has_vblanks.set(true)
        }

        let this: Pin<KBox<Crtc<T>>> = KBox::try_pin_init(
            try_pin_init!(Crtc::<T> {
                crtc: Opaque::new(bindings::drm_crtc {
                    helper_private: &T::OPS.helper_funcs,
                    ..Default::default()
                }),
                inner <- T::new(dev, &args),
                _p: PhantomPinned,
            }),
            GFP_KERNEL,
        )?;

        // SAFETY:
        // - `dev` handles destroying the CRTC and thus will outlive us.
        // - We just allocated `this`, and we won't move it since it's pinned
        // - `primary` and `cursor` share the lifetime 'a with `dev`
        // - This function will memcpy the contents of `name` into its own storage.
        to_result(unsafe {
            bindings::drm_crtc_init_with_planes(
                dev.as_raw(),
                this.as_raw(),
                primary.as_raw(),
                cursor.map_or(null_mut(), |c| c.as_raw()),
                &T::OPS.funcs,
                name.map_or(null(), |n| n.as_char_ptr()),
            )
        })?;

        // SAFETY: We don't move anything
        let this = unsafe { Pin::into_inner_unchecked(this) };

        // We'll re-assemble the box in crtc_destroy_callback()
        let this = KBox::into_raw(this);

        // UnregisteredCrtc has an equivalent data layout
        let this: *mut Self = this.cast();

        // SAFETY: We just allocated the crtc above, so this pointer must be valid
        Ok(unsafe { &*this })
    }
}

// SAFETY: We inherit all relevant invariants of `Crtc`
unsafe impl<T: DriverCrtc> AsRawCrtc for UnregisteredCrtc<T> {
    fn as_raw(&self) -> *mut bindings::drm_crtc {
        self.0.as_raw()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_crtc) -> &'a Self {
        // SAFETY: This is another from_raw() call, so this function shares the same safety contract
        let crtc = unsafe { Crtc::<T>::from_raw(ptr) };

        // SAFETY: Our data layout is identical via our type invariants.
        unsafe { mem::transmute(crtc) }
    }
}

impl<T: DriverCrtc> Deref for UnregisteredCrtc<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0.inner
    }
}

/// A trait implemented by any type that acts as a [`struct drm_crtc`] interface.
///
/// This is implemented internally by DRM.
///
/// # Safety
///
/// [`as_raw()`] must always return a valid pointer to a [`struct drm_crtc`].
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
/// [`as_raw()`]: AsRawCrtc::as_raw()
pub unsafe trait AsRawCrtc {
    /// Return a raw pointer to the `bindings::drm_crtc` for this object
    fn as_raw(&self) -> *mut bindings::drm_crtc;

    /// Convert a raw [`struct drm_crtc`] pointer into an object of this type.
    ///
    /// # Safety
    ///
    /// Callers promise that `ptr` points to a valid instance of this type
    ///
    /// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_crtc) -> &'a Self;
}

// SAFETY:
// - Via our type variants our data layout starts with `drm_crtc`
// - Since we don't expose `crtc` to users before it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_crtc`.
unsafe impl<T: DriverCrtc> AsRawCrtc for Crtc<T> {
    fn as_raw(&self) -> *mut bindings::drm_crtc {
        self.crtc.get()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_crtc) -> &'a Self {
        // Our data layout start with `bindings::drm_crtc`.
        let ptr: *mut Self = ptr.cast();

        // SAFETY: Our safety contract requires that `ptr` point to a valid intance of `Self`.
        unsafe { &*ptr }
    }
}

// SAFETY: We only expose this object to users directly after KmsDriver::create_objects has been
// called.
unsafe impl<T: DriverCrtc> ModesettableCrtc for Crtc<T> {
    type State = CrtcState<T::State>;
}

/// A supertrait of [`AsRawCrtc`] for [`struct drm_crtc`] interfaces that can perform modesets.
///
/// This is implemented internally by DRM.
///
/// # Safety
///
/// Any object implementing this trait must only be made directly available to the user after
/// [`create_objects`] has completed.
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
/// [`create_objects`]: KmsDriver::create_objects
pub unsafe trait ModesettableCrtc: AsRawCrtc {
    /// The type that should be returned for a CRTC state acquired using this CRTC interface
    type State: FromRawCrtcState;
}

/// Common methods available on any type which implements [`AsRawCrtc`].
///
/// This is implemented internally by DRM, and provides many of the basic methods for working with
/// CRTCs.
pub trait RawCrtc: AsRawCrtc {
    /// Return the index of this CRTC.
    fn index(&self) -> u32 {
        // SAFETY: The index is initialized by the time we expose Crtc objects to users, and is
        // invariant throughout the lifetime of the Crtc
        unsafe { (*self.as_raw()).index }
    }

    /// Return the index of this DRM CRTC in the form of a bitmask.
    fn mask(&self) -> u32 {
        1 << self.index()
    }
}
impl<T: AsRawCrtc> RawCrtc for T {}

/// A [`struct drm_crtc`] without a known [`DriverCrtc`] implementation.
///
/// This is mainly for situations where our bindings can't infer the [`DriverCrtc`] implementation
/// for a [`struct drm_crtc`] automatically. It is identical to [`Crtc`], except that it does not
/// provide access to the driver's private data.
///
/// It may be upcasted to a full [`Crtc`] using [`Crtc::from_opaque`] or
/// [`Crtc::try_from_opaque`].
///
/// # Invariants
///
/// - `crtc` is initialized for as long as this object is made available to users.
/// - The data layout of this structure is equivalent to [`struct drm_crtc`].
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
#[repr(transparent)]
pub struct OpaqueCrtc<T: KmsDriver> {
    crtc: Opaque<bindings::drm_crtc>,
    _p: PhantomData<T>,
}

impl<T: KmsDriver> Sealed for OpaqueCrtc<T> {}

// SAFETY:
// - Via our type variants our data layout is identical to `drm_crtc`
// - Since we don't expose `OpaqueCrtc` to users before it has been initialized, this and our data
//   layout ensure that `as_raw()` always returns a valid pointer to a `drm_crtc`.
unsafe impl<T: KmsDriver> AsRawCrtc for OpaqueCrtc<T> {
    fn as_raw(&self) -> *mut bindings::drm_crtc {
        self.crtc.get()
    }

    unsafe fn from_raw<'a>(ptr: *mut bindings::drm_crtc) -> &'a Self {
        // SAFETY: Our data layout starts with `bindings::drm_crtc`
        unsafe { &*ptr.cast() }
    }
}

// SAFETY: We only expose this object to users directly after KmsDriver::create_objects has been
// called.
unsafe impl<T: KmsDriver> ModesettableCrtc for OpaqueCrtc<T> {
    type State = OpaqueCrtcState<T>;
}

// SAFETY: We don't expose OpaqueCrtc<T> to users before `base` is initialized in Crtc::<T>::new(),
// so `raw_mode_obj` always returns a valid pointer to a bindings::drm_mode_object.
unsafe impl<T: KmsDriver> ModeObject for OpaqueCrtc<T> {
    type Driver = T;

    fn drm_dev(&self) -> &Device<Self::Driver> {
        // SAFETY: The parent device for a DRM connector will never outlive the connector, and this
        // pointer is invariant through the lifetime of the connector
        unsafe { Device::from_raw((*self.as_raw()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose DRM connectors to users before `base` is initialized
        unsafe { addr_of_mut!((*self.as_raw()).base) }
    }
}

// SAFETY: CRTCs are non-refcounted modesetting objects
unsafe impl<T: KmsDriver> StaticModeObject for OpaqueCrtc<T> {}

// SAFETY: Our CRTC interface is guaranteed to be thread-safe
unsafe impl<T: KmsDriver> Send for OpaqueCrtc<T> {}

// SAFETY: Our CRTC interface is guaranteed to be thread-safe
unsafe impl<T: KmsDriver> Sync for OpaqueCrtc<T> {}

// SAFETY: `funcs` is initialized when the CRTC is allocated
unsafe impl<T: KmsDriver> ModeObjectVtable for OpaqueCrtc<T> {
    type Vtable = bindings::drm_crtc_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        // SAFETY: `as_raw()` always returns a valid pointer to a crtc
        unsafe { (*self.as_raw()).funcs }
    }
}

impl<T: DriverCrtcState> Sealed for CrtcState<T> {}

/// The main trait for implementing the [`struct drm_crtc_state`] API for a [`Crtc`].
///
/// A driver may store driver-private data within the implementor's type, which will be available
/// when using a full typed [`CrtcState`] object.
///
/// # Invariants
///
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_crtc`] pointers are contained within a [`Crtc<Self::Crtc>`].
/// - Any C FFI callbacks generated using this trait are guaranteed that passed-in
///   [`struct drm_crtc_state`] pointers are contained within a [`CrtcState<Self>`].
///
/// [`struct drm_crtc`]: srctree/include/drm/drm_crtc.h
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
pub trait DriverCrtcState: Clone + Default + Unpin {
    /// The parent CRTC driver for this CRTC state
    type Crtc: DriverCrtc<State = Self>
    where
        Self: Sized;
}

/// The main interface for a [`struct drm_crtc_state`].
///
/// This type is the main interface for dealing with the atomic state of DRM crtcs. In addition, it
/// allows access to whatever private data is contained within an implementor's [`DriverCrtcState`]
/// type.
///
/// # Invariants
///
/// - `state` and `inner` initialized for as long as this object is exposed to users.
/// - The data layout of this structure begins with [`struct drm_crtc_state`].
/// - The CRTC for this type can always be assumed to be of type [`Crtc<T::Crtc>`].
///
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
#[repr(C)]
pub struct CrtcState<T: DriverCrtcState> {
    // It should be noted that CrtcState is a bit of an oddball - it's the only atomic state
    // structure that can be modified after it has been swapped in, which is why we need to have
    // `state` within an `Opaque<>`…
    state: Opaque<bindings::drm_crtc_state>,

    // …it is also one of the few atomic states that some drivers will embed work structures into,
    // which means there's a good chance in the future we may have pinned data here - making it
    // impossible for us to hold a mutable or immutable reference to the CrtcState. In preparation
    // for that possibility, we keep `T` in an UnsafeCell.
    inner: UnsafeCell<T>,
}

impl<T: DriverCrtcState> Deref for CrtcState<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Our interface ensures that `inner` will not be modified unless only a single
        // mutable reference exists to `inner`, so this is safe
        unsafe { &*self.inner.get() }
    }
}

impl<T: DriverCrtcState> DerefMut for CrtcState<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.get_mut()
    }
}

// SAFETY: Shares the safety guarantee of Crtc<T>'s ModeObjectVtable impl
unsafe impl<T: DriverCrtcState> ModeObjectVtable for CrtcState<T> {
    type Vtable = bindings::drm_crtc_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        self.crtc().vtable()
    }
}

impl<T: DriverCrtcState> CrtcState<T> {
    super::impl_from_opaque_mode_obj! {
        fn <'a, D, C>(&'a OpaqueCrtcState<D>) -> &'a Self
        where
            T: DriverCrtcState<Crtc = C>;
        use
            C as DriverCrtc,
            D as KmsDriver
    }
}

/// A trait implemented by any type which can produce a reference to a [`struct drm_crtc_state`].
///
/// This is implemented internally by DRM.
///
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
pub trait AsRawCrtcState: private::AsRawCrtcState {
    /// The type that this CRTC state interface returns to represent the parent CRTC
    type Crtc: ModesettableCrtc;
}

pub(crate) mod private {
    use super::*;

    #[allow(unreachable_pub)]
    pub trait AsRawCrtcState {
        /// Return a raw pointer to the DRM CRTC state
        ///
        /// Note that CRTC states are the only atomic state in KMS which don't nicely follow rust's
        /// data aliasing rules already.
        fn as_raw(&self) -> *mut bindings::drm_crtc_state;
    }
}

pub(super) use private::AsRawCrtcState as AsRawCrtcStatePrivate;

/// Common methods available on any type which implements [`AsRawCrtcState`].
///
/// This is implemented internally by DRM, and provides many of the basic methods for working with
/// the atomic state of [`Crtc`]s.
pub trait RawCrtcState: AsRawCrtcState {
    /// Copy the mode requested by this atomic state.
    fn mode(&self) -> bindings::drm_display_mode {
        // SAFETY: Atomic checking serializes changes to the mode. After state
        // publication, its timing fields remain immutable.
        unsafe { (*self.as_raw()).mode }
    }

    /// Copy the hardware timing selected during atomic checking.
    fn adjusted_mode(&self) -> bindings::drm_display_mode {
        // SAFETY: The check phase serializes timing changes, and the adjusted
        // mode is immutable after publication.
        unsafe { (*self.as_raw()).adjusted_mode }
    }

    /// Return whether activation, timings or connector routing require a modeset.
    fn needs_modeset(&self) -> bool {
        // SAFETY: The state remains live, and atomic checking serializes changes
        // to the flags read by this helper.
        unsafe { bindings::drm_atomic_crtc_needs_modeset(self.as_raw()) }
    }

    /// Return the CRTC that owns this state.
    fn crtc(&self) -> &Self::Crtc {
        // SAFETY:
        // - This type conversion is guaranteed by type invariance
        // - Our interface ensures that this access follows rust's data-aliasing rules
        // - `crtc` is guaranteed to never be NULL and is invariant throughout the lifetime of the
        //   state
        unsafe { <Self::Crtc as AsRawCrtc>::from_raw((*self.as_raw()).crtc) }
    }

    /// Returns whether or not the CRTC is active in this atomic state.
    fn active(&self) -> bool {
        // SAFETY: `active` and the rest of its containing bitfield can only be modified from the
        // atomic check context, and are invariant beyond that point - so our interface can ensure
        // this access is serialized
        unsafe { (*self.as_raw()).active }
    }
}
impl<T: AsRawCrtcState> RawCrtcState for T {}

/// A trait implemented for any type which can be constructed directly from a
/// [`struct drm_crtc_state`] pointer.
///
/// This is implemented internally by DRM.
///
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
pub trait FromRawCrtcState: AsRawCrtcState {
    /// Obtain a reference back to this type from a raw DRM crtc state pointer
    ///
    /// # Safety
    ///
    /// Callers must ensure that ptr contains a valid instance of this type.
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_crtc_state) -> &'a Self;
}

impl<T: DriverCrtcState> private::AsRawCrtcState for CrtcState<T> {
    #[inline]
    fn as_raw(&self) -> *mut bindings::drm_crtc_state {
        self.state.get()
    }
}

impl<T: DriverCrtcState> AsRawCrtcState for CrtcState<T> {
    type Crtc = Crtc<T::Crtc>;
}

impl<T: DriverCrtcState> FromRawCrtcState for CrtcState<T> {
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_crtc_state) -> &'a Self {
        // SAFETY: Our data layout starts with `bindings::drm_crtc_state`
        unsafe { &*(ptr.cast()) }
    }
}

/// A [`struct drm_crtc_state`] without a known [`DriverCrtcState`] implementation.
///
/// This is mainly for situations where our bindings can't infer the [`DriverCrtcState`]
/// implementation for a [`struct drm_crtc_state`] automatically. It is identical to [`Crtc`],
/// except that it does not provide access to the driver's private data.
///
/// # Invariants
///
/// - `state` is initialized for as long as this object is exposed to users.
/// - The data layout of this type is identical to [`struct drm_crtc_state`].
///
/// [`struct drm_crtc_state`]: srctree/include/drm/drm_crtc.h
#[repr(transparent)]
pub struct OpaqueCrtcState<T: KmsDriver> {
    state: Opaque<bindings::drm_crtc_state>,
    _p: PhantomData<T>,
}

impl<T: KmsDriver> AsRawCrtcState for OpaqueCrtcState<T> {
    type Crtc = OpaqueCrtc<T>;
}

impl<T: KmsDriver> private::AsRawCrtcState for OpaqueCrtcState<T> {
    fn as_raw(&self) -> *mut bindings::drm_crtc_state {
        self.state.get()
    }
}

impl<T: KmsDriver> FromRawCrtcState for OpaqueCrtcState<T> {
    unsafe fn from_raw<'a>(ptr: *const bindings::drm_crtc_state) -> &'a Self {
        // SAFETY: Our data layout is identical to `bindings::drm_crtc_state`
        unsafe { &*(ptr.cast()) }
    }
}

// SAFETY: Shares the safety guarantees of OpaqueCrtc<T>'s ModeObjectVtable impl
unsafe impl<T: KmsDriver> ModeObjectVtable for OpaqueCrtcState<T> {
    type Vtable = bindings::drm_crtc_funcs;

    fn vtable(&self) -> *const Self::Vtable {
        self.crtc().vtable()
    }
}

/// An interface for mutating a [`Crtc`]s atomic state.
///
/// This type is typically returned by an [`AtomicStateMutator`] within contexts where it is
/// possible to safely mutate a plane's state. In order to uphold rust's data-aliasing rules, only
/// [`CrtcStateMutator`] may exist at a time.
///
/// # Invariants
///
/// `self.state` always points to a valid instance of a [`FromRawCrtcState`] object.
pub struct CrtcStateMutator<'a, T: FromRawCrtcState> {
    state: NonNull<T>,
    mask: &'a Cell<u32>,
}

impl<'a, T: FromRawCrtcState> CrtcStateMutator<'a, T> {
    /// Send the pending page-flip event after the hardware update has completed.
    ///
    /// No vblank interrupt implementation is required. On devices without one,
    /// DRM supplies the current time and a zero sequence number.
    /// This is only valid for the new CRTC state in a post-swap commit callback.
    pub fn send_pending_event<D: KmsDriver>(&mut self) -> Result
    where
        T::Crtc: ModeObject<Driver = D>,
    {
        // SAFETY: This mutator exclusively accesses the state. DRM clears state
        // when installing the new state and sets it on states being replaced.
        if unsafe { !(*self.as_raw()).state.is_null() } {
            return Err(EINVAL);
        }
        let crtc = self.crtc();
        let _guard = crtc.drm_dev().event_lock().lock();
        // SAFETY: This mutator and event_lock serialize event ownership. The
        // CRTC and its state stay live throughout the call. Clearing the pointer
        // before transferring the event prevents it from being sent twice.
        unsafe {
            let event = core::mem::replace(&mut (*self.as_raw()).event, null_mut());
            if !event.is_null() {
                bindings::drm_crtc_send_vblank_event(crtc.as_raw(), event);
            }
        }
        Ok(())
    }

    /// Cancel the pending page-flip event after a failed hardware commit.
    ///
    /// Use only on the new state in a post-swap commit callback, after stopping
    /// the failed operation's hardware accesses. This retires the commit's flip
    /// completion and signals its out-fence with `error`, without delivering a
    /// successful page-flip event to userspace.
    pub fn fail_pending_event<D: KmsDriver>(&mut self, error: Error) -> Result
    where
        T::Crtc: ModeObject<Driver = D>,
    {
        // SAFETY: This mutator exclusively accesses the state. A pre-swap state
        // may still share the event with commit->event, which must not be freed.
        if unsafe { !(*self.as_raw()).state.is_null() } {
            return Err(EINVAL);
        }
        let crtc = self.crtc();
        let dev = crtc.drm_dev();
        let event = {
            let _guard = dev.event_lock().lock();
            // SAFETY: The mutator and event_lock serialize event ownership. The
            // post-swap check ensures the commit no longer owns commit->event.
            let event = unsafe { mem::replace(&mut (*self.as_raw()).event, null_mut()) };
            if event.is_null() {
                return Ok(());
            }
            // SAFETY: The pending event remains owned here. Retire the completion
            // in the same order as drm_send_event_locked(), releasing its extra
            // commit reference exactly once before canceling the event.
            unsafe {
                let base = &raw mut (*event).base;
                let completion = mem::replace(&mut (*base).completion, null_mut());
                if !completion.is_null() {
                    bindings::complete_all(completion);
                    if let Some(release) = (*base).completion_release {
                        release(completion);
                    }
                }
                let fence = (*base).fence;
                if !fence.is_null() {
                    let mut flags = 0;
                    bindings::dma_fence_lock_irqsave(fence, &mut flags);
                    if !bindings::dma_fence_test_signaled_flag(fence) {
                        bindings::dma_fence_set_error(fence, error.to_errno());
                        bindings::dma_fence_signal_locked(fence);
                    }
                    bindings::dma_fence_unlock_irqrestore(fence, &mut flags);
                }
            }
            event
        };
        // SAFETY: The event was removed from the state and its completion was
        // retired. This function acquires event_lock itself, drops the event's
        // fence reference and frees the event without sending it to userspace.
        unsafe { bindings::drm_event_cancel_free(dev.as_raw(), &raw mut (*event).base) };
        Ok(())
    }

    pub(super) fn new<D: KmsDriver>(
        mutator: &'a AtomicStateMutator<D>,
        state: NonNull<bindings::drm_crtc_state>,
    ) -> Option<Self> {
        // SAFETY: `crtc` is invariant throughout the lifetime of the atomic state, and always
        // points to a valid `Crtc<T::Crtc>`
        let crtc = unsafe { T::Crtc::from_raw((*state.as_ptr()).crtc) };
        let crtc_mask = crtc.mask();
        let borrowed_mask = mutator.borrowed_crtcs.get();

        if borrowed_mask & crtc_mask == 0 {
            mutator.borrowed_crtcs.set(borrowed_mask | crtc_mask);
            Some(Self {
                mask: &mutator.borrowed_crtcs,
                state: state.cast(),
            })
        } else {
            None
        }
    }
}

impl<'a, T: DriverCrtcState> CrtcStateMutator<'a, CrtcState<T>> {
    super::impl_from_opaque_mode_obj! {
        fn <D, C>(CrtcStateMutator<'a, OpaqueCrtcState<D>>) -> Self
        where
            T: DriverCrtcState<Crtc = C>;
        use
            T as DriverCrtc,
            D as KmsDriver
    }
}

impl<'a, T: FromRawCrtcState> Drop for CrtcStateMutator<'a, T> {
    fn drop(&mut self) {
        // SAFETY: Our interface is proof that we are the only ones with a reference to this data
        let mask = unsafe { self.state.as_ref() }.crtc().mask();
        self.mask.set(self.mask.get() & !mask);
    }
}

impl<'a, T: DriverCrtcState> Deref for CrtcStateMutator<'a, CrtcState<T>> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Our interface ensures that `self.state.inner` follows rust's data-aliasing rules,
        // so this is safe
        unsafe { &*(*self.state.as_ptr()).inner.get() }
    }
}

impl<'a, T: DriverCrtcState> DerefMut for CrtcStateMutator<'a, CrtcState<T>> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Our interface ensures that `self.state.inner` follows rust's data-aliasing rules,
        // so this is safe
        unsafe { (*self.state.as_ptr()).inner.get_mut() }
    }
}

impl<'a, T: FromRawCrtcState> AsRawCrtcState for CrtcStateMutator<'a, T> {
    type Crtc = T::Crtc;
}

impl<'a, T: FromRawCrtcState> private::AsRawCrtcState for CrtcStateMutator<'a, T> {
    fn as_raw(&self) -> *mut bindings::drm_crtc_state {
        self.state.as_ptr().cast()
    }
}

// SAFETY: Shares the safety guarantees of T::Crtc's ModeObjectVtable impl
unsafe impl<'a, T: FromRawCrtcState> ModeObjectVtable for CrtcStateMutator<'a, T>
where
    T::Crtc: ModeObjectVtable,
{
    type Vtable = <T::Crtc as ModeObjectVtable>::Vtable;

    fn vtable(&self) -> *const Self::Vtable {
        self.crtc().vtable()
    }
}

/// # Safety
///
/// DRM must pass the final live `Crtc<T>` allocation created by this module.
/// No references may access it after this callback reclaims its storage.
unsafe extern "C" fn crtc_destroy_callback<T: DriverCrtc>(crtc: *mut bindings::drm_crtc) {
    // SAFETY: DRM guarantees that `crtc` points to a valid initialized `drm_crtc`.
    unsafe { bindings::drm_crtc_cleanup(crtc) };

    // SAFETY:
    // - DRM guarantees we are now the only one with access to this [`drm_crtc`].
    // - This cast is safe via `DriverCrtc`s type invariants.
    // - We created this as a pinned type originally
    drop(unsafe { Pin::new_unchecked(KBox::from_raw(crtc.cast::<Crtc<T>>())) });
}

/// # Safety
///
/// DRM must pass a live `Crtc<T::Crtc>` whose current state is null or a
/// `CrtcState<T>` allocated by this module, with state access serialized.
unsafe extern "C" fn atomic_duplicate_state_callback<T: DriverCrtcState>(
    crtc: *mut bindings::drm_crtc,
) -> *mut bindings::drm_crtc_state {
    // SAFETY: DRM guarantees that `crtc` points to a valid initialized `drm_crtc`.
    let state = unsafe { (*crtc).state };
    if state.is_null() {
        return null_mut();
    }

    // SAFETY: This cast is safe via `DriverCrtcState`s type invariants.
    let crtc = unsafe { Crtc::<T::Crtc>::from_raw(crtc) };

    // SAFETY: This cast is safe via `DriverCrtcState`s type invariants.
    let old_state = unsafe { CrtcState::<T>::from_raw(state) };

    let new = KBox::try_init(
        try_init!(CrtcState::<T> {
            state: Opaque::new(Default::default()),
            inner: UnsafeCell::new((*old_state).clone()),
        }),
        GFP_KERNEL,
    );

    if let Ok(new) = new {
        let new = KBox::into_raw(new).cast();

        // SAFETY:
        // - `new` provides a valid pointer to a newly allocated `drm_crtc_state` via type
        // invariants
        // - This initializes `new` via memcpy()
        unsafe { bindings::__drm_atomic_helper_crtc_duplicate_state(crtc.as_raw(), new) }

        new
    } else {
        null_mut()
    }
}

/// # Safety
///
/// The state pointer must be a live `CrtcState<T>` allocation from this module.
/// DRM must have released all access to it before this callback destroys it.
unsafe extern "C" fn atomic_destroy_state_callback<T: DriverCrtcState>(
    _crtc: *mut bindings::drm_crtc,
    crtc_state: *mut bindings::drm_crtc_state,
) {
    // SAFETY: DRM guarantees that `state` points to a valid instance of `drm_crtc_state`
    unsafe { bindings::__drm_atomic_helper_crtc_destroy_state(crtc_state) };

    // SAFETY:
    // - DRM guarantees we are the only one with access to this `drm_crtc_state`
    // - This cast is safe via our type invariants.
    // - All data in `CrtcState` is either Unpin, or pinned
    drop(unsafe { KBox::from_raw(crtc_state.cast::<CrtcState<T>>()) });
}

/// # Safety
///
/// DRM must pass a live `Crtc<T::Crtc>`. The returned state is owned by DRM and
/// must be released through this module's state destruction callback.
unsafe extern "C" fn crtc_create_state_callback<T: DriverCrtcState>(
    crtc: *mut bindings::drm_crtc,
) -> *mut bindings::drm_crtc_state {
    let new = match KBox::try_init(
        try_init!(CrtcState::<T> {
            state: Opaque::new(Default::default()),
            inner: UnsafeCell::new(Default::default()),
        }),
        GFP_KERNEL,
    ) {
        Ok(new) => new,
        Err(error) => return error.to_ptr(),
    };

    let state = KBox::into_raw(new).cast();
    // SAFETY: `state` points to newly allocated, zeroed C state inside `CrtcState<T>`;
    // `crtc` is live by the callback contract. DRM takes ownership of the returned state.
    unsafe { bindings::__drm_atomic_helper_crtc_state_init(state, crtc) };
    state
}

/// # Safety
///
/// DRM must pass a live `Crtc<T>` and its same-device atomic commit. Both old
/// and new object states must be present, with access serialized for this callback.
unsafe extern "C" fn atomic_check_callback<T: DriverCrtc>(
    crtc: *mut bindings::drm_crtc,
    state: *mut bindings::drm_atomic_commit,
) -> i32 {
    // SAFETY:
    // - We're guaranteed `crtc` is of type `Crtc<T>` via type invariants.
    // - We're guaranteed by DRM that `crtc` is pointing to a valid initialized state.
    let crtc = unsafe { Crtc::from_raw(crtc) };

    // SAFETY: DRM guarantees `state` points to a valid `drm_atomic_commit`
    // We use a ManuallyDrop here to avoid AtomicStateComposer dropping an owned reference we never
    // acquired.
    let state =
        unsafe { ManuallyDrop::new(AtomicStateComposer::new(NonNull::new_unchecked(state))) };

    // SAFETY: Since we are in the atomic update callback, we're guaranteed by DRM that both the old
    // and new atomic state are present within `state`
    let (old_state, new_state) = unsafe {
        (
            state.get_old_crtc_state(crtc).unwrap_unchecked(),
            state.get_new_crtc_state(crtc).unwrap_unchecked(),
        )
    };

    from_result(|| {
        T::atomic_check(crtc, old_state, new_state, &state)?;
        Ok(0)
    })
}

/// # Safety
///
/// DRM must pass a live `Crtc<T>` and its same-device atomic commit. Both old
/// and new object states must be present, with access serialized for this callback.
unsafe extern "C" fn atomic_begin_callback<T: DriverCrtc>(
    crtc: *mut bindings::drm_crtc,
    state: *mut bindings::drm_atomic_commit,
) {
    // SAFETY:
    // * We're guaranteed `crtc` is of type `Crtc<T>` via type invariants.
    // * We're guaranteed by DRM that `crtc` is pointing to a valid initialized state.
    let crtc = unsafe { Crtc::from_raw(crtc) };

    // SAFETY: DRM guarantees that `state` points to a valid `drm_atomic_commit`.
    let state = unsafe { AtomicStateMutator::new(NonNull::new_unchecked(state)) };

    // SAFETY: We're guaranteed by DRM that both the old and new atomic state are present within
    // this `drm_atomic_commit`
    let (old_state, new_state) = unsafe {
        (
            state.get_old_crtc_state(crtc).unwrap_unchecked(),
            state.get_new_crtc_state(crtc).unwrap_unchecked(),
        )
    };

    T::atomic_begin(crtc, old_state, new_state, &state);
}

/// # Safety
///
/// DRM must pass a live `Crtc<T>` and its same-device atomic commit. Both old
/// and new object states must be present, with access serialized for this callback.
unsafe extern "C" fn atomic_flush_callback<T: DriverCrtc>(
    crtc: *mut bindings::drm_crtc,
    state: *mut bindings::drm_atomic_commit,
) {
    // SAFETY:
    // - We're guaranteed `crtc` is of type `Crtc<T>` via type invariants.
    // - We're guaranteed by DRM that `crtc` is pointing to a valid initialized state.
    let crtc = unsafe { Crtc::from_raw(crtc) };

    // SAFETY: DRM guarantees that `state` points to a valid `drm_atomic_commit`.
    let state = unsafe { AtomicStateMutator::new(NonNull::new_unchecked(state)) };

    // SAFETY: We're in an atomic flush callback, so we know that both the new and old state are
    // present
    let (old_state, new_state) = unsafe {
        (
            state.get_old_crtc_state(crtc).unwrap_unchecked(),
            state.get_new_crtc_state(crtc).unwrap_unchecked(),
        )
    };

    T::atomic_flush(crtc, old_state, new_state, &state);
}

/// # Safety
///
/// DRM must pass a live `Crtc<T>` and its same-device atomic commit. Both old
/// and new object states must be present, with access serialized for this callback.
unsafe extern "C" fn atomic_enable_callback<T: DriverCrtc>(
    crtc: *mut bindings::drm_crtc,
    state: *mut bindings::drm_atomic_commit,
) {
    // SAFETY:
    // - We're guaranteed `crtc` is of type `Crtc<T>` via type invariants.
    // - We're guaranteed by DRM that `crtc` is pointing to a valid initialized state.
    let crtc = unsafe { Crtc::from_raw(crtc) };

    // SAFETY: DRM never passes an invalid ptr for `state`
    let state = unsafe { AtomicStateMutator::new(NonNull::new_unchecked(state)) };

    // SAFETY: We're in an atomic enable callback, so we know that both the new and old state are
    // present
    let (old_state, new_state) = unsafe {
        (
            state.get_old_crtc_state(crtc).unwrap_unchecked(),
            state.get_new_crtc_state(crtc).unwrap_unchecked(),
        )
    };

    T::atomic_enable(crtc, old_state, new_state, &state);
}

/// # Safety
///
/// DRM must pass a live `Crtc<T>` and its same-device atomic commit. Both old
/// and new object states must be present, with access serialized for this callback.
unsafe extern "C" fn atomic_disable_callback<T: DriverCrtc>(
    crtc: *mut bindings::drm_crtc,
    state: *mut bindings::drm_atomic_commit,
) {
    // SAFETY:
    // - We're guaranteed `crtc` points to a valid instance of `drm_crtc`
    // - We're guaranteed that `crtc` is of type `Plane<T>` by `DriverPlane`s type invariants.
    let crtc = unsafe { Crtc::from_raw(crtc) };

    // SAFETY: We're guaranteed that `state` points to a valid `drm_crtc_state` by DRM
    let state = unsafe { AtomicStateMutator::new(NonNull::new_unchecked(state)) };

    // SAFETY: We're in an atomic commit callback, so we know that both the new and old state are
    // present
    let (old_state, new_state) = unsafe {
        (
            state.get_old_crtc_state(crtc).unwrap_unchecked(),
            state.get_new_crtc_state(crtc).unwrap_unchecked(),
        )
    };

    T::atomic_disable(crtc, old_state, new_state, &state);
}
