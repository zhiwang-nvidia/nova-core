// SPDX-License-Identifier: GPL-2.0

//! GEM objects backed by their displayable VRAM allocation.

use core::ops::Range;

use kernel::{
    auxiliary,
    device::Bound,
    devres::DevresLt,
    drm::{
        self,
        gem::{
            self,
            BaseObject, //
        },
        Registered, //
    },
    page,
    prelude::*,
    ptr::Alignment,
    sync::{
        aref::ARef,
        new_condvar,
        new_mutex,
        Arc,
        CondVar,
        Mutex, //
    },
    types::ForLt, //
};

use nova_core::api::{
    VramMapping,
    VramRegion, //
};

use crate::{
    driver::{
        NovaDevice,
        NovaDriver, //
    },
    file::File, //
};

/// Counts kernel mappings whose I/O pointers are still in use by GEM clients.
#[pin_data]
struct KernelMaps {
    #[pin]
    users: Mutex<usize>,
    #[pin]
    idle: CondVar,
}

/// BAR1 translations are torn down with the auxiliary binding, before the core GPU.
struct MappingResource<'bound> {
    mapping: VramMapping<'bound>,
    kernel_maps: Arc<KernelMaps>,
}

impl Drop for MappingResource<'_> {
    fn drop(&mut self) {
        let mut users = self.kernel_maps.users.lock();
        while *users != 0 {
            self.kernel_maps.idle.wait(&mut users);
        }
        // User PTEs are gone after the last GEM VMA closes or DRM unplug zaps them.
        // The last kernel vunmap lets this object's BAR1 slot be released.
    }
}

/// Physical backing outlives unbind; its CPU mapping is revoked at auxiliary teardown.
pub(crate) struct NovaObject {
    region: VramRegion,
    mapping: DevresLt<ForLt!(MappingResource<'_>)>,
    kernel_maps: Arc<KernelMaps>,
}

pub(crate) type Object = gem::Object<NovaObject>;

impl gem::DriverObject for NovaObject {
    type Driver = NovaDriver;
    type Args = ();

    const FBDEV_TTM: bool = true;

    fn new(dev: &NovaDevice, size: usize, _args: ()) -> impl PinInit<Self, Error> {
        Self::allocate(dev, size)
    }

    fn dumb_create(
        dev: &NovaDevice,
        file: &drm::File<File>,
        args: &mut gem::DumbCreate<'_>,
    ) -> Result {
        args.calculate_size(256, 0)?;
        let size = usize::try_from(args.size()).map_err(|_| EOVERFLOW)?;
        let object = Object::new_private(dev, size, ())?;
        args.set_handle(object.create_handle(file)?);
        Ok(())
    }

    fn mmap(obj: &Object, _mapping: &gem::Mmap<'_>) -> Result {
        let registration = obj.dev().registration_guard().ok_or(ENODEV)?;
        obj.with_mapping(&registration, |_| Ok(()))
    }

    fn fault(obj: &Object, fault: &gem::Fault<'_>) -> gem::FaultResult {
        let insert = || -> Result<gem::FaultResult> {
            let registration = obj.dev().registration_guard().ok_or(ENODEV)?;
            obj.with_mapping(&registration, |mapping| {
                let offset = fault.offset()?;
                let physical = mapping.physical_page_address(offset)?;
                let pfn = usize::try_from(physical >> page::PAGE_SHIFT).map_err(|_| EOVERFLOW)?;
                // SAFETY: The core checked this complete page against the live BAR1
                // mapping. The registration guard covers PTE insertion. Unplug
                // waits for the guard, then zaps DRM-file PTEs before auxiliary
                // devres releases the mapping. GEM VMA references prevent normal
                // object destruction and BAR1 slot reuse while this VMA survives.
                Ok(unsafe { fault.insert_pfn(pfn) })
            })
        };
        insert().unwrap_or(gem::FaultResult::SIGBUS)
    }

