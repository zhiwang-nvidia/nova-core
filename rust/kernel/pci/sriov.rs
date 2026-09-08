// SPDX-License-Identifier: GPL-2.0

//! Abstractions for PCI Single Root I/O Virtualization (SR-IOV) drivers.

use super::{
    Device as PciDevice,
    Driver,
    IdTable, //
};
use crate::{
    device,
    interop::ffi::{
        Abi,
        Descriptor, //
    },
    prelude::*,
    types::{
        CovariantForLt,
        ForLt,
        ForeignOwnable, //
    },
    ModuleMetadata, //
};
use core::{
    any::TypeId,
    marker::PhantomData,
    num::NonZero, //
};

/// A PCI Physical Function (PF) with an SR-IOV capability.
///
/// This is a capability view of the underlying [`PciDevice`]. It is created only after the PCI
/// abstraction verifies that the device is an SR-IOV PF. Its device context follows the same
/// hierarchy as [`PciDevice`], so a [`Device<device::Core>`] also provides the guarantees of
/// [`Device<device::Bound>`].
#[repr(transparent)]
pub struct Device<Ctx: device::DeviceContext = device::Normal>(PciDevice<Ctx>);

impl<Ctx: device::DeviceContext> Device<Ctx> {
    pub(super) fn try_from_pci(pdev: &PciDevice<Ctx>) -> Result<&Self> {
        // SAFETY: `pdev.as_raw()` is a valid pointer to a `struct pci_dev`.
        if unsafe { (*pdev.as_raw()).is_physfn() == 0 } {
            return Err(ENODEV);
        }

        // CAST: `Device` is a transparent capability view of `PciDevice` with the same context.
        // SAFETY: The check above establishes the additional invariant that `pdev` is an SR-IOV
        // PF, and the returned reference cannot outlive `pdev`.
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

// SAFETY: `Device` is a transparent wrapper around `PciDevice`, and neither type's layout depends
// on its device context.
kernel::impl_device_context_deref!(unsafe { Device });

#[repr(C)]
#[pin_data]
struct SriovPfRegistrationData<T> {
    ffi: Descriptor,
    type_id: TypeId,
    #[pin]
    data: T,
}

static_assert!(core::mem::offset_of!(SriovPfRegistrationData<()>, ffi) == 0);

/// Typed data published by a Physical Function (PF) for its Virtual Functions (VFs).
///
/// The type parameter `F` is a [`ForLt`](trait@ForLt) encoding of the data type. The data is owned
/// by the registration and can be accessed by a bound VF through
/// [`PciDevice::pf_registration_data()`] or [`PciDevice::pf_registration_data_with()`].
/// Managed SR-IOV creates a persistent device link with each VF as consumer and the PF as supplier
/// before probing the VF. The driver core therefore unbinds every consuming VF before unbinding the
/// PF driver and destroying this registration.
///
/// # Invariants
///
/// `self.pdev` is a bound PF whose `sriov_registration_data_rust` field points to a valid
/// `Pin<KBox<SriovPfRegistrationData<F::Of<'static>>>>`. The registration is dropped only after
/// SR-IOV has been disabled and every VF has been fully unbound, including destruction of its
/// driver data.
pub struct SriovPfRegistration<'a, F: ForLt + 'static> {
    pdev: &'a PciDevice<device::Bound>,
    _phantom: PhantomData<F::Of<'a>>,
}

impl<'a, F: ForLt> SriovPfRegistration<'a, F>
where
    for<'b> F::Of<'b>: Send + Sync,
{
    /// # Safety
    ///
    /// The caller must ensure exclusive access to the PF registration slot and keep the returned
    /// registration alive until all VFs have been removed. VFs must not be enabled before this
    /// function returns.
    unsafe fn new_with_descriptor<E>(
        pdev: &'a PciDevice<device::Bound>,
        data: impl PinInit<F::Of<'a>, E>,
        make_descriptor: impl FnOnce(Pin<&F::Of<'a>>) -> Descriptor,
    ) -> Result<Self>
    where
        Error: From<E>,
    {
        if !pdev.is_physfn() {
            return Err(EINVAL);
        }

        if pdev.num_vf() != 0 {
            return Err(EBUSY);
        }

        // SAFETY: `pdev.as_raw()` is a valid pointer to a `struct pci_dev`.
        let slot = unsafe { &raw mut (*pdev.as_raw()).sriov_registration_data_rust };

        // SAFETY: The caller guarantees exclusive control while creating the registration.
        if !unsafe { slot.read() }.is_null() {
            return Err(EBUSY);
        }

        let mut data = KBox::pin_init::<Error>(
            try_pin_init!(SriovPfRegistrationData {
                ffi: Descriptor::default(),
                type_id: TypeId::of::<F>(),
                data <- data,
            }),
            GFP_KERNEL,
        )?;

        let projection = data.as_mut().project();
        *projection.ffi = make_descriptor(projection.data.as_ref());

        // SAFETY: `'a` is tracked by `SriovPfRegistration`'s `PhantomData`. Lifetimes do not
        // affect layout, so both `SriovPfRegistrationData` instantiations have identical
        // representations.
        let data: Pin<KBox<SriovPfRegistrationData<F::Of<'static>>>> =
            unsafe { core::mem::transmute(data) };

        // SAFETY:
        // - the caller guarantees exclusive control over the slot;
        // - `data` is fully initialized and remains owned by the returned registration.
        unsafe { slot.write(data.into_foreign()) };

        Ok(Self {
            pdev,
            _phantom: PhantomData,
        })
    }

    /// Publishes typed data owned by a PF for use by its bound VFs.
    ///
    /// The data is published only after it has been fully initialized and pinned. VFs must be
    /// enabled only from the managed PCI driver's `sriov_configure` callback, after PF probe has
    /// succeeded and the PCI adapter has installed the complete driver data.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    ///
    /// - this is called from the PF probe callback, while the driver core serializes access to
    ///   `pdev` and before SR-IOV is enabled;
    /// - VFs are not enabled until the PF probe and installation of its driver data have completed;
    /// - the returned registration is neither dropped, replaced, nor forgotten until SR-IOV has
    ///   been disabled and every VF is fully unbound, including destruction of its driver data;
    ///   and
    /// - no other [`SriovPfRegistration`] exists for `pdev`.
    ///
    /// Storing the registration in the PF driver's private data satisfies the lifetime
    /// requirement when the driver uses managed SR-IOV and does not remove or replace the field.
    /// If the data accesses other PF-private fields, declare the registration first so
    /// that it is unpublished and dropped before those fields.
    pub unsafe fn new_with_lt<E>(
        pdev: &'a PciDevice<device::Bound>,
        data: impl PinInit<F::Of<'a>, E>,
    ) -> Result<Self>
    where
        Error: From<E>,
    {
        // SAFETY: The caller upholds the registration and PF/VF lifetime requirements documented
        // by this function. An unavailable descriptor exposes no C ABI operations.
        unsafe { Self::new_with_descriptor(pdev, data, |_| Descriptor::default()) }
    }

    /// Publishes typed PF data that is also callable through an FFI operations table.
    ///
    /// `A` defines the stable token, ABI version, and operations table exposed to C consumers.
    /// Rust VFs may continue to borrow the same data through
    /// [`PciDevice::pf_registration_data`] or [`PciDevice::pf_registration_data_with`].
    ///
    /// # Safety
    ///
    /// The caller must uphold all requirements of [`Self::new_with_lt`]. In addition, every C
    /// consumer must stop using the borrowed descriptor, operations table, and context before its
    /// VF remove callback returns.
    pub unsafe fn new_ffi_with_lt<A, E>(
        pdev: &'a PciDevice<device::Bound>,
        data: impl PinInit<F::Of<'a>, E>,
    ) -> Result<Self>
    where
        A: Abi<Context = F>,
        Error: From<E>,
    {
        // SAFETY: The caller upholds the registration, PF/VF, and C consumer lifetime requirements
        // documented by this function. `A::Context = F` ties the generated operations table to the
        // concrete pinned data stored in this registration.
        unsafe { Self::new_with_descriptor(pdev, data, |context| Descriptor::new::<A>(context)) }
    }
}

impl<F: ForLt> Drop for SriovPfRegistration<'_, F> {
    fn drop(&mut self) {
        // SAFETY: `self.pdev.as_raw()` is a valid pointer to a `struct pci_dev`.
        let slot = unsafe { &raw mut (*self.pdev.as_raw()).sriov_registration_data_rust };

        // SAFETY: By the type invariant, the slot contains the pointer stored by
        // `new_with_descriptor()`, and no VF can access it while this registration is being
        // dropped.
        let ptr = unsafe { slot.replace(core::ptr::null_mut()) };

        // SAFETY: `ptr` was produced by `into_foreign()` in `new_with_descriptor()` and is no
        // longer published to VF devices.
        drop(unsafe { Pin::<KBox<SriovPfRegistrationData<F::Of<'static>>>>::from_foreign(ptr) });
    }
}

// SAFETY: A PF registration may be released from any thread after all VFs have been removed.
unsafe impl<F: ForLt> Send for SriovPfRegistration<'_, F> where for<'a> F::Of<'a>: Send {}

// SAFETY: `SriovPfRegistration` exposes no operations through a shared reference. Its descriptor
// points to a static operations table and to the pinned data, which supports shared access.
unsafe impl<F: ForLt> Sync for SriovPfRegistration<'_, F> where for<'a> F::Of<'a>: Send + Sync {}

/// An SR-IOV Physical Function (PF) driver.
///
/// The adapter rejects VFs but permits conventional PCI functions, allowing one PF-side driver to
/// support devices both with and without an SR-IOV capability. [`Self::sriov_configure`] is invoked
/// only for an SR-IOV PF.
///
/// If VFs consume PF data, the PF [`Self::probe`] implementation must publish it through a
/// [`SriovPfRegistration`] before enabling VFs. The registration must be stored in [`Self::Data`]
/// so that managed SR-IOV removes every VF before the data is dropped.
pub trait SriovPfDriver {
    /// The type holding information about each device ID supported by the PF driver.
    type IdInfo: 'static;

    /// The PF driver's private data.
    type Data<'bound>: Send + 'bound;

    /// The table of PF device IDs supported by the driver.
    const ID_TABLE: IdTable<Self::IdInfo>;

    /// Probes a PF-side PCI function.
    fn probe<'bound>(
        dev: &'bound PciDevice<device::Core<'_>>,
        id_info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound;

    /// Unbinds a PF-side PCI function.
    fn unbind<'bound>(dev: &'bound PciDevice<device::Core<'_>>, this: Pin<&Self::Data<'bound>>) {
        let _ = (dev, this);
    }

    /// Enables or disables the PF's SR-IOV capability.
    ///
    /// The PCI core invokes this callback when userspace writes the number of Virtual Functions
    /// (VFs), or zero, to this PF's `sriov_numvfs` sysfs file. For managed SR-IOV it is also called
    /// with zero before [`Self::unbind`] when the PF still has enabled VFs.
    ///
    /// `dev` is an SR-IOV PF in the [`device::Core`] callback context and can be converted to the
    /// underlying PCI device through [`Device::as_pci`]. `this` is the private driver data returned
    /// by [`Self::probe`]. Both remain valid for the duration of the callback.
    ///
    /// Upon success, this callback must return the number of VFs that were enabled, or zero if
    /// SR-IOV was disabled.
    ///
    /// # Examples
    ///
    /// ```
    /// # use kernel::{device::Core, pci, prelude::*};
    /// # struct Data;
    /// fn sriov_configure(
    ///     dev: &pci::sriov::Device<Core<'_>>,
    ///     _this: Pin<&Data>,
    ///     nr_virtfn: i32,
    /// ) -> Result<i32> {
    ///     if nr_virtfn == 0 {
    ///         dev.disable_sriov();
    ///     } else {
    ///         dev.enable_sriov(nr_virtfn)?;
    ///     }
    ///
    ///     Ok(nr_virtfn)
    /// }
    /// ```
    fn sriov_configure<'bound>(
        dev: &'bound Device<device::Core<'_>>,
        this: Pin<&Self::Data<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32>;
}

/// An SR-IOV Virtual Function (VF) driver backed by typed PF data.
///
/// The adapter verifies that the device is a VF, obtains the data published by its PF, and passes
/// that data to [`Self::probe`]. A VF driver therefore does not need access to the PF
/// device or its complete private driver data.
///
/// PF and VF drivers may be registered by separate modules. The VF driver's [`Self::PfData`] must
/// be the same [`ForLt`](trait@ForLt) encoding used by the PF's [`SriovPfRegistration`]. Drivers in
/// separate Rust crates should define their PF data type in a crate visible to both; look-alike
/// types declared independently have distinct [`TypeId`]s even when their layouts match.
pub trait SriovVfDriver {
    /// The type holding information about each device ID supported by the VF driver.
    type IdInfo: 'static;

    /// The VF driver's private data.
    type Data<'bound>: Send + 'bound;

    /// The typed data published by the PF driver.
    type PfData: CovariantForLt + 'static;

    /// The table of VF device IDs supported by the driver.
    const ID_TABLE: IdTable<Self::IdInfo>;

    /// Probes an SR-IOV VF with the typed data published by its PF.
    fn probe<'bound>(
        dev: &'bound PciDevice<device::Core<'_>>,
        pf_data: Pin<&'bound <Self::PfData as ForLt>::Of<'bound>>,
        id_info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound;

    /// Unbinds an SR-IOV VF.
    fn unbind<'bound>(dev: &'bound PciDevice<device::Core<'_>>, this: Pin<&Self::Data<'bound>>) {
        let _ = (dev, this);
    }
}

#[doc(hidden)]
pub struct PfAdapter<T: SriovPfDriver, M: ModuleMetadata>(T, PhantomData<M>);

#[vtable]
impl<T: SriovPfDriver, M: ModuleMetadata> Driver for PfAdapter<T, M> {
    type OwnerModule = M;
    type IdInfo = T::IdInfo;
    type Data<'bound> = T::Data<'bound>;

    const ID_TABLE: IdTable<Self::IdInfo> = T::ID_TABLE;

    fn probe<'bound>(
        dev: &'bound PciDevice<device::Core<'_>>,
        id_info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            if dev.is_virtfn() {
                return Err(ENODEV);
            }

            Ok(T::probe(dev, id_info))
        })
    }

    fn unbind<'bound>(dev: &'bound PciDevice<device::Core<'_>>, this: Pin<&Self::Data<'bound>>) {
        T::unbind(dev, this);
    }

    fn sriov_configure<'bound>(
        dev: &'bound Device<device::Core<'_>>,
        this: Pin<&Self::Data<'bound>>,
        nr_virtfn: i32,
    ) -> Result<i32> {
        T::sriov_configure(dev, this, nr_virtfn)
    }
}

