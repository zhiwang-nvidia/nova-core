// SPDX-License-Identifier: GPL-2.0

//! Abstractions for PCI Single Root I/O Virtualization (SR-IOV) drivers.

use super::Device as PciDevice;
use crate::{
    bindings,
    device, //
    prelude::*,
    types::{
        CovariantForLt,
        ForLt, //
    },
};
use core::{
    any::TypeId,
    marker::PhantomPinned,
    num::NonZero, //
};

/// A PCI Physical Function (PF) with an SR-IOV capability.
///
/// This capability view is created only after the PCI abstraction verifies that the device is an
/// SR-IOV PF. Its device context follows the same hierarchy as [`PciDevice`].
#[repr(transparent)]
pub struct Device<Ctx: device::DeviceContext = device::Normal>(PciDevice<Ctx>);

impl<Ctx: device::DeviceContext> Device<Ctx> {
    pub(super) fn try_from_pci(pdev: &PciDevice<Ctx>) -> Result<&Self> {
        // SAFETY: `pdev.as_raw()` is a valid pointer to a `struct pci_dev`.
        if unsafe { (*pdev.as_raw()).is_physfn() == 0 } {
            return Err(ENODEV);
        }

        // CAST: `Device` is a transparent capability view of `PciDevice` with the same context.
        // SAFETY: The check above establishes the PF invariant, and the returned reference cannot
        // outlive `pdev`.
        Ok(unsafe { &*core::ptr::from_ref(pdev).cast() })
    }

    /// Returns the underlying PCI device with the same device context.
    #[inline]
    pub fn as_pci(&self) -> &PciDevice<Ctx> {
        &self.0
    }
}

impl<Ctx: device::DeviceContext> AsRef<PciDevice<Ctx>> for Device<Ctx> {
    #[inline]
    fn as_ref(&self) -> &PciDevice<Ctx> {
        self.as_pci()
    }
}

impl<Ctx: device::DeviceContext> AsRef<device::Device<Ctx>> for Device<Ctx> {
    #[inline]
    fn as_ref(&self) -> &device::Device<Ctx> {
        self.as_pci().as_ref()
    }
}

impl<'a> Device<device::Core<'a>> {
    /// Returns the total number of VFs, or [`None`] if SR-IOV is unavailable.
    #[inline]
    pub fn total_vfs(&self) -> Option<NonZero<u16>> {
        self.as_pci().sriov_get_totalvfs()
    }

    /// Enables `nr_virtfn` Virtual Functions (VFs).
    #[inline]
    pub fn enable_sriov(&self, nr_virtfn: i32) -> Result {
        self.as_pci().enable_sriov(nr_virtfn)
    }

    /// Disables all Virtual Functions (VFs).
    #[inline]
    pub fn disable_sriov(&self) {
        self.as_pci().disable_sriov();
    }
}

impl Device<device::Bound> {
    /// Returns the number of currently enabled Virtual Functions (VFs).
    #[inline]
    pub fn num_vfs(&self) -> i32 {
        self.as_pci().num_vf()
    }
}

// SAFETY: `Device` is a transparent wrapper around `PciDevice`, and neither type's layout
// depends on its device context.
kernel::impl_device_context_deref!(unsafe { Device });

#[repr(C)]
#[pin_data]
struct VfRegistrationData<'a, F: ForLt + 'static> {
    type_id: TypeId,
    #[pin]
    data: F::Of<'a>,
}

static_assert!(
    core::mem::offset_of!(VfRegistrationData<'static, CovariantForLt!(())>, type_id) == 0
);

impl<'a, F: ForLt + 'static> VfRegistrationData<'a, F> {
    fn new<D>(data: D) -> impl PinInit<Self, Error> + use<'a, D, F>
    where
        D: PinInit<F::Of<'a>, Error> + 'a,
    {
        try_pin_init!(Self {
            type_id: TypeId::of::<F>(),
            data <- data,
        })
    }
}

/// Typed data published by a Physical Function (PF) for its Virtual Functions (VFs).
///
/// The registration is initialized in place as part of the PF driver's pinned data. On an SR-IOV
/// PF it publishes the inline payload after initialization; on a conventional PCI function it is
/// inactive. A VF is rejected. Bound VFs access the payload through
/// [`PciDevice::vf_registration_data()`] or [`PciDevice::vf_registration_data_with()`].
///
/// Managed SR-IOV removes all VFs before the PF driver is unbound. As a fallback, pinned drop also
/// disables SR-IOV before withdrawing the payload.
#[pin_data(PinnedDrop)]
pub struct VfRegistration<'a, F: ForLt + 'static> {
    pdev: &'a PciDevice<device::Bound>,
    #[pin]
    inner: VfRegistrationData<'a, F>,
    published: bool,
    #[pin]
    _pin: PhantomPinned,
}

