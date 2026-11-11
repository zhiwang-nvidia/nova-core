// SPDX-License-Identifier: GPL-2.0

//! Display window component.

use kernel::prelude::*;

use super::{
    channel::{
        Channel,
        Core,
        WindowDma,
        WindowImmediate, //
    },
    hal::DisplayMethodHal,
    methods::core,
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
}

impl<'bound> Display<'bound> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'bound>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
