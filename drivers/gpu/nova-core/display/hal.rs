// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    sync::Mutex, //
};

use crate::{
    display::{
        channel::{
            Channel,
            Core,
            WindowDma,
            WindowImmediate, //
        },
        mode::DisplayMode,
        DisplayPixelFormat, //
    },
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::DisplayHwStaticInfo, //
    },
    mm::{
        bar_user::BarUser,
        vram::VramRegion,
        GpuMm, //
    },
};

mod ad102;
mod disp4;

/// Identifies a surface descriptor.
///
/// Surface descriptor lifecycle:
///
/// ```text
///          Target buffer
///                |
///                v
/// +-----------------------------+
/// | alloc_surface_descriptor    |
/// +-----------------------------+
///                | SurfaceDescriptor
///                v
/// +-----------------------------+
/// | bind_surface_descriptor     |
/// +-----------------------------+
///                | program channel methods
///                v
/// +-----------------------------+
/// | Channel accesses buffer     |
/// +-----------------------------+
///                | after channel access completes
///                v
/// +-----------------------------+
/// | unbind_surface_descriptor   |
/// +-----------------------------+
///                | after all bindings are removed
///                v
/// +-----------------------------+
/// | free_surface_descriptor     |
/// +-----------------------------+
/// ```
///
/// Note that a descriptor can be bound to multiple display channels at once.

// TODO: Encapsulate bind/unbind/free and revisit RAII. Shared references with
// internal locking could simplify cleanup, but keep passing mutable HAL
// references for now to avoid introducing more locks.

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct SurfaceDescriptor {
    context_dma_descriptor_id: disp4::context_dma::ContextDmaDescriptorId,
}

pub(super) trait DisplayMethodHal {
    fn encode_method_header(&self, method: u32, count: usize) -> Result<u32>;
}

pub(super) trait DisplayHal<'a, 'gpu>: DisplayMethodHal + Send {
    fn init_core_channel(&self, channel: &mut Channel<'a, 'gpu, Core>, head_mask: u32) -> Result;

    fn init_window_channel(
        &self,
        channel: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
        head_mask: u32,
    ) -> Result;

    /// Emits core notifier methods using the generation's handle or address methods.
    fn emit_core_notifier_methods(
        &self,
        channel: &mut Channel<'a, 'gpu, Core>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result;

    /// Emits window notifier methods using the generation's handle or address methods.
    fn emit_window_notifier_methods(
        &self,
        channel: &mut Channel<'a, 'gpu, WindowDma>,
        descriptor: Option<SurfaceDescriptor>,
    ) -> Result;

    /// Emits the window UPDATE sequence, including any class-specific transition methods.
    fn emit_window_update(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        interlock_immediate: bool,
    ) -> Result;

    /// Emits an immediate UPDATE with the flags supported by its class.
    fn emit_window_immediate_update(
        &self,
        immediate: &mut Channel<'a, 'gpu, WindowImmediate>,
    ) -> Result;

    /// Allocate a surface descriptor that associates with a VRAM region.
    fn alloc_surface_descriptor(
        &mut self,
        target_region: VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<SurfaceDescriptor>;

    /// Bind a surface descriptor to a channel.
    fn bind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result;

    /// Unbind a surface descriptor from a channel.
    fn unbind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result;

    /// Free a surface descriptor. It must be unbound.
    fn free_surface_descriptor(&mut self, descriptor: SurfaceDescriptor) -> Result;

    /// Queues methods to disable head auxiliary accesses and detach heads from SORs.
    ///
    /// `head_mask` selects heads whose auxiliary memory accesses are disabled and
    /// display IDs are unset. All SOR head owner masks are cleared, regardless of
    /// `head_mask`.
    /// Call this only before driver-owned routes exist, then submit a core UPDATE.
    fn emit_disable_heads_and_detach_sors(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head_mask: u32,
    ) -> Result;

    /// Queues methods to disable a head's auxiliary memory accesses and unset its display ID.
    ///
    /// SOR ownership is unchanged. The caller retires the head's window framebuffers
    /// and stops its outputs first, then submits a core UPDATE to apply these changes.
    fn emit_disable_head(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head: u32,
    ) -> Result;

    /// Queues a SOR's head owner mask and output protocol.
    ///
    /// The caller preserves any remaining owners. An empty mask detaches the SOR
    /// and ignores `rm_protocol`. Submit a core UPDATE with the corresponding head changes.
    fn emit_sor_control(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        sor: u32,
        rm_protocol: u32,
        head_mask: u32,
    ) -> Result;

    /// Checks whether the hardware can represent the mode timings.
    fn validate_mode(&self, mode: &DisplayMode) -> Result;

    /// Emits methods to configure head timing, viewport and output color.
    #[expect(dead_code)]
    fn emit_head_configuration(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        bar: Bar0<'_>,
        head: u32,
        mode: &DisplayMode,
        limited_range: bool,
    ) -> Result;

    /// Queues a head's GSP-RM display ID; zero unsets the association.
    #[expect(dead_code)]
    fn emit_head_display_id(
        &self,
        core: &mut Channel<'a, 'gpu, Core>,
        head: u32,
        display_id: u32,
    ) -> Result;

    /// Emits methods to configure a pitch-linear framebuffer with composition bypass.
    #[expect(dead_code)]
    fn emit_window_framebuffer_configuration(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
        mode: &DisplayMode,
        format: DisplayPixelFormat,
        pitch: u32,
        descriptor: SurfaceDescriptor,
    ) -> Result;

    /// Queues methods to disable framebuffer/LUT and semaphore accesses.
    fn emit_disable_window(
        &self,
        window: &mut Channel<'a, 'gpu, WindowDma>,
        bar: Bar0<'_>,
    ) -> Result;
}

pub(super) fn display_hal<'a, 'gpu: 'a>(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq<'gpu>,
    mm: &'a Mutex<GpuMm<'gpu>>,
    bar_user: &'a BarUser<'gpu>,
    ip_version: u32,
    info: &DisplayHwStaticInfo,
) -> Result<KBox<dyn DisplayHal<'a, 'gpu> + 'a>> {
    const IP_VERSION_MASK: u32 = 0xffff_0000;

    let hal: KBox<dyn DisplayHal<'a, 'gpu> + 'a> = match ip_version & IP_VERSION_MASK {
        ad102::IP_VERSION => ad102::Ad102::new(dev, cmdq, mm, bar_user, info)?,
        _ => return Err(EOPNOTSUPP),
    };

    Ok(hal)
}
