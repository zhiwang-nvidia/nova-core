// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::prelude::*;

use crate::{
    gsp::vgpu_bindings as bindings,
    mm::{
        bar_user::{
            Bar1Map,
            BarUser, //
        },
        vram::VramRegion,
        GpuMm, //
    },
};

type RawControlRegion = bindings::VGPU_CPU_GSP_CTRL_BUFF_REGION;

/// Physical VRAM regions containing the vGPU plugin logs.
pub(crate) struct PluginLogRegions {
    init: VramRegion,
    vgpu: VramRegion,
    kernel: VramRegion,
}

impl PluginLogRegions {
    /// Return the init-task log region.
    pub(crate) const fn init(&self) -> &VramRegion {
        &self.init
    }

    /// Return the vGPU-task log region.
    pub(crate) const fn vgpu(&self) -> &VramRegion {
        &self.vgpu
    }

    /// Return the kernel-task log region.
    pub(crate) const fn kernel(&self) -> &VramRegion {
        &self.kernel
    }
}

/// Take the next firmware-defined subregion from a communication buffer.
fn take_region(region: &VramRegion, cursor: &mut u64, size: u32) -> Result<VramRegion> {
    let end = cursor.checked_add(u64::from(size)).ok_or(EOVERFLOW)?;
    let subregion = region.subregion(*cursor..end)?;
    *cursor = end;
    Ok(subregion)
}

/// BAR1 mapping and semantic regions of a vGPU CPU-GSP communication buffer.
///
/// The host and GSP plugin exchange control, response, message, migration,
/// error, and diagnostic data through firmware-defined subregions of the
/// management heap. Firmware accesses that VRAM directly; the host accesses
/// the same storage through the owned BAR1 mapping.
///
/// The firmware bindings define each subregion's size and order, but are used
/// only to describe the layout. Field accesses must use the BAR1 I/O accessors
/// because bindgen does not preserve C `volatile` semantics. Keep this object
/// alive while the plugin or a host reader can use the buffer, then consume it
/// with [`Self::destroy`] after those users have stopped.
pub(crate) struct CommBufferRegion<'gpu> {
    map: Bar1Map<'gpu>,
    control: VramRegion,
    init_log: VramRegion,
    vgpu_log: VramRegion,
    kernel_log: VramRegion,
}

impl<'gpu> CommBufferRegion<'gpu> {
    /// Map the communication portion of a plugin management heap.
    pub(crate) fn new(
        bar_user: &BarUser<'gpu>,
        mm: &mut GpuMm<'_>,
        management_heap: &VramRegion,
    ) -> Result<Self> {
        let total_size = u64::from(bindings::VGPU_CPU_GSP_COMMUNICATION_BUFF_TOTAL_SIZE);
        let region = management_heap.subregion(0..total_size)?;
        let mut cursor = 0;

        let control = take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_CTRL_BUFF_REGION_SIZE,
        )?;
        take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_RESPONSE_BUFF_REGION_SIZE,
        )?;
        take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_MESSAGE_BUFF_REGION_SIZE,
        )?;
        take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_MIGRATION_BUFF_REGION_SIZE,
        )?;
        take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_ERROR_BUFF_REGION_SIZE,
        )?;
        let init_log = take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_INIT_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        let vgpu_log = take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_VGPU_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        let kernel_log = take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_KERNEL_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        take_region(
            &region,
            &mut cursor,
            bindings::VGPU_CPU_GSP_GUEST_RPC_TRACE_BUFF_REGION_SIZE,
        )?;

        if cursor != total_size || control.size() != u64::try_from(size_of::<RawControlRegion>())? {
            return Err(EINVAL);
        }

        let map = Bar1Map::new(bar_user, mm, region, true)?;

        Ok(Self {
            map,
            control,
            init_log,
            vgpu_log,
            kernel_log,
        })
    }

    fn region_offset(&self, region: &VramRegion) -> Result<usize> {
        let offset = region
            .address()
            .checked_sub(self.map.region().address())
            .ok_or(EINVAL)?;
        if offset.checked_add(region.size()).ok_or(EOVERFLOW)? > self.map.region().size() {
            return Err(EINVAL);
        }

        usize::try_from(offset).map_err(|_| EOVERFLOW)
    }

    fn io_offset(&self, region: &VramRegion, field: usize, width: usize) -> Result<usize> {
        let field_end = field.checked_add(width).ok_or(EOVERFLOW)?;
        if u64::try_from(field_end).map_err(|_| EOVERFLOW)? > region.size() {
            return Err(EINVAL);
        }

        self.region_offset(region)?
            .checked_add(field)
            .ok_or(EOVERFLOW)
    }

    fn read_u32(&self, region: &VramRegion, field: usize) -> Result<u32> {
        self.map
            .try_read32(self.io_offset(region, field, size_of::<u32>())?)
    }

    /// Return the physical regions occupied by the three plugin logs.
    pub(crate) fn plugin_logs(&self) -> Result<PluginLogRegions> {
        Ok(PluginLogRegions {
            init: self.init_log.clone(),
            vgpu: self.vgpu_log.clone(),
            kernel: self.kernel_log.clone(),
        })
    }

    /// Return whether firmware has published the plugin boot marker.
    pub(crate) fn is_plugin_ready(&self) -> Result<bool> {
        let value = self.read_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_seq_num),
        )?;

        Ok(value == bindings::GSP_PLUGIN_BOOTLOADED)
    }

    /// Invalidate the PTEs and release the communication mapping.
    pub(crate) fn destroy(self, bar_user: &BarUser<'gpu>, mm: &mut GpuMm<'_>) -> Result {
        self.map.destroy(bar_user, mm)
    }
}
