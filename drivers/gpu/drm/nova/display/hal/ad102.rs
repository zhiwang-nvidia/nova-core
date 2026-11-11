// SPDX-License-Identifier: GPL-2.0

use kernel::prelude::*;

use nova_core::api::{
    DisplayPrecompCaps,
    DisplayStaticInfo,
    NovaCoreApiHandle,
    VramRegion, //
};

use crate::display::{
    channel::{
        Channel,
        Core,
        WindowDma, //
    },
    format::DisplayPixelFormat,
    framebuffer::FramebufferLayout,
    mode::DisplayMode,
    scaler, //
};

use super::{
    disp4::{
        self,
        instance_memory::InstanceMemory,
        methods::{
            self,
            c67e,
            core as core_methods,
            window as window_methods, //
        },
        modeset,
        window, //
    },
    DisplayHal,
    DisplayMethodHal,
    SurfaceDescriptor, //
};

pub(super) const IP_VERSION: u32 = 0x0404_0000;

const NVC770_DISPLAY: u32 = 0xc770;
const NVC77D_CORE_CHANNEL_DMA: u32 = 0xc77d;
const NVC67E_WINDOW_CHANNEL_DMA: u32 = 0xc67e;
const NVC67B_WINDOW_IMM_CHANNEL_DMA: u32 = 0xc67b;
const NVC67A_CURSOR_IMM_CHANNEL_PIO: u32 = 0xc67a;

pub(super) struct Ad102<'bound> {
    api: NovaCoreApiHandle<'bound>,
    instance_memory: InstanceMemory<'bound>,
}

impl<'bound> Ad102<'bound> {
    pub(super) fn new(
        api: &NovaCoreApiHandle<'bound>,
        info: &DisplayStaticInfo,
    ) -> Result<KBox<Self>> {
        if !Self::supports_classes(info) {
            return Err(EOPNOTSUPP);
        }

        Ok(KBox::new(
            Self {
                api: api.clone(),
                instance_memory: InstanceMemory::new(api)?,
            },
            GFP_KERNEL,
        )?)
    }

    fn supports_classes(info: &DisplayStaticInfo) -> bool {
        info.display_class == NVC770_DISPLAY
            && info.core_class == NVC77D_CORE_CHANNEL_DMA
            && info.window_class == NVC67E_WINDOW_CHANNEL_DMA
            && info.immediate_class == NVC67B_WINDOW_IMM_CHANNEL_DMA
            && info.cursor_class == NVC67A_CURSOR_IMM_CHANNEL_PIO
    }

    fn emit_disable_window_methods(
        &self,
        window: &mut Channel<'bound, WindowDma>,
        caps: DisplayPrecompCaps,
    ) -> Result {
        // Acquire semaphore methods are supported by C67E/C87E, but not C57E.
        for method in [
            c67e::SET_CONTEXT_DMA_ACQ_SEMAPHORE,
            c67e::SET_ACQ_SEMAPHORE_CONTROL,
            c67e::SET_ACQ_SEMAPHORE_VALUE,
        ] {
            window.emit(self, method, &[0])?;
        }
        window::emit_disable_window_methods(self, window, caps)
    }
}

