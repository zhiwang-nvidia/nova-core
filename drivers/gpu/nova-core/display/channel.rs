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
//!
//! # Window Channel
//!
//! Each window channel programs the framebuffer for one display window,
//! including its address, format, size and layout. The core channel assigns the
//! window to a display head.
//!
//! # Window Immediate Channel
//!
//! Each window immediate channel controls the same window as the window channel
//! with the matching index. It carries output-position updates and can interlock
//! with that window channel.

use kernel::{
    io::{
        poll::read_poll_timeout,
        Io, //
    },
    prelude::*,
    ptr::Alignment,
    sizes::SZ_4K,
    sync::{
        Arc,
        Mutex, //
    },
    time::Delta, //
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
            DisplayChannelKind,
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
    num::IntoSafeCast,
    regs,
    sbuffer::SBufferIter, //
};

use super::hal::{
    DisplayHal,
    DisplayMethodHal,
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
    next_write_offset: usize,
    last_hw_put: usize,
}

impl PushBuffer<'_, '_> {
    /// Emit JUMP to zero so hardware GET returns to the start of the pushbuffer.
    /// Used when the next method packet does not fit in the remaining tail space.
    const JUMP: u32 = 0x2000_0000;

    /// When the CPU write position is below GET, require a gap greater than five
    /// words before writing the next packet.
    const CHASE_PAD: usize = 5 * size_of::<u32>();
}

impl<'a, 'gpu> PushBuffer<'a, 'gpu> {
    pub(super) fn new(
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        size: Alignment,
    ) -> Result<Self> {
        if size.as_usize() < 3 * size_of::<u32>() {
            return Err(EINVAL);
        }

        Ok(Self {
            mapping: alloc_channel_buffer(mm, bar_user, size)?,
            next_write_offset: 0,
            last_hw_put: 0,
        })
    }

    fn wait_for_space(&mut self, user_area: &ChannelUserArea<'_>, bytes: usize) -> Result {
        let limit = self.mapping.size() - size_of::<u32>();
        if bytes > limit {
            return Err(E2BIG);
        }

        read_poll_timeout(
            || {
                let get: usize = user_area.read_get()?.into_safe_cast();
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
                    self.mapping
                        .try_write32(Self::JUMP, self.next_write_offset)?;
                    self.write_hw_put(user_area, 0)?;
                    self.next_write_offset = 0;
                }

                self.advance(user_area)?;
                Ok(false)
            },
            |ready| *ready,
            Delta::from_micros(100),
            Delta::from_secs(5),
        )?;
        Ok(())
    }

    fn emit(&mut self, user_area: &ChannelUserArea<'_>, header: u32, params: &[u32]) -> Result {
        let bytes = (params.len() + 1) * size_of::<u32>();
        self.wait_for_space(user_area, bytes)?;

        let mut packet = SBufferIter::new_reader([header.as_bytes(), params.as_bytes()]);
        let mut offset = self.next_write_offset;
        while !packet.is_empty() {
            let mut word = [0; size_of::<u32>()];
            packet.read_exact(&mut word)?;
            self.mapping.try_write32(u32::from_le_bytes(word), offset)?;
            offset += size_of::<u32>();
        }
        self.next_write_offset = offset;
        Ok(())
    }

    fn write_hw_put(&mut self, user_area: &ChannelUserArea<'_>, put: usize) -> Result {
        let value = u32::try_from(put).map_err(|_| EOVERFLOW)?;
        // A BAR1 framebuffer read completes prior CPU writes before publishing PUT.
        self.mapping.try_read32(0)?;
        user_area.write_put(value)?;
        self.last_hw_put = put;
        Ok(())
    }

    fn advance(&mut self, user_area: &ChannelUserArea<'_>) -> Result {
        if self.next_write_offset != self.last_hw_put {
            self.write_hw_put(user_area, self.next_write_offset)?;
        }
        Ok(())
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

    fn read_get(&self) -> Result<u32> {
        self.bar.try_read32(self.offset + Self::GET)
    }

    fn write_put(&self, value: u32) -> Result {
        self.bar.try_write32(value, self.offset + Self::PUT)
    }
}

/// Identifies the core DMA channel in [`Channel`].
pub(super) struct Core;

/// Identifies a window DMA channel and its hardware window.
pub(super) struct WindowDma {
    pub(super) index: u32,
}

pub(super) struct WindowImmediate {
    pub(super) index: u32,
}

