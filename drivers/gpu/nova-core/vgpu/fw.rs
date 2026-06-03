// SPDX-License-Identifier: GPL-2.0

mod commands;
mod r000_00;

pub(crate) use commands::{
    RpcMessage,
    RpcResponse, //
};

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
type RawResponseRegion = r000::VGPU_CPU_GSP_RESPONSE_BUFF_REGION;

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
    response: VramRegion,
    message: VramRegion,
    migration: VramRegion,
    error: VramRegion,
    init_log: VramRegion,
    vgpu_log: VramRegion,
    kernel_log: VramRegion,
    guest_trace: VramRegion,
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

        if cursor != total_size
            || control.size() != u64::try_from(size_of::<RawControlRegion>())?
            || response.size() != u64::try_from(size_of::<RawResponseRegion>())?
        {
            return Err(EINVAL);
        }

        let map = Bar1Map::new(bar_user, mm, region, true)?;

        Ok(Self {
            map,
            control,
            response,
            message,
            migration,
            error,
            init_log,
            vgpu_log,
            kernel_log,
            guest_trace,
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

    fn write_u8(&self, region: &VramRegion, field: usize, value: u8) -> Result {
        self.map
            .try_write8(value, self.io_offset(region, field, size_of::<u8>())?)
    }

    fn write_u32(&self, region: &VramRegion, field: usize, value: u32) -> Result {
        self.map
            .try_write32(value, self.io_offset(region, field, size_of::<u32>())?)
    }

    fn write_u64(&self, region: &VramRegion, field: usize, value: u64) -> Result {
        self.map
            .try_write64(value, self.io_offset(region, field, size_of::<u64>())?)
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

        Ok(value == r000::GSP_PLUGIN_BOOTLOADED)
    }

    /// Initialize the shared control and response buffers for plugin RPC.
    pub(crate) fn initialize(&self) -> Result {
        self.write_u64(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.response_buff_offset),
            u64::try_from(self.region_offset(&self.response)?)?,
        )?;
        self.write_u64(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_buff_offset),
            u64::try_from(self.region_offset(&self.message)?)?,
        )?;
        self.write_u64(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.migration_buff_offset),
            u64::try_from(self.region_offset(&self.migration)?)?,
        )?;
        self.write_u64(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.error_buff_offset),
            u64::try_from(self.region_offset(&self.error)?)?,
        )?;
        self.write_u64(
            &self.control,
            core::mem::offset_of!(
                RawControlRegion,
                __bindgen_anon_1.guest_rpc_trace_buff_offset
            ),
            u64::try_from(self.region_offset(&self.guest_trace)?)?,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(
                RawControlRegion,
                __bindgen_anon_1.migration_buf_cpu_access_offset
            ),
            0,
        )?;
        self.write_u8(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.is_migration_in_progress),
            0,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.error_buff_cpu_get_idx),
            0,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(
                RawControlRegion,
                __bindgen_anon_1.guest_rpc_trace_buff_cpu_get_idx
            ),
            0,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.attached_vgpu_count),
            1,
        )?;
        self.write_u8(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.is_gr_init_done),
            0,
        )?;

        // The heap is not guaranteed to have been zeroed. Clear both sides'
        // sequence state before publishing the control-buffer version.
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_type),
            0,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_seq_num),
            0,
        )?;
        self.write_u32(
            &self.response,
            core::mem::offset_of!(
                RawResponseRegion,
                __bindgen_anon_1.message_seq_num_processed
            ),
            0,
        )?;
        self.write_u32(
            &self.response,
            core::mem::offset_of!(RawResponseRegion, __bindgen_anon_1.result_code),
            0,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.version),
            r000::VGPU_CPU_GSP_CTRL_BUFF_VERSION,
        )
    }

    /// Copy and publish one RPC request to firmware.
    pub(crate) fn submit(&self, message: RpcMessage, sequence: u32, data: &[u8]) -> Result {
        if u64::try_from(data.len()).map_err(|_| EOVERFLOW)? > self.message.size() {
            return Err(E2BIG);
        }

        for (index, chunk) in data.chunks(size_of::<u32>()).enumerate() {
            let mut bytes = [0u8; size_of::<u32>()];
            bytes[..chunk.len()].copy_from_slice(chunk);
            let field = index.checked_mul(size_of::<u32>()).ok_or(EOVERFLOW)?;
            self.write_u32(&self.message, field, u32::from_le_bytes(bytes))?;
        }

        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_type),
            message as u32,
        )?;
        self.write_u32(
            &self.control,
            core::mem::offset_of!(RawControlRegion, __bindgen_anon_1.message_seq_num),
            sequence,
        )
    }

    /// Read firmware's response for an expected RPC sequence.
    pub(crate) fn response(&self, expected_sequence: u32) -> Result<RpcResponse> {
        let sequence = self.read_u32(
            &self.response,
            core::mem::offset_of!(
                RawResponseRegion,
                __bindgen_anon_1.message_seq_num_processed
            ),
        )?;
        if sequence != expected_sequence {
            return Ok(RpcResponse::Pending { sequence });
        }

        let status = self.read_u32(
            &self.response,
            core::mem::offset_of!(RawResponseRegion, __bindgen_anon_1.result_code),
        )?;
        Ok(RpcResponse::Complete { status })
    }

    /// Invalidate the PTEs and release the communication mapping.
    pub(crate) fn destroy(self, bar_user: &BarUser<'gpu>, mm: &GpuMm<'gpu>) -> Result {
        self.map.destroy(bar_user, mm)
    }
}
