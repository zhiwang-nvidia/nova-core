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

use super::{
    hal::{
        DisplayHal,
        DisplayMethodHal,
        SurfaceDescriptor, //
    },
    methods::{
        core as core_methods,
        window as window_methods, //
    }, //
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
    /// Hardware may still write this record, including while a Window image is active.
    pending: bool,
    binding: Option<(u32, u32)>,
}

/// The hardware progress needed by a notifier wait.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum NotifierStatus {
    Begun,
    Finished,
}

impl<'a, 'gpu> NotifierBuffer<'a, 'gpu> {
    // DISP 4.x and 5.x use a 16-byte notifier with status in word 0, bits 31:30.
    const STATUS_MASK: u32 = 3 << 30;
    const STATUS_BEGUN: u32 = 1 << 30;
    const STATUS_FINISHED: u32 = 2 << 30;

    pub(super) fn new(
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        size: Alignment,
    ) -> Result<Self> {
        Ok(Self {
            mapping: alloc_channel_buffer(mm, bar_user, size)?,
            descriptor: None,
            pending: false,
            binding: None,
        })
    }

    pub(super) fn available(&self) -> Result<SurfaceDescriptor> {
        if self.pending {
            return Err(EBUSY);
        }
        self.descriptor.ok_or(ENODEV)
    }

    pub(super) fn prepare(&mut self) -> Result<SurfaceDescriptor> {
        let descriptor = self.available()?;
        self.mapping.try_write32(0, 0)?;
        // An UPDATE may still write after a timeout. Only confirmed completion or
        // retirement permits this record to be reset for another operation.
        self.pending = true;
        Ok(descriptor)
    }

    pub(super) fn pending_descriptor(&self) -> Result<SurfaceDescriptor> {
        if !self.pending {
            return Err(EINVAL);
        }
        self.descriptor.ok_or(ENODEV)
    }

    pub(super) fn wait(&self, expected: NotifierStatus) -> Result {
        if !self.pending {
            return Err(EINVAL);
        }
        // Hardware writes FINISHED when a core UPDATE method completes with notification enabled.
        let expected_status = match expected {
            NotifierStatus::Begun => Self::STATUS_BEGUN,
            NotifierStatus::Finished => Self::STATUS_FINISHED,
        };
        read_poll_timeout(
            || {
                let status = self.mapping.try_read32(0)? & Self::STATUS_MASK;
                if status == Self::STATUS_MASK
                    || (expected == NotifierStatus::Begun && status == Self::STATUS_FINISHED)
                {
                    // A finished image is already retired, not currently active.
                    return Err(EIO);
                }
                Ok(status)
            },
            |&status| status == expected_status,
            Delta::from_micros(100),
            Delta::from_secs(3),
        )
        .map(|_| ())
    }

