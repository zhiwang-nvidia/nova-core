// SPDX-License-Identifier: GPL-2.0
//! A port of the VKMS driver to rust.
pub(crate) mod connector;
pub(crate) mod crtc;
pub(crate) mod encoder;
pub(crate) mod file;
pub(crate) mod gem;
pub(crate) mod output;
pub(crate) mod plane;

use kernel::{
    device,
    drm::{
        self, driver,
        kms::{atomic::*, KmsDriver, ModeConfigInfo, UnregisteredKmsDevice},
    },
    faux,
    prelude::*,
    str::CStr,
};

/// Convienence type alias for the DRM device type for this driver
pub(crate) type RvkmsDevice = drm::device::Device<RvkmsDriver>;

/// The name of the driver
const NAME: &CStr = c"rvkms";

/// Driver metadata
const INFO: driver::DriverInfo = driver::DriverInfo {
    major: 0,
    minor: 0,
    patchlevel: 0,
    name: NAME,
    desc: c"Rust VKMS PoC",
};

/// The minimum supported resolution
const MIN_RES: (u32, u32) = (10, 10);

/// The maximum supported resolution
const MAX_RES: (u32, u32) = (8192, 8192);

/// The "preferred" resolution
const DEFAULT_RES: (i32, i32) = (1024, 768);

/// DRM Driver implementation for `RvkmsDriver`
#[vtable]
impl driver::Driver for RvkmsDriver {
    type Data = ();
    type RegistrationData<'a> = ();
    type ParentDevice<Ctx: device::DeviceContext> = faux::Device<Ctx>;
    type Object = gem::Object;
    type File = file::File;
    type Kms = Self;

    const INFO: driver::DriverInfo = INFO;

    kernel::declare_drm_ioctls! {}
}

#[vtable]
impl KmsDriver for RvkmsDriver {
    fn mode_config_info(_dev: &device::Device, _drm_data: &Self::Data) -> Result<ModeConfigInfo> {
        Ok(MODE_CONFIG_INFO)
    }

    fn create_objects(drm: &UnregisteredKmsDevice<'_, Self>) -> Result
    where
        Self: Sized,
    {
        output::create_output(drm, 0)
    }

    fn atomic_commit_tail<'a>(
        mut state: AtomicCommitTail<'a, Self>,
        modeset_token: ModesetsReadyToken<'_>,
        plane_update_token: PlaneUpdatesReadyToken<'_>,
    ) -> CommittedAtomicState<'a, Self>
    where
        Self: Sized,
    {
        let modeset_token = state.commit_modeset_disables(modeset_token);

        let plane_update_token = state.commit_planes(plane_update_token, Default::default());

        let modeset_token = state.commit_modeset_enables(modeset_token);

        state.fake_vblank();

        let state = state.commit_hw_done(modeset_token, plane_update_token);

        state.wait_for_flip_done();

        state
    }
}

pub(crate) struct RvkmsDriver;

const MODE_CONFIG_INFO: ModeConfigInfo = ModeConfigInfo {
    min_resolution: MIN_RES,
    max_resolution: MAX_RES,
    max_cursor: (512, 512),
    preferred_depth: 24,
    preferred_fourcc: None,
};

#[pin_data]
pub(crate) struct RvkmsModule {
    // Unregister DRM before destroying the faux parent device.
    #[pin]
    _drm: driver::Registration<'static, RvkmsDriver>,
    #[pin]
    dev: faux::Registration,
}

impl kernel::InPlaceModule for RvkmsModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        pr_info!("RVKMS Loaded\n");

        try_pin_init!(Self {
            dev: faux::Registration::new(NAME, None)?,
            _drm <- {
                // SAFETY: `dev` is pinned and outlives `_drm`, which is dropped first.
                let parent = unsafe { &*core::ptr::from_ref(dev.as_ref().get_ref().as_ref()) };
                dev_info!(parent, "Setting up DRM\n");
                let drm = drm::UnregisteredDevice::<RvkmsDriver>::new(parent, Ok(()))?;

                // SAFETY: The registration is owned by this pinned module and cannot be
                // forgotten independently. It is dropped before the faux parent is unbound.
                unsafe { driver::Registration::new(parent.as_ref(), drm, Ok::<(), Error>(())) }
            },
        })
    }
}

module! {
    type: RvkmsModule,
    name: "rvkms",
    authors: ["Lyude Paul"],
    description: "Rust VKMS Proof of Concept driver",
    license: "GPL v2",
}
