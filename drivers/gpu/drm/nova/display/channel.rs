// SPDX-License-Identifier: GPL-2.0

//! DMA channel resources and lifecycle.
//!
//! # Pushbuffer
//!
//! Each display DMA channel has its own pushbuffer holding methods and their
//! parameters. The CPU writes commands into it and advances PUT; the display
//! engine fetches the commands and advances GET.
//!
//! # Notifier Buffer
//!
//! The display engine writes completion notifications to a notifier buffer so
//! the CPU can determine when a requested operation has completed. Core and window
//! channels each have their own notifier buffer. Window immediate channels do not
//! support notifier methods, so they have no notifier buffer.
//!
//! Notification writes are triggered by channel methods, so the buffer must first
//! be bound to the channel through a surface descriptor.

use kernel::{
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K, //
};

use nova_core::api::{
    DisplayChannelInfo,
    DisplayChannelKind,
    DisplayStaticInfo,
    NovaCoreApiHandle,
    VramMapping,
    VramRegion, //
};

use super::hal::{
    DisplayHal,
    SurfaceDescriptor, //
};

fn alloc_channel_buffer<'bound>(
    api: &NovaCoreApiHandle<'bound>,
    size: Alignment,
) -> Result<(VramRegion, VramMapping<'bound>)> {
    let backing = api.with(|core| core.get_ref().alloc_vram(size))?;
    let region = backing.region(0..backing.size())?;
    let mapping = api.with(|core| core.get_ref().map_vram(region.clone(), true))?;
    for offset in (0..mapping.size()).step_by(size_of::<u32>()) {
        mapping.write32(0, offset)?;
    }
    Ok((region, mapping))
}

/// Command buffer for a display DMA channel.
pub(super) struct PushBuffer<'bound> {
    region: VramRegion,
    #[expect(dead_code)]
    mapping: VramMapping<'bound>,
}

impl<'bound> PushBuffer<'bound> {
    pub(super) fn new(api: &NovaCoreApiHandle<'bound>, size: Alignment) -> Result<Self> {
        let (region, mapping) = alloc_channel_buffer(api, size)?;
        Ok(Self { region, mapping })
    }
}

/// Completion notifications from the display engine.
pub(super) struct NotifierBuffer<'bound> {
    region: VramRegion,
    #[expect(dead_code)]
    mapping: VramMapping<'bound>,
    descriptor: Option<SurfaceDescriptor>,
    binding: Option<(u32, u32)>,
}

impl<'bound> NotifierBuffer<'bound> {
    pub(super) fn new(api: &NovaCoreApiHandle<'bound>, size: Alignment) -> Result<Self> {
        let (region, mapping) = alloc_channel_buffer(api, size)?;
        Ok(Self {
            region,
            mapping,
            descriptor: None,
            binding: None,
        })
    }

    fn bind(
        &mut self,
        hal: &mut dyn DisplayHal<'bound>,
        client_id: u32,
        display_channel: u32,
    ) -> Result {
        let descriptor = hal.alloc_surface_descriptor(self.region.clone(), true, false)?;
        self.descriptor = Some(descriptor);
        hal.bind_surface_descriptor(descriptor, client_id, display_channel)?;
        self.binding = Some((client_id, display_channel));
        Ok(())
    }

    fn unbind(&mut self, hal: &mut dyn DisplayHal<'bound>) -> Result {
        if let Some(descriptor) = self.descriptor {
            if let Some((client_id, display_channel)) = self.binding {
                hal.unbind_surface_descriptor(descriptor, client_id, display_channel)?;
                self.binding = None;
            }
            hal.free_surface_descriptor(descriptor)?;
            self.descriptor = None;
        }
        Ok(())
    }
}

pub(super) struct Channel<'bound, T> {
    api: NovaCoreApiHandle<'bound>,
    pushbuffer: PushBuffer<'bound>,
    notifier_buffer: Option<NotifierBuffer<'bound>>,
    info: Option<DisplayChannelInfo<'bound>>,
    created: bool,
    #[expect(dead_code)]
    pub(super) kind: T,
}

impl<'bound, T> Channel<'bound, T> {
    #[expect(dead_code)]
    fn alloc_channel_buffers(
        api: &NovaCoreApiHandle<'bound>,
        info: &DisplayStaticInfo,
        kind: T,
        with_notifier_buffer: bool,
    ) -> Result<Self> {
        if info.num_channels == 0
            || info.max_channels == 0
            || u64::from(info.max_pb_bytes) < SZ_4K as u64
        {
            return Err(EINVAL);
        }
        let pushbuffer = PushBuffer::new(api, Alignment::new::<SZ_4K>())?;
        let notifier_buffer = if with_notifier_buffer {
            Some(NotifierBuffer::new(api, Alignment::new::<SZ_4K>())?)
        } else {
            None
        };

        Ok(Self {
            api: api.clone(),
            pushbuffer,
            notifier_buffer,
            info: None,
            created: false,
            kind,
        })
    }

    #[expect(dead_code)]
    fn create_channel_inner(
        &mut self,
        kind: DisplayChannelKind,
        instance: u32,
        class_id: u32,
        num_channels: u32,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        let info = self.api.with(|core| {
            core.get_ref()
                .display_create_channel(kind, instance, &self.pushbuffer.region)
        })?;

        // Retain the firmware handle before validating the reply so failure can destroy it.
        let info = self.info.insert(info);
        if info.class_id() != class_id || info.hardware_index() >= num_channels {
            return Err(EINVAL);
        }
        info.read_get()?;
        if let Some(notifier_buffer) = self.notifier_buffer.as_mut() {
            notifier_buffer.bind(hal, info.ramht_client_id(), info.hardware_index())?;
        }
        self.created = true;
        Ok(())
    }

    #[expect(dead_code)]
    pub(super) fn destroy(&mut self, hal: &mut dyn DisplayHal<'bound>) -> Result {
        self.created = false;
        if let Some(info) = self.info.as_ref() {
            self.api.with(|core| core.display_destroy_channel(info))?;
            self.info = None;
        }

        if let Some(notifier_buffer) = self.notifier_buffer.as_mut() {
            notifier_buffer.unbind(hal)?;
        }
        Ok(())
    }
}
