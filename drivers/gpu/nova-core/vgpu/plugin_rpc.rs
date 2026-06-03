// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

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

/// Host-side ready limit from `vmiopd_negotiate_cpu_gsp_version()` in
/// `vmiop-vgpu.c`, which polls the same boot marker for 10 seconds.
const PLUGIN_READY_TIMEOUT: Delta = Delta::from_secs(10);

/// BAR1-backed channel used to communicate with the vGPU plugin.
pub(crate) struct PluginRpc<'gpu> {
    comm: CommBufferRegion<'gpu>,
}

impl<'gpu> PluginRpc<'gpu> {
    pub(crate) fn new(comm: CommBufferRegion<'gpu>) -> Self {
        Self { comm }
    }

    /// Return the physical regions occupied by the plugin logs.
    pub(crate) fn plugin_logs(&self) -> Result<PluginLogRegions> {
        self.comm.plugin_logs()
    }

    /// Poll the control buffer until the plugin publishes its boot marker.
    pub(crate) fn wait_plugin_ready(&self, dev: &device::Device<device::Bound>) -> Result {
        let start = Instant::<Monotonic>::now();

        loop {
            if self.comm.is_plugin_ready()? {
                dev_dbg!(dev, "vGPU plugin ready after {:?}\n", start.elapsed());
                return Ok(());
            }
            if start.elapsed() >= PLUGIN_READY_TIMEOUT {
                return Err(ETIMEDOUT);
            }
            fsleep(Delta::from_millis(1));
        }
    }

    /// Release the BAR1 mapping.
    pub(crate) fn destroy(self, bar_user: &BarUser<'gpu>, mm: &mut GpuMm<'_>) -> Result {
        self.comm.destroy(bar_user, mm)
    }
}