pub(super) struct Channel<'a, 'gpu, T> {
    cmdq: &'a Cmdq<'gpu>,
    pushbuffer: PushBuffer<'a, 'gpu>,
    notifier_buffer: Option<NotifierBuffer<'a, 'gpu>>,
    channel_token: Option<u64>,
    user_area_mmio: Option<ChannelUserArea<'gpu>>,
    client_id: u32,
    display_channel: u32,
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

    pub(super) fn emit(
        &mut self,
        hal: &dyn DisplayMethodHal,
        method: u32,
        params: &[u32],
    ) -> Result {
        let user_area = self.user_area_mmio.as_ref().ok_or(ENODEV)?;
        let header = hal.encode_method_header(method, params.len())?;
        self.pushbuffer.emit(user_area, header, params)
    }

    pub(super) fn advance(&mut self) -> Result {
        let user_area = self.user_area_mmio.as_ref().ok_or(ENODEV)?;
        self.pushbuffer.advance(user_area)
    }

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

impl<'a, 'gpu> Channel<'a, 'gpu, Core> {
    pub(super) fn new(
        cmdq: &'a Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        info: &DisplayHwStaticInfo,
    ) -> Result<Self> {
        Self::alloc_channel_buffers(cmdq, mm, bar_user, info, Core, true)
    }

    pub(super) fn create(
        &mut self,
        bar: Bar0<'gpu>,
        info: &DisplayHwStaticInfo,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
    ) -> Result {
        let pushbuffer = &self.pushbuffer.mapping;
        let command = DisplayCreateChannel::new(
            DisplayChannelKind::Core,
            0,
            pushbuffer.region().address(),
            pushbuffer.size().into_safe_cast(),
        )?;
        self.create_channel_inner(bar, command, info.core_class, info.num_channels, hal)?;
        hal.init_core_channel(self, info.head_mask)
    }

    /// Waits for this core channel's hardware state to become idle.
    ///
    /// DISP 4.x and 5.x share the core channel state register layout and idle
    /// state encoding.
    #[expect(dead_code)]
    pub(super) fn wait_idle(&mut self) -> Result {
        const STATE_IDLE: u32 = 0x0b;
        const STATE_SHUTDOWN2: u32 = 0x0e;
        let bar = self.user_area_mmio.as_ref().ok_or(ENODEV)?.bar;

        // NVKMS's 100 ms wait before reading the core armed state cache.
        read_poll_timeout(
            || {
                let state = bar.read(regs::NV_PDISP_FE_CHNSTATUS_CORE).state().get();
                if state > STATE_SHUTDOWN2 {
                    return Err(EIO);
                }
                Ok(state)
            },
            |&state| state == STATE_IDLE,
            Delta::from_micros(100),
            Delta::from_millis(100),
        )
        .map(|_| ())
    }
}

impl<'a, 'gpu> Channel<'a, 'gpu, WindowDma> {
    pub(super) fn new(
        cmdq: &'a Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        info: &DisplayHwStaticInfo,
        index: u32,
    ) -> Result<Self> {
        if index >= u32::BITS || info.window_mask & (1 << index) == 0 {
            return Err(EINVAL);
        }
        Self::alloc_channel_buffers(cmdq, mm, bar_user, info, WindowDma { index }, true)
    }

    pub(super) fn create(
        &mut self,
        bar: Bar0<'gpu>,
        info: &DisplayHwStaticInfo,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
    ) -> Result {
        let pushbuffer = &self.pushbuffer.mapping;
        let command = DisplayCreateChannel::new(
            DisplayChannelKind::Window,
            self.kind.index,
            pushbuffer.region().address(),
            pushbuffer.size().into_safe_cast(),
        )?;
        self.create_channel_inner(bar, command, info.window_class, info.num_channels, hal)?;
        hal.init_window_channel(self, bar, info.head_mask)
    }
}

impl<'a, 'gpu> Channel<'a, 'gpu, WindowImmediate> {
    pub(super) fn new(
        cmdq: &'a Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        info: &DisplayHwStaticInfo,
        window: &Channel<'a, 'gpu, WindowDma>,
    ) -> Result<Self> {
        Self::alloc_channel_buffers(
            cmdq,
            mm,
            bar_user,
            info,
            WindowImmediate {
                index: window.kind.index,
            },
            false,
        )
    }

    pub(super) fn create(
        &mut self,
        bar: Bar0<'gpu>,
        info: &DisplayHwStaticInfo,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
    ) -> Result {
        let pushbuffer = &self.pushbuffer.mapping;
        let command = DisplayCreateChannel::new(
            DisplayChannelKind::Immediate,
            self.kind.index,
            pushbuffer.region().address(),
            pushbuffer.size().into_safe_cast(),
        )?;
        self.create_channel_inner(bar, command, info.immediate_class, info.num_channels, hal)
    }
}
