// SPDX-License-Identifier: GPL-2.0

//! Display window component.
//!
//! Window ownership and usage-bound methods are shared by DISP 4.x
//! and 5.x. Physical window and tile assignments required by some DISP 5.x
//! GPUs are handled by their display HAL.

use kernel::{
    io::{
        register::Array,
        Io, //
    },
    prelude::*, //
};

use crate::{
    driver::Bar0,
    num::IntoSafeCast,
    regs, //
};

use super::{
    channel::{
        Channel,
        Core,
        WindowDma,
        WindowImmediate, //
    },
    framebuffer::Framebuffer,
    hal::DisplayMethodHal,
    methods::{
        core,
        window_immediate, //
    },
    mode::DisplayMode,
    Display,
    DisplayPixelFormat, //
};

/// A display window.
pub(super) struct Window<'a, 'gpu> {
    pub(super) id: u32,
    pub(super) immediate_channel: Option<Channel<'a, 'gpu, WindowImmediate>>,
    pub(super) channel: Option<Channel<'a, 'gpu, WindowDma>>,
    pub(super) head: Option<u32>,
    pub(super) framebuffer: Option<Framebuffer<'a, 'gpu>>,
}

impl Window<'_, '_> {
    pub(super) fn new(id: u32) -> Self {
        Self {
            id,
            immediate_channel: None,
            channel: None,
            head: None,
            framebuffer: None,
        }
    }

    /// Reads this window's armed head assignment, or `None` when unassigned.
    ///
    /// The value comes from the core channel's armed state cache. DISP 4.x and
    /// 5.x share this register offset and OWNER field encoding.
    ///
    /// The caller must wait for core idle and prevent further core submissions
    /// while reading the armed state.
    pub(super) fn read_armed_owner(&self, bar: Bar0<'_>) -> Result<Option<u32>> {
        // OWNER encodes HEAD0 through HEAD7, independently of which heads are usable.
        const OWNER_HEAD_COUNT: u32 = 8;
        const OWNER_NONE: u32 = 0x0f;

        let window: usize = self.id.into_safe_cast();
        let register =
            regs::NV_UDISP_FE_CHN_ARMED_CORE_WINDOW_SET_CONTROL::try_at(window).ok_or(EINVAL)?;
        let control = bar.read(register);
        // An all-ones bus response is not the valid OWNER_NONE method value.
        if control.into_raw() == u32::MAX {
            return Err(EIO);
        }
        match control.owner().get() {
            OWNER_NONE => Ok(None),
            head if head < OWNER_HEAD_COUNT => Ok(Some(head)),
            _ => Err(EIO),
        }
    }

    /// Queues this window's head assignment for a separate core UPDATE.
    pub(super) fn emit_owner_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, '_, Core>,
        head: Option<u32>,
    ) -> Result {
        if self.id >= core::WINDOW_COUNT || head.is_some_and(|head| head >= core::HEAD_COUNT) {
            return Err(EINVAL);
        }
        let offset = self.id * core::WINDOW_STRIDE;
        channel.emit(
            hal,
            core::WINDOW_SET_CONTROL + offset,
            &[head.unwrap_or(core::WINDOW_SET_CONTROL_OWNER_NONE)],
        )
    }

    /// Clears the window's allowed pixel formats.
    pub(super) fn emit_clear_format_usage_bounds_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, '_, Core>,
    ) -> Result {
        if self.id >= core::WINDOW_COUNT {
            return Err(EINVAL);
        }
        let offset = self.id * core::WINDOW_STRIDE;
        channel.emit(
            hal,
            core::WINDOW_SET_WINDOW_FORMAT_USAGE_BOUNDS + offset,
            &[0],
        )
    }

    /// Queues usage bounds for an unscaled window framebuffer.
    pub(super) fn emit_usage_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, '_, Core>,
        mode: &DisplayMode,
        format: DisplayPixelFormat,
    ) -> Result {
        const FETCH_EXTRA_PIXELS: u32 = 22;

        mode.validate()?;
        if self.id >= core::WINDOW_COUNT {
            return Err(EINVAL);
        }
        // nvGetMaxPixelsFetchedPerLine simplifies to width + 14 + 8 at 1:1.
        let max_fetch = mode.hdisplay + FETCH_EXTRA_PIXELS;
        if max_fetch > core::WINDOW_USAGE_MAX_PIXELS_FETCHED {
            return Err(ERANGE);
        }
        let offset = self.id * core::WINDOW_STRIDE;
        let format_usage = match format {
            DisplayPixelFormat::Xrgb8888 => core::WINDOW_FORMAT_USAGE_RGB_PACKED4BPP,
        };
        channel.emit(
            hal,
            core::WINDOW_SET_WINDOW_FORMAT_USAGE_BOUNDS + offset,
            &[format_usage],
        )?;
        channel.emit(
            hal,
            core::WINDOW_SET_MAX_INPUT_SCALE_FACTOR + offset,
            &[core::WINDOW_SCALE_FACTOR_1X
                | (core::WINDOW_SCALE_FACTOR_1X << core::WINDOW_SCALE_FACTOR_VERTICAL_SHIFT)],
        )?;
        // Composition bypass uses no ILUT/TMO fetch and no upscaling/overfetch.
        channel.emit(
            hal,
            core::WINDOW_SET_WINDOW_USAGE_BOUNDS + offset,
            &[max_fetch | core::WINDOW_USAGE_INPUT_SCALER_TAPS_2],
        )
    }
    /// Queues the window's output position at (0, 0).
    ///
    /// SET_POINT_OUT is shared by DISP 4.x and 5.x. UPDATE emission is separate
    /// because its encoding differs between classes.
    pub(super) fn emit_set_window_position(&mut self, hal: &dyn DisplayMethodHal) -> Result {
        let immediate = self.immediate_channel.as_mut().ok_or(ENODEV)?;
        if immediate.kind.index >= core::WINDOW_COUNT {
            return Err(EINVAL);
        }
        immediate.emit(hal, window_immediate::SET_POINT_OUT, &[0])
    }
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'a, 'gpu>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
