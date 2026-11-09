// SPDX-License-Identifier: GPL-2.0

//! Display streams and HDMI link operations through GSP-RM.

use kernel::prelude::*;

use crate::gsp::{
    cmdq::Cmdq,
    commands::{
        gmc::{
            DisplayAssignStreamRoute,
            DisplayGetPortCaps,
            DisplayHdmiSetSinkCapabilities,
            DisplayHdmiSetSourceActive,
            DisplayHdmiSetTmdsProtocol,
            DisplayReleaseStreamRoute,
            DisplaySetMonitorPower, //
        },
        DisplayOutputResourceType, //
    },
};

use super::DisplayConfiguration;

/// A display stream from one head to an output.
///
/// Only a pending commit owns an unassigned stream. Active streams have a validated
/// GSP-RM stream assignment and SOR.
pub(super) struct Stream {
    pub(super) display_id: u32,
    pub(super) head: u32,
    pub(super) stream_id: u64,
    pub(super) sor: u32,
    pub(super) protocol: u32,
    hdmi_sink_caps: u32,
    avi_infoframe: [u8; 17],
}

impl Stream {
    pub(super) fn new(configuration: &DisplayConfiguration, head: u32) -> Self {
        Self {
            display_id: configuration.display_id,
            head,
            stream_id: 0,
            sor: 0,
            protocol: 0,
            hdmi_sink_caps: configuration.hdmi_sink_caps,
            avi_infoframe: configuration.avi_infoframe,
        }
    }

    pub(super) fn matches_configuration(&self, configuration: &DisplayConfiguration) -> bool {
        self.display_id == configuration.display_id
            && self.hdmi_sink_caps == configuration.hdmi_sink_caps
            && self.avi_infoframe == configuration.avi_infoframe
    }

    pub(super) fn assign(&mut self, cmdq: &Cmdq<'_>, bracket: u64) -> Result {
        if self.stream_id != 0 {
            return Err(EBUSY);
        }
        let assign = DisplayAssignStreamRoute::new(bracket, self.display_id, self.head, 0)?;
        let route = cmdq.send_gmc_command(assign, |_| Ok(()))?;
        // Preserve the token even if validating the returned assignment fails.
        self.stream_id = route.stream_id;
        let mut sor = None;
        for (index, display_mask) in (0u32..).zip(&route.assigned) {
            if display_mask & self.display_id != 0 {
                if sor.is_some() {
                    return Err(EIO);
                }
                sor = Some(index);
            }
        }
        let sor = sor.ok_or(EIO)?;
        let port = cmdq.send_gmc_command(DisplayGetPortCaps::new(self.display_id)?, |_| Ok(()))?;
        if port.index != sor
            || port.resource_type() != DisplayOutputResourceType::Sor
            || port.is_internal_panel()
        {
            return Err(EIO);
        }
        self.sor = sor;
        self.protocol = port.protocol;
        Ok(())
    }

    pub(super) fn release(&self, cmdq: &Cmdq<'_>, bracket: u64) -> Result {
        cmdq.send_gmc_command(
            DisplayReleaseStreamRoute::new(self.stream_id, bracket)?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    pub(super) fn set_sink_capabilities(&self, cmdq: &Cmdq<'_>, bracket: u64) -> Result {
        cmdq.send_gmc_command(
            DisplayHdmiSetSinkCapabilities::new(self.stream_id, bracket, self.hdmi_sink_caps)?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    pub(super) fn set_tmds_protocol(&self, cmdq: &Cmdq<'_>, bracket: u64) -> Result {
        cmdq.send_gmc_command(
            DisplayHdmiSetTmdsProtocol::new(self.stream_id, bracket, true)?,
            |_| Ok(()),
        )?;
        Ok(())
    }

    pub(super) fn set_monitor_power(&self, cmdq: &Cmdq<'_>, bracket: u64, enabled: bool) -> Result {
        let power = cmdq.send_gmc_command(
            DisplaySetMonitorPower::new(self.stream_id, bracket, enabled)?,
            |_| Ok(()),
        )?;
        if power.accepted_state != u32::from(enabled) {
            return Err(EIO);
        }
        Ok(())
    }

    pub(super) fn set_source_active(&self, cmdq: &Cmdq<'_>, bracket: u64, active: bool) -> Result {
        cmdq.send_gmc_command(
            DisplayHdmiSetSourceActive::new(self.stream_id, bracket, active)?,
            |_| Ok(()),
        )?;
        Ok(())
    }
}
