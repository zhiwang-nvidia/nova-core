// SPDX-License-Identifier: GPL-2.0

use kernel::prelude::*;

use nova_core::api::{
    DisplayStaticInfo,
    NovaCoreApiHandle,
    VramRegion, //
};

mod ad102;
mod disp4;

pub(super) trait DisplayHal: Send + Sync {
    fn instance_memory(&self) -> &VramRegion;
}

pub(super) fn display_hal<'bound>(
    api: &NovaCoreApiHandle<'bound>,
    ip_version: u32,
    info: &DisplayStaticInfo,
) -> Result<KBox<dyn DisplayHal + 'bound>> {
    const IP_VERSION_MASK: u32 = 0xffff_0000;

    let hal: KBox<dyn DisplayHal + 'bound> = match ip_version & IP_VERSION_MASK {
        ad102::IP_VERSION => ad102::Ad102::new(api, info)?,
        _ => return Err(EOPNOTSUPP),
    };

    Ok(hal)
}
