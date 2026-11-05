// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    sync::Mutex, //
};

use crate::{
    display::{
        capability,
        channel::{
            Channel,
            Core,
            WindowDma,
            WindowImmediate, //
        },
        hal::{
            disp4::{
                self,
                instance_memory::InstanceMemory,
                methods::{
                    self,
                    c67b,
                    c67e,
                    c77d,
                    core as core_methods,
                    window as window_methods, //
                },
                modeset,
                window, //
            },
            DisplayHal,
            DisplayMethodHal,
            SurfaceDescriptor, //
        },
        methods as common_methods,
        mode::DisplayMode,
        scaler,
        sor,
        DisplayPixelFormat, //
    },
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::DisplayHwStaticInfo, //
    },
    mm::{
        bar_user::BarUser,
        vram::VramRegion,
        GpuMm, //
    },
    num::IntoSafeCast,
    regs, //
};

pub(super) const IP_VERSION: u32 = 0x0404_0000;

const NVC770_DISPLAY: u32 = 0xc770;
const NVC77D_CORE_CHANNEL_DMA: u32 = 0xc77d;
const NVC67E_WINDOW_CHANNEL_DMA: u32 = 0xc67e;
const NVC67B_WINDOW_IMM_CHANNEL_DMA: u32 = 0xc67b;
const NVC67A_CURSOR_IMM_CHANNEL_PIO: u32 = 0xc67a;

// The current Ada HAL uses four SORs; the implemented count is device-specific.
const SOR_COUNT: u32 = 4;

pub(super) struct Ad102<'a, 'gpu> {
    instance_memory: InstanceMemory<'a, 'gpu>,
}

impl<'a, 'gpu> Ad102<'a, 'gpu> {
    pub(super) fn new(
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq<'gpu>,
        mm: &'a Mutex<GpuMm<'gpu>>,
        bar_user: &'a BarUser<'gpu>,
        info: &DisplayHwStaticInfo,
    ) -> Result<KBox<Self>> {
        if !Self::supports_classes(info) {
            return Err(EOPNOTSUPP);
        }

        KBox::init(
            try_init!(Self {
                instance_memory: InstanceMemory::new(dev, cmdq, mm, bar_user)?,
            }),
            GFP_KERNEL,
        )
    }

    fn supports_classes(info: &DisplayHwStaticInfo) -> bool {
        info.display_class == NVC770_DISPLAY
            && info.core_class == NVC77D_CORE_CHANNEL_DMA
            && info.window_class == NVC67E_WINDOW_CHANNEL_DMA
            && info.immediate_class == NVC67B_WINDOW_IMM_CHANNEL_DMA
            && info.cursor_class == NVC67A_CURSOR_IMM_CHANNEL_PIO
    }

    fn emit_disable_head_methods(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        head: u32,
        caps: regs::NV_PDISP_FE_SW_POSTCOMP_HEAD_HDR_CAPA,
    ) -> Result {
        disp4::head::emit_disable_head_methods(self, core, head, caps)?;
        let offset = head * core_methods::HEAD_STRIDE;
        // A NULL ContextDMA disables each RG release semaphore.
        for index in 0..c77d::HEAD_RG_REL_SEMAPHORE_COUNT {
            core.emit(
                self,
                c77d::HEAD_SET_CONTEXT_DMA_RG_REL_SEMAPHORE
                    + offset
                    + index * methods::METHOD_ARRAY_STRIDE,
                &[0],
            )?;
        }
        Ok(())
    }

