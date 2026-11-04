// SPDX-License-Identifier: GPL-2.0

//! nova-core display support.

mod channel;
mod hal;
mod head;
mod window;

use channel::{
    Channel,
    Core,
    WindowDma, //
};

use kernel::{
    device,
    prelude::*,
    sync::Mutex,
    types::ScopeGuard, //
};

use crate::{
    driver::Bar0,
    gpu::Spec,
    gsp::{
        cmdq::Cmdq,
        commands::{
            gmc::{
                DisplayGetStaticInfo,
                DisplayInit,
                DisplayShutdown, //
            },
            DisplayCaps,
            DisplayHwStaticInfo, //
        },
    },
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
};

pub(crate) struct Display<'a, 'gpu> {
    dev: &'a device::Device<device::Bound>,
    cmdq: &'a Cmdq<'gpu>,
    windows: KVec<window::Window<'a, 'gpu>>,
    core_channel: Option<Channel<'a, 'gpu, Core>>,
    hal: KBox<dyn hal::DisplayHal<'a, 'gpu> + 'a>,
    #[expect(dead_code)]
    spec: Spec,
    bar_user: &'a BarUser<'gpu>,
    #[expect(dead_code)]
    caps: DisplayCaps,
    hw_static_info: DisplayHwStaticInfo,
    heads: KVec<head::Head>,
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    fn new(
        dev: &'a device::Device<device::Bound>,
        cmdq: &'a Cmdq<'gpu>,
        spec: Spec,
        bar_user: &'a BarUser<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
    ) -> Result<Self> {
        let caps = cmdq
            .send_gmc_command(DisplayInit::new()?, |_| Ok(()))
            .inspect_err(|error| {
                dev_err!(dev, "DISPLAY_INIT failed: {:?}\n", error);
            })?;

        let cleanup = ScopeGuard::new(|| {
            let _ = Self::clean(dev, cmdq);
        });

        let hw_static_info = cmdq
            .send_gmc_command(DisplayGetStaticInfo, |_| Ok(()))
            .inspect_err(|error| {
                dev_err!(dev, "DISPLAY_GET_STATIC_INFO failed: {:?}\n", error);
            })?;

        let mut heads = KVec::new();
        for id in 0..u32::BITS {
            if hw_static_info.head_mask & (1 << id) != 0 {
                heads.push(head::Head { id }, GFP_KERNEL)?;
            }
        }

        let mut windows = KVec::new();
        for id in 0..u32::BITS {
            if hw_static_info.window_mask & (1 << id) != 0 {
                windows.push(window::Window::new(id), GFP_KERNEL)?;
            }
        }

        let hal = hal::display_hal(
            dev,
            cmdq,
            mm,
            bar_user,
            caps.display_ip_version,
            &hw_static_info,
        )
        .inspect_err(|error| {
            dev_err!(
                dev,
                "Failed to initialize display HAL (IP {:#010x}): {:?}\n",
                caps.display_ip_version,
                error,
            );
        })?;

        cleanup.dismiss();

        Ok(Self {
            dev,
            cmdq,
            windows,
            core_channel: None,
            hal,
            spec,
            bar_user,
            caps,
            hw_static_info,
            heads,
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
        bar: Bar0<'gpu>,
        bar_user: &'a BarUser<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
    ) -> Result<Self> {
        let mut display = Self::new(dev, cmdq, spec, bar_user, mm)?;

        let channel = Channel::<Core>::new(cmdq, mm, display.bar_user, &display.hw_static_info)?;
        let channel = display.core_channel.insert(channel);
        channel
            .create(bar, &display.hw_static_info, &mut *display.hal)
            .inspect_err(|error| {
                dev_err!(dev, "Failed to create Core channel: {:?}\n", error);
            })?;

        for window in &mut display.windows {
            let channel = Channel::<WindowDma>::new(
                cmdq,
                mm,
                display.bar_user,
                &display.hw_static_info,
                window.id,
            )?;
            window.channel = Some(channel);
        }

        for window in &mut display.windows {
            let channel = window.channel.as_mut().ok_or(ENODEV)?;
            channel
                .create(bar, &display.hw_static_info, &mut *display.hal)
                .inspect_err(|error| {
                    dev_err!(
                        dev,
                        "Failed to create window channel {}: {:?}\n",
                        channel.kind.index,
                        error,
                    );
                })?;
        }

        Ok(display)
    }

    /// Called when nova-drm is unbinding or initialization fails.
    pub(crate) fn shutdown(self) {
        drop(self);
    }
}

impl Drop for Display<'_, '_> {
    fn drop(&mut self) {
        for channel in self
            .windows
            .iter_mut()
            .filter_map(|window| window.channel.as_mut())
        {
            if let Err(error) = channel.destroy(&mut *self.hal) {
                dev_err!(
                    self.dev,
                    "Failed to destroy window channel {}: {:?}\n",
                    channel.kind.index,
                    error,
                );
            }
        }
        if let Some(channel) = self.core_channel.as_mut() {
            if let Err(error) = channel.destroy(&mut *self.hal) {
                dev_err!(self.dev, "Failed to destroy Core channel: {:?}\n", error);
            }
        }
        let _ = Self::clean(self.dev, self.cmdq);
    }
}
