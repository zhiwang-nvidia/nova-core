// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM Encoder implementation
use crate::RvkmsDriver;
use core::marker::PhantomPinned;
use kernel::{
    drm::{device::Device, kms::encoder},
    prelude::*,
};

#[pin_data]
pub(crate) struct DriverEncoder {
    #[pin]
    _p: PhantomPinned,
}

#[allow(unused)]
pub(crate) type Encoder = encoder::Encoder<DriverEncoder>;
pub(crate) type UnregisteredEncoder = encoder::UnregisteredEncoder<DriverEncoder>;

#[vtable]
impl encoder::DriverEncoder for DriverEncoder {
    #[unique]
    const OPS: &'static encoder::DriverEncoderOps;

    type Driver = RvkmsDriver;
    type Args = ();

    fn new(_device: &Device<Self::Driver>, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }
}