    fn emit_disable_window_methods(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        caps: regs::NV_PDISP_FE_SW_PRECOMP_WIN_PIPE_HDR_CAPA,
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

// TODO: Factor out a common DISP 4.x method HAL when adding support for more
// DISP 4.x GPUs.
impl DisplayMethodHal for Ad102<'_, '_> {
    fn encode_method_header(&self, method: u32, count: usize) -> Result<u32> {
        methods::encode_method(method, count)
    }
}

// TODO: Factor out a common DISP 4.x HAL when adding support for more
// DISP 4.x GPUs.
impl<'a, 'gpu> DisplayHal<'a, 'gpu> for Ad102<'a, 'gpu> {
    fn emit_core_notifier_methods(
        &self,
        channel: &mut Channel<'a, 'gpu, Core>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result {
        disp4::notifier::emit_core_notifier_methods(self, channel, descriptor)
    }

    fn emit_window_notifier_methods(
        &self,
        channel: &mut Channel<'a, 'gpu, WindowDma>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result {
        disp4::notifier::emit_window_notifier_methods(self, channel, descriptor)
    }

    fn emit_window_update(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        interlock_immediate: bool,
    ) -> Result {
        // C67E needs no NULL-image transition workaround; C57E requires additional methods.
        let mut data = common_methods::window::UPDATE_RELEASE_ELV;
        if interlock_immediate {
            data |= common_methods::window::UPDATE_INTERLOCK_WITH_WIN_IMM;
        }
        window.emit(self, common_methods::window::UPDATE, &[data])
    }

    fn emit_window_immediate_update(
        &self,
        immediate: &mut Channel<'a, 'gpu, WindowImmediate>,
    ) -> Result {
        // C57B/C67B define RELEASE_ELV; C87B and DISP 5.x immediate classes omit it.
        immediate.emit(
            self,
            c67b::UPDATE,
            &[c67b::UPDATE_RELEASE_ELV | c67b::UPDATE_INTERLOCK_WITH_WINDOW],
        )
    }

    fn init_core_channel(&self, channel: &mut Channel<'a, 'gpu, Core>, head_mask: u32) -> Result {
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
        channel: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
        head_mask: u32,
    ) -> Result {
        self.emit_window_notifier_methods(channel, None)?;
        let caps = capability::read_precomp_caps(bar, channel.kind.index.into_safe_cast())?;

        // NVKMS: prefer postcomp scaling when any usable head provides it.
        if caps.sclr_present() && !scaler::has_postcomp_scaler(bar, head_mask)? {
            disp4::scaler::emit_precomp_scaler_coeffs_methods(self, channel)?;
        }

        if caps.csc0lut_present() && caps.csc1lut_present() {
            methods::csc::emit_csc_lut_methods(self, channel)?;
        }
        channel.advance()
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

    fn emit_disable_heads_and_detach_sors(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head_mask: u32,
    ) -> Result {
        if head_mask == 0 || head_mask >> core_methods::HEAD_COUNT != 0 {
            return Err(EINVAL);
        }
        for head in 0..core_methods::HEAD_COUNT {
            if head_mask & (1 << head) != 0 {
                let caps = capability::read_postcomp_caps(bar, head.into_safe_cast())?;
                self.emit_disable_head_methods(core, head, caps)?;
            }
        }
        modeset::emit_detach_heads_from_sors_methods(self, core, head_mask, SOR_COUNT)
    }

    fn emit_disable_head(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head: u32,
    ) -> Result {
        if head >= core_methods::HEAD_COUNT {
            return Err(EINVAL);
        }
        let caps = capability::read_postcomp_caps(bar, head.into_safe_cast())?;
        self.emit_disable_head_methods(core, head, caps)?;
        disp4::head::emit_head_display_id_methods(self, core, head, 0)
    }

    fn emit_sor_control(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        sor: u32,
        rm_protocol: u32,
        head_mask: u32,
    ) -> Result {
        if sor >= SOR_COUNT {
            return Err(EINVAL);
        }
        sor::emit_sor_control_methods(self, core, sor, rm_protocol, head_mask)
    }

    fn validate_mode(&self, mode: &DisplayMode) -> Result {
        modeset::validate_mode(mode)
    }

    /// Emits methods to configure head timing, viewport and output color.
    fn emit_head_configuration(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head: u32,
        mode: &DisplayMode,
        limited_range: bool,
    ) -> Result {
        modeset::validate_mode(mode)?;
        if head >= core_methods::HEAD_COUNT {
            return Err(EINVAL);
        }
        let caps = capability::read_postcomp_caps(bar, head.into_safe_cast())?;
        if limited_range && !caps.ocsc1_present() {
            return Err(EOPNOTSUPP);
        }

        self.emit_disable_head_methods(core, head, caps)?;
        disp4::head::emit_head_configuration_methods(self, core, head, mode, limited_range, caps)?;
        let offset = head * core_methods::HEAD_STRIDE;
        // C87D omits LOCK_OFFSET; C57D has no RASTER_HBLANK_DELAY method.
        core.emit(self, c77d::HEAD_SET_LOCK_OFFSET + offset, &[0])?;
        core.emit(self, c77d::HEAD_SET_RASTER_HBLANK_DELAY + offset, &[0])
    }

    fn emit_head_display_id(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        head: u32,
        display_id: u32,
    ) -> Result {
        if head >= core_methods::HEAD_COUNT || (display_id != 0 && !display_id.is_power_of_two()) {
            return Err(EINVAL);
        }
        disp4::head::emit_head_display_id_methods(self, core, head, display_id)
    }

    /// Emits methods to configure a pitch-linear framebuffer with composition bypass.
    fn emit_window_framebuffer_configuration(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
        mode: &DisplayMode,
        format: DisplayPixelFormat,
        pitch: u32,
        descriptor: SurfaceDescriptor,
    ) -> Result {
        const PITCH_ALIGNMENT: u32 = 256;

        mode.validate_framebuffer(format, pitch)?;
        modeset::validate_mode(mode)?;
        let encoded_pitch = pitch >> window_methods::SET_PLANAR_STORAGE_PITCH_BYTE_SHIFT;
        if !pitch.is_multiple_of(PITCH_ALIGNMENT)
            || encoded_pitch > window_methods::SET_PLANAR_STORAGE_PITCH_MASK
            || descriptor.context_dma_descriptor_id.handle == 0
        {
            return Err(EINVAL);
        }
        let caps = capability::read_precomp_caps(bar, window.kind.index.into_safe_cast())?;
        window::emit_window_geometry_methods(self, window, mode, pitch, caps)?;
        window::emit_window_color_methods(self, window, caps)?;
        let format = match format {
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

    /// Queues methods to disable framebuffer/LUT and semaphore accesses.
    fn emit_disable_window(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
    ) -> Result {
        let caps = capability::read_precomp_caps(bar, window.kind.index.into_safe_cast())?;
        self.emit_disable_window_methods(window, caps)
    }
}
