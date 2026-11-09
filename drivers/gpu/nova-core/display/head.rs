// SPDX-License-Identifier: GPL-2.0

//! Display head component.

use super::Display;

/// A display head.
pub(super) struct Head {
    pub(super) id: u32,
}

impl Display<'_, '_> {
    /// Finds an enumerated head by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn head(&self, id: u32) -> Option<&Head> {
        self.heads.iter().find(|head| head.id == id)
    }
}
