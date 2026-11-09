// SPDX-License-Identifier: GPL-2.0

//! Framebuffer memory and its window channel binding.

use kernel::{
    prelude::*,
    ptr::{
        Alignable,
        Alignment, //
    },
    sizes::SZ_4K,
    sync::{
        Arc,
        Mutex, //
    },
};

use crate::{
    mm::{
        bar_user::{
            BarMapping,
            BarUser, //
        },
        GpuMm, //
    },
    num::IntoSafeCast, //
};

use super::{
    channel::{
        Channel,
        WindowDma, //
    },
    hal::{
        DisplayHal,
        SurfaceDescriptor, //
    },
    DisplayConfiguration,
    DisplayPixelFormat, //
};

/// A framebuffer retained until the window has finished reading it or display shutdown.
pub(super) struct Framebuffer<'a, 'gpu> {
    mapping: BarMapping<'a, 'gpu>,
    descriptor: Option<SurfaceDescriptor>,
    bound: bool,
    format: DisplayPixelFormat,
    pitch: u32,
    width: u32,
    height: u32,
}

impl<'a, 'gpu> Framebuffer<'a, 'gpu> {
    pub(super) fn new(
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        configuration: &DisplayConfiguration,
        pixels: &[u8],
    ) -> Result<Self> {
        configuration.validate_framebuffer()?;
        let framebuffer_size =
            u64::from(configuration.pitch) * u64::from(configuration.mode.vdisplay);
        let framebuffer_size = usize::try_from(framebuffer_size).map_err(|_| EOVERFLOW)?;
        if pixels.len() != framebuffer_size || !framebuffer_size.is_multiple_of(size_of::<u32>()) {
            return Err(EINVAL);
        }
        let region_size = framebuffer_size
            .align_up(Alignment::new::<SZ_4K>())
            .ok_or(EOVERFLOW)?;
        let backing = Arc::new(
            mm.lock()
                .alloc_vram_aligned(region_size.into_safe_cast(), Alignment::new::<SZ_4K>())?,
            GFP_KERNEL,
        )?;
        let region = backing.region(0..region_size.into_safe_cast())?;
        let mapping = BarMapping::new(bar_user, mm, region, true)?;

        for (index, word) in pixels.chunks_exact(size_of::<u32>()).enumerate() {
            let word = u32::from_le_bytes(word.try_into().map_err(|_| EINVAL)?);
            mapping.try_write32(word, index * size_of::<u32>())?;
        }
        for offset in (framebuffer_size..region_size).step_by(size_of::<u32>()) {
            mapping.try_write32(0, offset)?;
        }
        mapping.try_read32(0)?;

        Ok(Self {
            mapping,
            descriptor: None,
            bound: false,
            format: configuration.format,
            pitch: configuration.pitch,
            width: configuration.mode.hdisplay,
            height: configuration.mode.vdisplay,
        })
    }

    pub(super) fn matches_layout(&self, configuration: &DisplayConfiguration) -> bool {
        self.format == configuration.format
            && self.pitch == configuration.pitch
            && self.width == configuration.mode.hdisplay
            && self.height == configuration.mode.vdisplay
    }

    /// Retain this framebuffer through display shutdown if binding fails after allocation.
    pub(super) fn bind(
        &mut self,
        channel: &Channel<'a, 'gpu, WindowDma>,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
    ) -> Result {
        if self.descriptor.is_some() {
            return Err(EBUSY);
        }
        if !channel.is_created() {
            return Err(ENODEV);
        }
        let descriptor =
            hal.alloc_surface_descriptor(self.mapping.region().clone(), false, false)?;
        self.descriptor = Some(descriptor);
        channel.bind_surface_descriptor(hal, descriptor)?;
        self.bound = true;
        Ok(())
    }

    pub(super) fn descriptor(&self) -> Result<SurfaceDescriptor> {
        if !self.bound {
            return Err(ENODEV);
        }
        self.descriptor.ok_or(ENODEV)
    }

    /// Unbinds and frees the descriptor after the window has finished reading the framebuffer.
    pub(super) fn unbind(
        &mut self,
        channel: &Channel<'a, 'gpu, WindowDma>,
        hal: &mut dyn DisplayHal<'a, 'gpu>,
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
