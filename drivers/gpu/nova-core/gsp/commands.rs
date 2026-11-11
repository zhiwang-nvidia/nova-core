// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

pub(crate) mod gmc;

use core::{
    ffi::FromBytesUntilNulError,
    ops::Range,
    str::Utf8Error, //
};

use kernel::{
    alloc::ArrayVec,
    prelude::*, //
};

/// Display capabilities returned by DISPLAY_INIT.
pub(crate) struct DisplayInitReply {
    /// Supported `GMCAPI_DISPLAY_FEATURE_*` service bits.
    pub(crate) features: u64,
    /// Raw `NV_PDISP_FE_IP_VER` encoding.
    pub(crate) display_ip_version: u32,
    /// Raw `NV0073 SYSTEM_GET_CAPS_V2` table.
    pub(crate) common_caps: ArrayVec<u8, { Self::COMMON_CAPS_LEN }>,
    /// Raw `NV5070 SYSTEM_GET_CAPS_V2` display-parent table.
    pub(crate) display_caps: ArrayVec<u8, { Self::DISPLAY_CAPS_LEN }>,
    /// Raw `SPECIFIC_GET_HDMI_GPU_CAPS.caps` word.
    pub(crate) hdmi_gpu_caps: u32,
}

impl DisplayInitReply {
    /// Size in bytes of the `NV0073 SYSTEM_GET_CAPS_V2` table.
    pub(crate) const COMMON_CAPS_LEN: usize = 2;
    /// Size in bytes of the `NV5070 SYSTEM_GET_CAPS_V2` display-parent table.
    pub(crate) const DISPLAY_CAPS_LEN: usize = 1;
}

/// No NVKV fields are returned in DISPLAY_SHUTDOWN.
pub(crate) struct DisplayShutdownReply;

/// No NVKV fields are returned in `SET_INSTANCE_MEMORY`.
pub(crate) struct DisplaySetInstanceMemoryReply;

#[derive(Clone, Copy)]
pub(crate) enum DisplayChannelKind {
    Core,
    Window,
    Immediate,
    Cursor,
}

impl From<DisplayChannelKind> for u32 {
    fn from(kind: DisplayChannelKind) -> Self {
        match kind {
            DisplayChannelKind::Core => 0,
            DisplayChannelKind::Window => 1,
            DisplayChannelKind::Immediate => 2,
            DisplayChannelKind::Cursor => 3,
        }
    }
}

pub(crate) struct DisplayCreateChannelReply {
    /// Opaque firmware channel token, consumed by successful `DESTROY_CHANNEL`.
    pub(crate) channel: u64,
    /// Native channel class selected for the requested kind.
    pub(crate) class_id: u32,
    /// Physical display channel index, used in RAMHT bindings.
    pub(crate) hardware_index: u32,
    /// Channel user-register byte offset from the GPU register-aperture base.
    pub(crate) control_offset: u32,
    /// Channel user-register aperture size in bytes.
    pub(crate) control_size: u32,
    /// Private RM client's low 14 bits, used in RAMHT bindings.
    pub(crate) ramht_client_id: u32,
}

pub(crate) struct DisplayDestroyChannelReply;

/// Static display information returned by GET_STATIC_INFO.
pub(crate) struct DisplayGetStaticInfoReply {
    /// Number of physical heads; not necessarily the number of usable heads.
    pub(crate) num_heads: u32,
    /// Usable heads, with bit N identifying hardware head N.
    pub(crate) head_mask: u32,
    /// Present windows, with bit N identifying hardware window N.
    pub(crate) window_mask: u32,
    /// Admitted static HDMI outputs, with each set bit's value identifying a GSP-RM display ID.
    pub(crate) display_mask: u32,
    /// Number of hardware display channels, not a limit on simultaneous outputs.
    pub(crate) num_channels: u32,
    /// Raw `NV_PDISP_FE_HW_SYS_CAP` value for native HAL interpretation.
    pub(crate) fe_hw_sys_cap: u32,
    /// Native display-parent class.
    pub(crate) display_class: u32,
    /// Native Core DMA channel class.
    pub(crate) core_class: u32,
    /// Native Window DMA channel class.
    pub(crate) window_class: u32,
    /// Native Window Immediate DMA channel class.
    pub(crate) immediate_class: u32,
    /// Native Cursor PIO channel class.
    pub(crate) cursor_class: u32,
    /// Maximum number of active heads admitted by the adapter.
    pub(crate) max_active_heads: u32,
    /// Maximum number of concurrently retained routes.
    pub(crate) max_streams: u32,
    /// Maximum number of display channels.
    pub(crate) max_channels: u32,
    /// Maximum AUX payload size in bytes; zero when AUX is unavailable.
    pub(crate) max_aux_bytes: u32,
    /// Maximum EDID reply size in bytes.
    pub(crate) max_edid_bytes: u32,
    /// Maximum NVKV payload size in bytes, excluding transport headers.
    pub(crate) max_message_bytes: u32,
    /// Maximum DMA pushbuffer size in bytes.
    pub(crate) max_pb_bytes: u32,
}

