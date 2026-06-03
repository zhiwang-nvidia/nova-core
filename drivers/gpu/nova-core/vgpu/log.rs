// SPDX-License-Identifier: GPL-2.0

use kernel::{
    debugfs,
    fs::file,
    prelude::*,
    uaccess::UserSliceWriter, //
};

use crate::{
    firmware::BuildId,
    gpu::Chipset,
    gsp::{
        build_log_buffer_header,
        LOG_BUFFER_HEADER_SIZE, //
    },
    vgpu::fw::{
        MappedPluginLogBuffer,
        MappedPluginLogBuffers, //
    }, //
};

const LOG_READ_CHUNK_SIZE: usize = 4096;

/// A single vGPU plugin log buffer backed by VRAM, read via BAR1 MMIO.
///
/// The GSP plugin writes encoded log entries into the management heap in
/// VRAM. The mapped buffer retains revocable access to those bytes without a
/// device reference.
///
/// A [`LOG_BUFFER_HEADER_SIZE`]-byte header is prepended so that
/// `nvlog_decoder` can identify the GPU architecture and firmware build.
pub(crate) struct VgpuLogBuffer {
    buffer: MappedPluginLogBuffer,
    header: [u8; LOG_BUFFER_HEADER_SIZE],
    header_len: usize,
}

impl VgpuLogBuffer {
    fn new(
        buffer: MappedPluginLogBuffer,
        chipset: Chipset,
        build_id: Option<&BuildId>,
        task_prefix: &str,
    ) -> Result<Self> {
        let (header, header_len) = match build_id {
            Some(bid) => (
                build_log_buffer_header(chipset, bid, task_prefix),
                LOG_BUFFER_HEADER_SIZE,
            ),
            None => ([0u8; LOG_BUFFER_HEADER_SIZE], 0),
        };

        Ok(Self {
            buffer,
            header,
            header_len,
        })
    }
}

impl debugfs::BinaryWriter for VgpuLogBuffer {
    fn write_to_slice(
        &self,
        writer: &mut UserSliceWriter,
        offset: &mut file::Offset,
    ) -> Result<usize> {
        if offset.is_negative() {
            return Err(EINVAL);
        }

        let offset_val: usize = (*offset).try_into().map_err(|_| EINVAL)?;
        let total_len = self
            .header_len
            .checked_add(self.buffer.size())
            .ok_or(EOVERFLOW)?;

        if offset_val >= total_len {
            return Ok(0);
        }

        let count = (total_len - offset_val).min(writer.len());
        if count == 0 {
            return Ok(0);
        }

        // Keep the staging buffer on the heap to avoid putting a page-sized
        // object on the kernel stack.
        let staging_size = count.min(LOG_READ_CHUNK_SIZE);
        let mut staging = KVec::new();
        staging.resize(staging_size, 0, GFP_KERNEL)?;

        let mut written = 0usize;
        while written < count {
            let chunk_len = (count - written).min(staging.len());
            let chunk = &mut staging[..chunk_len];
            let chunk_offset = offset_val.checked_add(written).ok_or(EOVERFLOW)?;
            let mut filled = 0usize;

            if chunk_offset < self.header_len {
                let header_len = (self.header_len - chunk_offset).min(chunk_len);
                chunk[..header_len]
                    .copy_from_slice(&self.header[chunk_offset..chunk_offset + header_len]);
                filled = header_len;
            }

            if filled < chunk_len {
                let log_offset = chunk_offset
                    .checked_add(filled)
                    .ok_or(EOVERFLOW)?
                    .checked_sub(self.header_len)
                    .ok_or(EINVAL)?;

                // The mapped buffer drops its revocable access guard before
                // the potentially sleeping userspace copy below.
                self.buffer.read(log_offset, &mut chunk[filled..])?;
            }

            writer.write_slice(chunk)?;
            written = written.checked_add(chunk_len).ok_or(EOVERFLOW)?;
        }

        *offset = (*offset)
            .checked_add(i64::try_from(written).map_err(|_| EOVERFLOW)?)
            .ok_or(EOVERFLOW)?;
        Ok(written)
    }
}

/// Aggregated log buffers for a single vGPU instance.
///
/// Each vGPU plugin produces three log streams within the management heap:
/// - `init_log`: init task log (128 KB)
/// - `vgpu_log`: vGPU task log (256 KB)
/// - `kernel_log`: kernel task log (64 KB)
pub(crate) struct VgpuLogBuffers {
    init_log: VgpuLogBuffer,
    vgpu_log: VgpuLogBuffer,
    kernel_log: VgpuLogBuffer,
}

impl VgpuLogBuffers {
    pub(crate) fn new(
        buffers: MappedPluginLogBuffers,
        chipset: Chipset,
        build_id: Option<&BuildId>,
    ) -> Result<Self> {
        let (init, vgpu, kernel) = buffers.into_parts();

        Ok(Self {
            init_log: VgpuLogBuffer::new(init, chipset, build_id, "INIT")?,
            vgpu_log: VgpuLogBuffer::new(vgpu, chipset, build_id, "VGPU")?,
            kernel_log: VgpuLogBuffer::new(kernel, chipset, build_id, "KERN")?,
        })
    }

    /// Register debugfs binary files for these log buffers within a scoped directory.
    pub(crate) fn register_debugfs<'data, 'dir>(
        logs: &'data VgpuLogBuffers,
        dir: &'dir debugfs::ScopedDir<'data, 'dir>,
    ) {
        dir.read_binary_file(c"init_log", &logs.init_log);
        dir.read_binary_file(c"vgpu_log", &logs.vgpu_log);
        dir.read_binary_file(c"kernel_log", &logs.kernel_log);
    }
}
