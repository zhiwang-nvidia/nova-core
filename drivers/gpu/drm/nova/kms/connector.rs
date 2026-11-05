// SPDX-License-Identifier: GPL-2.0

//! HDMI connectors, sink modes and AVI configuration.

use kernel::{
    bindings,
    drm::{
        edid::Edid,
        hdmi::RgbConfig,
        kms::{
            connector::{
                self,
                ConnectionStatus,
                ConnectorDisplayGuard,
                ConnectorProbeGuard,
                DriverConnectorOps,
                ModeStatus, //
            },
            ModeConfigGuard,
            ModeObject, //
        },
    },
    num::casts::{
        self,
        IntoSafeCast, //
    },
    prelude::*,
    sync::{
        new_mutex,
        Mutex, //
    },
};

use nova_core::api::{
    DisplayConfiguration,
    DisplayMode,
    DisplayOutput,
    DisplayPixelFormat, //
};

use crate::driver::{
    NovaDevice,
    NovaDriver, //
};

pub(super) type Connector = connector::Connector<NovaConnector>;
pub(super) type UnregisteredConnector = connector::UnregisteredConnector<NovaConnector>;

#[pin_data]
pub(super) struct NovaConnector {
    pub(super) output: DisplayOutput,
    #[pin]
    modes: Mutex<KVec<DisplayMode>>,
}

#[derive(Clone, Default)]
pub(super) struct NovaConnectorState;

impl connector::DriverConnectorState for NovaConnectorState {
    type Connector = NovaConnector;
}

#[vtable]
impl connector::DriverConnector for NovaConnector {
    #[unique]
    const OPS: &'static DriverConnectorOps;

    type Driver = NovaDriver;
    type State = NovaConnectorState;
    type Args = DisplayOutput;

    fn new(_dev: &NovaDevice, output: DisplayOutput) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            output,
            modes <- new_mutex!(KVec::new()),
        })
    }

    fn detect(connector: &connector::Connector<Self>, _force: bool) -> Result<ConnectionStatus> {
        let registration = connector.drm_dev().registration_guard().ok_or(ENODEV)?;
        let outputs =
            registration.registration_data_with(|reg| reg.api.with(|api| api.display_outputs()))?;
        let connected = outputs
            .iter()
            .any(|output| output.display_id == connector.output.display_id && output.connected);
        if connected {
            Ok(ConnectionStatus::Connected)
        } else {
            connector.modes.lock().clear();
            Ok(ConnectionStatus::Disconnected)
        }
    }

    fn get_modes(
        connector: ConnectorProbeGuard<'_, Self>,
        _guard: &ModeConfigGuard<'_, NovaDriver>,
    ) -> i32 {
        connector.modes.lock().clear();
        match read_modes(&connector) {
            Ok(count) => count,
            Err(error) => {
                let _ = connector.update_edid(None);
                pr_warn!(
                    "nova-drm: display {:#x} EDID probe failed: {:?}\n",
                    connector.output.display_id,
                    error,
                );
                0
            }
        }
    }

    fn mode_valid(
        connector: ConnectorDisplayGuard<'_, Self>,
        mode: &bindings::drm_display_mode,
    ) -> ModeStatus {
        match check_mode(&connector, mode) {
            Ok(_) => ModeStatus::Ok,
            Err(EINVAL | EOPNOTSUPP | EOVERFLOW) => ModeStatus::Bad,
            Err(_) => ModeStatus::Error,
        }
    }
}

fn read_modes(connector: &ConnectorProbeGuard<'_, NovaConnector>) -> Result<i32> {
    const BLOCK_SIZE: usize = casts::u32_as_usize(bindings::EDID_LENGTH);

    let registration = connector.drm_dev().registration_guard().ok_or(ENODEV)?;
    let mut snapshot = None;
    let edid = Edid::read(connector, |block, output| {
        if snapshot.is_none() {
            let bytes = registration.registration_data_with(|reg| {
                reg.api
                    .with(|api| api.display_read_edid(connector.output.display_id))
            })?;
            if bytes.is_empty() || !bytes.len().is_multiple_of(BLOCK_SIZE) {
                return Err(EPROTO);
            }
            snapshot = Some(bytes);
        }
        let block: usize = block.into_safe_cast();
        let start = block.checked_mul(BLOCK_SIZE).ok_or(EOVERFLOW)?;
        let end = start.checked_add(output.len()).ok_or(EOVERFLOW)?;
        let bytes = snapshot.as_ref().ok_or(EPROTO)?;
        output.copy_from_slice(bytes.get(start..end).ok_or(EPROTO)?);
        Ok(())
    })?;
    connector.update_edid(Some(&edid))?;
    let count = connector.add_edid_modes();
    let mut modes = KVec::new();
    for mode in connector.snapshot_probed_modes()? {
        if let Ok(mode) = mode_from_drm(mode.as_raw()) {
            modes.push(mode, GFP_KERNEL)?;
        }
    }
    *connector.modes.lock() = modes;
    Ok(count)
}

