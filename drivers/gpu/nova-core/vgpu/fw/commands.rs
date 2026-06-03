// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use crate::gsp::vgpu_bindings as bindings;

/// State observed in the response buffer for an expected RPC sequence.
pub(crate) enum RpcResponse {
    /// Firmware has not completed the expected sequence.
    Pending {
        /// Last sequence completed by firmware.
        sequence: u32,
    },
    /// Firmware has completed the expected sequence.
    Complete {
        /// Firmware result code.
        status: u32,
    },
}

/// Message types supported by the nova-core plugin RPC channel.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(crate) enum RpcMessage {
    VersionNegotiation = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_VERSION_NEGOTIATION,
    SetupConfigParamsAndInit = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_SETUP_CONFIG_PARAMS_AND_INIT,
    Reset = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_RESET,
    UpdateBmeState = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_UPDATE_BME_STATE,
}