impl<'a, F: ForLt + 'static> VfRegistration<'a, F>
where
    for<'b> F::Of<'b>: Send + Sync,
{
    /// Publishes typed PF data for bound VF drivers.
    ///
    /// This returns a pin-initializer so the registration and payload can be embedded directly in
    /// the PF driver's pinned data.
    ///
    /// Initialization returns [`ENODEV`] for a VF and [`EBUSY`] if the PF has enabled VFs or
    /// already has a registration.
    ///
    /// # Safety
    ///
    /// The caller must invoke this during the PCI driver's probe and embed the result in the driver
    /// data. On an SR-IOV PF, no VF may be enabled before probe successfully installs the complete
    /// driver data, and the driver must use managed SR-IOV. The registration must be dropped before
    /// anything its payload borrows and must not be forgotten. Probe must have exclusive access to
    /// the PF registration slot. On a conventional PCI function, the registration remains
    /// inactive.
    pub unsafe fn new<'core, D>(
        pdev: &'a PciDevice<device::Core<'core>>,
        data: D,
    ) -> impl PinInit<Self, Error> + use<'a, 'core, D, F>
    where
        D: PinInit<F::Of<'a>, Error> + 'a,
    {
        pin_init::pin_init_scope(move || {
            if pdev.is_virtfn() {
                return Err(ENODEV);
            }

            let published = pdev.is_physfn();
            if published {
                if pdev.num_vf() != 0 {
                    return Err(EBUSY);
                }

                if !pdev.vf_registration_data_rust().is_null() {
                    return Err(EBUSY);
                }
            }

            Ok(try_pin_init!(Self {
                pdev,
                inner <- VfRegistrationData::new(data),
                published,
                _pin: PhantomPinned,
                _: {
                    if *published {
                        pdev.set_vf_registration_data_rust(
                            core::ptr::from_ref(inner.as_ref().get_ref()).cast_mut().cast(),
                        );
                    }
                },
            }))
        })
    }
}

#[pinned_drop]
impl<F: ForLt + 'static> PinnedDrop for VfRegistration<'_, F> {
    fn drop(self: Pin<&mut Self>) {
        if !self.published {
            return;
        }

        // SAFETY: `self.pdev` is the PF on which this registration was published. The call is a
        // no-op on the normal managed-SR-IOV teardown path, where all VFs are already disabled.
        unsafe { bindings::pci_disable_sriov(self.pdev.as_raw()) };
        self.pdev
            .set_vf_registration_data_rust(core::ptr::null_mut());
    }
}

// SAFETY: The registration and its inline payload may be released from another thread after the
// PCI core has removed all VFs.
unsafe impl<F: ForLt> Send for VfRegistration<'_, F> where for<'a> F::Of<'a>: Send {}

// SAFETY: VF consumers receive shared references only, and the payload supports shared access.
unsafe impl<F: ForLt> Sync for VfRegistration<'_, F> where for<'a> F::Of<'a>: Send + Sync {}

impl<Ctx: device::DeviceContext> PciDevice<Ctx> {
    fn vf_registration_data_rust(&self) -> *mut core::ffi::c_void {
        // SAFETY: `self.as_raw()` is a valid pointer to a `struct pci_dev`.
        unsafe { (*self.as_raw()).vf_registration_data_rust }
    }

    fn set_vf_registration_data_rust(&self, data: *mut core::ffi::c_void) {
        // SAFETY: Publication and withdrawal are serialized by PCI probe and managed teardown.
        unsafe { (*self.as_raw()).vf_registration_data_rust = data };
    }
}

impl PciDevice<device::Bound> {
    /// # Safety
    ///
    /// The returned borrow must be confined by a closure higher-ranked independently over its
    /// borrow and data lifetimes, or `F` must be covariant in its encoded lifetime.
    unsafe fn vf_registration_data_pinned<F: ForLt + 'static>(&self) -> Result<Pin<&F::Of<'_>>> {
        if !self.is_virtfn() {
            return Err(ENODEV);
        }

        // SAFETY: A VF's `physfn` pointer remains valid for the VF's lifetime. Managed SR-IOV also
        // keeps the PF driver bound until this VF is unbound.
        let pf_dev = unsafe { (*self.as_raw()).__bindgen_anon_1.physfn };
        if pf_dev.is_null() {
            return Err(ENODEV);
        }

        // SAFETY: The PF cannot withdraw the pointer until managed teardown has removed this VF.
        let ptr = unsafe { (*pf_dev).vf_registration_data_rust };
        if ptr.is_null() {
            return Err(ENOENT);
        }

        // SAFETY: The published pointer addresses a `VfRegistrationData`, whose first field is a
        // `TypeId`.
        let type_id = unsafe { ptr.cast::<TypeId>().read() };
        if type_id != TypeId::of::<F>() {
            return Err(EINVAL);
        }

        // SAFETY: The type check identifies `F`; lifetime parameters do not affect layout, and the
        // inline data remains pinned for this VF borrow.
        let data = unsafe {
            let registration = ptr.cast::<VfRegistrationData<'_, F>>();
            &raw const (*registration).data
        };

        // SAFETY: `data` is structurally pinned in the PF driver's pinned registration.
        Ok(unsafe { Pin::new_unchecked(&*data) })
    }

    /// Accesses typed data published by this VF's PF through a closure.
    ///
    /// Returns [`ENODEV`] if this device is not a VF, [`ENOENT`] if its PF has not published
    /// data, or [`EINVAL`] if the registered type does not match `F`.
    ///
    /// The closure's borrow and the registration data's lifetime are independent, so a borrow of
    /// the context cannot be stored in invariant registration data.
    pub fn vf_registration_data_with<F: ForLt + 'static, R>(
        &self,
        f: impl for<'borrow, 'data> FnOnce(Pin<&'borrow F::Of<'data>>) -> R,
    ) -> Result<R> {
        // SAFETY: The higher-ranked closure prevents the borrow from escaping or being stored in
        // invariant data by keeping its lifetime independent of the erased data lifetime.
        let data = unsafe { self.vf_registration_data_pinned::<F>()? };
        Ok(f(data))
    }

    /// Returns typed data published by this VF's PF.
    ///
    /// This direct accessor is available only when the encoded data is covariant in its lifetime.
    /// Use [`Self::vf_registration_data_with()`] for invariant data.
    ///
    /// It returns the same errors as [`Self::vf_registration_data_with()`].
    pub fn vf_registration_data<F: CovariantForLt + 'static>(&self) -> Result<Pin<&F::Of<'_>>> {
        // SAFETY: `CovariantForLt` permits shortening the encoded lifetime to this borrow.
        unsafe { self.vf_registration_data_pinned::<F>() }
    }
}
