// SPDX-License-Identifier: GPL-2.0

use kernel::{
    drm,
    drm::{
        gem,
        gem::BaseObject, //
    },
    page,
    prelude::*,
    sync::aref::ARef,
};

use crate::{
    driver::{NovaDevice, NovaDriver},
    file::File,
};

/// GEM Object inner driver data
#[pin_data]
pub(crate) struct NovaObject {}

pub(crate) type Object = gem::shmem::Object<NovaObject>;

impl gem::DriverObject for NovaObject {
    type Driver = NovaDriver;
    type Args = ();

    fn new(_dev: &NovaDevice, _size: usize, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(NovaObject {})
    }
}

impl NovaObject {
    /// Create a new DRM GEM object.
    pub(crate) fn new(dev: &NovaDevice, size: usize) -> Result<ARef<Object>> {
        if size == 0 {
            return Err(EINVAL);
        }
        let aligned_size = page::page_align(size).ok_or(EINVAL)?;

        Object::new(dev, aligned_size, Default::default(), ())
    }

    /// Look up a GEM object handle for a `File` and return an `ObjectRef` for it.
    #[inline]
    pub(crate) fn lookup_handle(file: &drm::File<File>, handle: u32) -> Result<ARef<Object>> {
        Object::lookup_handle(file, handle)
    }
}
