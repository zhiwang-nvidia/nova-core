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
    io::Io,
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K,
    sync::{
        Arc,
        Mutex, //
    },
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::{
            gmc::{
                DisplayCreateChannel,
                DisplayDestroyChannel, //
            },
            DisplayHwStaticInfo, //
        },
    },
    mm::{
        bar_user::{
            BarMapping,
            BarUser, //
        },
        GpuMm, //
    },
    num::IntoSafeCast, //
};

use super::hal::{
    DisplayHal,
    SurfaceDescriptor, //
};

fn alloc_channel_buffer<'a, 'gpu>(
    mm: &'a Mutex<GpuMm<'gpu>>,
    bar_user: &'a BarUser<'gpu>,
    size: Alignment,
) -> Result<BarMapping<'a, 'gpu>> {
    let backing = Arc::new(mm.lock().alloc_vram(size)?, GFP_KERNEL)?;
    let region = backing.region(0..backing.size)?;
    let mapping = BarMapping::new(bar_user, mm, region, true)?;
    for offset in (0..mapping.size()).step_by(size_of::<u32>()) {
        mapping.try_write32(0, offset)?;
    }
    Ok(mapping)
}

/// Command buffer for a display DMA channel.
pub(super) struct PushBuffer<'a, 'gpu> {
    mapping: BarMapping<'a, 'gpu>,
}

impl<'a, 'gpu> PushBuffer<'a, 'gpu> {
    pub(super) fn new(
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        size: Alignment,
    ) -> Result<Self> {
        Ok(Self {
            mapping: alloc_channel_buffer(mm, bar_user, size)?,
        })
    }
}

/// Completion notifications from the display engine.
pub(super) struct NotifierBuffer<'a, 'gpu> {
    mapping: BarMapping<'a, 'gpu>,
    descriptor: Option<SurfaceDescriptor>,
    binding: Option<(u32, u32)>,
}

impl<'a, 'gpu> NotifierBuffer<'a, 'gpu> {
    pub(super) fn new(
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        size: Alignment,
    ) -> Result<Self> {
        Ok(Self {
            mapping: alloc_channel_buffer(mm, bar_user, size)?,
            descriptor: None,
            binding: None,
        })
    }

    fn bind(
        &mut self,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
        client_id: u32,
        display_channel: u32,
    ) -> Result {
        let descriptor =
            hal.alloc_surface_descriptor(self.mapping.region().clone(), true, false)?;
        self.descriptor = Some(descriptor);
        hal.bind_surface_descriptor(descriptor, client_id, display_channel)?;
        self.binding = Some((client_id, display_channel));
        Ok(())
    }

    fn unbind(&mut self, hal: &mut dyn DisplayHal<'a, 'gpu>) -> Result {
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

struct ChannelUserArea<'gpu> {
    bar: Bar0<'gpu>,
    offset: usize,
}

impl<'gpu> ChannelUserArea<'gpu> {
    const PUT: usize = 0x0000;
    const GET: usize = 0x0004;

    /// Returns a view of a display DMA channel's BAR0 user area using the firmware-provided range.
    ///
    /// PUT and GET have the following layout, with byte offsets relative to the
    /// start of the channel user area:
    ///
    /// ```text
    /// +-------------+------+----------+------------------------+
    /// | Byte offset | Word | Register | Purpose                |
    /// +-------------+------+----------+------------------------+
    /// | 0x0000      | 0    | PUT      | CPU submission pointer |
    /// | 0x0004      | 1    | GET      | Hardware fetch pointer |
    /// +-------------+------+----------+------------------------+
    /// ```
    fn new(bar: Bar0<'gpu>, offset: u32, size: u32) -> Result<Self> {
        let offset: usize = offset.into_safe_cast();
        let size: usize = size.into_safe_cast();
        let end = offset.checked_add(size).ok_or(EOVERFLOW)?;
        if size < Self::GET + size_of::<u32>()
            || !offset.is_multiple_of(size_of::<u32>())
            || !size.is_multiple_of(size_of::<u32>())
            || end > bar.size()
        {
            return Err(EINVAL);
        }

        Ok(Self { bar, offset })
    }

