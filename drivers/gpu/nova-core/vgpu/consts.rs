// SPDX-License-Identifier: GPL-2.0

/// Development OpenRM GMC command identifiers used by vGPU management.
pub(crate) mod gmc {
    pub(crate) const VGPU_MGMT_QUERY_PROPERTIES: u32 = 0x0002_0006;
    pub(crate) const VGPU_MGMT_QUERY_ASSIGNED_VF: u32 = 0x0002_0007;
    pub(crate) const BOOTLOAD: u32 = 0x0002_0020;
    pub(crate) const SHUTDOWN: u32 = 0x0002_0021;
    pub(crate) const SHUTDOWN_COMPLETE: u32 = 0x0002_0022;
    pub(crate) const CLEANUP: u32 = 0x0002_0023;
}

/// vGPU plugin RPC values not provided by the firmware bindings.
pub(crate) mod plugin_rpc {
    pub(crate) const PLUGIN_BOOT_TIMEOUT_MS: i64 = 10_000;
}
