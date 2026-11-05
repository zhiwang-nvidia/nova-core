// SPDX-License-Identifier: GPL-2.0

//! One HDMI output with completion reported by the display channel notifier.

use core::marker::PhantomData;

use kernel::{
    bindings,
    drm::{
        fourcc,
        gem::BaseObject,
        kms::{
            atomic::{
                AtomicStateComposer,
                AtomicStateMutator, //
            },
            connector::RawConnectorState,
            crtc::{
                self,
                CrtcStateMutator,
                DriverCrtcOps,
                RawCrtc,
                RawCrtcState, //
            },
            framebuffer::{
                Framebuffer,
                PlaneLayout, //
            },
            plane::RawPlaneState,
            ModeObject, //
        },
    },
    io::Io,
    num::casts::IntoSafeCast,
    prelude::*,
};

use nova_core::api::{
    DisplayConfiguration,
    DisplayMode,
    DisplayPixelFormat, //
};

use super::connector;
use crate::driver::{
    NovaDevice,
    NovaDriver, //
};

pub(super) type UnregisteredCrtc = crtc::UnregisteredCrtc<NovaCrtc>;
type Crtc = crtc::Crtc<NovaCrtc>;
type CrtcState = crtc::CrtcState<NovaCrtcState>;

pub(super) struct NovaCrtc;

#[derive(Clone, Default)]
pub(super) struct NovaCrtcState {
    configuration: Option<DisplayConfiguration>,
}

impl crtc::DriverCrtcState for NovaCrtcState {
    type Crtc = NovaCrtc;
}

#[vtable]
impl crtc::DriverCrtc for NovaCrtc {
    #[unique]
    const OPS: &'static DriverCrtcOps;

    type Args = ();
    type State = NovaCrtcState;
    type Driver = NovaDriver;
    type VblankImpl = PhantomData<Self>;

    fn new(_dev: &NovaDevice, _args: &()) -> impl PinInit<Self, Error> {
        Ok(Self)
    }

    fn atomic_check(
        crtc: &Crtc,
        _old_state: &CrtcState,
        mut new_state: CrtcStateMutator<'_, CrtcState>,
        state: &AtomicStateComposer<NovaDriver>,
    ) -> Result {
        new_state.configuration = None;
        if new_state.active() && !state.allow_modeset() {
            return Err(EINVAL);
        }
        state.add_affected_planes(crtc)?;
        state.add_affected_connectors(crtc)?;
        if !new_state.active() {
            return Ok(());
        }

        let raw_mode = new_state.adjusted_mode();
        let mode = connector::mode_from_drm(&raw_mode)?;
        let plane = state
            .get_new_plane_state(crtc.primary_plane())
            .ok_or(EINVAL)?;
        if plane
            .crtc()
            .is_none_or(|assigned| assigned.index() != crtc.index())
        {
            return Err(EINVAL);
        }
        let source = plane.source();
        let destination = plane.destination();
        if source.x != 0
            || source.y != 0
            || source.width != mode.hdisplay.checked_shl(16).ok_or(EOVERFLOW)?
            || source.height != mode.vdisplay.checked_shl(16).ok_or(EOVERFLOW)?
            || destination.x != 0
            || destination.y != 0
            || destination.width != mode.hdisplay
            || destination.height != mode.vdisplay
            || plane.rotation() != bindings::DRM_MODE_ROTATE_0
        {
            return Err(EINVAL);
        }
        let framebuffer = plane.framebuffer().ok_or(EINVAL)?;
        let (layout, format) = check_framebuffer(framebuffer, &mode)?;

        let mut selected = None;
        for candidate in state.connectors() {
            let connector = connector::Connector::try_from_opaque(candidate).map_err(|_| EINVAL)?;
            let connector_state = state.get_new_connector_state(connector).ok_or(EBUSY)?;
            if connector_state
                .crtc()
                .is_some_and(|assigned| assigned.index() == crtc.index())
            {
                if selected.replace(connector).is_some() {
                    return Err(EINVAL);
                }
            }
        }
        let selected = selected.ok_or(EINVAL)?;
        let display = state.connector_display(selected)?;
        let configuration = connector::configuration(&display, &raw_mode, format, layout.pitch)?;
        let registration = crtc.drm_dev().registration_guard().ok_or(ENODEV)?;
        registration.registration_data_with(|reg| {
            reg.api
                .with(|core| core.display_check_configuration(&configuration))
        })?;
        new_state.configuration = Some(configuration);
        Ok(())
    }

