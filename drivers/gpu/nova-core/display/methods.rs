// SPDX-License-Identifier: GPL-2.0

//! Channel method definitions shared by DISP 4.x and DISP 5.x.

pub(crate) mod core {
    //! Shared core notifier-control fields.

    pub(crate) const SET_NOTIFIER_CONTROL: u32 = 0x020c;
    pub(crate) const SET_NOTIFIER_CONTROL_NOTIFY_ENABLE: u32 = 1 << 12;
    // Window ownership fields match DISP 4.x and 5.x core classes.
    pub(crate) const HEAD_COUNT: u32 = 8;
    pub(crate) const WINDOW_COUNT: u32 = 32;
    pub(crate) const WINDOW_STRIDE: u32 = 0x0080;
    pub(crate) const WINDOW_SET_CONTROL: u32 = 0x1000;
    pub(crate) const WINDOW_SET_CONTROL_OWNER_NONE: u32 = 0x0f;
}

pub(crate) mod window {
    //! Shared window notifier-control fields.

    pub(crate) const SET_NOTIFIER_CONTROL: u32 = 0x0220;
    pub(crate) const SET_NOTIFIER_CONTROL_MODE_WRITE: u32 = 0;
}
