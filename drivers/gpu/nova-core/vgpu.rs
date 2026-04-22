// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    devres::Devres,
    gpu::buddy::{
        AllocatedBlocks,
        GpuBuddyAllocFlags,
        GpuBuddyAllocMode, //
    },
    io::Io,
    pci,
    prelude::*,
    ptr::Alignment,
    sync::Arc, //
};

use crate::{
    driver::Bar1,
    gpu::Architecture,
    mm::GpuMm, //
};

/// VRAM allocation with RAII lifetime. Drop frees blocks back to buddy allocator.
pub(crate) struct VramBlock {
    _blocks: Pin<KBox<AllocatedBlocks>>,
    pub addr: u64,
    pub size: u64,
}

pub(crate) fn alloc_vram(mm: &GpuMm, size: u64, align: u64) -> Result<VramBlock> {
    let min_block_size =
        Alignment::new_checked(core::cmp::max(align, 4096) as usize).ok_or(EINVAL)?;
    let blocks = KBox::pin_init(
        mm.buddy().alloc_blocks(
            GpuBuddyAllocMode::Simple,
            size,
            min_block_size,
            GpuBuddyAllocFlags::default(),
        ),
        GFP_KERNEL,
    )?;
    let addr = blocks.as_ref().iter().next().ok_or(ENOMEM)?.offset();
    Ok(VramBlock {
        _blocks: blocks,
        addr,
        size,
    })
}

/// BAR1 sub-mapping with RAII lifetime.
pub(crate) struct Bar1Map {
    bar1: Arc<Devres<Bar1>>,
    pub offset: u64,
    #[expect(dead_code)]
    pub size: u64,
}

impl Bar1Map {
    pub fn new(bar1: &Arc<Devres<Bar1>>, offset: u64, size: u64) -> Result<Self> {
        Ok(Self {
            bar1: bar1.clone(),
            offset,
            size,
        })
    }

    pub fn read32(&self, dev: &device::Device<device::Bound>, off: u64) -> Result<u32> {
        let bar = self.bar1.access(dev)?;
        bar.try_read32((self.offset + off) as usize)
    }

    pub fn write32(
        &self,
        dev: &device::Device<device::Bound>,
        off: u64,
        val: u32,
    ) -> Result {
        let bar = self.bar1.access(dev)?;
        bar.try_write32(val, (self.offset + off) as usize)
    }
}

/// GSP-level configuration for vGPU resource sizing.
pub(crate) struct GspConfig {
    pub vmmu_segment_size: u64,
    pub total_avail_chids: u32,
    pub total_fbmem_size: u64,
}

impl Default for GspConfig {
    fn default() -> Self {
        Self {
            vmmu_segment_size: 0,
            total_avail_chids: 0,
            total_fbmem_size: 0,
        }
    }
}

// --- CommBuffLayout ---

const CTRL_SIZE: u64 = 4 * 1024;
const RESPONSE_SIZE: u64 = 4 * 1024;
const MESSAGE_SIZE: u64 = 4 * 1024;
const MIGRATION_SIZE: u64 = 2 * 1024 * 1024;
const ERROR_SIZE: u64 = 4 * 1024;
const INIT_LOG_SIZE: u64 = 128 * 1024;
const VGPU_LOG_SIZE: u64 = 256 * 1024;
const KERNEL_LOG_SIZE: u64 = 64 * 1024;

/// Communication buffer layout within the management heap.
pub(crate) struct CommBuffLayout {
    pub total_size: u64,
    pub init_task_log_offset: u64,
    pub init_task_log_size: u64,
    pub vgpu_task_log_size: u64,
    pub kernel_log_size: u64,
}

impl CommBuffLayout {
    pub fn calculate() -> Self {
        let init_task_log_offset =
            CTRL_SIZE + RESPONSE_SIZE + MESSAGE_SIZE + MIGRATION_SIZE + ERROR_SIZE;
        let total_size =
            init_task_log_offset + INIT_LOG_SIZE + VGPU_LOG_SIZE + KERNEL_LOG_SIZE;
        Self {
            total_size,
            init_task_log_offset,
            init_task_log_size: INIT_LOG_SIZE,
            vgpu_task_log_size: VGPU_LOG_SIZE,
            kernel_log_size: KERNEL_LOG_SIZE,
        }
    }
}

