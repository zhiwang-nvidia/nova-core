// SPDX-License-Identifier: GPL-2.0 OR MIT

//! HDMI sink information and RGB AVI infoframes under connector locking.

use crate::{
    bindings,
    drm::kms::connector::{
        AsRawConnector,
        ConnectorDisplayGuard,
        DriverConnector, //
    },
    error::to_result,
    num::casts,
    prelude::*, //
};

/// Size of a packed AVI infoframe, including header and checksum.
pub const AVI_BYTES: usize =
    casts::u32_as_usize(bindings::HDMI_INFOFRAME_HEADER_SIZE + bindings::HDMI_AVI_INFOFRAME_SIZE);

/// Sink properties copied from DRM's EDID interpretation.
#[derive(Clone, Copy)]
pub struct SinkInfo {
    /// Whether the sink advertises HDMI operation.
    pub is_hdmi: bool,
    /// Maximum TMDS clock in kHz, or zero if no limit was advertised.
    pub max_tmds_clock_khz: u32,
    /// Maximum bits per component, or zero if unknown.
    pub bpc: u32,
    /// Whether SCDC access is supported.
    pub scdc: bool,
    /// Whether scrambling at or below 340 MHz is supported.
    pub low_rate_scrambling: bool,
    /// Whether the sink permits selectable RGB quantization.
    pub rgb_quant_range_selectable: bool,
}

/// An RGB configuration that owns its sink information and packet bytes.
pub struct RgbConfig {
    /// Capabilities used to construct this configuration.
    pub sink: SinkInfo,
    /// Packed AVI infoframe, including header and checksum.
    pub avi: [u8; AVI_BYTES],
    /// Whether the source must transmit limited-range RGB samples.
    pub limited_range: bool,
    /// Whether the mode also needs a vendor infoframe for HDMI VIC or 3D.
    pub requires_vendor_infoframe: bool,
}

impl<T: DriverConnector> ConnectorDisplayGuard<'_, T> {
    /// Copy HDMI properties while excluding EDID updates.
    fn hdmi_sink_info(&self) -> Result<SinkInfo> {
        // SAFETY: This guard holds connection_mutex. EDID updates exposed by
        // the Rust connector API require the same lock and no reference escapes.
        let info = unsafe { &(*self.as_raw()).display_info };
        Ok(SinkInfo {
            is_hdmi: info.is_hdmi,
            max_tmds_clock_khz: u32::try_from(info.max_tmds_clock).map_err(|_| EINVAL)?,
            bpc: info.bpc,
            scdc: info.hdmi.scdc.supported,
            low_rate_scrambling: info.hdmi.scdc.scrambling.low_rates,
            rgb_quant_range_selectable: info.rgb_quant_range_selectable,
        })
    }

    /// Construct an RGB AVI packet using DRM's mode and quantization policies.
    ///
    /// Modes that the sink only supports as YCbCr 4:2:0 are rejected.
    /// This does not validate the mode against the sink's timings or TMDS limit.
    /// Drivers without vendor-packet support must reject a configuration with
    /// `requires_vendor_infoframe` before programming the source.
    pub fn hdmi_rgb_config(&self, mode: &bindings::drm_display_mode) -> Result<RgbConfig> {
        let sink = self.hdmi_sink_info()?;
        // SAFETY: The guard retains the connector and connection_mutex, excluding
        // EDID updates while this helper reads the sink's parsed CTA mode bitmap.
        if unsafe { bindings::drm_mode_is_420_only(&raw const (*self.as_raw()).display_info, mode) }
        {
            return Err(EOPNOTSUPP);
        }
        let mut avi = bindings::hdmi_avi_infoframe::default();
        let mut vendor = bindings::hdmi_vendor_infoframe::default();
        // SAFETY: The guard retains connection_mutex and the connector's parsed
        // CTA data. Both output structures are exclusively owned for these calls.
        let vendor_status = unsafe {
            bindings::drm_hdmi_vendor_infoframe_from_display_mode(&mut vendor, self.as_raw(), mode)
        };
        if vendor_status != 0 && vendor_status != EINVAL.to_errno() {
            to_result(vendor_status)?;
        }
        // SAFETY: The same locked connector and borrowed mode satisfy the AVI helper.
        to_result(unsafe {
            bindings::drm_hdmi_avi_infoframe_from_display_mode(&mut avi, self.as_raw(), mode)
        })?;
        avi.colorspace = bindings::hdmi_colorspace_HDMI_COLORSPACE_RGB;
        let range = if !sink.is_hdmi || sink.rgb_quant_range_selectable {
            bindings::hdmi_quantization_range_HDMI_QUANTIZATION_RANGE_FULL
        } else {
            // SAFETY: This helper only reads the initialized, borrowed mode.
            unsafe { bindings::drm_default_rgb_quant_range(mode) }
        };
        // SAFETY: The AVI structure is exclusive, and connector information stays
        // protected by connection_mutex for the duration of this call.
        unsafe {
            bindings::drm_hdmi_avi_infoframe_quant_range(&mut avi, self.as_raw(), mode, range)
        };

        let mut bytes = [0; AVI_BYTES];
        // SAFETY: The helper receives an initialized buffer of the supplied size
        // and has exclusive access to it until this synchronous call returns.
        let length = unsafe {
            bindings::hdmi_avi_infoframe_pack(&mut avi, bytes.as_mut_ptr().cast(), bytes.len())
        };
        if length < 0 {
            return Err(Error::from_errno(
                i32::try_from(length).map_err(|_| EOVERFLOW)?,
            ));
        }
        if usize::try_from(length).map_err(|_| EOVERFLOW)? != AVI_BYTES {
            return Err(EPROTO);
        }
        Ok(RgbConfig {
            sink,
            avi: bytes,
            limited_range: range
                == bindings::hdmi_quantization_range_HDMI_QUANTIZATION_RANGE_LIMITED,
            requires_vendor_infoframe: vendor_status == 0
                && (vendor.vic != 0
                    || vendor.s3d_struct != bindings::hdmi_3d_structure_HDMI_3D_STRUCTURE_INVALID),
        })
    }
}
