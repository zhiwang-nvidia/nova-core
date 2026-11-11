// SPDX-License-Identifier: GPL-2.0

//! Display support.

use kernel::{
    auxiliary,
    device::{
        self,
        Bound, //
    },
    prelude::*,
    types::ScopeGuard, //
};

use nova_core::api::{
    DisplayCaps,
    DisplayStaticInfo,
    NovaCoreApi,
    NovaCoreApiHandle, //
};

pub(crate) struct Display<'bound> {
    dev: &'bound device::Device<Bound>,
    api: NovaCoreApiHandle<'bound>,
    #[expect(dead_code)]
    caps: DisplayCaps,
    #[expect(dead_code)]
    hw_static_info: DisplayStaticInfo,
}

impl<'bound> Display<'bound> {
    /// Initializes the GMC display context and retains its capabilities.
    pub(crate) fn init(adev: &'bound auxiliary::Device<Bound>) -> Result<Self> {
        let api = NovaCoreApi::of(adev)?;
        // Firmware may have initialized the context before the reply failed.
        let cleanup = ScopeGuard::new(|| {
            if let Err(error) = api.with(|core| core.display_shutdown()) {
                dev_err!(
                    adev,
                    "Failed to shut down display after initialization error: {:?}\n",
                    error
                );
            }
        });

        let caps = api.with(|core| core.display_init()).inspect_err(|error| {
            dev_err!(adev, "Failed to initialize display: {:?}\n", error);
        })?;
        let hw_static_info = api
            .with(|core| core.display_static_info())
            .inspect_err(|error| {
                dev_err!(
                    adev,
                    "Failed to query display static information: {:?}\n",
                    error
                );
            })?;

        cleanup.dismiss();

        Ok(Self {
            dev: adev.as_ref(),
            api,
            caps,
            hw_static_info,
        })
    }
}

impl Drop for Display<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.api.with(|core| core.display_shutdown()) {
            dev_err!(self.dev, "Failed to shut down display: {:?}\n", error);
        }
    }
}