    fn vmap(obj: &Object) -> Result<gem::IoMapping> {
        let registration = obj.dev().registration_guard().ok_or(ENODEV)?;
        obj.with_mapping(&registration, |mapping| {
            let mut users = obj.kernel_maps.users.lock();
            let next = users.checked_add(1).ok_or(EOVERFLOW)?;
            // SAFETY: The bound registration prevents revocation during this
            // access. The count published below then keeps MappingResource's
            // destructor waiting until the matching vunmap callback.
            let pointer = unsafe { mapping.iomem_ptr()? };
            // SAFETY: BAR1 supplies an I/O pointer for the whole GEM object.
            // KernelMaps retains this mapping until the matching vunmap; GEM
            // clients retain their object reference for that same interval.
            let result = unsafe { gem::IoMapping::from_ptr(pointer) };
            *users = next;
            Ok(result)
        })
    }

    unsafe fn vunmap(obj: &Object, _mapping: gem::IoMapping) {
        let mut users = obj.kernel_maps.users.lock();
        let Some(remaining) = users.checked_sub(1) else {
            pr_warn!("nova-drm: unmatched GEM vunmap\n");
            return;
        };
        *users = remaining;
        if remaining == 0 {
            obj.kernel_maps.idle.notify_all();
        }
        // The GEM object owns KernelMaps independently of the revoked resource,
        // so waking its destructor cannot invalidate this callback's lock.
    }
}

impl NovaObject {
    fn allocate(dev: &NovaDevice, size: usize) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(page::PAGE_SIZE) {
            return Err(EINVAL);
        }
        let kernel_maps = Arc::pin_init(
            pin_init!(KernelMaps {
                users <- new_mutex!(0),
                idle <- new_condvar!(),
            }),
            GFP_KERNEL,
        )?;
        let registration = dev.registration_guard().ok_or(ENODEV)?;
        let auxiliary: &auxiliary::Device<Bound> = registration.as_ref();
        registration.registration_data_with(|reg| {
            let backing = reg.api.with(|core| {
                core.alloc_vram_aligned(size as u64, Alignment::new::<{ page::PAGE_SIZE }>())
            })?;
            let region = backing.region(0..backing.size())?;
            let mapping = reg
                .api
                .with(|core| core.get_ref().map_vram(region.clone(), true))?;
            // Never expose a previous owner's VRAM contents through dumb mmap.
            for offset in (0..size).step_by(size_of::<u32>()) {
                mapping.write32(0, offset)?;
            }
            mapping.read32(0)?;
            let resource = MappingResource {
                mapping,
                kernel_maps: kernel_maps.clone(),
            };
            // SAFETY: The mapping borrows the parent core GPU, which outlives
            // auxiliary unbind and its devres teardown. DRM unplug first stops
            // guarded faults and zaps its user PTEs. MappingResource waits for
            // kernel vmap leases before its BAR1 mapping is dropped.
            let mapping = unsafe { DevresLt::new(auxiliary.as_ref(), Ok::<_, Error>(resource))? };
            Ok(Self {
                region,
                mapping,
                kernel_maps,
            })
        })
    }

    fn with_mapping<R>(
        &self,
        dev: &NovaDevice<Registered>,
        f: impl for<'a> FnOnce(&VramMapping<'a>) -> Result<R>,
    ) -> Result<R> {
        // A previous binding's GEM object can survive a later bind of the same
        // auxiliary device. Check revocation before the sleepable bound access.
        self.mapping.try_access_with(|_| ()).ok_or(ENODEV)?;
        let auxiliary: &auxiliary::Device<Bound> = dev.as_ref();
        self.mapping
            .access_with(auxiliary.as_ref(), |resource| f(&resource.mapping))?
    }

    /// Borrows the current binding long enough to obtain owned framebuffer backing.
    pub(crate) fn region(
        &self,
        dev: &NovaDevice<Registered>,
        range: Range<u64>,
    ) -> Result<VramRegion> {
        self.with_mapping(dev, |_| self.region.subregion(range))
    }

    /// Creates a userspace GEM allocation in VRAM.
    pub(crate) fn new(dev: &NovaDevice, size: usize) -> Result<ARef<Object>> {
        if size == 0 {
            return Err(EINVAL);
        }
        let size = page::page_align(size).ok_or(EOVERFLOW)?;
        Object::new_private(dev, size, ())
    }

    /// Looks up a GEM object retained by this DRM file.
    pub(crate) fn lookup_handle(file: &drm::File<File>, handle: u32) -> Result<ARef<Object>> {
        Object::lookup_handle(file, handle)
    }
}
