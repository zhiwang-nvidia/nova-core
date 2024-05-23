// SPDX-License-Identifier: GPL-2.0
//! RVKMS's display outputs.
use crate::{
    connector::UnregisteredConnector, crtc::UnregisteredCrtc, encoder::UnregisteredEncoder,
    plane::UnregisteredPlane, RvkmsDriver,
};
use kernel::{
    drm::{
        fourcc::*,
        kms::{connector, encoder, plane, UnregisteredKmsDevice},
    },
    prelude::*,
};

pub(crate) fn create_output(dev: &UnregisteredKmsDevice<'_, RvkmsDriver>, index: u8) -> Result {
    let possible_crtcs = 1 << index;

    let primary = UnregisteredPlane::new(
        dev,
        possible_crtcs,
        &[XRGB888],
        None,
        plane::Type::Primary,
        None,
        (),
    )?;

    let _crtc = UnregisteredCrtc::new(dev, primary, Option::<&UnregisteredPlane>::None, None, ())?;

    let connector = UnregisteredConnector::new(dev, connector::Type::Virtual, ())?;

    let encoder =
        UnregisteredEncoder::new(dev, encoder::Type::Virtual, possible_crtcs, 0, None, ())?;

    connector.attach_encoder(encoder)
}
