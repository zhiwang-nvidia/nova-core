// SPDX-License-Identifier: GPL-2.0

//! Framebuffer memory and its window channel binding.

use kernel::{
    prelude::*,
    sizes::SizeConstants, //
};

use nova_core::api::VramRegion;

use super::{
    channel::{
        Channel,
        WindowDma, //
    },
    format::DisplayPixelFormat,
    hal::{
        DisplayHal,
        SurfaceDescriptor, //
    },
    DisplayConfiguration, //
};

/// The dimensions and storage layout of a complete framebuffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FramebufferLayout {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: DisplayPixelFormat,
    pub(crate) pitch: u32,
}

impl FramebufferLayout {
    // DISP 4.x and 5.x window surface dimensions have 16-bit fields.
    pub(crate) const MAX_DIMENSION: u32 = 0xffff;

    pub(crate) fn validate(&self) -> Result {
        if self.width == 0
            || self.height == 0
            || self.width > Self::MAX_DIMENSION
            || self.height > Self::MAX_DIMENSION
        {
            return Err(EINVAL);
        }
        let minimum_pitch = self
            .width
            .checked_mul(self.format.bytes_per_pixel())
            .ok_or(EOVERFLOW)?;
        if self.pitch < minimum_pitch {
            return Err(EINVAL);
        }
        Ok(())
    }
}

/// A framebuffer retained until the window has finished reading it or display shutdown.
pub(super) struct Framebuffer {
    region: VramRegion,
    descriptor: Option<SurfaceDescriptor>,
    bound: bool,
}

impl Framebuffer {
    pub(super) fn new(configuration: &DisplayConfiguration, region: VramRegion) -> Result<Self> {
        configuration.validate_framebuffer()?;
        let layout = configuration.framebuffer;
        let required_size = u64::from(layout.pitch) * u64::from(layout.height);
        // DISP 4.x Context DMA descriptors cover page-aligned VRAM ranges.
        if region.size() < required_size
            || !region.address().is_multiple_of(u64::SZ_4K)
            || !region.size().is_multiple_of(u64::SZ_4K)
        {
            return Err(EINVAL);
        }

        Ok(Self {
            region,
            descriptor: None,
            bound: false,
        })
    }

    pub(super) fn retain_backing(&self) {
        core::mem::forget(self.region.clone());
    }

    /// Binds the framebuffer before any methods can refer to its descriptor.
    pub(super) fn bind<'bound>(
        &mut self,
        channel: &mut Channel<'bound, WindowDma>,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        if self.descriptor.is_some() {
            return Err(EBUSY);
        }
        if !channel.is_created() {
            return Err(ENODEV);
        }
        let descriptor = hal.alloc_surface_descriptor(self.region.clone(), false, false)?;
        self.descriptor = Some(descriptor);
        channel.bind_surface_descriptor(hal, descriptor)?;
        self.bound = true;
        Ok(())
    }

    #[expect(dead_code)]
    pub(super) fn descriptor(&self) -> Result<SurfaceDescriptor> {
        if !self.bound {
            return Err(ENODEV);
        }
        self.descriptor.ok_or(ENODEV)
    }

    /// Unbinds and frees the descriptor after the window has finished reading the framebuffer.
    pub(super) fn unbind<'bound>(
        &mut self,
        channel: &mut Channel<'bound, WindowDma>,
        hal: &mut dyn DisplayHal<'bound>,
    ) -> Result {
        if let Some(descriptor) = self.descriptor {
            if self.bound {
                channel.unbind_surface_descriptor(hal, descriptor)?;
                self.bound = false;
            }
            hal.free_surface_descriptor(descriptor)?;
            self.descriptor = None;
        }
        Ok(())
    }
}