impl Default for CommBuffLayout {
    fn default() -> Self {
        Self::calculate()
    }
}

// --- Gfid / Dbdf ---

/// Guest Function ID. GFID 0 is reserved for PF, VFs start at 1.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Gfid(pub u32);

/// PCI address encoding: domain[31:16] bus[15:8] devfn[7:0].
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct Dbdf(pub u32);

// --- VgpuType ---

/// vGPU type descriptor, populated from QUERY_VGPU_PROPERTIES NVKV response.
pub(crate) struct VgpuType {
    pub name: [u8; 64],
    pub class: [u8; 64],
    pub vgpu_type_id: u32,
    pub bar1_length: u64,
    pub max_instance: u32,
    pub ecc_supported: u32,
    pub profile_size: u64,
    pub max_fps: u32,
    pub num_heads: u32,
    pub max_res_x: u32,
    pub max_res_y: u32,
    pub fb_length: u64,
    pub gsp_heap_size: u64,
    pub fb_reservation: u64,
}

impl Default for VgpuType {
    fn default() -> Self {
        Self {
            name: [0u8; 64],
            class: [0u8; 64],
            vgpu_type_id: 0,
            bar1_length: 0,
            max_instance: 0,
            ecc_supported: 0,
            profile_size: 0,
            max_fps: 0,
            num_heads: 0,
            max_res_x: 0,
            max_res_y: 0,
            fb_length: 0,
            gsp_heap_size: 0,
            fb_reservation: 0,
        }
    }
}

/// NVKV key constants for QUERY_VGPU_PROPERTIES response decoding.
pub(crate) mod vgpu_prop_keys {
    pub const TYPE_NAME: u16 = 0x3100;
    pub const CLASS: u16 = 0x3101;
    pub const TYPE_ID: u16 = 0x3102;
    pub const BAR1_LENGTH: u16 = 0x3103;
    pub const MAX_INSTANCE: u16 = 0x3104;
    pub const ECC: u16 = 0x3105;
    pub const PROFILE_SIZE: u16 = 0x3106;
    pub const MAX_FPS: u16 = 0x3107;
    pub const NUM_HEADS: u16 = 0x3108;
    pub const MAX_RES_X: u16 = 0x3109;
    pub const MAX_RES_Y: u16 = 0x310A;
}

// --- VgpuInstance ---

/// A live vGPU instance with allocated resources.
pub(crate) struct VgpuInstance {
    pub id: i32,
    pub gfid: Gfid,
    pub dbdf: Dbdf,
    pub vgpu_type: VgpuType,
    pub vm_pid: u32,
    pub chid_offset: u32,
    pub num_chid: u32,
    pub num_plugin_channels: u32,
    pub fbmem_heap: Option<VramBlock>,
    pub mgmt_heap: Option<VramBlock>,
    pub plugin_rpc: Option<PluginRpc>,
    pub active: bool,
}

pub(crate) mod gmcapi {
    pub const VGPU_MGMT_QUERY_PROPERTIES: u32 = 0x1000_0006;
    pub const VGPU_MGMT_QUERY_ASSIGNED_VF: u32 = 0x1000_0007;

    pub const VGPU_SHUTDOWN: u32 = 0x1000_0021;
    pub const VGPU_SHUTDOWN_COMPLETE: u32 = 0x1000_0022;
    pub const VGPU_CLEANUP: u32 = 0x1000_0023;

    pub const VGPU_BOOTLOAD: u32 = 0x1000_0020;
}

/// Plugin RPC channel for vGPU plugin communication.
pub(crate) struct PluginRpc {
    bar1_map: Bar1Map,
    ctrl_off: usize,
    resp_off: usize,
    msg_off: usize,
    msg_seq_num: u32,
}

const GSP_PLUGIN_BOOTLOADED: u32 = 0x4E65_4A6F;
const CTRL_BUF_MSG_SEQ_NUM_OFFSET: u64 = 8;
const PLUGIN_BOOT_TIMEOUT_MS: u64 = 10_000;

impl PluginRpc {
    pub fn new(bar1_map: Bar1Map, _comm_layout: &CommBuffLayout) -> Self {
        Self {
            bar1_map,
            ctrl_off: 0,
            resp_off: CTRL_SIZE as usize,
            msg_off: (CTRL_SIZE + RESPONSE_SIZE) as usize,
            msg_seq_num: 0,
        }
    }

