// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM Connector implementation
use super::{RvkmsDriver, DEFAULT_RES, MAX_RES};
use core::marker::PhantomPinned;
use kernel::{
    drm::{
        device::Device,
        kms::{
            connector::{self, ConnectorProbeGuard, DriverConnectorOps},
            ModeConfigGuard,
        },
    },
    prelude::*,
};

#[pin_data]
pub(crate) struct DriverConnector {
    #[pin]
    _p: PhantomPinned,
}

#[allow(unused)]
pub(crate) type Connector = connector::Connector<DriverConnector>;
pub(crate) type UnregisteredConnector = connector::UnregisteredConnector<DriverConnector>;

#[vtable]
impl connector::DriverConnector for DriverConnector {
    #[unique]
    const OPS: &'static DriverConnectorOps;

    type State = ConnectorState;
    type Driver = RvkmsDriver;
    type Args = ();

    fn new(_dev: &Device<Self::Driver>, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn get_modes(
        connector: ConnectorProbeGuard<'_, Self>,
        _guard: &ModeConfigGuard<'_, Self::Driver>,
    ) -> i32 {
        let count = connector.add_modes_noedid(MAX_RES);

        connector.set_preferred_mode(DEFAULT_RES);
        count
    }
}

#[derive(Clone, Default)]
pub(crate) struct ConnectorState;

impl connector::DriverConnectorState for ConnectorState {
    type Connector = DriverConnector;
}
