// SPDX-License-Identifier: GPL-2.0

//! Channel method definitions shared by DISP 4.x and DISP 5.x.

pub(crate) mod core {
    //! Shared core notifier-control fields.

    pub(crate) const SET_NOTIFIER_CONTROL: u32 = 0x020c;
    pub(crate) const SET_NOTIFIER_CONTROL_NOTIFY_ENABLE: u32 = 1 << 12;
    // Window ownership and usage fields match DISP 4.x and 5.x core classes.
    pub(crate) const HEAD_COUNT: u32 = 8;
    pub(crate) const WINDOW_COUNT: u32 = 32;
    pub(crate) const WINDOW_STRIDE: u32 = 0x0080;
    pub(crate) const WINDOW_SET_CONTROL: u32 = 0x1000;
    pub(crate) const WINDOW_SET_CONTROL_OWNER_NONE: u32 = 0x0f;
    pub(crate) const WINDOW_SET_WINDOW_FORMAT_USAGE_BOUNDS: u32 = 0x1004;
    pub(crate) const WINDOW_FORMAT_USAGE_RGB_PACKED4BPP: u32 = 1 << 2;
    pub(crate) const WINDOW_SET_MAX_INPUT_SCALE_FACTOR: u32 = 0x100c;
    pub(crate) const WINDOW_SCALE_FACTOR_1X: u32 = 1024;
    pub(crate) const WINDOW_SCALE_FACTOR_VERTICAL_SHIFT: u32 = 16;
    pub(crate) const WINDOW_SET_WINDOW_USAGE_BOUNDS: u32 = 0x1010;
    pub(crate) const WINDOW_USAGE_MAX_PIXELS_FETCHED: u32 = 0x7fff;
    pub(crate) const WINDOW_USAGE_INPUT_SCALER_TAPS_2: u32 = 1 << 20;
}

pub(crate) mod window {
    //! Shared window notifier-control fields.

    pub(crate) const SET_NOTIFIER_CONTROL: u32 = 0x0220;
    pub(crate) const SET_NOTIFIER_CONTROL_MODE_WRITE: u32 = 0;
}

pub(crate) mod window_immediate {
    //! Point-out method shared by DISP 4.x and 5.x immediate channels.
    pub(crate) const SET_POINT_OUT: u32 = 0x0208;
    pub(crate) const SET_POINT_OUT_Y_SHIFT: u32 = 16;
}