    /// Poll ctrl_buf for plugin boot completion magic.
    pub fn wait_plugin_ready(&self, dev: &device::Device<device::Bound>) -> Result {
        use kernel::time::{delay::fsleep, Delta, Instant, Monotonic};

        let start = Instant::<Monotonic>::now();
        let timeout = Delta::from_millis(PLUGIN_BOOT_TIMEOUT_MS as i64);
        loop {
            let val = self.bar1_map.read32(dev, CTRL_BUF_MSG_SEQ_NUM_OFFSET)?;
            if val == GSP_PLUGIN_BOOTLOADED {
                return Ok(());
            }
            if start.elapsed() > timeout {
                return Err(ETIMEDOUT);
            }
            fsleep(Delta::from_millis(1));
        }
    }
}

// --- EngineBitmap ---

const NV2080_GPU_MAX_ENGINES: usize = 84;
const ENGINE_BITMAP_WORDS: usize = 2;

/// 96-bit engine capability bitmap built from GspStaticConfigInfo.engineCaps.
pub(crate) struct EngineBitmap {
    pub bitmap: [u64; ENGINE_BITMAP_WORDS],
}

impl EngineBitmap {
    pub fn new() -> Self {
        Self { bitmap: [0; ENGINE_BITMAP_WORDS] }
    }

    pub fn from_caps(caps: &[u32; 3]) -> Self {
        Self {
            bitmap: [
                u64::from(caps[1]) << 32 | u64::from(caps[0]),
                u64::from(caps[2]),
            ],
        }
    }

    pub fn has_engine(&self, idx: usize) -> bool {
        if idx >= NV2080_GPU_MAX_ENGINES {
            return false;
        }
        let word = idx / 64;
        let bit = idx % 64;
        self.bitmap[word] & (1u64 << bit) != 0
    }
}

/// Unified vGPU manager.
///
/// On creation, performs platform detection to determine whether vGPU is
/// requested (PRC knob + totalvfs for Blackwell). The `vgpu_requested`
/// flag may be further refined during boot (e.g. FSP PRC knob read).
pub(crate) struct VgpuManager {
    pub(crate) vgpu_requested: bool,
    pub(crate) vgpu_enabled: bool,
    pub(crate) total_vfs: u16,
    pub(crate) gsp_config: GspConfig,
    pub(crate) comm_layout: CommBuffLayout,
    pub(crate) engine_bitmap: EngineBitmap,
    pub(crate) instances: KVec<VgpuInstance>,
}

impl VgpuManager {
    pub(crate) fn new(
        pdev: &pci::Device<device::Core>,
        arch: Architecture,
    ) -> Result<VgpuManager> {
        let total_vfs: u16 = if arch.supports_vgpu() {
            pdev.sriov_get_totalvfs()
                .ok()
                .and_then(|n| n.try_into().ok())
                .unwrap_or(0)
        } else {
            0
        };

        Ok(VgpuManager {
            vgpu_requested: total_vfs > 0,
            vgpu_enabled: false,
            total_vfs,
            gsp_config: GspConfig::default(),
            comm_layout: CommBuffLayout::default(),
            engine_bitmap: EngineBitmap::new(),
            instances: KVec::new(),
        })
    }

    pub(crate) fn set_vgpu_enabled(&mut self, enabled: bool) {
        self.vgpu_enabled = enabled;
    }

    fn next_id(&mut self) -> i32 {
        let id = self.instances.len() as i32;
        id
    }

    /// One-time initialization after GSP boot completes.
    pub(crate) fn init_post_gsp_boot(
        &mut self,
        engine_caps: &[u32; 3],
        total_vram: u64,
    ) -> Result {
        self.gsp_config.vmmu_segment_size = 512 * 1024 * 1024; // 512MB for Blackwell
        self.gsp_config.total_fbmem_size = total_vram;
        self.gsp_config.total_avail_chids = 2048;
        self.engine_bitmap = EngineBitmap::from_caps(engine_caps);
        self.comm_layout = CommBuffLayout::calculate();
        Ok(())
    }
}

/// Channel ID allocator using a bitmap over 2048 channels.
pub(crate) struct ChidAllocator {
    bitmap: [u64; 32],
    total: u32,
}

impl ChidAllocator {
    pub fn new(total: u32) -> Self {
        Self {
            bitmap: [0u64; 32],
            total,
        }
    }