fn check_mode(
    connector: &ConnectorDisplayGuard<'_, NovaConnector>,
    raw_mode: &bindings::drm_display_mode,
) -> Result<(DisplayMode, RgbConfig)> {
    let mode = mode_from_drm(raw_mode)?;
    if !connector.modes.lock().contains(&mode) {
        return Err(EINVAL);
    }
    let hdmi = connector.hdmi_rgb_config(raw_mode)?;
    if !hdmi.sink.is_hdmi
        || hdmi.sink.bpc != 0 && hdmi.sink.bpc < 8
        || hdmi.requires_vendor_infoframe
    {
        return Err(EOPNOTSUPP);
    }
    let max_clock = if hdmi.sink.max_tmds_clock_khz == 0 {
        165_000
    } else {
        hdmi.sink.max_tmds_clock_khz
    };
    // Conservatively cap this implementation at 165 MHz until the output path
    // selects its limit from both GPU and sink capabilities.
    if mode.clock_khz > max_clock.min(165_000) {
        return Err(EINVAL);
    }
    Ok((mode, hdmi))
}

pub(super) fn configuration(
    connector: &ConnectorDisplayGuard<'_, NovaConnector>,
    raw_mode: &bindings::drm_display_mode,
    format: DisplayPixelFormat,
    pitch: u32,
) -> Result<DisplayConfiguration> {
    let (mode, hdmi) = check_mode(connector, raw_mode)?;
    let minimum_pitch = mode
        .hdisplay
        .checked_mul(format.bytes_per_pixel())
        .ok_or(EOVERFLOW)?;
    if pitch < minimum_pitch || !pitch.is_multiple_of(256) {
        return Err(EINVAL);
    }
    let scdc = hdmi.sink.scdc;
    let hdmi_sink_caps = u32::from(scdc && hdmi.sink.max_tmds_clock_khz > 340_000)
        | (u32::from(scdc && hdmi.sink.low_rate_scrambling) << 1)
        | (u32::from(scdc) << 2);
    Ok(DisplayConfiguration {
        display_id: connector.output.display_id,
        mode,
        format,
        pitch,
        hdmi_sink_caps,
        avi_infoframe: hdmi.avi,
        limited_range: hdmi.limited_range,
    })
}

pub(super) fn mode_from_drm(mode: &bindings::drm_display_mode) -> Result<DisplayMode> {
    let horizontal = bindings::DRM_MODE_FLAG_PHSYNC | bindings::DRM_MODE_FLAG_NHSYNC;
    let vertical = bindings::DRM_MODE_FLAG_PVSYNC | bindings::DRM_MODE_FLAG_NVSYNC;
    if mode.flags & !(horizontal | vertical) != 0 || mode.hskew != 0 || mode.vscan > 1 {
        return Err(EOPNOTSUPP);
    }
    if mode.flags & horizontal == horizontal
        || mode.flags & vertical == vertical
        || mode.clock <= 0
        || mode.hdisplay == 0
        || mode.hdisplay >= mode.hsync_start
        || mode.hsync_start >= mode.hsync_end
        || mode.hsync_end >= mode.htotal
        || mode.vdisplay == 0
        || mode.vdisplay >= mode.vsync_start
        || mode.vsync_start >= mode.vsync_end
        || mode.vsync_end >= mode.vtotal
    {
        return Err(EINVAL);
    }
    Ok(DisplayMode {
        clock_khz: u32::try_from(mode.clock).map_err(|_| EINVAL)?,
        hdisplay: u32::from(mode.hdisplay),
        hsync_start: u32::from(mode.hsync_start),
        hsync_end: u32::from(mode.hsync_end),
        htotal: u32::from(mode.htotal),
        vdisplay: u32::from(mode.vdisplay),
        vsync_start: u32::from(mode.vsync_start),
        vsync_end: u32::from(mode.vsync_end),
        vtotal: u32::from(mode.vtotal),
        negative_hsync: mode.flags & bindings::DRM_MODE_FLAG_NHSYNC != 0,
        negative_vsync: mode.flags & bindings::DRM_MODE_FLAG_NVSYNC != 0,
    })
}
