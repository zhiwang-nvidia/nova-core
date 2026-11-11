// SPDX-License-Identifier: GPL-2.0

//! CRTC state and completion for the display channels.

use core::marker::PhantomData;

use kernel::{
    bindings,
    drm::{
        fourcc,
        gem::BaseObject,
        kms::{
            atomic::AtomicStateComposer,
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
            plane::RawPlaneState, //
        },
    },
    num::casts::IntoSafeCast,
    prelude::*,
    sizes::SZ_4K, //
};

use crate::{
    display::{
        DisplayConfiguration,
        DisplayMode,
        DisplayPixelFormat,
        FramebufferLayout, //
    },
    driver::{
        NovaDevice,
        NovaDriver, //
    }, //
};

use super::connector;

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
}

/// Checks the complete candidate after the DRM helpers have checked object state.
pub(super) fn check(state: &mut AtomicStateComposer<NovaDriver>) -> Result {
    for candidate in state.crtcs() {
        let crtc = Crtc::try_from_opaque(candidate).map_err(|_| EINVAL)?;
        state.add_affected_planes(crtc)?;
        state.add_affected_connectors(crtc)?;
    }
    state.check_helpers()?;

    let mut target = None;
    for candidate in state.crtcs() {
        let crtc = Crtc::try_from_opaque(candidate).map_err(|_| EINVAL)?;
        state.add_affected_connectors(crtc)?;
        let mut new_state = state.get_new_crtc_state(crtc).ok_or(EBUSY)?;
        new_state.driver_data_mut()?.configuration = None;
        if !new_state.active() {
            continue;
        }
        let configuration = configuration(crtc, &new_state, state)?;
        let old_state = state.get_old_crtc_state(crtc).ok_or(EINVAL)?;
        if !state.allow_modeset() && old_state.configuration != Some(configuration) {
            return Err(EINVAL);
        }
        // The current display implementation admits one active HDMI output.
        if target.replace(configuration).is_some() {
            return Err(EOPNOTSUPP);
        }
        new_state.driver_data_mut()?.configuration = Some(configuration);
    }
    if let Some(configuration) = target {
        let registration = state.drm_dev().registration_guard().ok_or(ENODEV)?;
        registration.registration_data_with(|reg| {
            reg.display
                .lock()
                .as_ref()
                .ok_or(ENODEV)?
                .check_configuration(&configuration)
                .map(|_| ())
        })?;
    }
    Ok(())
}

fn configuration(
    crtc: &Crtc,
    new_state: &CrtcStateMutator<'_, CrtcState>,
    state: &AtomicStateComposer<NovaDriver>,
) -> Result<DisplayConfiguration> {
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
            && selected.replace(connector).is_some()
        {
            return Err(EINVAL);
        }
    }
    let display = state.connector_display(selected.ok_or(EINVAL)?)?;
    connector::configuration(
        &display,
        &raw_mode,
        FramebufferLayout {
            width: layout.width,
            height: layout.height,
            format,
            pitch: layout.pitch,
        },
    )
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
    let minimum_pitch = layout
        .width
        .checked_mul(format.bytes_per_pixel())
        .ok_or(EOVERFLOW)?;
    // ContextDMA surfaces start on a page boundary; SET_OFFSET stays zero.
    let offset: usize = layout.offset.into_safe_cast();
    // TODO: Support block-linear modifiers. Validate the block-aligned size and
    // offset against GEM backing, and configure Window storage and GMC bandwidth
    // checks for the selected layout.
    if !offset.is_multiple_of(SZ_4K)
        || layout.modifier != fourcc::FORMAT_MOD_LINEAR
        || layout.num_planes != 1
        || layout.width < mode.hdisplay
        || layout.height < mode.vdisplay
        || layout.pitch < minimum_pitch
        || !layout.pitch.is_multiple_of(256)
    {
        return Err(EINVAL);
    }
    let object = framebuffer.gem_object(0)?;
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
