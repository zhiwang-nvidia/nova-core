// SPDX-License-Identifier: GPL-2.0

//! Common head helpers for DISP 4.x.
//!
//! These helpers use Context DMA handles, a 0x400 head method stride and 15-bit
//! raster fields. DISP 5.x uses surface addresses and a different head layout.
//! Methods absent from some DISP 4.x classes are emitted separately by the HAL.

use kernel::prelude::*;

use nova_core::api::DisplayPostcompCaps;

use crate::display::{
    channel::{
        Channel,
        Core, //
    },
    hal::DisplayMethodHal,
    mode::DisplayMode, //
};

use super::methods::{
    self,
    core, //
};

pub(crate) fn emit_desktop_color_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head_mask: u32,
) -> Result {
    for head in 0..core::HEAD_COUNT {
        if head_mask & (1 << head) == 0 {
            continue;
        }
        let offset = head * core::HEAD_STRIDE;
        channel.emit(
            hal,
            core::HEAD_SET_DESKTOP_COLOR_ALPHA_RED + offset,
            &[0xff],
        )?;
        channel.emit(hal, core::HEAD_SET_DESKTOP_COLOR_GREEN_BLUE + offset, &[0])?;
    }
    Ok(())
}

/// Disables cursor, output LUT and CRC memory accesses, and the output CSC0 matrix.
pub(crate) fn emit_disable_head_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head: u32,
    caps: &DisplayPostcompCaps,
) -> Result {
    let offset = head * core::HEAD_STRIDE;
    // Keep a valid cursor format while disabling the cursor and unsetting its handles.
    channel.emit(
        hal,
        core::HEAD_SET_CONTROL_CURSOR + offset,
        &[core::HEAD_CURSOR_FORMAT_A8R8G8B8],
    )?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_CURSOR + offset, &[0, 0])?;
    channel.emit(hal, core::HEAD_SET_OFFSET_CURSOR + offset, &[0, 0])?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_OLUT + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_OFFSET_OLUT + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_CONTEXT_DMA_CRC + offset, &[0])?;
    channel.emit(hal, core::HEAD_SET_CRC_CONTROL + offset, &[0])?;
    if caps.ocsc0 {
        channel.emit(hal, core::HEAD_SET_OCSC0CONTROL + offset, &[0])?;
    }
    Ok(())
}

/// Emits a head's GSP-RM display ID; zero unsets the association.
pub(crate) fn emit_head_display_id_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    head: u32,
    display_id: u32,
) -> Result {
    channel.emit(
        hal,
        core::HEAD_SET_DISPLAY_ID + head * core::HEAD_STRIDE,
        &[display_id],
    )
}

/// Core methods pack raster and viewport coordinates into two 15-bit fields.
fn coordinate_pair(x: u32, y: u32) -> u32 {
    x | (y << core::COORDINATE_Y_SHIFT)
}

