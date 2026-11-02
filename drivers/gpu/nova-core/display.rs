// SPDX-License-Identifier: GPL-2.0

//! nova-core display support.

use kernel::{
    device,
    prelude::*,
    types::ScopeGuard, //
};

use crate::{
    gpu::Spec,
    gsp::{
        cmdq::Cmdq,
        commands::{
            gmc::{
                DisplayInit,
                DisplayShutdown, //
            },
            DisplayCaps, //
        },
    },
    mm::bar_user::BarUser, //
};

pub(crate) struct Display<'a, 'gpu> {
    dev: &'a device::Device<device::Bound>,
    cmdq: &'a Cmdq<'gpu>,
    #[expect(dead_code)]
    spec: Spec,
    #[expect(dead_code)]
    bar_user: &'a BarUser<'gpu>,
    #[expect(dead_code)]
    caps: DisplayCaps,
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &'a Cmdq<'gpu>,
        spec: Spec,
        bar_user: &'a BarUser<'gpu>,
    ) -> Result<Self> {
        let caps = cmdq
            .send_gmc_command(DisplayInit::new()?, |_| Ok(()))
            .inspect_err(|error| {
                dev_err!(dev, "DISPLAY_INIT failed: {:?}\n", error);
            })?;

        let cleanup = ScopeGuard::new(|| {
            let _ = Self::clean(dev, cmdq);
        });

        cleanup.dismiss();

        Ok(Self {
            dev,
            cmdq,
            spec,
            bar_user,
            caps,
        })
    }

    fn clean(dev: &device::Device<device::Bound>, cmdq: &Cmdq<'_>) -> Result {
        cmdq.send_gmc_command(DisplayShutdown, |_| Ok(()))
            .inspect_err(|error| {
                dev_err!(dev, "DISPLAY_SHUTDOWN failed: {:?}\n", error);
            })?;

        Ok(())
    }

    /// Called when nova-drm is binding.
    pub(crate) fn init(
        dev: &'a device::Device<device::Bound>,
        cmdq: &'a Cmdq<'gpu>,
        spec: Spec,
        bar_user: &'a BarUser<'gpu>,
    ) -> Result<Self> {
        Self::new(dev, cmdq, spec, bar_user)
    }

    /// Called when nova-drm is unbinding or initialization fails.
    pub(crate) fn shutdown(self) {
        drop(self);
    }
}

impl Drop for Display<'_, '_> {
    fn drop(&mut self) {
        let _ = Self::clean(self.dev, self.cmdq);
    }
}
