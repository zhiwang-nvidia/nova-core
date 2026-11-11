// SPDX-License-Identifier: GPL-2.0

//! Display support.

mod channel;
mod format;
mod framebuffer;
mod hal;
mod head;
mod methods;
mod mode;
mod modeset;
mod output;
mod scaler;
mod sor;
mod stream;
mod tables;
mod window;

pub(crate) use format::DisplayPixelFormat;
pub(crate) use framebuffer::FramebufferLayout;
pub(crate) use mode::{
    DisplayConfiguration,
    DisplayMode, //
};
pub(crate) use output::DisplayOutput;

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

use channel::{
    Channel,
    Core,
    WindowDma,
    WindowImmediate, //
};

pub(crate) struct Display<'bound> {
    dev: &'bound device::Device<Bound>,
    api: NovaCoreApiHandle<'bound>,
    pending_commit: Option<modeset::DisplayCommit>,
    prepared_commit_id: Option<u64>,
    next_commit_id: u64,
    streams: KVec<stream::Stream>,
    initial_detach_pending: bool,
    core_channel: Option<Channel<'bound, Core>>,
    hal: KBox<dyn hal::DisplayHal<'bound> + 'bound>,
    caps: DisplayCaps,
    hw_static_info: DisplayStaticInfo,
    heads: KVec<head::Head>,
    windows: KVec<window::Window<'bound>>,
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

        let mut heads = KVec::new();
        for id in 0..u32::BITS {
            if hw_static_info.head_mask & (1 << id) != 0 {
                heads.push(
                    head::Head {
                        id,
                        mode: None,
                        limited_range: false,
                    },
                    GFP_KERNEL,
                )?;
            }
        }

        let mut windows = KVec::new();
        for id in 0..u32::BITS {
            if hw_static_info.window_mask & (1 << id) != 0 {
                windows.push(window::Window::new(id), GFP_KERNEL)?;
            }
        }

        let hal = hal::display_hal(&api, caps.display_ip_version, &hw_static_info).inspect_err(
            |error| {
                dev_err!(
                    adev,
                    "Failed to initialize display HAL (IP {:#010x}): {:?}\n",
                    caps.display_ip_version,
                    error
                );
            },
        )?;

        cleanup.dismiss();

        let mut display = Self {
            dev: adev.as_ref(),
            api,
            pending_commit: None,
            prepared_commit_id: None,
            next_commit_id: 0,
            streams: KVec::new(),
            initial_detach_pending: true,
            core_channel: None,
            hal,
            caps,
            hw_static_info,
            heads,
            windows,
        };

        // Own the backing before GSP-RM can start using it, even if its reply fails.
        display
            .api
            .with(|core| core.display_set_instance_memory(display.hal.instance_memory()))
            .inspect_err(|error| {
                dev_err!(adev, "Failed to set display instance memory: {:?}\n", error);
            })?;

        let channel = Channel::<Core>::new(&display.api, &display.hw_static_info)?;
        let channel = display.core_channel.insert(channel);
        channel
            .create(&display.hw_static_info, &mut *display.hal)
            .inspect_err(|error| {
                dev_err!(adev, "Failed to create core channel: {:?}\n", error);
            })?;

        for window in &mut display.windows {
            let channel =
                Channel::<WindowDma>::new(&display.api, &display.hw_static_info, window.id)?;
            let immediate =
                Channel::<WindowImmediate>::new(&display.api, &display.hw_static_info, &channel)?;
            window.channel = Some(channel);
            window.immediate_channel = Some(immediate);
        }

        for window in &mut display.windows {
            let channel = window.channel.as_mut().ok_or(ENODEV)?;
            let immediate = window.immediate_channel.as_mut().ok_or(ENODEV)?;
            channel
                .create(&display.hw_static_info, &mut *display.hal)
                .inspect_err(|error| {
                    dev_err!(
                        adev,
                        "Failed to create window channel {}: {:?}\n",
                        window.id,
                        error
                    );
                })?;
            immediate
                .create(&display.hw_static_info, &mut *display.hal)
                .inspect_err(|error| {
                    dev_err!(
                        adev,
                        "Failed to create window immediate channel {}: {:?}\n",
                        window.id,
                        error
                    );
                })?;
        }

        Ok(display)
    }

    /// Disables owned output paths before normal display teardown.
    pub(crate) fn shutdown(mut self) {
        if let Err(error) = self.disable_all() {
            dev_err!(self.dev, "Failed to disable display: {:?}\n", error);
        }
    }
}

impl Drop for Display<'_> {
    fn drop(&mut self) {
        for channel in self
            .windows
            .iter_mut()
            .filter_map(|window| window.immediate_channel.as_mut())
        {
            if let Err(error) = channel.destroy(&mut *self.hal) {
                dev_err!(
                    self.dev,
                    "Failed to destroy window immediate channel {}: {:?}\n",
                    channel.kind.index,
                    error
                );
            }
        }
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
                    error
                );
            }
        }
        if let Some(channel) = self.core_channel.as_mut() {
            if let Err(error) = channel.destroy(&mut *self.hal) {
                dev_err!(self.dev, "Failed to destroy core channel: {:?}\n", error);
            }
        }
        if let Err(error) = self.api.with(|core| core.display_shutdown()) {
            dev_err!(self.dev, "Failed to shut down display: {:?}\n", error);
            // GSP-RM may still reference the physical backing; the CPU mapping can be dropped.
            core::mem::forget(self.hal.instance_memory().clone());
            if let Some(pending) = self.pending_commit.as_ref() {
                pending.retain_backing();
            }
            for window in &self.windows {
                if let Some(framebuffer) = window.framebuffer.as_ref() {
                    framebuffer.retain_backing();
                }
            }
            for channel in self
                .windows
                .iter()
                .filter_map(|window| window.immediate_channel.as_ref())
            {
                channel.retain_backing();
            }
            for channel in self
                .windows
                .iter()
                .filter_map(|window| window.channel.as_ref())
            {
                channel.retain_backing();
            }
            if let Some(channel) = self.core_channel.as_ref() {
                channel.retain_backing();
            }
        }
    }
}
