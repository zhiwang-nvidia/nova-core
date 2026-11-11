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
    types::ForLt, //
};

pub use crate::gpu::Spec;

use crate::{
    driver,
    gpu::Gpu,
    gsp::commands::{
        DisplayInitReply,
        GspStaticInfo, //
    }, //
};

/// API handle for the auxiliary bus child drivers to interact with nova-core.
pub struct NovaCoreApi<'bound> {
    pub(crate) gpu: Pin<&'bound Gpu<'bound>>,
    pub(crate) pdev: &'bound pci::Device<Bound>,
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