/// Connector types reported by RM.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayConnectorType {
    HdmiA,
    /// Connector type not currently handled, preserving the RM encoding.
    Unknown(u32),
}

/// Connection state and connector information for one RM display ID.
pub(crate) struct DisplayGetPortStateReply {
    /// RM display identifier echoed from the request, with exactly one bit set.
    pub(crate) display_id: u32,
    /// One if a monitor was detected on this output, zero otherwise.
    pub(super) connected: u32,
    /// Index of the physical connector on the board.
    ///
    /// Separate RM display IDs can share this index, for example the DisplayPort
    /// and HDMI/DVI paths of a dual-mode connector. DisplayPort Multi-Stream
    /// Transport (MST) also allows multiple displays to share one physical connector.
    pub(crate) connector_index: u32,
    /// Raw RM connector type.
    pub(super) connector_type: u32,
    /// RM location code describing the connector's relative position on the board.
    pub(crate) location: u32,
    /// Mask of RM display IDs sharing this output's Display Data Channel (DDC) line.
    pub(crate) ddcp_partners: u32,
    /// Firmware's usable head mask, with bit N identifying hardware head N.
    pub(crate) head_mask: u32,
    /// RM connector-data flags, including whether connector data is present.
    pub(crate) flags: u32,
    /// RM detection status (`NV_STATUS`); zero means success.
    pub(crate) status: u32,
    /// Delay in milliseconds before retrying detection when RM requests a retry.
    pub(crate) retry_ms: u32,
}

impl DisplayGetPortStateReply {
    /// Returns whether connection detection found an attached display.
    pub(crate) fn is_connected(&self) -> bool {
        self.connected != 0
    }

    /// Returns the connector type reported by RM.
    #[expect(dead_code)]
    pub(crate) fn connector_type(&self) -> DisplayConnectorType {
        match self.connector_type {
            0x61 => DisplayConnectorType::HdmiA,
            value => DisplayConnectorType::Unknown(value),
        }
    }
}

pub(crate) struct DisplayReadEdidReply {
    pub(crate) edid: KVec<u8>,
}

/// Hardware resource type used by a GSP-RM display output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayOutputResourceType {
    /// Serial output resource for digital display links.
    Sor,
    /// Resource type not currently handled, preserving the GSP-RM encoding.
    Unknown(u32),
}

/// Output hardware resource information for one RM display ID.
pub(crate) struct DisplayGetPortCapsReply {
    /// Index of the hardware output resource within its resource type.
    pub(crate) index: u32,
    /// Raw RM output resource type.
    pub(super) resource_type: u32,
    /// Output protocol in RM's encoding for this resource type.
    pub(crate) protocol: u32,
    /// Output resource location: 0 for the GPU chip, 1 for the board.
    pub(crate) location: u32,
    /// RM display ID of the DisplayPort root port; zero when not applicable.
    pub(crate) root_port_id: u32,
    /// Output entry index in the VBIOS Display Configuration Block (DCB).
    pub(crate) dcb_index: u32,
    /// One if firmware identifies an internal panel, zero otherwise.
    pub(super) internal_panel: u32,
}

impl DisplayGetPortCapsReply {
    /// Returns whether firmware identifies this output as an internal panel.
    pub(crate) fn is_internal_panel(&self) -> bool {
        self.internal_panel != 0
    }

    /// Returns the output hardware resource type.
    pub(crate) fn resource_type(&self) -> DisplayOutputResourceType {
        match self.resource_type {
            2 => DisplayOutputResourceType::Sor,
            value => DisplayOutputResourceType::Unknown(value),
        }
    }
}

