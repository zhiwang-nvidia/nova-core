// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    sync::Mutex, //
};

use crate::{
    display::{
        channel::{
            Channel,
            Core,
            WindowDma, //
        },
        hal::{
            disp4::{
                self,
                instance_memory::InstanceMemory,
                methods::{
                    self,
                    core as core_methods, //
                },
            },
            DisplayHal,
            DisplayMethodHal,
            SurfaceDescriptor, //
        },
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
};

pub(super) const IP_VERSION: u32 = 0x0404_0000;

const NVC770_DISPLAY: u32 = 0xc770;
const NVC77D_CORE_CHANNEL_DMA: u32 = 0xc77d;
const NVC67E_WINDOW_CHANNEL_DMA: u32 = 0xc67e;
const NVC67B_WINDOW_IMM_CHANNEL_DMA: u32 = 0xc67b;
const NVC67A_CURSOR_IMM_CHANNEL_PIO: u32 = 0xc67a;

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

    fn init_core_channel(&self, channel: &mut Channel<'a, 'gpu, Core>, head_mask: u32) -> Result {
        self.emit_core_notifier_methods(channel, None)?;
        if head_mask == 0 || head_mask >> core_methods::HEAD_COUNT != 0 {
            return Err(EINVAL);
        }
        disp4::head::emit_desktop_color_methods(self, channel, head_mask)?;
        channel.advance()
    }

    fn init_window_channel(
        &self,
        channel: &mut Channel<'a, 'gpu, WindowDma>,
        _bar: Bar0<'_>,
        _head_mask: u32,
    ) -> Result {
        self.emit_window_notifier_methods(channel, None)?;
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
}
