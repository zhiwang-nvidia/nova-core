// SPDX-License-Identifier: GPL-2.0

mod r000_00;

use r000_00 as r000;

use kernel::prelude::*;

use crate::mm::{
    bar_user::{
        Bar1Map,
        BarUser, //
    },
    vram::VramRegion,
    GpuMm, //
};

type RawControlRegion = r000::VGPU_CPU_GSP_CTRL_BUFF_REGION;

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
/// The firmware header defines each region size and the order in which the
/// regions appear. Field accesses still use BAR1 I/O accessors because
/// bindgen does not preserve C `volatile` semantics.
pub(crate) struct CommBufferRegion<'gpu> {
    map: Bar1Map<'gpu>,
    control: VramRegion,
    _response: VramRegion,
    _message: VramRegion,
    _migration: VramRegion,
    _error: VramRegion,
    init_log: VramRegion,
    vgpu_log: VramRegion,
    kernel_log: VramRegion,
    _guest_trace: VramRegion,
}

impl<'gpu> CommBufferRegion<'gpu> {
    /// Map the communication portion of a plugin management heap.
    pub(crate) fn new(
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
        management_heap: &VramRegion,
    ) -> Result<Self> {
        let total_size = u64::from(r000::VGPU_CPU_GSP_COMMUNICATION_BUFF_TOTAL_SIZE);
        let region = management_heap.subregion(0..total_size)?;
        let mut cursor = 0;

        let control = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_CTRL_BUFF_REGION_SIZE,
        )?;
        let response = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_RESPONSE_BUFF_REGION_SIZE,
        )?;
        let message = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_MESSAGE_BUFF_REGION_SIZE,
        )?;
        let migration = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_MIGRATION_BUFF_REGION_SIZE,
        )?;
        let error = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_ERROR_BUFF_REGION_SIZE,
        )?;
        let init_log = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_INIT_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        let vgpu_log = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_VGPU_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        let kernel_log = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_KERNEL_TASK_LOG_BUFF_REGION_SIZE,
        )?;
        let guest_trace = take_region(
            &region,
            &mut cursor,
            r000::VGPU_CPU_GSP_GUEST_RPC_TRACE_BUFF_REGION_SIZE,
        )?;

        if cursor != total_size || control.size() != u64::try_from(size_of::<RawControlRegion>())? {
            return Err(EINVAL);
        }

        let map = Bar1Map::new(bar_user, mm, region, true)?;

        Ok(Self {
            map,
            control,
            _response: response,
            _message: message,
            _migration: migration,
            _error: error,
            init_log,
            vgpu_log,
            kernel_log,
            _guest_trace: guest_trace,
        })
    }

    fn io_offset(&self, region: &VramRegion, field: usize, width: usize) -> Result<usize> {
        let field = u64::try_from(field).map_err(|_| EOVERFLOW)?;
        let width = u64::try_from(width).map_err(|_| EOVERFLOW)?;
        if field.checked_add(width).ok_or(EOVERFLOW)? > region.size() {
            return Err(EINVAL);
        }

        let region_offset = region
            .address()
            .checked_sub(self.map.region().address())
            .ok_or(EINVAL)?;
        usize::try_from(region_offset.checked_add(field).ok_or(EOVERFLOW)?).map_err(|_| EOVERFLOW)
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
        let offset = self.io_offset(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_seq_num),
            size_of::<u32>(),
        )?;

        Ok(self.map.try_read32(offset)? == r000::GSP_PLUGIN_BOOTLOADED)
    }

    /// Invalidate the PTEs and release the communication mapping.
    pub(crate) fn destroy(self, bar_user: &BarUser<'gpu>, mm: &GpuMm<'gpu>) -> Result {
        self.map.destroy(bar_user, mm)
    }
}
