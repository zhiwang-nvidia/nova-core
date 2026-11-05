// SPDX-License-Identifier: GPL-2.0

//! DRM encoder objects for HDMI TMDS outputs.

use kernel::{
    drm::kms::encoder,
    prelude::*, //
};

use crate::driver::{
    NovaDevice,
    NovaDriver, //
};

pub(super) struct NovaEncoder;

pub(super) type UnregisteredEncoder = encoder::UnregisteredEncoder<NovaEncoder>;

#[vtable]
impl encoder::DriverEncoder for NovaEncoder {
    #[unique]
    const OPS: &'static encoder::DriverEncoderOps;

    type Driver = NovaDriver;
    type Args = ();

    fn new(_dev: &NovaDevice, _args: ()) -> impl PinInit<Self, Error> {
        Ok(Self)
    }
}
