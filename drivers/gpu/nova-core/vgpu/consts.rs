// SPDX-License-Identifier: GPL-2.0

/// Development OpenRM GMC command identifiers used by vGPU management.
pub(crate) mod gmc {
    pub(crate) const VGPU_MGMT_QUERY_PROPERTIES: u32 = 0x0002_0006;
    pub(crate) const VGPU_MGMT_QUERY_ASSIGNED_VF: u32 = 0x0002_0007;
}
