// SPDX-License-Identifier: GPL-2.0

use kernel::prelude::*;

use nova_core::api::DisplayStaticInfo;

mod ad102;

pub(super) trait DisplayHal: Send + Sync {}

pub(super) fn display_hal(
    ip_version: u32,
    info: &DisplayStaticInfo,
) -> Result<KBox<dyn DisplayHal>> {
    const IP_VERSION_MASK: u32 = 0xffff_0000;

    let hal: KBox<dyn DisplayHal> = match ip_version & IP_VERSION_MASK {
        ad102::IP_VERSION => ad102::Ad102::new(info)?,
        _ => return Err(EOPNOTSUPP),
    };

    Ok(hal)
}
