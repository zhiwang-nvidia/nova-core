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
    mapping: VramMapping<'bound>,
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

impl<'bound> NotifierBuffer<'bound> {
    // DISP 4.x and 5.x use a 16-byte notifier with status in word 0, bits 31:30.
    const STATUS_MASK: u32 = 3 << 30;
    const STATUS_BEGUN: u32 = 1 << 30;
    const STATUS_FINISHED: u32 = 2 << 30;

    pub(super) fn new(api: &NovaCoreApiHandle<'bound>, size: Alignment) -> Result<Self> {
        let (region, mapping) = alloc_channel_buffer(api, size)?;
        Ok(Self {
            region,
            mapping,
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
        self.mapping.write32(0, 0)?;
        // An UPDATE may still write after a timeout. Only confirmed completion or
        // retirement permits this record to be reset for another operation.
        self.pending = true;
        Ok(descriptor)
    }

    pub(super) fn wait(&self, expected: NotifierStatus) -> Result {
        if !self.pending {
            return Err(EINVAL);
        }
        // With notification enabled, core UPDATE completion writes FINISHED.
        // Window UPDATE writes BEGUN on activation and FINISHED on retirement.
        let expected_status = match expected {
            NotifierStatus::Begun => Self::STATUS_BEGUN,
            NotifierStatus::Finished => Self::STATUS_FINISHED,
        };
        read_poll_timeout(
            || {
                let status = self.mapping.read32(0)? & Self::STATUS_MASK;
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

/// Identifies a window DMA channel and its hardware window.
pub(super) struct WindowDma {
    pub(super) index: u32,
}

pub(super) struct WindowImmediate {
    pub(super) index: u32,
}

pub(super) struct Channel<'bound, T> {
    api: NovaCoreApiHandle<'bound>,
    pushbuffer: PushBuffer<'bound>,
    notifier_buffer: Option<NotifierBuffer<'bound>>,
    info: Option<DisplayChannelInfo<'bound>>,
    created: bool,
    pub(super) kind: T,
}

impl<'bound, T> Channel<'bound, T> {
    pub(super) fn is_created(&self) -> bool {
        self.created
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
        )?;
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
    pub(super) fn update(&mut self, hal: &dyn DisplayHal<'bound>) -> Result {
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
    #[expect(dead_code)]
    pub(super) fn wait_idle(&mut self) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }

        // NVKMS's 100 ms wait before reading the core armed state cache.
        read_poll_timeout(
            || self.api.with(|core| core.display_core_channel_is_idle()),
            |idle| *idle,
            Delta::from_micros(100),
            Delta::from_millis(100),
        )
        .map(|_| ())
    }
}

impl<'bound> Channel<'bound, WindowDma> {
    pub(super) fn new(
        api: &NovaCoreApiHandle<'bound>,
        info: &DisplayStaticInfo,
        index: u32,
    ) -> Result<Self> {
        if index >= u32::BITS || info.window_mask & (1 << index) == 0 {
            return Err(EINVAL);
        }
        Self::alloc_channel_buffers(api, info, WindowDma { index }, true)
    }

    pub(super) fn create(
        &mut self,
        info: &DisplayStaticInfo,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        self.create_channel_inner(
            DisplayChannelKind::Window,
            self.kind.index,
            info.window_class,
            info.num_channels,
            hal,
        )?;
        hal.init_window_channel(self, info.head_mask)
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

    /// Submits an interlocked window update and waits for the new state to become active.
    ///
    /// Before activation, establish window ownership with a separate core-only UPDATE.
    /// Failed updates leave notifier records pending; retain framebuffer backing until the
    /// channels are quiesced.
    #[expect(dead_code)]
    pub(super) fn update(
        &mut self,
        hal: &dyn DisplayHal<'bound>,
        core: &mut Channel<'bound, Core>,
        immediate: Option<&mut Channel<'bound, WindowImmediate>>,
    ) -> Result {
        if !self.is_created() {
            return Err(ENODEV);
        }
        let window_mask = 1u32.checked_shl(self.kind.index).ok_or(EINVAL)?;

        // Check all participants before reserving either notifier record.
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

        let core_descriptor = core.prepare_notifier()?;
        let window_descriptor = self.prepare_notifier()?;
        hal.emit_core_notifier_methods(core, Some(core_descriptor))?;
        hal.emit_window_notifier_methods(self, Some(window_descriptor))?;
        self.submit_interlocked_update(hal, core, window_mask, immediate)?;

        // Core cannot complete its interlock until Window has also been kicked.
        core.wait_notifier(NotifierStatus::Finished)?;
        self.wait_notifier(NotifierStatus::Begun)?;
        hal.emit_core_notifier_methods(core, None)?;
        core.advance()?;
        core.notifier_buffer.as_mut().ok_or(ENODEV)?.finish();
        // Keep the window record pending until this submission is retired.
        Ok(())
    }

    fn submit_interlocked_update(
        &mut self,
        hal: &dyn DisplayHal<'bound>,
        core: &mut Channel<'bound, Core>,
        window_mask: u32,
        immediate: Option<&mut Channel<'bound, WindowImmediate>>,
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

impl<'bound> Channel<'bound, WindowImmediate> {
    pub(super) fn new(
        api: &NovaCoreApiHandle<'bound>,
        info: &DisplayStaticInfo,
        window: &Channel<'bound, WindowDma>,
    ) -> Result<Self> {
        Self::alloc_channel_buffers(
            api,
            info,
            WindowImmediate {
                index: window.kind.index,
            },
            false,
        )
    }

    pub(super) fn create(
        &mut self,
        info: &DisplayStaticInfo,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        self.create_channel_inner(
            DisplayChannelKind::Immediate,
            self.kind.index,
            info.immediate_class,
            info.num_channels,
            hal,
        )
    }
}
