// SPDX-License-Identifier: GPL-2.0

//! Common window helpers for DISP 4.x.

use kernel::prelude::*;

use nova_core::api::DisplayPrecompCaps;

use crate::display::{
    channel::{
        Channel,
        WindowDma, //
    },
    framebuffer::FramebufferLayout,
    hal::DisplayMethodHal,
    mode::DisplayMode, //
};

use super::methods::{
    self,
    window as window_methods, //
};

/// Emits framebuffer geometry and presentation state shared by DISP 4.x.
///
/// Storage layout, scan direction and semaphore setup are selected by the HAL.
pub(crate) fn emit_window_geometry_methods(
    hal: &dyn DisplayMethodHal,
    window: &mut Channel<'_, WindowDma>,
    mode: &DisplayMode,
    framebuffer: &FramebufferLayout,
    caps: &DisplayPrecompCaps,
) -> Result {
    let encoded_pitch = framebuffer.pitch >> window_methods::SET_PLANAR_STORAGE_PITCH_BYTE_SHIFT;
    let size = framebuffer.width | (framebuffer.height << window_methods::SET_SIZE_HEIGHT_SHIFT);
    let visible = mode.hdisplay | (mode.vdisplay << window_methods::SET_SIZE_HEIGHT_SHIFT);
    for (method, value) in [
        (
            window_methods::SET_PRESENT_CONTROL,
            window_methods::SET_PRESENT_CONTROL_MIN_PRESENT_INTERVAL_ONE
                | window_methods::SET_PRESENT_CONTROL_BEGIN_MODE_NON_TEARING
                | window_methods::SET_PRESENT_CONTROL_TIMESTAMP_MODE_DISABLE
                | window_methods::SET_PRESENT_CONTROL_STEREO_MODE_MONO,
        ),
        (window_methods::SET_SIZE, size),
        (window_methods::SET_SIZE_IN, visible),
        (window_methods::SET_SIZE_OUT, visible),
        (window_methods::SET_UPDATE_TIMESTAMP_LO, 0),
        (window_methods::SET_UPDATE_TIMESTAMP_HI, 0),
        (
            window_methods::SET_COMPOSITION_CONTROL,
            window_methods::SET_COMPOSITION_CONTROL_BYPASS_ENABLE,
        ),
    ] {
        window.emit(hal, method, &[value])?;
    }
    for eye in 0..window_methods::EYE_COUNT {
        window.emit(
            hal,
            window_methods::SET_POINT_IN + eye * methods::METHOD_ARRAY_STRIDE,
            &[0],
        )?;
    }
    for plane in 0..window_methods::PLANE_COUNT {
        window.emit(
            hal,
            window_methods::SET_PLANAR_STORAGE + plane * methods::METHOD_ARRAY_STRIDE,
            &[if plane == 0 { encoded_pitch } else { 0 }],
        )?;
    }
    if caps.scaler {
        window.emit(
            hal,
            window_methods::SET_CONTROL_INPUT_SCALER,
            &[window_methods::SET_CONTROL_INPUT_SCALER_VERTICAL_TAPS_2
                | window_methods::SET_CONTROL_INPUT_SCALER_HORIZONTAL_TAPS_2],
        )?;
    }
    Ok(())
}

/// Emits identity color state shared by DISP 4.x window classes.
pub(crate) fn emit_window_color_methods(
    hal: &dyn DisplayMethodHal,
    window: &mut Channel<'_, WindowDma>,
    caps: &DisplayPrecompCaps,
) -> Result {
    for (index, value) in (0u32..).zip(window_methods::FMT_IDENTITY_COEFFICIENTS) {
        window.emit(
            hal,
            window_methods::SET_FMT_COEFFICIENT_C00 + index * methods::METHOD_ARRAY_STRIDE,
            &[value],
        )?;
    }
    // Identity CSCs are represented by disabling their matrix, as in NVKMS.
    for (present, method) in [
        (caps.csc00, window_methods::SET_CSC00CONTROL),
        (caps.csc01, window_methods::SET_CSC01CONTROL),
        (caps.csc0_lut, window_methods::SET_CSC0LUT_CONTROL),
        (caps.csc10, window_methods::SET_CSC10CONTROL),
        (caps.csc11, window_methods::SET_CSC11CONTROL),
        (caps.csc1_lut, window_methods::SET_CSC1LUT_CONTROL),
    ] {
        if present {
            window.emit(hal, method, &[0])?;
        }
    }
    Ok(())
}

/// Emits methods to disable framebuffer/LUT and release semaphore accesses.
///
/// Acquire semaphores are absent from C57E and remain a separate HAL operation.
pub(crate) fn emit_disable_window_methods(
    hal: &dyn DisplayMethodHal,
    window: &mut Channel<'_, WindowDma>,
    caps: DisplayPrecompCaps,
) -> Result {
    for method in [
        window_methods::SET_CONTEXT_DMA_SEMAPHORE,
        window_methods::SET_SEMAPHORE_CONTROL,
        window_methods::SET_SEMAPHORE_RELEASE,
    ] {
        window.emit(hal, method, &[0])?;
    }
    for slot in 0..window_methods::ISO_CONTEXT_COUNT {
        let offset = slot * methods::METHOD_ARRAY_STRIDE;
        window.emit(hal, window_methods::SET_CONTEXT_DMA_ISO + offset, &[0])?;
        window.emit(hal, window_methods::SET_OFFSET + offset, &[0])?;
    }
    window.emit(hal, window_methods::SET_CONTEXT_DMA_ILUT, &[0])?;
    window.emit(hal, window_methods::SET_OFFSET_ILUT, &[0])?;
    if caps.tmo_lut {
        window.emit(hal, window_methods::SET_CONTEXT_DMA_TMO_LUT, &[0])?;
        window.emit(hal, window_methods::SET_OFFSET_TMO_LUT, &[0])?;
    }
    Ok(())
}
