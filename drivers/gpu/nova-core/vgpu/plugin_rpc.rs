// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    }, //
};

use crate::{
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
    vgpu::fw::{
        CommBufferRegion,
        PluginLogRegions, //
    }, //
};

use super::consts::plugin_rpc as consts;

/// BAR1-backed channel used to communicate with the vGPU plugin.
pub(crate) struct PluginRpc<'gpu> {
    comm: Option<CommBufferRegion<'gpu>>,
}

impl<'gpu> PluginRpc<'gpu> {
    pub(crate) fn new(comm: CommBufferRegion<'gpu>) -> Self {
        Self { comm: Some(comm) }
    }

    fn comm(&self) -> Result<&CommBufferRegion<'gpu>> {
        self.comm.as_ref().ok_or(EIO)
    }

    /// Return the physical regions occupied by the plugin logs.
    pub(crate) fn plugin_logs(&self) -> Result<PluginLogRegions> {
        self.comm()?.plugin_logs()
    }

    /// Poll the control buffer until the plugin publishes its boot marker.
    pub(crate) fn wait_plugin_ready(&self, dev: &device::Device<device::Bound>) -> Result {
        let start = Instant::<Monotonic>::now();
        let timeout = Delta::from_millis(consts::PLUGIN_BOOT_TIMEOUT_MS);

        loop {
            if self.comm()?.is_plugin_ready()? {
                dev_dbg!(dev, "vGPU plugin ready after {:?}\n", start.elapsed());
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(ETIMEDOUT);
            }
            fsleep(Delta::from_millis(1));
        }
    }

    /// Release the BAR1 mapping.
    pub(crate) fn destroy(&mut self, bar_user: &BarUser<'gpu>, mm: &GpuMm<'gpu>) -> Result {
        self.comm.take().ok_or(EIO)?.destroy(bar_user, mm)
    }
}