    /// Allocate `count` contiguous channels, aligned to `count` boundary.
    pub fn alloc(&mut self, count: u32) -> Result<u32> {
        if count == 0 {
            return Err(EINVAL);
        }
        let stride = count as usize;
        let total_bits = self.bitmap.len() * 64;
        let mut offset = 0usize;

        while offset + stride <= total_bits {
            if self.is_range_free(offset, stride) {
                self.set_range(offset, stride);
                return Ok(offset as u32);
            }
            offset += stride;
        }
        Err(ENOSPC)
    }

    /// Free `count` channels starting at `offset`.
    pub fn free(&mut self, offset: u32, count: u32) {
        let start = offset as usize;
        for i in start..start + count as usize {
            let word = i / 64;
            let bit = i % 64;
            self.bitmap[word] &= !(1u64 << bit);
        }
    }

    fn is_range_free(&self, start: usize, count: usize) -> bool {
        for i in start..start + count {
            let word = i / 64;
            let bit = i % 64;
            if self.bitmap[word] & (1u64 << bit) != 0 {
                return false;
            }
        }
        true
    }

    fn set_range(&mut self, start: usize, count: usize) {
        for i in start..start + count {
            let word = i / 64;
            let bit = i % 64;
            self.bitmap[word] |= 1u64 << bit;
        }
    }
}

/// Round down to the nearest power of 2.
pub(crate) fn prev_pow2(x: u32) -> u32 {
    if x == 0 {
        return 0;
    }
    1 << (31 - x.leading_zeros())
}


use crate::gsp::{
    cmdq::Cmdq,
    nvkv::{
        self,
        NvkvValue, //
    },
};
use crate::driver::Bar0;

/// Query the vGPU type assigned to a VF by its DBDF.
pub(crate) fn query_assigned_vf_type(cmdq: &Cmdq, bar: &Bar0, dbdf: Dbdf) -> Result<u32> {
    let in_params = dbdf.0.to_le_bytes();
    let resp = cmdq.send_gmc_and_receive(
        bar,
        gmcapi::VGPU_MGMT_QUERY_ASSIGNED_VF,
        &in_params,
        4,
    )?;
    if resp.payload.len() < 4 {
        return Err(EINVAL);
    }
    Ok(u32::from_le_bytes(
        resp.payload[..4].try_into().map_err(|_| EINVAL)?,
    ))
}

/// Query vGPU type properties and decode NVKV response.
pub(crate) fn query_vgpu_type(cmdq: &Cmdq, bar: &Bar0, type_id: u32) -> Result<VgpuType> {
    let in_params = type_id.to_le_bytes();
    let resp = cmdq.send_gmc_and_receive(
        bar,
        gmcapi::VGPU_MGMT_QUERY_PROPERTIES,
        &in_params,
        4096,
    )?;

    let mut vt = VgpuType::default();
    nvkv::nvkv_decode(&resp.payload, |key, value| match key {
        vgpu_prop_keys::TYPE_NAME => nvkv::nvkv_read_string8(&value, &mut vt.name),
        vgpu_prop_keys::CLASS => nvkv::nvkv_read_string8(&value, &mut vt.class),
        vgpu_prop_keys::TYPE_ID => vt.vgpu_type_id = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::BAR1_LENGTH => vt.bar1_length = nvkv::nvkv_read_u64(&value),
        vgpu_prop_keys::MAX_INSTANCE => vt.max_instance = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::ECC => vt.ecc_supported = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::PROFILE_SIZE => vt.profile_size = nvkv::nvkv_read_u64(&value),
        vgpu_prop_keys::MAX_FPS => vt.max_fps = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::NUM_HEADS => vt.num_heads = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::MAX_RES_X => vt.max_res_x = nvkv::nvkv_read_u32(&value),
        vgpu_prop_keys::MAX_RES_Y => vt.max_res_y = nvkv::nvkv_read_u32(&value),
        _ => {}
    })?;

    Ok(vt)
}


