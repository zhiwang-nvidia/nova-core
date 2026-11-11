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
//!
//! # Core Channel
//!
//! Each GPU has one core channel, shared by all display heads. It carries methods
//! for head timings, output configuration and window-to-head assignment.
//! It must be created before window, window immediate and cursor channels.

use kernel::{
    io::poll::read_poll_timeout,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K,
    time::Delta, //
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
    DisplayMethodHal,
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
    mapping: VramMapping<'bound>,
    next_write_offset: usize,
    last_hw_put: usize,
}

impl PushBuffer<'_> {
    /// JUMP to zero when the next method packet cannot fit in the tail space.
    const JUMP: u32 = 0x2000_0000;

    /// Keep more than five words between the CPU write position and GET.
    const CHASE_PAD: usize = 5 * size_of::<u32>();
}

impl<'bound> PushBuffer<'bound> {
    pub(super) fn new(api: &NovaCoreApiHandle<'bound>, size: Alignment) -> Result<Self> {
        if size.as_usize() < 3 * size_of::<u32>() {
            return Err(EINVAL);
        }
        let (region, mapping) = alloc_channel_buffer(api, size)?;
        Ok(Self {
            region,
            mapping,
            next_write_offset: 0,
            last_hw_put: 0,
        })
    }

    fn wait_for_space(&mut self, channel: &DisplayChannelInfo<'_>, bytes: usize) -> Result {
        let limit = self.mapping.size() - size_of::<u32>();
        if bytes > limit {
            return Err(E2BIG);
        }

        read_poll_timeout(
            || {
                let get: usize = channel.read_get()? as usize;
                if get >= self.mapping.size() || !get.is_multiple_of(size_of::<u32>()) {
                    return Err(EIO);
                }

                if self.next_write_offset < get {
                    let available = get - self.next_write_offset;
                    if available > Self::CHASE_PAD && bytes + size_of::<u32>() < available {
                        return Ok(true);
                    }
                } else if bytes <= limit - self.next_write_offset {
                    return Ok(true);
                } else if get != 0 {
                    // GET must leave the old beginning before PUT can wrap to zero.
                    self.mapping.write32(Self::JUMP, self.next_write_offset)?;
                    self.write_hw_put(channel, 0)?;
                    self.next_write_offset = 0;
                }

                self.advance(channel)?;
                Ok(false)
            },
            |ready| *ready,
            Delta::from_micros(100),
            Delta::from_secs(5),
        )?;
        Ok(())
    }

    fn emit(&mut self, channel: &DisplayChannelInfo<'_>, header: u32, params: &[u32]) -> Result {
        let bytes = (params.len() + 1) * size_of::<u32>();
        self.wait_for_space(channel, bytes)?;

        let mut offset = self.next_write_offset;
        for word in Iterator::chain(core::iter::once(header), params.iter().copied()) {
            self.mapping.write32(word, offset)?;
            offset += size_of::<u32>();
        }
        self.next_write_offset = offset;
        Ok(())
    }

    fn write_hw_put(&mut self, channel: &DisplayChannelInfo<'_>, put: usize) -> Result {
        let value = u32::try_from(put).map_err(|_| EOVERFLOW)?;
        // A BAR1 framebuffer read completes prior CPU writes before publishing PUT.
        self.mapping.read32(0)?;
        channel.write_put(value)?;
        self.last_hw_put = put;
        Ok(())
    }

    fn advance(&mut self, channel: &DisplayChannelInfo<'_>) -> Result {
        if self.next_write_offset != self.last_hw_put {
            self.write_hw_put(channel, self.next_write_offset)?;
        }
        Ok(())
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

/// Identifies the core DMA channel in [`Channel`].
pub(super) struct Core;

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
    pub(super) fn is_created(&self) -> bool {
        self.created
    }

    #[expect(dead_code)]
    pub(super) fn bind_surface_descriptor(
        &mut self,
        hal: &mut dyn DisplayHal<'bound>,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let info = self.info.as_ref().ok_or(ENODEV)?;
        hal.bind_surface_descriptor(descriptor, info.ramht_client_id(), info.hardware_index())
    }

    #[expect(dead_code)]
    pub(super) fn unbind_surface_descriptor(
        &mut self,
        hal: &mut dyn DisplayHal<'bound>,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let info = self.info.as_ref().ok_or(ENODEV)?;
        hal.unbind_surface_descriptor(descriptor, info.ramht_client_id(), info.hardware_index())
    }

    #[expect(dead_code)]
    pub(super) fn emit(
        &mut self,
        hal: &dyn DisplayMethodHal,
        method: u32,
        params: &[u32],
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let info = self.info.as_ref().ok_or(ENODEV)?;
        let header = hal.encode_method_header(method, params.len())?;
        self.pushbuffer.emit(info, header, params)
    }

    #[expect(dead_code)]
    pub(super) fn advance(&mut self) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let info = self.info.as_ref().ok_or(ENODEV)?;
        self.pushbuffer.advance(info)
    }

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

    /// Retains physical buffers when display shutdown cannot establish that DMA has stopped.
    pub(super) fn retain_backing(&self) {
        core::mem::forget(self.pushbuffer.region.clone());
        if let Some(notifier) = self.notifier_buffer.as_ref() {
            core::mem::forget(notifier.region.clone());
        }
    }

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

impl<'bound> Channel<'bound, Core> {
    pub(super) fn new(api: &NovaCoreApiHandle<'bound>, info: &DisplayStaticInfo) -> Result<Self> {
        Self::alloc_channel_buffers(api, info, Core, true)
    }

    pub(super) fn create(
        &mut self,
        info: &DisplayStaticInfo,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        self.create_channel_inner(
            DisplayChannelKind::Core,
            0,
            info.core_class,
            info.num_channels,
            hal,
        )
    }
}