/// Framebuffer format accepted by the GMC display configuration check.
#[derive(Clone, Copy)]
pub(crate) enum DisplayPixelFormat {
    #[expect(dead_code)]
    Xrgb8888,
}

impl From<DisplayPixelFormat> for u32 {
    fn from(format: DisplayPixelFormat) -> Self {
        match format {
            DisplayPixelFormat::Xrgb8888 => 1,
        }
    }
}

pub(crate) struct DisplayConfiguration {
    pub(crate) head: u32,
    pub(crate) window: u32,
    pub(crate) display_id: u32,
    pub(crate) clock_khz: u32,
    pub(crate) htotal: u32,
    pub(crate) vtotal: u32,
    pub(crate) hblank_start: u32,
    pub(crate) hblank_end: u32,
    pub(crate) vblank_start: u32,
    pub(crate) vblank_end: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pitch: u32,
    pub(crate) format: DisplayPixelFormat,
}

#[expect(dead_code)]
pub(crate) struct DisplayCheckConfigurationReply {
    pub(crate) possible: u32,
    pub(crate) checked_domains: u32,
}

pub(crate) struct DisplayModesetBeginReply {
    pub(crate) modeset_id: u64,
}

/// How to end a GSP-RM modeset bracket.
#[derive(Clone, Copy)]
pub(crate) enum DisplayModesetEndMode {
    /// Ends the bracket after verifying the target set of enabled outputs.
    Commit,
}

impl From<DisplayModesetEndMode> for u32 {
    fn from(end_mode: DisplayModesetEndMode) -> Self {
        match end_mode {
            DisplayModesetEndMode::Commit => 0,
        }
    }
}

pub(crate) struct DisplayModesetEndReply;

/// The static GPU configuration, which GSP-RM reports in reply to [`GspInit`](gmc::GspInit).
pub struct GspStaticInfo {
    gpu_name: [u8; Self::MAX_GPU_NAME_LEN],
    gpu_short_name: [u8; Self::MAX_GPU_NAME_LEN],
    /// The 16-byte SHA-1 based GPU identifier (GID) reported by GSP-RM.
    pub gpu_gid: [u8; Self::GID_LEN],
    /// BAR1 Page Directory Entry base address.
    pub(crate) bar1_pde_base: u64,
    /// Usable FB (VRAM) regions for driver memory allocation.
    pub(crate) usable_fb_regions: KVec<Range<u64>>,
    /// Exclusive end of the FB physical address space.
    pub(crate) total_fb_end: u64,
}

/// Error type for [`GspStaticInfo::gpu_name`].
#[derive(Debug)]
pub enum GpuNameError {
    /// The GPU name string does not contain a null terminator.
    NoNullTerminator(FromBytesUntilNulError),

    /// The GPU name string contains invalid UTF-8.
    InvalidUtf8(Utf8Error),
}

impl GspStaticInfo {
    /// Length of the buffers holding the GPU names, NUL terminator included.
    pub(crate) const MAX_GPU_NAME_LEN: usize = 64;
    /// Length of the GPU identifier.
    pub(crate) const GID_LEN: usize = 16;

    /// Returns the name of the GPU as a string.
    ///
    /// Returns an error if the string given by the GSP does not contain a null terminator or
    /// contains invalid UTF-8.
    pub fn gpu_name(&self) -> Result<&str, GpuNameError> {
        CStr::from_bytes_until_nul(&self.gpu_name)
            .map_err(GpuNameError::NoNullTerminator)?
            .to_str()
            .map_err(GpuNameError::InvalidUtf8)
    }

    /// Returns the short name of the GPU as a string.
    ///
    /// Returns an error if the string given by the GSP does not contain a null terminator or
    /// contains invalid UTF-8.
    pub fn gpu_short_name(&self) -> core::result::Result<&str, GpuNameError> {
        CStr::from_bytes_until_nul(&self.gpu_short_name)
            .map_err(GpuNameError::NoNullTerminator)?
            .to_str()
            .map_err(GpuNameError::InvalidUtf8)
    }

    /// Returns the total usable VRAM size in bytes, i.e. the summed lengths of all usable FB
    /// regions.
    pub fn vram_size(&self) -> u64 {
        self.usable_fb_regions.iter().fold(0, |size, region| {
            size.saturating_add(region.end - region.start)
        })
    }
}
