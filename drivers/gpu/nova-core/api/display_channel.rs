// SPDX-License-Identifier: GPL-2.0

//! Display channel firmware operations and user-register access.

use kernel::{
    device::Bound,
    io::Io,
    pci,
    prelude::*,
    sync::Arc, //
};

use crate::{
    driver::Bar0,
    gsp::commands,
    num::IntoSafeCast, //
};

use super::{
    NovaCoreApi,
    VramRegion, //
};

/// Kind of display channel requested from GSP-RM.
#[derive(Clone, Copy)]
pub enum DisplayChannelKind {
    /// Core DMA channel shared by the display heads.
    Core,
    /// Window DMA channel.
    Window,
    /// Window immediate DMA channel.
    Immediate,
    /// Cursor immediate channel.
    Cursor,
}

impl From<DisplayChannelKind> for commands::DisplayChannelKind {
    fn from(kind: DisplayChannelKind) -> Self {
        match kind {
            DisplayChannelKind::Core => Self::Core,
            DisplayChannelKind::Window => Self::Window,
            DisplayChannelKind::Immediate => Self::Immediate,
            DisplayChannelKind::Cursor => Self::Cursor,
        }
    }
}

impl NovaCoreApi<'_> {
    /// Creates a display channel using a VRAM pushbuffer belonging to this GPU.
    ///
    /// Keep the pushbuffer alive until channel destruction or display shutdown
    /// succeeds, including when this call fails. Retain the returned information
    /// before validating its class, index or user area so cleanup can destroy it.
    pub fn display_create_channel(
        &self,
        kind: DisplayChannelKind,
        instance: u32,
        pushbuffer: &VramRegion,
    ) -> Result<DisplayChannelInfo<'_>> {
        if !Arc::ptr_eq(&self.vram_owner, &pushbuffer.owner) {
            return Err(EXDEV);
        }
        let reply = self.gpu.display_create_channel(
            kind.into(),
            instance,
            pushbuffer.address(),
            pushbuffer.size(),
        )?;
        Ok(DisplayChannelInfo {
            channel_token: reply.channel,
            owner: self.pdev,
            bar: self.gpu.display_channel_bar(),
            class_id: reply.class_id,
            hardware_index: reply.hardware_index,
            ramht_client_id: reply.ramht_client_id,
            control_offset: reply.control_offset,
            control_size: reply.control_size,
        })
    }

    /// Destroys a channel belonging to this GPU.
    ///
    /// Retain its buffers and bindings if destruction fails. After success,
    /// discard its information and stop accessing its user area.
    pub fn display_destroy_channel(&self, channel: &DisplayChannelInfo<'_>) -> Result {
        if !core::ptr::eq(self.pdev, channel.owner) {
            return Err(EXDEV);
        }
        if channel.channel_token == 0 {
            return Err(EINVAL);
        }
        self.gpu.display_destroy_channel(channel.channel_token)
    }
}

/// Firmware channel information retained for validation and destruction.
pub struct DisplayChannelInfo<'bound> {
    channel_token: u64,
    owner: &'bound pci::Device<Bound>,
    bar: Bar0<'bound>,
    class_id: u32,
    hardware_index: u32,
    ramht_client_id: u32,
    control_offset: u32,
    control_size: u32,
}

impl DisplayChannelInfo<'_> {
    const PUT: usize = 0;
    const GET: usize = 4;

    /// Returns the channel class selected by GSP-RM.
    pub fn class_id(&self) -> u32 {
        self.class_id
    }

    /// Returns the hardware channel index used in surface bindings.
    pub fn hardware_index(&self) -> u32 {
        self.hardware_index
    }

    /// Returns the RAMHT client identity used in surface bindings.
    pub fn ramht_client_id(&self) -> u32 {
        self.ramht_client_id
    }

    /// Reads the hardware command-fetch byte offset of the created channel.
    pub fn read_get(&self) -> Result<u32> {
        self.bar.try_read32(self.user_area_offset()? + Self::GET)
    }

    /// Writes PUT after pushbuffer writes are visible to the created channel.
    pub fn write_put(&self, value: u32) -> Result {
        self.bar
            .try_write32(value, self.user_area_offset()? + Self::PUT)
    }

    fn user_area_offset(&self) -> Result<usize> {
        let offset: usize = self.control_offset.into_safe_cast();
        let size: usize = self.control_size.into_safe_cast();
        let end = offset.checked_add(size).ok_or(EOVERFLOW)?;
        if self.channel_token == 0
            || size < Self::GET + size_of::<u32>()
            || !offset.is_multiple_of(size_of::<u32>())
            || !size.is_multiple_of(size_of::<u32>())
            || end > self.bar.size()
        {
            return Err(EINVAL);
        }
        Ok(offset)
    }
}