#[doc(hidden)]
pub struct VfAdapter<T: SriovVfDriver, M: ModuleMetadata>(T, PhantomData<M>);

#[vtable]
impl<T: SriovVfDriver, M: ModuleMetadata> Driver for VfAdapter<T, M> {
    type OwnerModule = M;
    type IdInfo = T::IdInfo;
    type Data<'bound> = T::Data<'bound>;

    const ID_TABLE: IdTable<Self::IdInfo> = T::ID_TABLE;

    fn probe<'bound>(
        dev: &'bound PciDevice<device::Core<'_>>,
        id_info: Option<&'bound Self::IdInfo>,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        pin_init::pin_init_scope(move || {
            if !dev.is_virtfn() {
                return Err(ENODEV);
            }

            let bound: &'bound PciDevice<device::Bound> = dev;
            let pf_data = bound.pf_registration_data::<T::PfData>()?;

            Ok(T::probe(dev, pf_data, id_info))
        })
    }

    fn unbind<'bound>(dev: &'bound PciDevice<device::Core<'_>>, this: Pin<&Self::Data<'bound>>) {
        T::unbind(dev, this);
    }
}

/// Declares a kernel module that exposes a single PF-side PCI driver.
///
/// The module metadata `name` is also used as the PCI driver name.
#[macro_export]
macro_rules! module_pci_sriov_pf_driver {
    ($($f:tt)*) => {
        $crate::module_driver!(
            <T>,
            $crate::pci::Adapter<$crate::pci::sriov::PfAdapter<T, DriverModule>>,
            { $($f)* }
        );
    };
}

