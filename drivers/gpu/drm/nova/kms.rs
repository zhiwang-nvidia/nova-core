// SPDX-License-Identifier: GPL-2.0

//! HDMI modesetting through nova-core's display channels.

mod connector;
mod crtc;
mod encoder;
mod plane;

use kernel::{
    device,
    drm::{
        fourcc,
        kms::{
            atomic::{
                AtomicCommitTail,
                CommittedAtomicState,
                ModesetsReadyToken,
                PlaneUpdatesReadyToken, //
            },
            connector::Type as ConnectorType,
            encoder::Type as EncoderType,
            plane::Type as PlaneType,
            KmsDriver,
            ModeConfigInfo,
            UnregisteredKmsDevice, //
        },
    },
    prelude::*,
};

use nova_core::api::DisplayOutput;

use crate::driver::NovaDriver;

#[vtable]
impl KmsDriver for NovaDriver {
    fn mode_config_info(_dev: &device::Device, _data: &Self::Data) -> Result<ModeConfigInfo> {
        Ok(ModeConfigInfo {
            min_resolution: (1, 1),
            max_resolution: (8192, 8192),
            max_cursor: (0, 0),
            preferred_depth: 24,
            preferred_fourcc: Some(fourcc::XRGB888),
        })
    }

    fn create_objects(dev: &UnregisteredKmsDevice<'_, Self>) -> Result {
        let primary = plane::UnregisteredPlane::new(
            dev,
            1,
            &[fourcc::XRGB888],
            Some(&[fourcc::FORMAT_MOD_LINEAR]),
            PlaneType::Primary,
            None,
            (),
        )?;
        crtc::UnregisteredCrtc::new(
            dev,
            primary,
            Option::<&plane::UnregisteredPlane>::None,
            None,
            (),
        )?;
        for output in &dev.outputs {
            let connector = connector::UnregisteredConnector::new(
                dev,
                ConnectorType::HdmiA,
                DisplayOutput {
                    display_id: output.display_id,
                    connected: output.connected,
                    head_mask: output.head_mask,
                    connector_index: output.connector_index,
                },
            )?;
            let encoder =
                encoder::UnregisteredEncoder::new(dev, EncoderType::Tmds, 1, 0, None, ())?;
            connector.attach_encoder(encoder)?;
        }
        Ok(())
    }

    fn atomic_commit_tail<'a>(
        mut state: AtomicCommitTail<'a, Self>,
        modeset_token: ModesetsReadyToken<'_>,
        plane_token: PlaneUpdatesReadyToken<'_>,
    ) -> CommittedAtomicState<'a, Self> {
        let modeset_token = state.commit_modeset_disables(modeset_token);
        let plane_token = state.commit_planes(plane_token, Default::default());
        let modeset_token = state.commit_modeset_enables(modeset_token);
        let state = state.commit_hw_done(modeset_token, plane_token);
        state.wait_for_flip_done();
        state
    }
}
