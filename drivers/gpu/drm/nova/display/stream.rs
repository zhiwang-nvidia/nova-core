// SPDX-License-Identifier: GPL-2.0

//! Display streams and HDMI link operations through GSP-RM.

use kernel::prelude::*;

use nova_core::api::{
    DisplayOutputResourceType,
    NovaCoreApiHandle, //
};

use super::DisplayConfiguration;

/// A display stream from one head to an output.
pub(super) struct Stream {
    pub(super) display_id: u32,
    pub(super) head: u32,
    pub(super) stream_id: u64,
    pub(super) sor: u32,
    pub(super) protocol: u32,
    hdmi_sink_caps: u32,
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
        }
    }

    pub(super) fn assign(&mut self, api: &NovaCoreApiHandle<'_>, bracket: u64) -> Result {
        if self.stream_id != 0 {
            return Err(EBUSY);
        }
        let route = api.with(|core| {
            core.display_assign_stream_route(bracket, self.display_id, self.head, 0)
        })?;
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
        let port = api.with(|core| core.display_port_caps(self.display_id))?;
        if port.index != sor
            || port.resource_type != DisplayOutputResourceType::Sor
            || port.internal_panel
        {
            return Err(EIO);
        }
        self.sor = sor;
        self.protocol = port.protocol;
        Ok(())
    }

    pub(super) fn release(&self, api: &NovaCoreApiHandle<'_>, bracket: u64) -> Result {
        api.with(|core| core.display_release_stream_route(self.stream_id, bracket))
    }

    pub(super) fn set_sink_capabilities(
        &self,
        api: &NovaCoreApiHandle<'_>,
        bracket: u64,
    ) -> Result {
        api.with(|core| {
            core.display_hdmi_set_sink_capabilities(self.stream_id, bracket, self.hdmi_sink_caps)
        })
    }
}
