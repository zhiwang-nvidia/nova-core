// SPDX-License-Identifier: GPL-2.0 OR MIT

//! EDID reads and connector mode enumeration through DRM's parser.

use core::{
    ptr::{
        self,
        NonNull, //
    },
    slice, //
};

use crate::{
    bindings,
    drm::kms::connector::{
        AsRawConnector,
        ConnectorProbeGuard,
        DriverConnector, //
    },
    error::to_result,
    num::casts,
    prelude::*, //
};

const BLOCK_SIZE: usize = casts::u32_as_usize(bindings::EDID_LENGTH);

/// A validated EDID allocation returned by DRM.
pub struct Edid(NonNull<bindings::drm_edid>);

impl Edid {
    /// Read EDID blocks synchronously, including DRM validation and overrides.
    ///
    /// The reader must fill the entire destination or return an error. DRM may
    /// retry a block. Firmware returning a complete EDID should serve every
    /// block from the same snapshot for this call.
    pub fn read<T: DriverConnector, F>(
        connector: &ConnectorProbeGuard<'_, T>,
        mut read_block: F,
    ) -> Result<Self>
    where
        F: FnMut(u32, &mut [u8]) -> Result,
    {
        // SAFETY: The probe guard retains both connector locks. DRM invokes
        // the reader synchronously and does not retain its stack context.
        let edid = unsafe {
            bindings::drm_edid_read_custom(
                connector.as_raw(),
                Some(read_callback::<F>),
                ptr::from_mut(&mut read_block).cast(),
            )
        };
        // The C API does not distinguish read, validation and allocation failures.
        NonNull::new(edid.cast_mut()).map(Self).ok_or(EINVAL)
    }
}

impl Drop for Edid {
    fn drop(&mut self) {
        // SAFETY: This owns exactly the allocation returned by the read helper.
        unsafe { bindings::drm_edid_free(self.0.as_ptr()) };
    }
}

impl<T: DriverConnector> ConnectorProbeGuard<'_, T> {
    /// Update sink information and properties, or clear a stale EDID with `None`.
    pub fn update_edid(&self, edid: Option<&Edid>) -> Result {
        // SAFETY: The probe guard holds both update locks. DRM copies the
        // supplied EDID while its allocation remains borrowed; NULL clears it.
        to_result(unsafe {
            bindings::drm_edid_connector_update(
                self.as_raw(),
                edid.map_or(ptr::null(), |edid| edid.0.as_ptr()),
            )
        })
    }

    /// Add modes from the EDID supplied by the preceding [`Self::update_edid`].
    pub fn add_edid_modes(&self) -> i32 {
        // SAFETY: The guard retains the connector and both mode-update locks.
        unsafe { bindings::drm_edid_connector_add_modes(self.as_raw()) }
    }
}

/// # Safety
///
/// DRM must call synchronously with the live reader passed by `Edid::read`,
/// with no overlapping calls and a separate writable EDID-block allocation.
unsafe extern "C" fn read_callback<F>(
    context: *mut c_void,
    buffer: *mut u8,
    block: c_uint,
    length: usize,
) -> c_int
where
    F: FnMut(u32, &mut [u8]) -> Result,
{
    if length != BLOCK_SIZE || buffer.is_null() {
        return EINVAL.to_errno();
    }
    // SAFETY: Edid::read exclusively owns this reader until the C call returns.
    let read = unsafe { &mut *context.cast::<F>() };
    // SAFETY: DRM's writable block may be uninitialized. Initialize all bytes
    // before presenting it as a readable Rust slice to the reader.
    unsafe { ptr::write_bytes(buffer, 0, length) };
    // SAFETY: The checked block length fits isize and its entire allocation is
    // initialized above. u8 requires no alignment and DRM grants exclusive access.
    let bytes = unsafe { slice::from_raw_parts_mut(buffer, length) };
    read(block, bytes).map_or_else(Error::to_errno, |()| 0)
}
