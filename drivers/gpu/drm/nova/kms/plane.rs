// SPDX-License-Identifier: GPL-2.0

//! Primary plane for a fullscreen XRGB8888 surface.

use kernel::{
    drm::kms::{
        atomic::{
            AtomicStateComposer,
            AtomicStateMutator, //
        },
        plane::{
            self,
            DriverPlaneOps,
            PlaneStateMutator,
            RawPlaneState, //
        },
    },
    prelude::*, //
};

use crate::driver::{
    NovaDevice,
    NovaDriver, //
};

pub(super) type UnregisteredPlane = plane::UnregisteredPlane<NovaPlane>;
type Plane = plane::Plane<NovaPlane>;
type PlaneState = plane::PlaneState<NovaPlaneState>;

pub(super) struct NovaPlane;

#[derive(Clone, Default)]
pub(super) struct NovaPlaneState;

impl plane::DriverPlaneState for NovaPlaneState {
    type Plane = NovaPlane;
}

#[vtable]
impl plane::DriverPlane for NovaPlane {
    #[unique]
    const OPS: &'static DriverPlaneOps;

    type State = NovaPlaneState;
    type Driver = NovaDriver;
    type Args = ();

    fn new(_dev: &NovaDevice, _args: ()) -> impl PinInit<Self, Error> {
        Ok(Self)
    }

    fn atomic_check(
        _plane: &Plane,
        mut new_state: PlaneStateMutator<'_, PlaneState>,
        _old_state: &PlaneState,
        state: &AtomicStateComposer<NovaDriver>,
    ) -> Result {
        if let Some(crtc) = new_state.crtc() {
            let crtc_state = state.add_crtc_state(crtc)?;
            new_state.atomic_helper_check(&crtc_state, false, false)?;
        } else if new_state.framebuffer().is_some() {
            return Err(EINVAL);
        }
        Ok(())
    }

    fn atomic_update(
        _plane: &Plane,
        _new_state: PlaneStateMutator<'_, PlaneState>,
        _old_state: &PlaneState,
        _state: &AtomicStateMutator<NovaDriver>,
    ) {
        // CRTC atomic_flush submits the framebuffer with the complete display state.
    }
}
