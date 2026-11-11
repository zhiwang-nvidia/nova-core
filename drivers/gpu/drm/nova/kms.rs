// SPDX-License-Identifier: GPL-2.0

//! HDMI modesetting through the display channels.

#[expect(dead_code)]
mod connector;
#[expect(dead_code)]
mod crtc;
#[expect(dead_code)]
mod encoder;
#[expect(dead_code)]
mod plane;

use kernel::{
    device,
    drm::{
        fourcc,
        kms::{
            atomic::AtomicStateComposer,
            KmsDriver,
            ModeConfigInfo,
            UnregisteredKmsDevice, //
        }, //
    },
    prelude::*, //
};

use crate::display::FramebufferLayout;

use crate::driver::NovaDriver;

#[vtable]
impl KmsDriver for NovaDriver {
    fn mode_config_info(_dev: &device::Device, _data: &Self::Data) -> Result<ModeConfigInfo> {
        Ok(ModeConfigInfo {
            min_resolution: (1, 1),
            max_resolution: (
                FramebufferLayout::MAX_DIMENSION,
                FramebufferLayout::MAX_DIMENSION,
            ),
            max_cursor: (0, 0),
            preferred_depth: 24,
            preferred_fourcc: Some(fourcc::XRGB888),
        })
    }

    fn create_objects(_dev: &UnregisteredKmsDevice<'_, Self>) -> Result {
        Ok(())
    }

    fn atomic_check(state: &mut AtomicStateComposer<Self>) -> Result {
        crtc::check(state)
    }
}
