// SPDX-License-Identifier: GPL-2.0

//! Display window component.
//!
//! Window ownership and usage-bound methods are shared by DISP 4.x
//! and 5.x. Physical window and tile assignments required by some DISP 5.x
//! GPUs are handled by their display HAL.

use kernel::prelude::*;

use super::{
    channel::{
        Channel,
        Core,
        WindowDma,
        WindowImmediate, //
    },
    format::DisplayPixelFormat,
    hal::DisplayMethodHal,
    methods::core,
    mode::DisplayMode,
    Display, //
};

/// A display window.
pub(super) struct Window<'bound> {
    pub(super) id: u32,
    pub(super) channel: Option<Channel<'bound, WindowDma>>,
    pub(super) immediate_channel: Option<Channel<'bound, WindowImmediate>>,
}

impl Window<'_> {
    pub(super) fn new(id: u32) -> Self {
        Self {
            id,
            channel: None,
            immediate_channel: None,
        }
    }

    /// Queues this window's head assignment for a separate core UPDATE.
    #[expect(dead_code)]
    pub(super) fn emit_owner_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, Core>,
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
    #[expect(dead_code)]
    pub(super) fn emit_clear_format_usage_bounds_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, Core>,
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
    #[expect(dead_code)]
    pub(super) fn emit_usage_methods(
        &self,
        hal: &dyn DisplayMethodHal,
        channel: &mut Channel<'_, Core>,
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
}

impl<'bound> Display<'bound> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'bound>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