    fn atomic_flush(
        crtc: &Crtc,
        _old_state: &CrtcState,
        mut new_state: CrtcStateMutator<'_, CrtcState>,
        state: &AtomicStateMutator<NovaDriver>,
    ) {
        if !new_state.active() {
            retire_event(crtc, &mut new_state, Ok(()));
            return;
        }
        let result = commit(crtc, &new_state, state);
        retire_event(crtc, &mut new_state, result);
    }

    fn atomic_disable(
        crtc: &Crtc,
        old_state: &CrtcState,
        mut new_state: CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<NovaDriver>,
    ) {
        // A replacement mode is applied by display_modeset in atomic_flush.
        if new_state.active() {
            return;
        }
        let Some(configuration) = old_state.configuration.as_ref() else {
            retire_event(crtc, &mut new_state, Ok(()));
            return;
        };
        let result = crtc
            .drm_dev()
            .registration_guard()
            .ok_or(ENODEV)
            .and_then(|registration| {
                registration.registration_data_with(|reg| {
                    reg.api
                        .with(|core| core.display_disable_output(configuration.display_id))
                })
            });
        retire_event(crtc, &mut new_state, result);
    }
}

fn check_framebuffer(
    framebuffer: &Framebuffer<NovaDriver>,
    mode: &DisplayMode,
) -> Result<(PlaneLayout, DisplayPixelFormat)> {
    let layout = framebuffer.plane_layout(0)?;
    let format = match layout.format {
        fourcc::XRGB888 => DisplayPixelFormat::Xrgb8888,
        _ => return Err(EOPNOTSUPP),
    };
    let minimum_pitch = mode
        .hdisplay
        .checked_mul(format.bytes_per_pixel())
        .ok_or(EOVERFLOW)?;
    if layout.modifier != fourcc::FORMAT_MOD_LINEAR
        || layout.num_planes != 1
        || layout.width != mode.hdisplay
        || layout.height != mode.vdisplay
        || layout.pitch < minimum_pitch
        || !layout.pitch.is_multiple_of(256)
        || !layout.offset.is_multiple_of(4)
    {
        return Err(EINVAL);
    }
    let object = framebuffer.gem_object(0)?;
    let offset: usize = layout.offset.into_safe_cast();
    let pitch: usize = layout.pitch.into_safe_cast();
    let height: usize = layout.height.into_safe_cast();
    let end = offset
        .checked_add(pitch.checked_mul(height).ok_or(EOVERFLOW)?)
        .ok_or(EOVERFLOW)?;
    if end > object.size() {
        return Err(EINVAL);
    }
    Ok((layout, format))
}

fn commit(
    crtc: &Crtc,
    crtc_state: &CrtcStateMutator<'_, CrtcState>,
    state: &AtomicStateMutator<NovaDriver>,
) -> Result {
    let configuration = crtc_state.configuration.as_ref().ok_or(EINVAL)?;
    let plane = state
        .get_new_plane_state(crtc.primary_plane())
        .ok_or(EINVAL)?;
    let framebuffer = plane.framebuffer().ok_or(EINVAL)?;
    let (layout, _) = check_framebuffer(framebuffer, &configuration.mode)?;
    let object = framebuffer.gem_object(0)?;
    let mapping = object.vmap::<0>()?;
    let pitch: usize = layout.pitch.into_safe_cast();
    let height: usize = layout.height.into_safe_cast();
    let offset: usize = layout.offset.into_safe_cast();
    let length = pitch.checked_mul(height).ok_or(EOVERFLOW)?;
    let mut pixels = VVec::with_capacity(length, GFP_KERNEL)?;
    pixels.resize(length, 0, GFP_KERNEL)?;
    for (index, pixel) in pixels.chunks_exact_mut(size_of::<u32>()).enumerate() {
        pixel.copy_from_slice(
            &mapping
                .try_read32(offset + index * size_of::<u32>())?
                .to_ne_bytes(),
        );
    }
    let registration = crtc.drm_dev().registration_guard().ok_or(ENODEV)?;
    registration.registration_data_with(|reg| {
        reg.api
            .with(|core| core.display_modeset(configuration, &pixels))
    })
}

fn retire_event(crtc: &Crtc, state: &mut CrtcStateMutator<'_, CrtcState>, result: Result) {
    let result = match result {
        Ok(()) => state.send_pending_event(),
        Err(error) => {
            dev_err!(
                crtc.drm_dev().as_ref(),
                "Display commit failed: {:?}\n",
                error
            );
            state.fail_pending_event(error)
        }
    };
    if let Err(error) = result {
        dev_err!(
            crtc.drm_dev().as_ref(),
            "Failed to retire display event: {:?}\n",
            error
        );
    }
}
