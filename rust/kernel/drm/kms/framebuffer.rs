// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM framebuffers.
//!
//! C header: [`include/drm/drm_framebuffer.h`](srctree/include/drm/drm_framebuffer.h)

use super::{KmsDriver, ModeObject, RcModeObject};
use crate::{
    drm::{
        device::Device,
        driver::AllocImpl,
        gem::IntoGEMObject, //
    },
    prelude::*,
    types::*, //
};
use bindings;
use core::{marker::*, ptr};

/// The main interface for [`struct drm_framebuffer`].
///
/// # Invariants
///
/// - `self.0` is initialized for as long as this object is exposed to users.
/// - This type has an identical data layout to [`struct drm_framebuffer`]
///
/// [`struct drm_framebuffer`]: srctree/include/drm/drm_framebuffer.h
#[repr(transparent)]
pub struct Framebuffer<T: KmsDriver>(Opaque<bindings::drm_framebuffer>, PhantomData<T>);

// SAFETY:
// - `self.0` is initialized for as long as this object is exposed to users
// - `base` is initialized by DRM when `self.0` is initialized, thus `raw_mode_obj()` always returns
//   a valid pointer.
unsafe impl<T: KmsDriver> ModeObject for Framebuffer<T> {
    type Driver = T;

    fn drm_dev(&self) -> &Device<Self::Driver> {
        // SAFETY: `dev` points to an initialized `struct drm_device` for as long as this type is
        // initialized
        unsafe { Device::from_raw((*self.0.get()).dev) }
    }

    fn raw_mode_obj(&self) -> *mut bindings::drm_mode_object {
        // SAFETY: We don't expose Framebuffer<T> to users before its initialized, so `base` is
        // always initialized
        unsafe { ptr::addr_of_mut!((*self.0.get()).base) }
    }
}

// SAFETY: Framebuffers are refcounted mode objects.
unsafe impl<T: KmsDriver> RcModeObject for Framebuffer<T> {}

// SAFETY: References to framebuffers are safe to be accessed from any thread
unsafe impl<T: KmsDriver> Send for Framebuffer<T> {}
// SAFETY: References to framebuffers are safe to be accessed from any thread
unsafe impl<T: KmsDriver> Sync for Framebuffer<T> {}

// For implementing ModeObject
impl<T: KmsDriver> crate::drm::private::Sealed for Framebuffer<T> {}

impl<T: KmsDriver> PartialEq for Framebuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        ptr::eq(self.0.get(), other.0.get())
    }
}
impl<T: KmsDriver> Eq for Framebuffer<T> {}

/// Immutable storage information for one framebuffer plane.
#[derive(Clone, Copy, Debug)]
pub struct PlaneLayout {
    /// Framebuffer width in pixels.
    pub width: u32,
    /// Framebuffer height in pixels.
    pub height: u32,
    /// DRM fourcc pixel format.
    pub format: u32,
    /// DRM format modifier.
    pub modifier: u64,
    /// Number of storage planes described by the format.
    pub num_planes: u8,
    /// Byte stride for this storage plane.
    pub pitch: u32,
    /// Byte offset within this plane's GEM object.
    pub offset: u32,
}

impl<T: KmsDriver> Framebuffer<T> {
    /// Borrow the GEM object backing a framebuffer storage plane.
    pub fn gem_object(&self, plane: usize) -> Result<&T::Object>
    where
        T::Object: AllocImpl<Driver = T>,
    {
        let fb = self.0.get();
        // SAFETY: Framebuffer format and object pointers are initialized before
        // publication and stay immutable while the framebuffer is referenced.
        let format = unsafe { &*(*fb).format };
        if plane >= usize::from(format.num_planes) {
            return Err(EINVAL);
        }
        // SAFETY: The framebuffer retains every object in this immutable array.
        let obj = unsafe { &(*fb).obj }.get(plane).copied().ok_or(EINVAL)?;
        if obj.is_null() {
            return Err(ENOENT);
        }
        // SAFETY: KmsDriver uses the GEM framebuffer helper and this driver has
        // one GEM allocation implementation, including PRIME imports. The
        // framebuffer owns the reference throughout the returned borrow.
        Ok(unsafe { T::Object::from_raw(obj) })
    }

    /// Return the immutable layout of a framebuffer storage plane.
    pub fn plane_layout(&self, plane: usize) -> Result<PlaneLayout> {
        let fb = self.0.get();
        // SAFETY: DRM initializes the immutable format before publishing a framebuffer.
        let format = unsafe { &*(*fb).format };
        // SAFETY: The immutable pitch array remains valid while this framebuffer is borrowed.
        let pitches = unsafe { &(*fb).pitches };
        // SAFETY: The immutable offset array has the same lifetime and bound.
        let offsets = unsafe { &(*fb).offsets };
        if plane >= usize::from(format.num_planes) {
            return Err(EINVAL);
        }
        let pitch = *pitches.get(plane).ok_or(EINVAL)?;
        let offset = *offsets.get(plane).ok_or(EINVAL)?;
        // SAFETY: These layout fields are immutable throughout the framebuffer's
        // lifetime. No reference to the mutable C framebuffer header is created.
        Ok(unsafe {
            PlaneLayout {
                width: (*fb).width,
                height: (*fb).height,
                format: format.format,
                modifier: (*fb).modifier,
                num_planes: format.num_planes,
                pitch,
                offset,
            }
        })
    }

    /// Convert a raw pointer to a `struct drm_framebuffer` into a [`Framebuffer`]
    ///
    /// # Safety
    ///
    /// The caller guarantews that `ptr` points to a initialized `struct drm_framebuffer` for at
    /// least the entire lifetime of `'a`.
    #[inline]
    pub(super) unsafe fn from_raw<'a>(ptr: *const bindings::drm_framebuffer) -> &'a Self {
        // SAFETY: Our data layout is identical to drm_framebuffer
        unsafe { &*ptr.cast() }
    }
}
