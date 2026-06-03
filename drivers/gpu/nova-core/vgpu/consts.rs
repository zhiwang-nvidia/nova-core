// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

/// Development OpenRM GMC command identifiers used by vGPU management.
pub(crate) mod gmc {
    use crate::gsp::vgpu_bindings as bindings;

    pub(crate) const VGPU_MGMT_QUERY_PROPERTIES: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_QUERY_VGPU_PROPERTIES;
    pub(crate) const VGPU_MGMT_QUERY_ASSIGNED_VF: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_QUERY_ASSIGNED_VF_VGPU_TYPE;
    pub(crate) const BOOTLOAD: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_BOOTLOAD_GSP_VGPU_PLUGIN_TASK;
    pub(crate) const SHUTDOWN: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK;
    pub(crate) const SHUTDOWN_COMPLETE: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_SHUTDOWN_GSP_VGPU_PLUGIN_TASK_COMPLETE;
    pub(crate) const CLEANUP: u32 =
        bindings::GMCAPI_COMMANDS_GMCAPI_CMD_CLEANUP_GSP_VGPU_PLUGIN_RESOURCES;
}