    #[expect(dead_code)]
    fn read_get(&self) -> Result<u32> {
        self.bar.try_read32(self.offset + Self::GET)
    }

    #[expect(dead_code)]
    fn write_put(&self, value: u32) -> Result {
        self.bar.try_write32(value, self.offset + Self::PUT)
    }
}

pub(super) struct Channel<'a, 'gpu, T> {
    cmdq: &'a Cmdq<'gpu>,
    #[expect(dead_code)]
    pushbuffer: PushBuffer<'a, 'gpu>,
    notifier_buffer: Option<NotifierBuffer<'a, 'gpu>>,
    channel_token: Option<u64>,
    user_area_mmio: Option<ChannelUserArea<'gpu>>,
    client_id: u32,
    display_channel: u32,
    #[expect(dead_code)]
    pub(super) kind: T,
}

impl<'a, 'gpu, T> Channel<'a, 'gpu, T> {
    pub(super) fn is_created(&self) -> bool {
        self.channel_token.is_some() && self.user_area_mmio.is_some()
    }

    /// Binds a surface descriptor to this created channel.
    #[expect(dead_code)]
    pub(super) fn bind_surface_descriptor(
        &self,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        hal.bind_surface_descriptor(descriptor, self.client_id, self.display_channel)
    }

    /// Unbinds a surface after its channel accesses have completed.
    ///
    /// Runtime surface bindings must be removed before destroying the channel.
    #[expect(dead_code)]
    pub(super) fn unbind_surface_descriptor(
        &self,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        hal.unbind_surface_descriptor(descriptor, self.client_id, self.display_channel)
    }

    #[expect(dead_code)]
    fn alloc_channel_buffers(
        cmdq: &'a Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        info: &DisplayHwStaticInfo,
        kind: T,
        with_notifier_buffer: bool,
    ) -> Result<Self> {
        if info.num_channels == 0 || info.max_channels == 0 {
            return Err(EINVAL);
        }
        let pushbuffer = PushBuffer::new(mm, bar_user, Alignment::new::<SZ_4K>())?;
        let pushbuffer_size: u64 = pushbuffer.mapping.size().into_safe_cast();
        if pushbuffer_size > u64::from(info.max_pb_bytes) {
            return Err(EINVAL);
        }
        let notifier_buffer = if with_notifier_buffer {
            Some(NotifierBuffer::new(
                mm,
                bar_user,
                Alignment::new::<SZ_4K>(),
            )?)
        } else {
            None
        };

        Ok(Self {
            cmdq,
            pushbuffer,
            notifier_buffer,
            channel_token: None,
            user_area_mmio: None,
            client_id: 0,
            display_channel: 0,
            kind,
        })
    }

    #[expect(dead_code)]
    fn create_channel_inner(
        &mut self,
        bar: Bar0<'gpu>,
        command: DisplayCreateChannel,
        class_id: u32,
        num_channels: u32,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
    ) -> Result {
        let reply = self.cmdq.send_gmc_command(command, |_| Ok(()))?;

        // Save the token before validating the rest of the reply so failure can destroy it.
        self.channel_token = Some(reply.channel);
        if reply.channel == 0 || reply.class_id != class_id || reply.hardware_index >= num_channels
        {
            return Err(EINVAL);
        }
        self.user_area_mmio = Some(ChannelUserArea::new(
            bar,
            reply.control_offset,
            reply.control_size,
        )?);
        self.client_id = reply.ramht_client_id;
        self.display_channel = reply.hardware_index;
        if let Some(notifier_buffer) = self.notifier_buffer.as_mut() {
            notifier_buffer.bind(hal, self.client_id, self.display_channel)?;
        }
        Ok(())
    }

    #[expect(dead_code)]
    pub(super) fn destroy(&mut self, hal: &mut dyn DisplayHal<'a, 'gpu>) -> Result {
        if let Some(channel_token) = self.channel_token.take() {
            self.user_area_mmio = None;
            let command = DisplayDestroyChannel::new(channel_token)?;
            self.cmdq.send_gmc_command(command, |_| Ok(()))?;
        }

        if let Some(notifier_buffer) = self.notifier_buffer.as_mut() {
            notifier_buffer.unbind(hal)?;
        }
        Ok(())
    }
}
