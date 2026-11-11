// SPDX-License-Identifier: GPL-2.0

//! Display window component.

use super::{
    channel::{
        Channel,
        WindowDma,
        WindowImmediate, //
    },
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
}

impl<'bound> Display<'bound> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'bound>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
