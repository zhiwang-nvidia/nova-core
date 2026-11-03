// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    sync::Mutex, //
};

use crate::{
    gsp::{
        cmdq::Cmdq,
        commands::DisplayHwStaticInfo, //
    },
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
};

mod ad102;
mod disp4;

pub(super) trait DisplayHal<'a, 'gpu>: Send {}

pub(super) fn display_hal<'a, 'gpu: 'a>(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'gpu>,
    mm: &'a Mutex<GpuMm<'gpu>>,
    bar_user: &'a BarUser<'gpu>,
    ip_version: u32,
    info: &DisplayHwStaticInfo,
) -> Result<KBox<dyn DisplayHal<'a, 'gpu> + 'a>> {
    const IP_VERSION_MASK: u32 = 0xffff_0000;

    let hal: KBox<dyn DisplayHal<'a, 'gpu> + 'a> = match ip_version & IP_VERSION_MASK {
        ad102::IP_VERSION => ad102::Ad102::new(dev, cmdq, mm, bar_user, info)?,
        _ => return Err(EOPNOTSUPP),
    };

    Ok(hal)
}