impl VgpuManager {
    /// Create a new vGPU instance with allocated resources.
    pub(crate) fn create_instance(
        &mut self,
        mm: &GpuMm,
        bar1: &Arc<Devres<Bar1>>,
        chid_alloc: &mut ChidAllocator,
        gfid: Gfid,
        dbdf: Dbdf,
        vgpu_type: VgpuType,
        vm_pid: u32,
    ) -> Result<VgpuInstance> {
        let num_chid = prev_pow2(
            self.gsp_config.total_avail_chids / vgpu_type.max_instance.max(1),
        );
        let chid_offset = chid_alloc.alloc(num_chid)?;

        let fb_size = vgpu_type.fb_length;
        let fb_align = self.gsp_config.vmmu_segment_size;
        let fbmem = alloc_vram(mm, fb_size, fb_align)
            .inspect_err(|_| chid_alloc.free(chid_offset, num_chid))?;

        let mgmt = alloc_vram(mm, vgpu_type.gsp_heap_size.max(self.comm_layout.total_size), 4096)
            .inspect_err(|_| chid_alloc.free(chid_offset, num_chid))?;

        let bar1_map = Bar1Map::new(bar1, mgmt.addr, self.comm_layout.total_size)?;
        let plugin_rpc = PluginRpc::new(bar1_map, &self.comm_layout);

        Ok(VgpuInstance {
            id: self.next_id(),
            gfid,
            dbdf,
            vgpu_type,
            vm_pid,
            chid_offset,
            num_chid,
            num_plugin_channels: 3,
            fbmem_heap: Some(fbmem),
            mgmt_heap: Some(mgmt),
            plugin_rpc: Some(plugin_rpc),
            active: false,
        })
    }

    /// Destroy a vGPU instance by GFID: shutdown, cleanup, free resources.
    pub(crate) fn destroy_instance(
        &mut self,
        cmdq: &Cmdq,
        bar: &Bar0,
        chid_alloc: &mut ChidAllocator,
        gfid: Gfid,
    ) -> Result {
        let idx = self
            .instances
            .iter()
            .position(|i| i.gfid == gfid)
            .ok_or(ENOENT)?;

        cmdq.send_gmc_fire_and_forget(
            bar,
            gmcapi::VGPU_SHUTDOWN,
            &gfid.0.to_le_bytes(),
        )?;

        cmdq.wait_gmc_event(kernel::time::Delta::from_secs(10), |cmd_id, payload| {
            cmd_id == gmcapi::VGPU_SHUTDOWN_COMPLETE
                && payload.len() >= 4
                && u32::from_le_bytes(payload[..4].try_into().unwrap_or([0; 4])) == gfid.0
        })?;

        cmdq.send_gmc_no_response(bar, gmcapi::VGPU_CLEANUP, &gfid.0.to_le_bytes())?;

        let instance = self.instances.remove(idx).map_err(|_| EINVAL)?;
        chid_alloc.free(instance.chid_offset, instance.num_chid);

        Ok(())
    }
}


mod bootload_keys {
    pub const DBDF: u16 = 0x0001;
    pub const GFID: u16 = 0x0002;
    pub const VGPU_TYPE: u16 = 0x0003;
    pub const VM_PID: u16 = 0x0004;
    #[expect(dead_code)]
    pub const SWIZZ_ID: u16 = 0x0005;
    pub const NUM_CHANNELS: u16 = 0x0006;
    pub const NUM_PLUGIN_CHANNELS: u16 = 0x0007;
    pub const GUEST_FB_SEGMENT_COUNT: u16 = 0x0008;

    pub const CHANNEL_MAPPING: u16 = 0x1001;
    pub const GUEST_FB_SEGMENT_PHYS_ADDR: u16 = 0x1002;
    pub const GUEST_FB_SEGMENT_LENGTH: u16 = 0x1003;
    pub const PLUGIN_HEAP_PHYS_ADDR: u16 = 0x1004;
    pub const PLUGIN_HEAP_LENGTH: u16 = 0x1005;
    pub const CTRL_BUFF_OFFSET: u16 = 0x1006;
    pub const INIT_TASK_LOG_OFFSET: u16 = 0x1007;
    pub const INIT_TASK_LOG_SIZE: u16 = 0x1008;
    pub const VGPU_TASK_LOG_OFFSET: u16 = 0x1009;
    pub const VGPU_TASK_LOG_SIZE: u16 = 0x100A;
    pub const KERNEL_LOG_OFFSET: u16 = 0x100B;
    pub const KERNEL_LOG_SIZE: u16 = 0x100C;
}

// GMC engine type constants (ABI-stable)
const NVGMC_ENGINE_GR0: u64 = 0;
const NVGMC_ENGINE_CE0: u64 = 0x10;

