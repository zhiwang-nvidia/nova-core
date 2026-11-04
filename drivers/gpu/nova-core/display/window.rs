// SPDX-License-Identifier: GPL-2.0

//! Display window component.

use super::{
    channel::{
        Channel,
        WindowDma, //
    },
    Display, //
};

/// A display window.
pub(super) struct Window<'a, 'gpu> {
    pub(super) id: u32,
    pub(super) channel: Option<Channel<'a, 'gpu, WindowDma>>,
}

impl Window<'_, '_> {
    pub(super) fn new(id: u32) -> Self {
        Self { id, channel: None }
    }
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'a, 'gpu>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
