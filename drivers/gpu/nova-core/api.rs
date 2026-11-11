// SPDX-License-Identifier: GPL-2.0

//! Nova-core auxbus data. Contains all the methods used by the auxbus drivers
//! to interact with nova-core.

use core::pin::Pin;

use kernel::{
    alloc::ArrayVec,
    auxiliary,
    device::Bound,
    pci,
    prelude::*,
    sync::Arc,
    types::ForLt, //
};

pub use crate::gpu::Spec;

use crate::{
    driver,
    gpu::Gpu,
    gsp::commands::{
        DisplayGetStaticInfoReply,
        DisplayInitReply,
        GspStaticInfo, //
    }, //
};

mod display_capability;
mod display_channel;
mod vram;

pub use display_capability::{
    DisplayPostcompCaps,
    DisplayPrecompCaps, //
};
pub use display_channel::{
    DisplayChannelInfo,
    DisplayChannelKind, //
};
pub use vram::{
    VramBlock,
    VramMapping,
    VramRegion, //
};

/// API handle for the auxiliary bus child drivers to interact with nova-core.
pub struct NovaCoreApi<'bound> {
    pub(crate) gpu: Pin<&'bound Gpu<'bound>>,
    pub(crate) pdev: &'bound pci::Device<Bound>,
    // Distinguishes allocations from this binding, including after PCI rebind.
    pub(crate) vram_owner: Arc<()>,
}

impl NovaCoreApi<'_> {
    /// Obtain a [`NovaCoreApi`] handle from an auxiliary device registered
    /// by nova-core.
    pub fn of(adev: &auxiliary::Device<Bound>) -> Result<NovaCoreApiHandle<'_>> {
        NovaCoreApiHandle::of(adev)
    }

    /// Initializes the GMC display context and returns its capabilities.
    pub fn display_init(&self) -> Result<DisplayCaps> {
        self.gpu.display_init().map(DisplayCaps::from)
    }

    /// Shuts down the GMC display context.
    pub fn display_shutdown(&self) -> Result {
        self.gpu.display_shutdown()
    }

    /// Queries GSP-RM's static display information.
    pub fn display_static_info(&self) -> Result<DisplayStaticInfo> {
        self.gpu.display_static_info().map(DisplayStaticInfo::from)
    }

    /// Sets GSP-RM's display instance memory to a VRAM region.
    ///
    /// Keep the region alive until display shutdown succeeds, including when this call fails.
    pub fn display_set_instance_memory(&self, region: &VramRegion) -> Result {
        if !Arc::ptr_eq(&self.vram_owner, &region.owner) {
            return Err(EXDEV);
        }
        self.gpu
            .display_set_instance_memory(region.address(), region.size())
    }

    /// Returns the GPU [`GspStaticInfo`].
    pub fn gsp_static_info(&self) -> &GspStaticInfo {
        self.gpu.gsp_static_info()
    }

    /// Returns the GPU [`Spec`].
    pub fn spec(&self) -> &Spec {
        &self.gpu.spec
    }

    /// Returns the size of the PCIe BAR used for accessing VRAM, typically
    /// BAR1.
    pub fn bar1_size(&self) -> Result<u64> {
        let bar1 = driver::bar1_resource_index(self.pdev)?;

        self.pdev.resource_len(bar1)
    }
}

/// Expose a handle to nova-core API
#[derive(Clone)]
pub struct NovaCoreApiHandle<'a> {
    adev: &'a auxiliary::Device<Bound>,
}

