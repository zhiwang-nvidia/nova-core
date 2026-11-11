// SPDX-License-Identifier: GPL-2.0

//! Display window component.

use super::Display;

/// A display window.
pub(super) struct Window {
    pub(super) id: u32,
}

impl Display<'_> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }
}
