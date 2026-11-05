// SPDX-License-Identifier: GPL-2.0

//! Display window component.

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
        WindowDma,
        WindowImmediate, //
    },
    Display, //
};

/// A display window.
pub(super) struct Window<'a, 'gpu> {
    pub(super) id: u32,
    pub(super) immediate_channel: Option<Channel<'a, 'gpu, WindowImmediate>>,
    pub(super) channel: Option<Channel<'a, 'gpu, WindowDma>>,
}

impl Window<'_, '_> {
    pub(super) fn new(id: u32) -> Self {
        Self {
            id,
            immediate_channel: None,
            channel: None,
        }
    }

    /// Reads this window's armed head assignment, or `None` when unassigned.
    ///
    /// The value comes from the core channel's armed state cache. DISP 4.x and
    /// 5.x share this register offset and OWNER field encoding.
    ///
    /// The caller must wait for core idle and prevent further core submissions
    /// while reading the armed state.
    #[expect(dead_code)]
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
}

impl<'a, 'gpu> Display<'a, 'gpu> {
    /// Finds an enumerated window by its hardware ID.
    #[expect(dead_code)]
    pub(super) fn window(&self, id: u32) -> Option<&Window<'a, 'gpu>> {
        self.windows.iter().find(|window| window.id == id)
    }
}