impl<'a> NovaCoreApiHandle<'a> {
    fn of(adev: &'a auxiliary::Device<Bound>) -> Result<Self> {
        adev.registration_data_with::<ForLt!(NovaCoreApi<'_>), ()>(|_| ())?;
        Ok(Self { adev })
    }

    /// Access the [`NovaCoreApi`] through a closure.
    pub fn with<R>(&self, f: impl for<'b> FnOnce(Pin<&'a NovaCoreApi<'b>>) -> R) -> R {
        self.adev
            .registration_data_with::<ForLt!(NovaCoreApi<'_>), R>(f)
            .expect("TypeId was validated in NovaCoreApiHandle::of()")
    }
}

/// Display capabilities available to nova-drm.
///
/// TODO: Expose only the capabilities nova-drm uses. For now, forward all
/// capabilities returned by GSP-RM.
pub struct DisplayCaps {
    /// Supported `GMCAPI_DISPLAY_FEATURE_*` service bits.
    pub features: u64,
    /// Raw `NV_PDISP_FE_IP_VER` encoding.
    pub display_ip_version: u32,
    /// Raw `NV0073 SYSTEM_GET_CAPS_V2` table.
    pub common_caps: ArrayVec<u8, 2>,
    /// Raw `NV5070 SYSTEM_GET_CAPS_V2` display-parent table.
    pub display_caps: ArrayVec<u8, 1>,
    /// Raw `SPECIFIC_GET_HDMI_GPU_CAPS.caps` word.
    pub hdmi_gpu_caps: u32,
}

impl From<DisplayInitReply> for DisplayCaps {
    fn from(reply: DisplayInitReply) -> Self {
        Self {
            features: reply.features,
            display_ip_version: reply.display_ip_version,
            common_caps: reply.common_caps,
            display_caps: reply.display_caps,
            hdmi_gpu_caps: reply.hdmi_gpu_caps,
        }
    }
}

/// Static display information available to nova-drm.
///
/// TODO: Expose only the information nova-drm uses. For now, forward all
/// fields returned by GSP-RM.
pub struct DisplayStaticInfo {
    /// Number of physical heads; not necessarily the number of usable heads.
    pub num_heads: u32,
    /// Usable heads, with bit N identifying hardware head N.
    pub head_mask: u32,
    /// Present windows, with bit N identifying hardware window N.
    pub window_mask: u32,
    /// Admitted static HDMI outputs, with each set bit's value identifying a GSP-RM display ID.
    pub display_mask: u32,
    /// Number of hardware display channels, not a limit on simultaneous outputs.
    pub num_channels: u32,
    /// Raw `NV_PDISP_FE_HW_SYS_CAP` value for native HAL interpretation.
    pub fe_hw_sys_cap: u32,
    /// Native display-parent class.
    pub display_class: u32,
    /// Native Core DMA channel class.
    pub core_class: u32,
    /// Native Window DMA channel class.
    pub window_class: u32,
    /// Native Window Immediate DMA channel class.
    pub immediate_class: u32,
    /// Native Cursor PIO channel class.
    pub cursor_class: u32,
    /// Maximum number of active heads admitted by the adapter.
    pub max_active_heads: u32,
    /// Maximum number of concurrently retained routes.
    pub max_streams: u32,
    /// Maximum number of display channels.
    pub max_channels: u32,
    /// Maximum AUX payload size in bytes; zero when AUX is unavailable.
    pub max_aux_bytes: u32,
    /// Maximum EDID reply size in bytes.
    pub max_edid_bytes: u32,
    /// Maximum NVKV payload size in bytes, excluding transport headers.
    pub max_message_bytes: u32,
    /// Maximum DMA pushbuffer size in bytes.
    pub max_pb_bytes: u32,
}

impl From<DisplayGetStaticInfoReply> for DisplayStaticInfo {
    fn from(reply: DisplayGetStaticInfoReply) -> Self {
        Self {
            num_heads: reply.num_heads,
            head_mask: reply.head_mask,
            window_mask: reply.window_mask,
            display_mask: reply.display_mask,
            num_channels: reply.num_channels,
            fe_hw_sys_cap: reply.fe_hw_sys_cap,
            display_class: reply.display_class,
            core_class: reply.core_class,
            window_class: reply.window_class,
            immediate_class: reply.immediate_class,
            cursor_class: reply.cursor_class,
            max_active_heads: reply.max_active_heads,
            max_streams: reply.max_streams,
            max_channels: reply.max_channels,
            max_aux_bytes: reply.max_aux_bytes,
            max_edid_bytes: reply.max_edid_bytes,
            max_message_bytes: reply.max_message_bytes,
            max_pb_bytes: reply.max_pb_bytes,
        }
    }
}