/// Declares a kernel module that exposes a single SR-IOV VF driver.
///
/// The module metadata `name` is also used as the PCI driver name.
#[macro_export]
macro_rules! module_pci_sriov_vf_driver {
    ($($f:tt)*) => {
        $crate::module_driver!(
            <T>,
            $crate::pci::Adapter<$crate::pci::sriov::VfAdapter<T, DriverModule>>,
            { $($f)* }
        );
    };
}

impl PciDevice<device::Bound> {
    /// # Safety
    ///
    /// Callers must ensure that borrowing storage typed as `F::Of<'static>` as `F::Of<'_>` is
    /// sound, for example through a higher-ranked closure or the [`trait@CovariantForLt`]
    /// guarantee.
    unsafe fn pf_registration_data_pinned<F: ForLt + 'static>(&self) -> Result<Pin<&F::Of<'_>>> {
        if !self.is_virtfn() {
            return Err(EINVAL);
        }

        // SAFETY: `self.as_raw()` is a valid VF `struct pci_dev`, so `physfn` is a valid pointer
        // to its PF for at least as long as the VF exists.
        let pf_dev = unsafe { (*self.as_raw()).__bindgen_anon_1.physfn };

        // SAFETY: `pf_dev` is valid as explained above. A non-null value is owned by a
        // `SriovPfRegistration` which remains alive until this VF is fully unbound.
        let ptr = unsafe { (*pf_dev).sriov_registration_data_rust };
        if ptr.is_null() {
            return Err(ENOENT);
        }

        // SAFETY: `ptr` is non-null and points to `SriovPfRegistrationData`. Its `repr(C)` layout
        // makes the offset of `type_id` independent of the trailing data type.
        let type_id = unsafe {
            core::ptr::addr_of!((*ptr.cast::<SriovPfRegistrationData<()>>()).type_id).read()
        };
        if type_id != TypeId::of::<F>() {
            return Err(EINVAL);
        }

        // SAFETY: The `TypeId` check proves that the stored type has the `F` encoding. Lifetime
        // parameters do not affect layout, and the registration remains valid for this bound VF
        // borrow.
        let wrapper = unsafe { Pin::<KBox<SriovPfRegistrationData<F::Of<'_>>>>::borrow(ptr) };

        // SAFETY: `data` is structurally pinned by `SriovPfRegistrationData`.
        Ok(unsafe { wrapper.map_unchecked(|wrapper| &wrapper.data) })
    }

    /// Accesses the typed data published by this VF's PF through a closure.
    ///
    /// Use this method for any [`ForLt`](trait@ForLt) data. For covariant data, the direct
    /// [`Self::pf_registration_data`] accessor is more convenient.
    ///
    /// Returns [`EINVAL`] if this device is not a VF or `F` does not match the registered type,
    /// and [`ENOENT`] if the PF has not published Rust data.
    pub fn pf_registration_data_with<F: ForLt + 'static, R>(
        &self,
        f: impl for<'a> FnOnce(Pin<&'a F::Of<'a>>) -> R,
    ) -> Result<R> {
        // SAFETY: The higher-ranked closure prevents references with a concrete short lifetime
        // from escaping, so the stored `'static` lifetime may be replaced with the callback
        // lifetime even for invariant types.
        let data = unsafe { self.pf_registration_data_pinned::<F>()? };

        Ok(f(data))
    }

    /// Returns the typed data published by this VF's PF as a pinned reference.
    ///
    /// This direct accessor is available only for types that implement
    /// [`trait@CovariantForLt`]. Use [`Self::pf_registration_data_with`] for invariant data
    /// types.
    ///
    /// Returns [`EINVAL`] if this device is not a VF or `F` does not match the registered type,
    /// and [`ENOENT`] if the PF has not published Rust data.
    pub fn pf_registration_data<F: CovariantForLt + 'static>(&self) -> Result<Pin<&F::Of<'_>>> {
        // SAFETY: `CovariantForLt` guarantees that shortening the stored lifetime is sound.
        unsafe { self.pf_registration_data_pinned::<F>() }
    }
}
