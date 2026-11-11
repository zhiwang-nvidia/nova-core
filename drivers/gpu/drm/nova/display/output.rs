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
//! - nova-drm obtains the supported static IDs from nova-core's `display_mask`.
//!   Each set bit's value is one ID: `0x700` contains IDs `0x100`, `0x200` and `0x400`.
//!
//! - nova-drm queries nova-core for connection state and output resources using each
//!   ID. GET_PORT_STATE and GET_PORT_CAPS supply these facts. READ_EDID uses the
//!   same ID to read the attached monitor's EDID.
//!
//! - Connecting or disconnecting a monitor changes the output's connection state.
//!   Its static display ID remains in the supported mask, so later queries can
//!   continue to address the same output.

use kernel::prelude::*;

use nova_core::api::DisplayOutputResourceType;

use super::Display;

/// An output admitted by the display firmware and its current connection state.
#[expect(dead_code)]
pub(crate) struct DisplayOutput {
    /// Single-bit GSP-RM display identifier.
    pub(crate) display_id: u32,
    /// Whether sink detection found a connected monitor.
    pub(crate) connected: bool,
    /// Heads available for this output, with bit N identifying hardware head N.
    pub(crate) head_mask: u32,
    /// Board connector index reported by GSP-RM.
    pub(crate) connector_index: u32,
}

impl Display<'_> {
    pub(crate) fn outputs(&self) -> Result<KVec<DisplayOutput>> {
        let mut outputs = KVec::new();
        for index in 0..u32::BITS {
            let display_id = 1 << index;
            if self.hw_static_info.display_mask & display_id == 0 {
                continue;
            }
            let state = self.api.with(|core| core.display_port_state(display_id))?;
            if state.connected {
                let caps = self.api.with(|core| core.display_port_caps(display_id))?;
                if caps.resource_type != DisplayOutputResourceType::Sor || caps.internal_panel {
                    return Err(EOPNOTSUPP);
                }
            }
            outputs.push(
                DisplayOutput {
                    display_id,
                    connected: state.connected,
                    head_mask: state.head_mask & self.hw_static_info.head_mask,
                    connector_index: state.connector_index,
                },
                GFP_KERNEL,
            )?;
        }
        Ok(outputs)
    }

    fn validate_display_id(&self, display_id: u32) -> Result {
        if !display_id.is_power_of_two() || self.hw_static_info.display_mask & display_id == 0 {
            return Err(EINVAL);
        }
        Ok(())
    }

    /// Reads EDID for an output admitted by GSP-RM's static display mask.
    #[expect(dead_code)]
    pub(crate) fn read_edid(&self, display_id: u32) -> Result<KVec<u8>> {
        self.validate_display_id(display_id)?;
        self.api.with(|core| core.display_read_edid(display_id))
    }
}
