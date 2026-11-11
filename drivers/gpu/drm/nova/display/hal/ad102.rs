// SPDX-License-Identifier: GPL-2.0

use kernel::prelude::*;

use nova_core::api::{
    DisplayStaticInfo,
    NovaCoreApiHandle,
    VramRegion, //
};

use super::{
    disp4::{
        instance_memory::InstanceMemory,
        methods, //
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
}

// TODO: Factor out a common DISP 4.x HAL when adding support for more
// DISP 4.x GPUs.
impl<'bound> DisplayHal<'bound> for Ad102<'bound> {
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
}

impl DisplayMethodHal for Ad102<'_> {
    fn encode_method_header(&self, method: u32, count: usize) -> Result<u32> {
        methods::encode_method(method, count)
    }
}
