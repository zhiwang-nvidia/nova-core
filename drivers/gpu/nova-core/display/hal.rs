// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    sync::Mutex, //
};

use crate::{
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

pub(super) trait DisplayHal<'a, 'gpu>: Send {
    /// Allocate a surface descriptor that associates with a VRAM region.
    #[expect(dead_code)]
    fn alloc_surface_descriptor(
        &mut self,
        target_region: VramRegion,
        writable: bool,
        block_linear: bool,
    ) -> Result<SurfaceDescriptor>;

    /// Bind a surface descriptor to a channel.
    #[expect(dead_code)]
    fn bind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result;

    /// Unbind a surface descriptor from a channel.
    #[expect(dead_code)]
    fn unbind_surface_descriptor(
        &mut self,
        descriptor: SurfaceDescriptor,
        client_id: u32,
        display_channel: u32,
    ) -> Result;

    /// Free a surface descriptor. It must be unbound.
    #[expect(dead_code)]
    fn free_surface_descriptor(&mut self, descriptor: SurfaceDescriptor) -> Result;
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
