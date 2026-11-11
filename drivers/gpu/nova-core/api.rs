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
        DisplayAssignStreamRouteReply,
        DisplayGetPortCapsReply,
        DisplayGetPortStateReply,
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

    /// Queries one GSP-RM output's connection and connector state.
    pub fn display_port_state(&self, display_id: u32) -> Result<DisplayPortState> {
        self.gpu
            .display_port_state(display_id)
            .map(DisplayPortState::from)
    }

    /// Queries the hardware resource used by one GSP-RM display output.
    pub fn display_port_caps(&self, display_id: u32) -> Result<DisplayPortCaps> {
        self.gpu
            .display_port_caps(display_id)
            .map(DisplayPortCaps::from)
    }

    /// Returns a display's Extended Display Identification Data (EDID).
    pub fn display_read_edid(&self, display_id: u32) -> Result<KVec<u8>> {
        self.gpu.display_read_edid(display_id)
    }

    /// Checks display bandwidth without changing the current display state.
    ///
    /// Returns whether GSP-RM accepts the configuration. Malformed or incomplete
    /// replies are errors; callers apply mode and framebuffer policy separately.
    pub fn display_check_configuration(
        &self,
        configuration: &DisplayConfiguration,
    ) -> Result<bool> {
        self.gpu.display_check_configuration(&configuration.into())
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

    /// Starts a modeset bracket covering the affected outputs.
    ///
    /// `target_display_mask` is the complete set of outputs to leave enabled,
    /// including outputs outside the affected `display_mask`. An error may leave
    /// a firmware bracket open; shut down display before submitting more work.
    pub fn display_modeset_begin(
        &self,
        display_mask: u32,
        target_display_mask: u32,
    ) -> Result<u64> {
        self.gpu
            .display_modeset_begin(display_mask, target_display_mask)
    }

    /// Ends the modeset bracket identified by a token returned on this GPU.
    ///
    /// Success consumes the token but does not prove framebuffer retirement.
    /// An error may leave the bracket open; retain resources until display is
    /// shut down successfully.
    pub fn display_modeset_end(&self, modeset_id: u64, end_mode: DisplayModesetEndMode) -> Result {
        self.gpu.display_modeset_end(modeset_id, end_mode.into())
    }

    /// Assigns a head to an output within the specified modeset bracket.
    ///
    /// Retain the returned stream token before validating its SOR assignment.
    /// An error may leave firmware resources allocated; retain display resources
    /// through successful display shutdown before retrying.
    pub fn display_assign_stream_route(
        &self,
        modeset_id: u64,
        display_id: u32,
        head: u32,
        exclude_mask: u32,
    ) -> Result<DisplayStreamRoute> {
        self.gpu
            .display_assign_stream_route(modeset_id, display_id, head, exclude_mask)
            .map(DisplayStreamRoute::from)
    }

    /// Releases a stream after its output activity and head association are stopped.
    ///
    /// Keep the stream token if release fails. Release does not establish
    /// framebuffer retirement; wait for the display channels separately.
    pub fn display_release_stream_route(&self, stream_id: u64, modeset_id: u64) -> Result {
        self.gpu.display_release_stream_route(stream_id, modeset_id)
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

/// Output resources assigned by GSP-RM to a display stream.
pub struct DisplayStreamRoute {
    /// Opaque stream token used by subsequent route and HDMI operations.
    pub stream_id: u64,
    /// Display ID mask for each serial output resource (SOR), indexed by SOR number.
    pub assigned: ArrayVec<u32, 4>,
}

impl From<DisplayAssignStreamRouteReply> for DisplayStreamRoute {
    fn from(reply: DisplayAssignStreamRouteReply) -> Self {
        Self {
            stream_id: reply.stream_id,
            assigned: reply.assigned,
        }
    }
}

/// How to end a display modeset bracket.
#[derive(Clone, Copy)]
pub enum DisplayModesetEndMode {
    /// Completes the bracket after verifying the target set of enabled outputs.
    Commit,
}

impl From<DisplayModesetEndMode> for crate::gsp::commands::DisplayModesetEndMode {
    fn from(end_mode: DisplayModesetEndMode) -> Self {
        match end_mode {
            DisplayModesetEndMode::Commit => Self::Commit,
        }
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

/// Connection and connector state for one GSP-RM display output.
pub struct DisplayPortState {
    /// Whether detection found a connected monitor.
    pub connected: bool,
    /// Physical connector index reported by GSP-RM.
    pub connector_index: u32,
    /// Usable heads, with bit N identifying hardware head N.
    pub head_mask: u32,
}

impl From<DisplayGetPortStateReply> for DisplayPortState {
    fn from(reply: DisplayGetPortStateReply) -> Self {
        Self {
            connected: reply.is_connected(),
            connector_index: reply.connector_index,
            head_mask: reply.head_mask,
        }
    }
}

/// Hardware resource type used by a GSP-RM display output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayOutputResourceType {
    /// Serial output resource for digital display links.
    Sor,
    /// Resource type not currently handled, preserving the GSP-RM encoding.
    Unknown(u32),
}

/// Hardware resources used by one GSP-RM display output.
pub struct DisplayPortCaps {
    /// Index within the hardware output resource type.
    pub index: u32,
    /// GSP-RM protocol encoding for this output resource.
    pub protocol: u32,
    /// Hardware output resource type.
    pub resource_type: DisplayOutputResourceType,
    /// Whether this output connects to an internal panel.
    pub internal_panel: bool,
}

impl From<DisplayGetPortCapsReply> for DisplayPortCaps {
    fn from(reply: DisplayGetPortCapsReply) -> Self {
        use crate::gsp::commands::DisplayOutputResourceType as ResourceType;

        Self {
            index: reply.index,
            protocol: reply.protocol,
            resource_type: match reply.resource_type() {
                ResourceType::Sor => DisplayOutputResourceType::Sor,
                ResourceType::Unknown(value) => DisplayOutputResourceType::Unknown(value),
            },
            internal_panel: reply.is_internal_panel(),
        }
    }
}

/// Framebuffer format supplied to GSP-RM's display configuration check.
#[derive(Clone, Copy)]
pub enum DisplayPixelFormat {
    /// Eight-bit RGB components in a 32-bit pixel, with an unused high byte.
    Xrgb8888,
}

/// Timing, framebuffer and output parameters for GSP-RM's bandwidth check.
///
/// These are protocol query inputs. The display driver owns mode selection,
/// format policy and the resources used to apply a configuration.
pub struct DisplayConfiguration {
    /// Hardware head ID.
    pub head: u32,
    /// Hardware window ID.
    pub window: u32,
    /// Single-bit GSP-RM display identifier.
    pub display_id: u32,
    /// Pixel clock in kHz.
    pub clock_khz: u32,
    /// Total horizontal pixels, including blanking.
    pub htotal: u32,
    /// Total vertical lines, including blanking.
    pub vtotal: u32,
    /// Horizontal blanking start measured from sync start.
    pub hblank_start: u32,
    /// Horizontal blanking end measured from sync start.
    pub hblank_end: u32,
    /// Vertical blanking start measured from sync start.
    pub vblank_start: u32,
    /// Vertical blanking end measured from sync start.
    pub vblank_end: u32,
    /// Framebuffer width in pixels.
    pub width: u32,
    /// Framebuffer height in lines.
    pub height: u32,
    /// Framebuffer row stride in bytes.
    pub pitch: u32,
    /// Framebuffer pixel format.
    pub format: DisplayPixelFormat,
}

impl From<&DisplayConfiguration> for crate::gsp::commands::DisplayConfiguration {
    fn from(configuration: &DisplayConfiguration) -> Self {
        Self {
            head: configuration.head,
            window: configuration.window,
            display_id: configuration.display_id,
            clock_khz: configuration.clock_khz,
            htotal: configuration.htotal,
            vtotal: configuration.vtotal,
            hblank_start: configuration.hblank_start,
            hblank_end: configuration.hblank_end,
            vblank_start: configuration.vblank_start,
            vblank_end: configuration.vblank_end,
            width: configuration.width,
            height: configuration.height,
            pitch: configuration.pitch,
            format: match configuration.format {
                DisplayPixelFormat::Xrgb8888 => crate::gsp::commands::DisplayPixelFormat::Xrgb8888,
            },
        }
    }
}
