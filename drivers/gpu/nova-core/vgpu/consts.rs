// SPDX-License-Identifier: GPL-2.0

/// Development OpenRM GMC command identifiers used by vGPU management.
pub(crate) mod gmc {
    pub(crate) const VGPU_MGMT_ADD_TYPE: u32 = 0x0002_0001;
    pub(crate) const VGPU_MGMT_QUERY_SUPPORTED: u32 = 0x0002_0002;
    pub(crate) const VGPU_MGMT_QUERY_CREATABLE: u32 = 0x0002_0003;
    pub(crate) const VGPU_MGMT_ASSIGN_TYPE: u32 = 0x0002_0004;
    pub(crate) const VGPU_MGMT_DEASSIGN_TYPE: u32 = 0x0002_0005;
    pub(crate) const VGPU_MGMT_QUERY_PROPERTIES: u32 = 0x0002_0006;
    pub(crate) const VGPU_MGMT_QUERY_ASSIGNED_VF: u32 = 0x0002_0007;
    pub(crate) const BOOTLOAD: u32 = 0x0002_0020;
    pub(crate) const SHUTDOWN: u32 = 0x0002_0021;
    pub(crate) const SHUTDOWN_COMPLETE: u32 = 0x0002_0022;
    pub(crate) const CLEANUP: u32 = 0x0002_0023;
    pub(crate) const SCRUB_GUEST_FB: u32 = 0x0002_0025;
    pub(crate) const ALLOC_GSP_CEUTILS: u32 = 0x0002_0026;
    pub(crate) const FREE_GSP_CEUTILS: u32 = 0x0002_0027;
}

/// vGPU plugin RPC values not provided by the firmware bindings.
pub(crate) mod plugin_rpc {
    pub(crate) const DOORBELL_STRIDE: u32 = 32;
    pub(crate) const DOORBELL_VECTOR: u32 = 17;
    pub(crate) const PLUGIN_BOOT_TIMEOUT_MS: i64 = 10_000;
    pub(crate) const NV_VIRTUAL_FUNCTION_PRIV_DOORBELL: usize = 0xb8_0000 + 0x2200;
}
