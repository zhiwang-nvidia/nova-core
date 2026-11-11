// SPDX-License-Identifier: GPL-2.0

use kernel::prelude::*;

use nova_core::api::{
    DisplayStaticInfo,
    NovaCoreApiHandle,
    VramRegion, //
};

use super::{
    disp4::instance_memory::InstanceMemory,
    DisplayHal, //
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

impl DisplayHal for Ad102<'_> {
    fn instance_memory(&self) -> &VramRegion {
        self.instance_memory.region()
    }
}
