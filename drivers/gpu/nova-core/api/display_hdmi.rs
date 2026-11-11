// SPDX-License-Identifier: GPL-2.0

//! HDMI packet operations.
//!
//! Call these operations while the display context is initialized and serialize
//! them with HDMI source ON/OFF. The implemented packet layout is limited to Ada.

use kernel::prelude::*;

use super::NovaCoreApi;

impl NovaCoreApi<'_> {
    /// Configures HDMI packet transmission with a validated RGB AVI infoframe.
    pub fn display_configure_hdmi_packets(&self, head: u32, avi: &[u8; 17]) -> Result {
        self.gpu.display_configure_hdmi_packets(head, avi)
    }

    /// Sets or clears HDMI AVMUTE through a General Control Packet.
    pub fn display_set_hdmi_avmute(&self, head: u32, mute: bool) -> Result {
        self.gpu.display_set_hdmi_avmute(head, mute)
    }

    /// Mutes audio and disables HDMI packet transmission on the specified head.
    pub fn display_disable_hdmi_packets(&self, head: u32) -> Result {
        self.gpu.display_disable_hdmi_packets(head)
    }
}
