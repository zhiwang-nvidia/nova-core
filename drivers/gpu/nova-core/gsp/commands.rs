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
pub(crate) struct DisplayCaps {
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

impl DisplayCaps {
    /// Size in bytes of the `NV0073 SYSTEM_GET_CAPS_V2` table.
    pub(crate) const COMMON_CAPS_LEN: usize = 2;
    /// Size in bytes of the `NV5070 SYSTEM_GET_CAPS_V2` display-parent table.
    pub(crate) const DISPLAY_CAPS_LEN: usize = 1;
}

/// No NVKV fields are returned in DISPLAY_SHUTDOWN.
pub(crate) struct DisplayShutdownReply;

pub(crate) struct DisplayHwStaticInfo {
    /// Number of physical heads; not necessarily the number of usable heads.
    pub(crate) num_heads: u32,
    /// Usable heads, with bit N identifying hardware head N.
    pub(crate) head_mask: u32,
    /// Present Windows, with bit N identifying hardware Window N.
    pub(crate) window_mask: u32,
    /// Admitted static HDMI outputs. Each set bit's value is an RM `displayId`, not a DRM ID.
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