// TODO: Factor out a common DISP 4.x HAL when adding support for more
// DISP 4.x GPUs.
impl<'bound> DisplayHal<'bound> for Ad102<'bound> {
    fn emit_core_notifier_methods(
        &self,
        channel: &mut Channel<'bound, Core>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result {
        disp4::notifier::emit_core_notifier_methods(self, channel, descriptor)
    }

    fn emit_window_notifier_methods(
        &self,
        channel: &mut Channel<'bound, WindowDma>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result {
        disp4::notifier::emit_window_notifier_methods(self, channel, descriptor)
    }

    fn init_core_channel(&self, channel: &mut Channel<'bound, Core>, head_mask: u32) -> Result {
        self.emit_core_notifier_methods(channel, None)?;
        if head_mask == 0 || head_mask >> core_methods::HEAD_COUNT != 0 {
            return Err(EINVAL);
        }
        disp4::scaler::emit_postcomp_scaler_coeffs_methods(self, channel, head_mask)?;
        disp4::head::emit_desktop_color_methods(self, channel, head_mask)?;
        channel.advance()
    }

    fn init_window_channel(
        &self,
        channel: &mut Channel<'bound, WindowDma>,
        head_mask: u32,
    ) -> Result {
        self.emit_window_notifier_methods(channel, None)?;
        let caps = self
            .api
            .with(|core| core.display_precomp_caps(channel.kind.index as usize))?;

        // NVKMS: prefer postcomp scaling when any usable head provides it.
        if caps.scaler && !scaler::has_postcomp_scaler(&self.api, head_mask)? {
            disp4::scaler::emit_precomp_scaler_coeffs_methods(self, channel)?;
        }

        if caps.csc0_lut && caps.csc1_lut {
            disp4::csc::emit_csc_lut_methods(self, channel)?;
        }
        channel.advance()
    }

    fn instance_memory(&self) -> &VramRegion {
        self.instance_memory.region()
    }

    fn alloc_surface_descriptor(
        &mut self,
        target_region: VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<SurfaceDescriptor> {
        let context_dma_descriptor_id =
            self.instance_memory
                .alloc_context_dma(target_region, writable, block_linear)?;
        Ok(SurfaceDescriptor {
            context_dma_descriptor_id,
        })
    }

    fn bind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result {
        self.instance_memory.bind_context_dma(
            descriptor.context_dma_descriptor_id,
            client_id,
            display_channel,
        )
    }

    fn unbind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result {
        self.instance_memory.unbind_context_dma(
            descriptor.context_dma_descriptor_id,
            client_id,
            display_channel,
        )
    }

    fn free_surface_descriptor(&mut self, descriptor: SurfaceDescriptor) -> Result {
        self.instance_memory
            .free_context_dma(descriptor.context_dma_descriptor_id)
    }

    fn validate_mode(&self, mode: &DisplayMode) -> Result {
        modeset::validate_mode(mode)?;
        // Ada limits each head viewport dimension to 8192 pixels.
        // The current path uses the visible mode for both input and output.
        if mode.hdisplay > 8192 || mode.vdisplay > 8192 {
            return Err(ERANGE);
        }
        Ok(())
    }

    /// Emits methods to configure a pitch-linear framebuffer with composition bypass.
    fn emit_window_framebuffer_configuration(
        &self,
        window: &mut Channel<'bound, WindowDma>,
        mode: &DisplayMode,
        framebuffer: &FramebufferLayout,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        const PITCH_ALIGNMENT: u32 = 256;

        mode.validate_framebuffer(framebuffer)?;
        self.validate_mode(mode)?;
        let pitch = framebuffer.pitch;
        let encoded_pitch = pitch >> window_methods::SET_PLANAR_STORAGE_PITCH_BYTE_SHIFT;
        if !pitch.is_multiple_of(PITCH_ALIGNMENT)
            || encoded_pitch > window_methods::SET_PLANAR_STORAGE_PITCH_MASK
            || descriptor.context_dma_descriptor_id.handle == 0
        {
            return Err(EINVAL);
        }
        let caps = self
            .api
            .with(|core| core.display_precomp_caps(window.kind.index as usize))?;
        window::emit_window_geometry_methods(self, window, mode, framebuffer, &caps)?;
        window::emit_window_color_methods(self, window, &caps)?;
        let format = match framebuffer.format {
            DisplayPixelFormat::Xrgb8888 => c67e::SET_PARAMS_FORMAT_X8R8G8B8,
        };
        // C57E has a MEMORY_LAYOUT field here; C67E uses its Context DMA descriptor.
        window.emit(
            self,
            c67e::SET_STORAGE,
            &[c67e::SET_STORAGE_BLOCK_HEIGHT_ONE_GOB],
        )?;
        window.emit(self, c67e::SET_PARAMS, &[format])?;
        // SET_SCAN_DIRECTION is available in C67E/C87E, but not C57E.
        window.emit(
            self,
            c67e::SET_SCAN_DIRECTION,
            &[c67e::SET_SCAN_DIRECTION_FROM_TOP_LEFT],
        )?;
        self.emit_disable_window_methods(window, caps)?;
        window.emit(
            self,
            window_methods::SET_CONTEXT_DMA_ISO,
            &[descriptor.context_dma_descriptor_id.handle],
        )
    }

    fn emit_disable_window(&self, window: &mut Channel<'bound, WindowDma>) -> Result {
        let caps = self
            .api
            .with(|core| core.display_precomp_caps(window.kind.index as usize))?;
        self.emit_disable_window_methods(window, caps)
    }
}

impl DisplayMethodHal for Ad102<'_> {
    fn encode_method_header(&self, method: u32, count: usize) -> Result<u32> {
        methods::encode_method(method, count)
    }
}