/// Convert NV2080 engine index to GMC engine type.
fn nv2080_to_gmc_engine(idx: usize) -> Option<u64> {
    match idx {
        0 => Some(NVGMC_ENGINE_GR0),
        1..=7 => Some(NVGMC_ENGINE_CE0 + (idx - 1) as u64),
        _ => None,
    }
}

/// Send GMCAPI VGPU_BOOTLOAD with NVKV-encoded parameters.
pub(crate) fn bootload_plugin(
    cmdq: &Cmdq,
    bar: &Bar0,
    instance: &VgpuInstance,
    engine_bitmap: &EngineBitmap,
    comm_layout: &CommBuffLayout,
) -> Result {
    let mut kvs: KVec<u64> = KVec::new();

    nvkv::nvkv_push_imm32(&mut kvs, bootload_keys::DBDF, instance.dbdf.0)?;
    nvkv::nvkv_push_imm32(&mut kvs, bootload_keys::GFID, instance.gfid.0)?;
    nvkv::nvkv_push_imm32(
        &mut kvs,
        bootload_keys::VGPU_TYPE,
        instance.vgpu_type.vgpu_type_id,
    )?;
    nvkv::nvkv_push_imm32(&mut kvs, bootload_keys::VM_PID, instance.vm_pid)?;
    nvkv::nvkv_push_imm32(&mut kvs, bootload_keys::NUM_CHANNELS, instance.num_chid)?;
    nvkv::nvkv_push_imm32(
        &mut kvs,
        bootload_keys::NUM_PLUGIN_CHANNELS,
        instance.num_plugin_channels,
    )?;

    // Channel mapping: [gmc_engine_id, chid_offset] pairs for active engines
    let mut channel_map: KVec<u64> = KVec::new();
    for i in 0..NV2080_GPU_MAX_ENGINES {
        if engine_bitmap.has_engine(i) {
            if let Some(gmc_id) = nv2080_to_gmc_engine(i) {
                channel_map.push(gmc_id, GFP_KERNEL)?;
                channel_map.push(u64::from(instance.chid_offset), GFP_KERNEL)?;
            }
        }
    }
    nvkv::nvkv_push_seq64(&mut kvs, bootload_keys::CHANNEL_MAPPING, channel_map.as_slice())?;

    // Guest FB segments
    let fb = instance.fbmem_heap.as_ref().ok_or(EINVAL)?;
    nvkv::nvkv_push_imm32(&mut kvs, bootload_keys::GUEST_FB_SEGMENT_COUNT, 1)?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::GUEST_FB_SEGMENT_PHYS_ADDR,
        &[fb.addr],
    )?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::GUEST_FB_SEGMENT_LENGTH,
        &[fb.size],
    )?;

    // Plugin heap
    let mgmt = instance.mgmt_heap.as_ref().ok_or(EINVAL)?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::PLUGIN_HEAP_PHYS_ADDR,
        &[mgmt.addr],
    )?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::PLUGIN_HEAP_LENGTH,
        &[mgmt.size],
    )?;

    // Log buffer offsets
    nvkv::nvkv_push_seq64(&mut kvs, bootload_keys::CTRL_BUFF_OFFSET, &[0u64])?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::INIT_TASK_LOG_OFFSET,
        &[comm_layout.init_task_log_offset],
    )?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::INIT_TASK_LOG_SIZE,
        &[comm_layout.init_task_log_size],
    )?;
    let vgpu_log_offset =
        comm_layout.init_task_log_offset + comm_layout.init_task_log_size;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::VGPU_TASK_LOG_OFFSET,
        &[vgpu_log_offset],
    )?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::VGPU_TASK_LOG_SIZE,
        &[comm_layout.vgpu_task_log_size],
    )?;
    let kernel_log_offset = vgpu_log_offset + comm_layout.vgpu_task_log_size;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::KERNEL_LOG_OFFSET,
        &[kernel_log_offset],
    )?;
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::KERNEL_LOG_SIZE,
        &[comm_layout.kernel_log_size],
    )?;

    // Convert u64 kvs to byte slice for GMC API
    let payload: &[u8] = unsafe {
        core::slice::from_raw_parts(kvs.as_ptr() as *const u8, kvs.len() * 8)
    };
    cmdq.send_gmc_no_response(bar, gmcapi::VGPU_BOOTLOAD, payload)
}
