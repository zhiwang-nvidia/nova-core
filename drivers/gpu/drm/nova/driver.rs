// SPDX-License-Identifier: GPL-2.0

use kernel::{
    auxiliary,
    device::{
        Core,
        DeviceContext, //
    },
    drm::{
        self,
        ioctl, //
    },
    prelude::*,
    sync::{
        aref::ARef,
        new_mutex,
        Mutex, //
    }, //
};

use crate::{
    display::{
        Display,
        DisplayOutput, //
    },
    file::File,
    gem::Object, //
};

use nova_core::api::{
    NovaCoreApi,
    NovaCoreApiHandle, //
};

pub(crate) struct NovaDriver;

pub(crate) struct NovaData {
    pub(crate) outputs: KVec<DisplayOutput>,
}

#[pin_data]
pub(crate) struct Nova<'bound> {
    drm: ARef<drm::Device<NovaDriver>>,
    #[pin]
    reg: drm::Registration<'bound, NovaDriver>,
}

/// DRM registration data, accessible while its parent remains bound.
#[pin_data(PinnedDrop)]
pub(crate) struct DrmRegData<'bound> {
    pub(crate) api: NovaCoreApiHandle<'bound>,
    #[pin]
    pub(crate) display: Mutex<Option<Display<'bound>>>,
}

#[pinned_drop]
impl PinnedDrop for DrmRegData<'_> {
    fn drop(self: Pin<&mut Self>) {
        if let Some(display) = self.display.lock().take() {
            display.shutdown();
        }
    }
}

/// Convienence type alias for the DRM device type for this driver
pub(crate) type NovaDevice<Ctx = drm::Normal> = drm::Device<NovaDriver, Ctx>;

const INFO: drm::DriverInfo = drm::DriverInfo {
    major: 0,
    minor: 0,
    patchlevel: 0,
    name: c"nova-drm",
    desc: c"NVIDIA Graphics and Compute",
};

const NOVA_CORE_MODULE_NAME: &CStr = c"nova-core";
const AUXILIARY_NAME: &CStr = c"nova-drm";

kernel::auxiliary_device_table!(
    AUX_TABLE,
    <NovaDriver as auxiliary::Driver>::IdInfo,
    [(
        auxiliary::DeviceId::new(NOVA_CORE_MODULE_NAME, AUXILIARY_NAME),
        ()
    )]
);

impl auxiliary::Driver for NovaDriver {
    type IdInfo = ();
    type Data<'bound> = Nova<'bound>;
    const ID_TABLE: auxiliary::IdTable<Self::IdInfo> = &AUX_TABLE;

    fn probe<'bound>(
        adev: &'bound auxiliary::Device<Core<'_>>,
        _info: &'bound Self::IdInfo,
    ) -> impl PinInit<Self::Data<'bound>, Error> + 'bound {
        try_pin_init!(Self::Data {
            reg <- {
                let api = NovaCoreApi::of(adev)?;
                let display = Display::init(adev)?;
                let outputs = display.outputs()?;
                let drm =
                    drm::UnregisteredDevice::<Self>::new(adev, Ok(NovaData { outputs }))?;
                let reg_data = try_pin_init!(DrmRegData {
                    api,
                    display <- new_mutex!(Some(display)),
                });

                // SAFETY: `reg` is stored in `Self::Data` and dropped when the driver is unbound;
                // it is never forgotten.
                unsafe { drm::Registration::new(adev.as_ref(), drm, reg_data) }
            },
            drm: reg.device().into(),
        })
    }
}

#[vtable]
impl drm::Driver for NovaDriver {
    type Data = NovaData;
    type RegistrationData<'a> = DrmRegData<'a>;
    type File = File;
    type Object = Object;
    #[cfg(CONFIG_RUST_DRM_KMS)]
    type Kms = Self;
    type ParentDevice<Ctx: DeviceContext> = auxiliary::Device<Ctx>;

    const INFO: drm::DriverInfo = INFO;
    const FEAT_RENDER: bool = true;

    kernel::declare_drm_ioctls! {
        (NOVA_INFO, drm_nova_info, ioctl::RENDER_ALLOW, File::info),
        (NOVA_GEM_CREATE, drm_nova_gem_create, ioctl::AUTH | ioctl::RENDER_ALLOW, File::gem_create),
        (NOVA_GEM_INFO, drm_nova_gem_info, ioctl::AUTH | ioctl::RENDER_ALLOW, File::gem_info),
    }
}
