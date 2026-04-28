// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::{
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use super::{
    r000,
    r570, //
};

/// Payload of the `GetGspStaticInfo` command and message.
#[expect(dead_code)]
#[repr(transparent)]
#[derive(Zeroable)]
pub(crate) struct GspStaticConfigInfo(r000::GspStaticConfigInfo_t);

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for GspStaticConfigInfo {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for GspStaticConfigInfo {}

/// Power level requested to the [`UnloadingGuestDriver`] command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
#[expect(unused)]
pub(crate) enum PowerStateLevel {
    /// Full unload.
    Level0 = r570::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_0,
    /// S3 (suspend to RAM).
    Level3 = r570::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3,
    /// Hibernate (suspend to disk).
    Level7 = r570::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_7,
}

impl PowerStateLevel {
    /// Returns `true` if this state represents a power management transition, i.e. some GPU state
    /// must survive it (as opposed to a full unload).
    pub(crate) fn is_power_transition(self) -> bool {
        self != PowerStateLevel::Level0
    }
}

/// Payload of the `UnloadingGuestDriver` command and message.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Zeroable)]
pub(crate) struct UnloadingGuestDriver(r570::rpc_unloading_guest_driver_v1F_07);

impl UnloadingGuestDriver {
    pub(crate) fn new(level: PowerStateLevel) -> Self {
        Self(r570::rpc_unloading_guest_driver_v1F_07 {
            bInPMTransition: u8::from(level.is_power_transition()),
            bGc6Entering: 0,
            newLevel: level as u32,
            ..Zeroable::zeroed()
        })
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for UnloadingGuestDriver {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for UnloadingGuestDriver {}