    /// Allows reuse after completion or retirement has stopped hardware writes.
    pub(super) fn finish(&mut self) {
        self.pending = false;
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

/// Completion required for an interlocked window update.
#[expect(dead_code)]
pub(super) enum Update {
    /// Waits for new framebuffer state to become active and keeps its notifier pending.
    WaitForActive,
    /// Waits for the current submission to retire and allows its notifier to be reused.
    WaitForRetire,
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

    fn available_notifier(&self) -> Result<SurfaceDescriptor> {
        if !self.is_created() {
            return Err(ENODEV);
        }
        self.notifier_buffer.as_ref().ok_or(ENODEV)?.available()
    }

    fn prepare_notifier(&mut self) -> Result<SurfaceDescriptor> {
        self.available_notifier()?;
        self.notifier_buffer.as_mut().ok_or(ENODEV)?.prepare()
    }

    fn wait_notifier(&self, status: NotifierStatus) -> Result {
        self.notifier_buffer.as_ref().ok_or(ENODEV)?.wait(status)
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

    /// Emits core interlock methods for the participating window mask.
    pub(super) fn emit_interlock_methods(
        &mut self,
        hal: &dyn DisplayMethodHal,
        window_mask: u32,
    ) -> Result {
        self.emit(hal, core_methods::SET_INTERLOCK_FLAGS, &[0])?;
        self.emit(
            hal,
            core_methods::SET_WINDOW_INTERLOCK_FLAGS,
            &[window_mask],
        )
    }

    /// Emits the core UPDATE method.
    pub(super) fn emit_update_methods(&mut self, hal: &dyn DisplayMethodHal) -> Result {
        self.emit(
            hal,
            core_methods::UPDATE,
            &[core_methods::UPDATE_RELEASE_ELV],
        )
    }

    /// Submits a core-only update and waits for it to finish.
    #[expect(dead_code)]
    pub(super) fn update(&mut self, hal: &dyn DisplayHal<'a, 'gpu>) -> Result {
        let descriptor = self.prepare_notifier()?;
        hal.emit_core_notifier_methods(self, Some(descriptor))?;
        self.emit_interlock_methods(hal, 0)?;
        self.emit_update_methods(hal)?;
        self.advance()?;
        self.wait_notifier(NotifierStatus::Finished)?;
        hal.emit_core_notifier_methods(self, None)?;
        // Disable notification without generating another UPDATE.
        self.advance()?;
        self.notifier_buffer.as_mut().ok_or(ENODEV)?.finish();
        Ok(())
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

    /// Emits window interlock methods to participate in a core update.
    pub(super) fn emit_interlock_methods(
        &mut self,
        hal: &dyn DisplayMethodHal,
        window_mask: u32,
    ) -> Result {
        self.emit(
            hal,
            window_methods::SET_INTERLOCK_FLAGS,
            &[window_methods::SET_INTERLOCK_FLAGS_INTERLOCK_WITH_CORE],
        )?;
        // NVKMS includes this window itself in the participating window mask.
        self.emit(
            hal,
            window_methods::SET_WINDOW_INTERLOCK_FLAGS,
            &[window_mask],
        )
    }

    /// Submits an interlocked window update and waits for activation or retirement.
    ///
    /// Before activation, establish window ownership with a separate core-only UPDATE.
    /// Before retirement, queue methods to stop framebuffer fetches. Keep window ownership
    /// unchanged until retirement succeeds and pass no immediate channel.
    /// Failed updates leave notifier records pending; retain framebuffer backing until the
    /// channels are quiesced.
    #[expect(dead_code)]
    pub(super) fn update(
        &mut self,
        hal: &dyn DisplayHal<'a, 'gpu>,
        core: &mut Channel<'a, 'gpu, Core>,
        immediate: Option<&mut Channel<'a, 'gpu, WindowImmediate>>,
        update: Update,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let window_mask = 1u32.checked_shl(self.kind.index).ok_or(EINVAL)?;

        // Check all participants before reserving either notifier record.
        match update {
            Update::WaitForActive => {
                if let Some(immediate) = immediate.as_ref() {
                    if !immediate.is_created() {
                        return Err(ENODEV);
                    }
                    if immediate.kind.index != self.kind.index {
                        return Err(EINVAL);
                    }
                }
                core.available_notifier()?;
                self.available_notifier()?;
            }
            Update::WaitForRetire => {
                if immediate.is_some() {
                    return Err(EINVAL);
                }
                self.notifier_buffer
                    .as_ref()
                    .ok_or(ENODEV)?
                    .pending_descriptor()?;
            }
        }

        let core_descriptor = core.prepare_notifier()?;
        let (window_descriptor, window_status) = match update {
            Update::WaitForActive => (Some(self.prepare_notifier()?), NotifierStatus::Begun),
            // Preserve the old record and disable notification for the retirement UPDATE.
            Update::WaitForRetire => (None, NotifierStatus::Finished),
        };
        hal.emit_core_notifier_methods(core, Some(core_descriptor))?;
        hal.emit_window_notifier_methods(self, window_descriptor)?;
        self.submit_interlocked_update(hal, core, window_mask, immediate)?;

        // Core cannot complete its interlock until Window has also been kicked.
        core.wait_notifier(NotifierStatus::Finished)?;
        self.wait_notifier(window_status)?;
        hal.emit_core_notifier_methods(core, None)?;
        core.advance()?;
        core.notifier_buffer.as_mut().ok_or(ENODEV)?.finish();
        // Only FINISHED permits reusing the window record; BEGUN leaves it pending.
        if window_status == NotifierStatus::Finished {
            self.notifier_buffer.as_mut().ok_or(ENODEV)?.finish();
        }
        Ok(())
    }

    fn submit_interlocked_update(
        &mut self,
        hal: &dyn DisplayHal<'a, 'gpu>,
        core: &mut Channel<'a, 'gpu, Core>,
        window_mask: u32,
        immediate: Option<&mut Channel<'a, 'gpu, WindowImmediate>>,
    ) -> Result {
        core.emit_interlock_methods(hal, window_mask)?;
        self.emit_interlock_methods(hal, window_mask)?;

        // Emit each UPDATE sequence immediately before its kickoff: emit may publish
        // earlier methods while making pushbuffer space.
        core.emit_update_methods(hal)?;
        core.advance()?;

        let interlock_immediate = immediate.is_some();
        if let Some(immediate) = immediate {
            hal.emit_window_immediate_update(immediate)?;
            immediate.advance()?;
        }
        hal.emit_window_update(self, interlock_immediate)?;
        self.advance()
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
