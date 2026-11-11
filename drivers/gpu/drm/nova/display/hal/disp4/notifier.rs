// SPDX-License-Identifier: GPL-2.0

//! Notifier method programming shared by DISP 4.x channels.
//!
//! DISP 4.x selects notifier memory through a Context DMA handle. DISP 5.x
//! supplies a surface address, target and enable field instead.

use kernel::prelude::*;

use crate::display::{
    channel::{
        Channel,
        Core,
        WindowDma, //
    },
    hal::{
        DisplayMethodHal,
        SurfaceDescriptor, //
    },
    methods as common, //
};

use super::methods::{
    core,
    window, //
};

pub(crate) fn emit_core_notifier_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, Core>,
    descriptor: Option<SurfaceDescriptor>,
) -> Result {
    let (handle, control) = match descriptor {
        Some(descriptor) => (
            descriptor.context_dma_descriptor_id.handle,
            common::core::SET_NOTIFIER_CONTROL_NOTIFY_ENABLE,
        ),
        None => (0, 0),
    };
    channel.emit(hal, core::SET_CONTEXT_DMA_NOTIFIER, &[handle])?;
    channel.emit(hal, common::core::SET_NOTIFIER_CONTROL, &[control])
}

pub(crate) fn emit_window_notifier_methods(
    hal: &dyn DisplayMethodHal,
    channel: &mut Channel<'_, WindowDma>,
    descriptor: Option<SurfaceDescriptor>,
) -> Result {
    let handle = descriptor.map_or(0, |descriptor| descriptor.context_dma_descriptor_id.handle);
    channel.emit(hal, window::SET_CONTEXT_DMA_NOTIFIER, &[handle])?;
    // Window notification is enabled by the ContextDMA; there is no NOTIFY bit.
    channel.emit(
        hal,
        common::window::SET_NOTIFIER_CONTROL,
        &[common::window::SET_NOTIFIER_CONTROL_MODE_WRITE],
    )
}
