// SPDX-License-Identifier: GPL-2.0

//! Display streams.

use super::DisplayConfiguration;

/// A display stream from one head to an output.
pub(super) struct Stream {
    pub(super) display_id: u32,
    pub(super) head: u32,
}

impl Stream {
    pub(super) fn new(configuration: &DisplayConfiguration, head: u32) -> Self {
        Self {
            display_id: configuration.display_id,
            head,
        }
    }
}
