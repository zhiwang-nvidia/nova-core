// SPDX-License-Identifier: GPL-2.0

//! SOR owner-mask and TMDS control programming shared by DISP 4.x and DISP 5.x.

use kernel::prelude::*;

use super::{
    channel::{
        Channel,
        Core, //
    },
    hal::DisplayMethodHal,
    methods::core, //
};

/// Encodes a supported GSP-RM TMDS protocol as a SOR control field.
pub(super) fn encode_protocol(rm_protocol: u32) -> Result<u32> {
    const RM_SINGLE_TMDS_A: u32 = 1;
    const RM_SINGLE_TMDS_B: u32 = 2;

    match rm_protocol {
        RM_SINGLE_TMDS_A => Ok(core::SOR_SET_CONTROL_PROTOCOL_SINGLE_TMDS_A),
        RM_SINGLE_TMDS_B => Ok(core::SOR_SET_CONTROL_PROTOCOL_SINGLE_TMDS_B),
        _ => Err(EOPNOTSUPP),
    }
}

/// Queues a SOR's head owner mask and TMDS protocol through the core channel.
///
/// An empty owner mask detaches the SOR without interpreting `rm_protocol`.
/// The caller validates the device's SOR index and submits the core UPDATE.
pub(super) fn emit_sor_control_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    sor: u32,
    rm_protocol: u32,
    head_mask: u32,
) -> Result {
    if head_mask >> core::HEAD_COUNT != 0 {
        return Err(EINVAL);
    }
    let control = if head_mask == 0 {
        0
    } else {
        encode_protocol(rm_protocol)? | head_mask
    };
    channel.emit(
        hal,
        core::SOR_SET_CONTROL + sor * core::SOR_STRIDE,
        &[control],
    )
}
