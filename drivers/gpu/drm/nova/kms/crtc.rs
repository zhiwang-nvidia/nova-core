// SPDX-License-Identifier: GPL-2.0

//! CRTC state and completion for the display channels.

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
    num::casts::IntoSafeCast,
    prelude::*,
    sizes::SZ_4K,
    sync::{
        new_mutex,
        Mutex, //
    }, //
};

use crate::{
    display::{
        Display,
        DisplayCommit,
        DisplayConfiguration,
        DisplayDisableCommit,
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

#[derive(Default)]
pub(super) struct NovaCrtcState {
    configuration: Option<DisplayConfiguration>,
    prepared: Option<Pin<KBox<Mutex<Option<PreparedUpdate>>>>>,
}

impl Clone for NovaCrtcState {
    fn clone(&self) -> Self {
        Self {
            configuration: self.configuration,
            // Bindings belong to one submission and must not be duplicated.
            prepared: None,
        }
    }
}

impl crtc::DriverCrtcState for NovaCrtcState {
    type Crtc = NovaCrtc;
}

enum PreparedUpdate {
    Framebuffer(KBox<DisplayCommit>),
    Disable(DisplayDisableCommit),
}

impl PreparedUpdate {
    fn commit(self, display: &mut Display<'_>) -> Result {
        match self {
            Self::Framebuffer(commit) => display.commit(KBox::into_inner(commit)),
            Self::Disable(commit) => display.commit_disable_output(commit),
        }
    }

    fn abort(self, display: &mut Display<'_>) -> Result {
        match self {
            Self::Framebuffer(commit) => display.abort_commit(KBox::into_inner(commit)),
            Self::Disable(commit) => display.abort_disable_output(commit),
        }
    }
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

    fn atomic_flush(
        crtc: &Crtc,
        _old_state: &CrtcState,
        mut new_state: CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<NovaDriver>,
    ) {
        let result = if new_state.active() {
            apply(crtc, &new_state)
        } else {
            Ok(())
        };
        retire_event(crtc, &mut new_state, result);
    }

    fn atomic_disable(
        crtc: &Crtc,
        _old_state: &CrtcState,
        mut new_state: CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<NovaDriver>,
    ) {
        if !new_state.active() {
            let result = apply(crtc, &new_state);
            retire_event(crtc, &mut new_state, result);
        }
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

/// Binds the existing GEM backing before the DRM state is swapped.
pub(super) fn prepare(state: &AtomicStateMutator<NovaDriver>) -> Result {
    let Some(registration) = state.drm_dev().registration_guard() else {
        // DRM shutdown follows unplug. Let its disabled state replace the
        // installed state and release framebuffer references. Display retains
        // the hardware backing until registration-data shutdown stops it.
        for candidate in state.crtcs() {
            let crtc = Crtc::try_from_opaque(candidate).map_err(|_| EINVAL)?;
            if state.get_new_crtc_state(crtc).ok_or(EBUSY)?.active() {
                return Err(ENODEV);
            }
        }
        return Ok(());
    };
    for candidate in state.crtcs() {
        let crtc = Crtc::try_from_opaque(candidate).map_err(|_| EINVAL)?;
        let mut new_state = state.get_new_crtc_state(crtc).ok_or(EBUSY)?;
        let new_data = new_state.driver_data_mut()?;
        if new_data
            .prepared
            .as_ref()
            .is_some_and(|slot| slot.lock().is_some())
        {
            return Err(EBUSY);
        }
        let prepared_slot = KBox::pin_init(new_mutex!(None), GFP_KERNEL)?;
        let region = if let Some(configuration) = new_data.configuration.as_ref() {
            let plane = state
                .get_new_plane_state(crtc.primary_plane())
                .ok_or(EINVAL)?;
            let framebuffer = plane.framebuffer().ok_or(EINVAL)?;
            let (layout, _) = check_framebuffer(framebuffer, &configuration.mode)?;
            let offset = u64::from(layout.offset);
            let object = framebuffer.gem_object(0)?;
            let end = object.size().into_safe_cast();
            Some(object.region(&registration, offset..end)?)
        } else {
            None
        };
        let old_state = state.get_old_crtc_state(crtc).ok_or(EINVAL)?;
        let prepared = registration.registration_data_with(|reg| {
            let slot = &mut *reg.display.lock();
            let display = slot.as_mut().ok_or(ENODEV)?;
            let result = match (new_data.configuration.as_ref(), region) {
                (Some(configuration), Some(region)) => {
                    let storage = KBox::<DisplayCommit>::new_uninit(GFP_KERNEL)?;
                    display
                        .prepare_commit(configuration, region)
                        .map(|commit| Some(PreparedUpdate::Framebuffer(storage.write(commit))))
                }
                (None, None) => match old_state.configuration.as_ref() {
                    Some(configuration) => display
                        .prepare_disable_output(configuration.display_id)
                        .map(|commit| commit.map(PreparedUpdate::Disable)),
                    None => Ok(None),
                },
                _ => Err(EINVAL),
            };
            if result.is_err() && display.needs_shutdown() {
                slot.take();
            }
            result
        })?;
        *prepared_slot.lock() = prepared;
        new_data.prepared = Some(prepared_slot);
    }
    Ok(())
}

/// Releases prepared bindings when submission fails before the state swap.
pub(super) fn abort(state: &AtomicStateMutator<NovaDriver>) {
    for candidate in state.crtcs() {
        let result = (|| {
            let crtc = Crtc::try_from_opaque(candidate).map_err(|_| EINVAL)?;
            let new_state = state.get_new_crtc_state(crtc).ok_or(EBUSY)?;
            let Some(prepared) = new_state
                .prepared
                .as_ref()
                .and_then(|slot| slot.lock().take())
            else {
                return Ok(());
            };
            let registration = state.drm_dev().registration_guard().ok_or(ENODEV)?;
            registration.registration_data_with(|reg| {
                let slot = &mut *reg.display.lock();
                let display = slot.as_mut().ok_or(ENODEV)?;
                let result = prepared.abort(display);
                if result.is_err() {
                    slot.take();
                }
                result
            })
        })();
        if let Err(error) = result {
            dev_err!(
                state.drm_dev().as_ref(),
                "Failed to abort display preparation: {:?}\n",
                error
            );
        }
    }
}

fn apply(crtc: &Crtc, state: &CrtcStateMutator<'_, CrtcState>) -> Result {
    let Some(prepared) = state.prepared.as_ref().and_then(|slot| slot.lock().take()) else {
        return if state.active() { Err(EIO) } else { Ok(()) };
    };
    let registration = crtc.drm_dev().registration_guard().ok_or(ENODEV)?;
    registration.registration_data_with(|reg| {
        let slot = &mut *reg.display.lock();
        let display = slot.as_mut().ok_or(ENODEV)?;
        let result = prepared.commit(display);
        if result.is_err() {
            slot.take();
        }
        result
    })
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
