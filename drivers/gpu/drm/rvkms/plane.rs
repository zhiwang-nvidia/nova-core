// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM plane implementation
use super::RvkmsDriver;
use core::marker::PhantomPinned;
use kernel::{
    drm::{
        device::Device,
        kms::{
            atomic::*,
            plane::{self, DriverPlaneOps, DriverPlaneState, PlaneStateMutator, RawPlaneState},
        },
    },
    prelude::*,
};

#[pin_data]
pub(crate) struct RvkmsPlane {
    #[pin]
    _p: PhantomPinned,
}

pub(crate) type Plane = plane::Plane<RvkmsPlane>;
pub(crate) type UnregisteredPlane = plane::UnregisteredPlane<RvkmsPlane>;
pub(crate) type PlaneState = plane::PlaneState<RvkmsPlaneState>;

#[vtable]
impl plane::DriverPlane for RvkmsPlane {
    #[unique]
    const OPS: &'static DriverPlaneOps;

    type State = RvkmsPlaneState;
    type Driver = RvkmsDriver;
    type Args = ();

    fn new(_device: &Device<Self::Driver>, _args: Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self { _p: PhantomPinned })
    }

    fn atomic_check(
        _plane: &Plane,
        mut new_state: PlaneStateMutator<'_, PlaneState>,
        _old_state: &PlaneState,
        state: &AtomicStateComposer<Self::Driver>,
    ) -> Result {
        if new_state.framebuffer().is_none() {
            return Ok(());
        }

        if let Some(crtc) = new_state.crtc() {
            let crtc_state = state.add_crtc_state(crtc)?;
            new_state.atomic_helper_check(&crtc_state, true, true)
        } else {
            // TODO: We should be printing a warning here if we have no CRTC but do have an fb
            Ok(())
        }
    }

    fn atomic_update(
        _plane: &Plane,
        _new_state: PlaneStateMutator<'_, PlaneState>,
        _old_state: &PlaneState,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        // TODO, no-op for now
    }
}

#[derive(Clone, Default)]
pub(crate) struct RvkmsPlaneState;

impl DriverPlaneState for RvkmsPlaneState {
    type Plane = RvkmsPlane;
}
