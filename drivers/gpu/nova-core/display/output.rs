// SPDX-License-Identifier: GPL-2.0

//! Output operations through GSP-RM.
//!
//! A display ID identifies a GSP-RM output object. It selects the output for
//! connection detection, EDID access and display routing.
//!
//! # Display ID lifecycle
//!
//! - GSP-RM builds static output objects from output paths described by the VBIOS
//!   Display Configuration Block (DCB), assigning each a display ID with one bit set.
//!   These objects exist even when no monitor is connected.
//!
//! - nova-core obtains the supported static IDs from GET_STATIC_INFO's `display_mask`.
//!   Each set bit's value is one ID: `0x700` contains IDs `0x100`, `0x200` and `0x400`.
//!
//! - nova-core uses each ID to query connection state with GET_PORT_STATE and output
//!   resources with GET_PORT_CAPS. READ_EDID uses the same ID to read the attached
//!   monitor's EDID.
//!
//! - Connecting or disconnecting a monitor changes the output's connection state.
//!   Its static display ID remains in the supported mask, so later queries can
//!   continue to address the same output.

use kernel::prelude::*;

use crate::gsp::commands::{
    gmc::{
        DisplayGetPortCaps,
        DisplayGetPortState, //
    },
    DisplayOutputResourceType, //
};

use super::Display;

/// An output admitted by the display firmware and its current connection state.
pub struct DisplayOutput {
    /// Single-bit GSP-RM display identifier.
    pub display_id: u32,
    /// Whether sink detection found a connected monitor.
    pub connected: bool,
    /// Heads available for this output, with bit N identifying hardware head N.
    pub head_mask: u32,
    /// Board connector index reported by GSP-RM.
    pub connector_index: u32,
}

impl Display<'_, '_> {
    pub(crate) fn outputs(&self) -> Result<KVec<DisplayOutput>> {
        let mut outputs = KVec::new();
        for index in 0..u32::BITS {
            let display_id = 1 << index;
            if self.hw_static_info.display_mask & display_id == 0 {
                continue;
            }
            let state = self
                .cmdq
                .send_gmc_command(DisplayGetPortState::new(display_id)?, |_| Ok(()))?;
            if state.display_id != display_id || state.status != 0 {
                return Err(EIO);
            }
            if state.is_connected() {
                let caps = self
                    .cmdq
                    .send_gmc_command(DisplayGetPortCaps::new(display_id)?, |_| Ok(()))?;
                if caps.resource_type() != DisplayOutputResourceType::Sor
                    || caps.is_internal_panel()
                {
                    return Err(EOPNOTSUPP);
                }
            }
            outputs.push(
                DisplayOutput {
                    display_id,
                    connected: state.is_connected(),
                    head_mask: state.head_mask & self.hw_static_info.head_mask,
                    connector_index: state.connector_index,
                },
                GFP_KERNEL,
            )?;
        }
        Ok(outputs)
    }
}