/// Emits the head state shared by DISP 4.x core classes, without an UPDATE.
pub(crate) fn emit_head_configuration_methods(
    hal: &dyn DisplayMethodHal,
    core: &mut Channel<'_, Core>,
    head: u32,
    mode: &DisplayMode,
    limited_range: bool,
    caps: &DisplayPostcompCaps,
) -> Result {
    const HERTZ_PER_KHZ: u32 = 1000;
    const CLAMP_FULL_LOW: u32 = 0;
    const CLAMP_FULL_HIGH: u32 = 0xfff;
    const CLAMP_LIMITED_LOW: u32 = 0x100;
    const CLAMP_LIMITED_HIGH: u32 = 0xeb0;
    const LIMITED_RGB_SCALE: u32 = 0xdb04;
    const LIMITED_RGB_OFFSET: u32 = 0x1000;
    const MATRIX_ROWS: u32 = 3;
    const MATRIX_COLUMNS: u32 = 4;

    let offset = head * core::HEAD_STRIDE;
    let clock_hz = mode.clock_khz * HERTZ_PER_KHZ;
    let raster = mode.raster_timings()?;
    let visible = coordinate_pair(mode.hdisplay, mode.vdisplay);
    let mut output = core::HEAD_OUTPUT_CRC_COMPLETE_RASTER
        | core::HEAD_OUTPUT_PIXEL_DEPTH_BPP_24_444
        | core::HEAD_OUTPUT_EXT_PACKET_WIN_NONE;
    if mode.negative_hsync {
        output |= core::HEAD_OUTPUT_HSYNC_NEGATIVE;
    }
    if mode.negative_vsync {
        output |= core::HEAD_OUTPUT_VSYNC_NEGATIVE;
    }

    core.emit(hal, core::SET_CONTROL, &[0])?;
    // All-zero controls select progressive, unlocked RGB with no stereo,
    // clock hopping, dithering, YUV420 packing, VRR or merged heads.
    for (method, value) in [
        (core::HEAD_SET_CONTROL_OUTPUT_RESOURCE, output),
        (core::HEAD_SET_CONTROL, 0),
        (core::HEAD_SET_PIXEL_CLOCK_FREQUENCY, clock_hz),
        (core::HEAD_SET_PIXEL_CLOCK_CONFIGURATION, 0),
        (core::HEAD_SET_PIXEL_CLOCK_FREQUENCY_MAX, clock_hz),
        (core::HEAD_SET_DITHER_CONTROL, 0),
        (core::HEAD_SET_STALL_LOCK, 0),
        (core::HEAD_SET_LOCK_CHAIN, 0),
        (core::HEAD_SET_VIEWPORT_POINT_IN, 0),
        (core::HEAD_SET_VIEWPORT_SIZE_IN, visible),
        (core::HEAD_SET_VIEWPORT_SIZE_OUT, visible),
        (core::HEAD_SET_VIEWPORT_POINT_OUT_ADJUST, 0),
        (
            core::HEAD_SET_RASTER_SIZE,
            coordinate_pair(mode.htotal, mode.vtotal),
        ),
        (
            core::HEAD_SET_RASTER_SYNC_END,
            coordinate_pair(raster.hsync_end, raster.vsync_end),
        ),
        (
            core::HEAD_SET_RASTER_BLANK_END,
            coordinate_pair(raster.hblank_end, raster.vblank_end),
        ),
        (
            core::HEAD_SET_RASTER_BLANK_START,
            coordinate_pair(raster.hblank_start, raster.vblank_start),
        ),
        (
            core::HEAD_SET_MIN_FRAME_IDLE,
            coordinate_pair(raster.leading_idle, raster.trailing_idle),
        ),
        (core::HEAD_SET_OVERSCAN_COLOR, 0),
        (core::HEAD_SET_FRAME_PACKED_VACTIVE_COLOR, 0),
        (core::HEAD_SET_HDMI_CTRL, 0),
        (core::HEAD_SET_DISPLAY_RATE, 0),
        (core::HEAD_SET_TILE_POSITION, 0),
        (core::HEAD_SET_DSC_CONTROL, 0),
        (core::HEAD_SET_DSC_PPS_CONTROL, 0),
        (core::HEAD_SET_RG_MERGE, 0),
        (
            core::HEAD_SET_MAX_OUTPUT_SCALE_FACTOR,
            coordinate_pair(core::SCALE_FACTOR_1X, core::SCALE_FACTOR_1X),
        ),
        (
            core::HEAD_SET_HEAD_USAGE_BOUNDS,
            core::HEAD_USAGE_OUTPUT_SCALER_TAPS_2,
        ),
    ] {
        core.emit(hal, method + offset, &[value])?;
    }
    if caps.scaler {
        core.emit(
            hal,
            core::HEAD_SET_CONTROL_OUTPUT_SCALER + offset,
            &[core::HEAD_OUTPUT_SCALER_TAPS_2],
        )?;
    }

    core.emit(
        hal,
        core::HEAD_SET_PROCAMP + offset,
        &[if limited_range {
            core::HEAD_SET_PROCAMP_DYNAMIC_RANGE_CEA
        } else {
            0
        }],
    )?;
    if caps.ocsc1 {
        let clamp = if limited_range {
            coordinate_pair(CLAMP_LIMITED_LOW, CLAMP_LIMITED_HIGH)
        } else {
            coordinate_pair(CLAMP_FULL_LOW, CLAMP_FULL_HIGH)
        };
        core.emit(hal, core::HEAD_SET_CLAMP_RANGE_GREEN + offset, &[clamp])?;
        core.emit(hal, core::HEAD_SET_CLAMP_RANGE_RED_BLUE + offset, &[clamp])?;
        core.emit(
            hal,
            core::HEAD_SET_OCSC1CONTROL + offset,
            &[if limited_range {
                core::HEAD_OCSC1CONTROL_ENABLE
            } else {
                0
            }],
        )?;
        if limited_range {
            for row in 0..MATRIX_ROWS {
                for column in 0..MATRIX_COLUMNS {
                    let coefficient = if column == MATRIX_COLUMNS - 1 {
                        LIMITED_RGB_OFFSET
                    } else if row == column {
                        LIMITED_RGB_SCALE
                    } else {
                        0
                    };
                    let method = core::HEAD_SET_OCSC1COEFFICIENT_C00
                        + offset
                        + (row * MATRIX_COLUMNS + column) * methods::METHOD_ARRAY_STRIDE;
                    core.emit(hal, method, &[coefficient])?;
                }
            }
        }
    }
    Ok(())
}
