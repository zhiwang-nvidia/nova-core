// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! Wire types and codecs for vGPU commands.
//!
//! Defines request encoders and response schemas independently of command
//! submission and instance lifecycle operations.

use kernel::{
    alloc::ArrayVec,
    bitfield,
    prelude::*, //
};

use crate::gsp::nvkv::{
    nvkv_decode,
    nvkv_encode,
    Array,
    Encodable,
    EncodedStream,
    Encoder,
    Index,
    Key,
    KeyId,
    Required, //
};

use super::bindings;

/// Message types supported by the nova-core plugin RPC channel.
#[derive(Clone, Copy)]
#[repr(u32)]
pub(in crate::vgpu) enum RpcMessage {
    VersionNegotiation = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_VERSION_NEGOTIATION,
    SetupConfigParamsAndInit = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_SETUP_CONFIG_PARAMS_AND_INIT,
    Reset = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_RESET,
    UpdateBmeState = bindings::MESSAGE_NV_VGPU_CPU_RPC_MSG_UPDATE_BME_STATE,
}

bitfield! {
    pub(in crate::vgpu) struct Dbdf(u32) {
        2:0 function;
        7:3 device;
        15:8 bus;
        31:16 domain;
    }
}

#[derive(Clone, Copy)]
struct SwizzId(u32);

impl SwizzId {
    const WHOLE_GPU: Self = Self(0xFFFF_FFFF);
}

impl From<SwizzId> for u32 {
    fn from(value: SwizzId) -> Self {
        value.0
    }
}

bitfield! {
    pub(in crate::vgpu) struct ChannelMapEntry(u64) {
        15:0 engine_type;
        31:16 index;
        63:32 chid_offset;
    }
}

impl ChannelMapEntry {
    const KEY: KeyId = 0x1001;

    pub(in crate::vgpu) fn new(engine_type: usize, index: u32, chid_offset: u32) -> Result<Self> {
        Self::zeroed()
            .try_with_engine_type(u64::try_from(engine_type).map_err(|_| EOVERFLOW)?)
            .and_then(|entry| entry.try_with_index(u64::from(index)))
            .and_then(|entry| entry.try_with_chid_offset(u64::from(chid_offset)))
    }
}

impl Encodable for KVVec<ChannelMapEntry> {
    fn encode(&self, encoder: &mut Encoder) -> Result {
        // SAFETY: `ChannelMapEntry` is a `bitfield!` over `u64`, i.e.
        // `#[repr(transparent)]` around a `u64`, so the entries are
        // layout-compatible with `u64` and can be viewed as a `u64` slice.
        let slice = unsafe { core::slice::from_raw_parts(self.as_ptr().cast::<u64>(), self.len()) };
        encoder.encode_array64(ChannelMapEntry::KEY, Index::new::<0>(), slice)
    }
}

bitfield! {
    struct VgpuBootloadOptions(u64) {
    }
}

nvkv_encode! {
    struct VgpuBootloadRequest {
        dbdf: Key<Dbdf, { Self::DBDF_KEY }, u32>,
        gfid: Key<u32, { Self::GFID_KEY }>,
        vgpu_type: Key<u32, { Self::VGPU_TYPE_KEY }>,
        vm_pid: Key<u32, { Self::VM_PID_KEY }>,
        swizz_id: Key<SwizzId, { Self::SWIZZ_ID_KEY }, u32>,
        num_channels: Key<u32, { Self::NUM_CHANNELS_KEY }>,
        num_plugin_channels: Key<u32, { Self::NUM_PLUGIN_CHANNELS_KEY }>,
        guest_fb_segment_count: Key<u32, { Self::GUEST_FB_SEGMENT_COUNT_KEY }>,
        options: Key<VgpuBootloadOptions, { Self::OPTIONS_KEY }, u64>,
        channel_mapping: KVVec<ChannelMapEntry>,
        guest_fb_segment_phys_addr: Array<u64, 8, { Self::GUEST_FB_SEGMENT_PHYS_ADDR_KEY }>,
        guest_fb_segment_length: Array<u64, 8, { Self::GUEST_FB_SEGMENT_LENGTH_KEY }>,
        plugin_heap_phys_addr: Key<u64, { Self::PLUGIN_HEAP_PHYS_ADDR_KEY }>,
        plugin_heap_length: Key<u64, { Self::PLUGIN_HEAP_LENGTH_KEY }>,
        ctrl_buff_offset: Key<u64, { Self::CTRL_BUFF_OFFSET_KEY }>,
        init_task_log_offset: Key<u64, { Self::INIT_TASK_LOG_OFFSET_KEY }>,
        init_task_log_size: Key<u64, { Self::INIT_TASK_LOG_SIZE_KEY }>,
        vgpu_task_log_offset: Key<u64, { Self::VGPU_TASK_LOG_OFFSET_KEY }>,
        vgpu_task_log_size: Key<u64, { Self::VGPU_TASK_LOG_SIZE_KEY }>,
        kernel_log_offset: Key<u64, { Self::KERNEL_LOG_OFFSET_KEY }>,
        kernel_log_size: Key<u64, { Self::KERNEL_LOG_SIZE_KEY }>,
        mig_rm_heap_phys_addr: Key<u64, { Self::MIG_RM_HEAP_PHYS_ADDR_KEY }>,
        mig_rm_heap_length: Key<u64, { Self::MIG_RM_HEAP_LENGTH_KEY }>,
    }
}

impl VgpuBootloadRequest {
    const DBDF_KEY: KeyId = 0x0001;
    const GFID_KEY: KeyId = 0x0002;
    const VGPU_TYPE_KEY: KeyId = 0x0003;
    const VM_PID_KEY: KeyId = 0x0004;
    const SWIZZ_ID_KEY: KeyId = 0x0005;
    const NUM_CHANNELS_KEY: KeyId = 0x0006;
    const NUM_PLUGIN_CHANNELS_KEY: KeyId = 0x0007;
    const GUEST_FB_SEGMENT_COUNT_KEY: KeyId = 0x0008;
    const OPTIONS_KEY: KeyId = 0x1000;
    const GUEST_FB_SEGMENT_PHYS_ADDR_KEY: KeyId = 0x1002;
    const GUEST_FB_SEGMENT_LENGTH_KEY: KeyId = 0x1003;
    const PLUGIN_HEAP_PHYS_ADDR_KEY: KeyId = 0x1004;
    const PLUGIN_HEAP_LENGTH_KEY: KeyId = 0x1005;
    const CTRL_BUFF_OFFSET_KEY: KeyId = 0x1006;
    const INIT_TASK_LOG_OFFSET_KEY: KeyId = 0x1007;
    const INIT_TASK_LOG_SIZE_KEY: KeyId = 0x1008;
    const VGPU_TASK_LOG_OFFSET_KEY: KeyId = 0x1009;
    const VGPU_TASK_LOG_SIZE_KEY: KeyId = 0x100A;
    const KERNEL_LOG_OFFSET_KEY: KeyId = 0x100B;
    const KERNEL_LOG_SIZE_KEY: KeyId = 0x100C;
    const MIG_RM_HEAP_PHYS_ADDR_KEY: KeyId = 0x100D;
    const MIG_RM_HEAP_LENGTH_KEY: KeyId = 0x100E;
}

/// Encodes a `VGPU_BOOTLOAD` request using the typed NVKV schema.
#[expect(clippy::too_many_arguments)]
pub(in crate::vgpu) fn encode_vgpu_bootload(
    dbdf: Dbdf,
    gfid: u32,
    vgpu_type: u32,
    vm_pid: u32,
    num_channels: u32,
    num_plugin_channels: u32,
    channel_mapping: KVVec<ChannelMapEntry>,
    guest_fb_address: u64,
    guest_fb_length: u64,
    plugin_heap_address: u64,
    plugin_heap_length: u64,
    ctrl_buffer_offset: u64,
    init_log_address: u64,
    init_log_size: u64,
    vgpu_log_address: u64,
    vgpu_log_size: u64,
    kernel_log_address: u64,
    kernel_log_size: u64,
) -> Result<EncodedStream> {
    let request = VgpuBootloadRequest {
        dbdf: dbdf.into(),
        gfid: gfid.into(),
        vgpu_type: vgpu_type.into(),
        vm_pid: vm_pid.into(),
        swizz_id: SwizzId::WHOLE_GPU.into(),
        num_channels: num_channels.into(),
        num_plugin_channels: num_plugin_channels.into(),
        guest_fb_segment_count: 1.into(),
        options: VgpuBootloadOptions::zeroed().into(),
        channel_mapping,
        guest_fb_segment_phys_addr: Array::new(&[guest_fb_address])?,
        guest_fb_segment_length: Array::new(&[guest_fb_length])?,
        plugin_heap_phys_addr: plugin_heap_address.into(),
        plugin_heap_length: plugin_heap_length.into(),
        ctrl_buff_offset: ctrl_buffer_offset.into(),
        init_task_log_offset: init_log_address.into(),
        init_task_log_size: init_log_size.into(),
        vgpu_task_log_offset: vgpu_log_address.into(),
        vgpu_task_log_size: vgpu_log_size.into(),
        kernel_log_offset: kernel_log_address.into(),
        kernel_log_size: kernel_log_size.into(),
        mig_rm_heap_phys_addr: 0.into(),
        mig_rm_heap_length: 0.into(),
    };

    let mut encoder = Encoder::new();
    request.encode(&mut encoder)?;
    Ok(encoder.finish())
}

nvkv_decode! {
    pub(in crate::vgpu) struct VgpuPropertiesSchema => VgpuProperties {
        // TODO: `name`/`class` required?
        name: Array<u8, { VgpuProperties::STRING_LEN }, { Self::TYPE_NAME_KEY }>,
        class: Array<u8, { VgpuProperties::STRING_LEN }, { Self::CLASS_KEY }>,
        type_id: Required<u32, { Self::TYPE_ID_KEY }>,
        bar1_length: Required<u64, { Self::BAR1_LENGTH_KEY }>,
        max_instance: Required<u32, { Self::MAX_INSTANCE_KEY }>,
        ecc: Key<u32, { Self::ECC_KEY }>,
        profile_size: Required<u64, { Self::PROFILE_SIZE_KEY }>,
        max_fps: Key<u32, { Self::MAX_FPS_KEY }>,
        num_heads: Key<u32, { Self::NUM_HEADS_KEY }>,
        max_res_x: Key<u32, { Self::MAX_RES_X_KEY }>,
        max_res_y: Key<u32, { Self::MAX_RES_Y_KEY }>,
        dev_id: Required<u32, { Self::DEV_ID_KEY }>,
        subsystem_id: Required<u32, { Self::SUBSYSTEM_ID_KEY }>,
        fb_length: Required<u64, { Self::FB_LENGTH_KEY }>,
        gsp_heap_size: Required<u64, { Self::GSP_HEAP_SIZE_KEY }>,
        fb_reservation: Required<u64, { Self::FB_RESERVATION_KEY }>,
    }
}

impl VgpuPropertiesSchema {
    const TYPE_NAME_KEY: KeyId = 0x3100;
    const CLASS_KEY: KeyId = 0x3101;
    const TYPE_ID_KEY: KeyId = 0x3102;
    const BAR1_LENGTH_KEY: KeyId = 0x3103;
    const MAX_INSTANCE_KEY: KeyId = 0x3104;
    const ECC_KEY: KeyId = 0x3105;
    const PROFILE_SIZE_KEY: KeyId = 0x3106;
    const MAX_FPS_KEY: KeyId = 0x3107;
    const NUM_HEADS_KEY: KeyId = 0x3108;
    const MAX_RES_X_KEY: KeyId = 0x3109;
    const MAX_RES_Y_KEY: KeyId = 0x310A;
    const DEV_ID_KEY: KeyId = 0x310B;
    const SUBSYSTEM_ID_KEY: KeyId = 0x310C;
    const FB_LENGTH_KEY: KeyId = 0x310D;
    const GSP_HEAP_SIZE_KEY: KeyId = 0x310E;
    const FB_RESERVATION_KEY: KeyId = 0x310F;
}

pub(in crate::vgpu) struct VgpuProperties {
    name: ArrayVec<u8, { Self::STRING_LEN }>,
    class: ArrayVec<u8, { Self::STRING_LEN }>,
    pub(in crate::vgpu) type_id: u32,
    pub(in crate::vgpu) bar1_length: u64,
    pub(in crate::vgpu) max_instance: u32,
    ecc: u32,
    profile_size: u64,
    max_fps: u32,
    num_heads: u32,
    max_res_x: u32,
    max_res_y: u32,
    pub(in crate::vgpu) dev_id: u32,
    pub(in crate::vgpu) subsystem_id: u32,
    pub(in crate::vgpu) fb_length: u64,
    pub(in crate::vgpu) gsp_heap_size: u64,
    fb_reservation: u64,
}

impl VgpuProperties {
    const STRING_LEN: usize = 64;
}

#[derive(Clone, Copy)]
enum HypervisorType {
    Unknown = 4,
}

impl From<HypervisorType> for u32 {
    fn from(value: HypervisorType) -> Self {
        value as u32
    }
}

#[derive(Clone, Copy)]
enum CpuArch {
    Aarch64 = 1,
    X86_64 = 2,
}

impl CpuArch {
    fn host() -> Result<Self> {
        if cfg!(target_arch = "x86_64") {
            Ok(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Ok(Self::Aarch64)
        } else {
            Err(EOPNOTSUPP)
        }
    }
}

impl From<CpuArch> for u32 {
    fn from(value: CpuArch) -> Self {
        value as u32
    }
}

#[derive(Clone, Copy)]
struct MigrationFeature(u32);

impl MigrationFeature {
    const PRESERVE_CTX_BUF: Self = Self(0x4000);
}

impl From<MigrationFeature> for u32 {
    fn from(value: MigrationFeature) -> Self {
        value.0
    }
}

bitfield! {
    struct FeatureFlags(u64) {
        3:3 enable_uvm => bool;
        5:5 vmm_migration => bool;
    }
}

nvkv_encode! {
    struct PluginConfigParamsRequest {
        uuid: Key<[u8; 16], { Self::UUID_KEY }>,
        dbdf: Key<Dbdf, { Self::DBDF_KEY }, u32>,
        dev_inst: Key<u32, { Self::DEV_INST_KEY }>,
        vgpu_type: Key<u32, { Self::VGPU_TYPE_KEY }>,
        vm_pid: Key<u32, { Self::VM_PID_KEY }>,
        swizz_id: Key<SwizzId, { Self::SWIZZ_ID_KEY }, u32>,
        num_channels: Key<u32, { Self::NUM_CHANNELS_KEY }>,
        num_plugin_channels: Key<u32, { Self::NUM_PLUGIN_CHANNELS_KEY }>,
        vmm_cap: Key<u32, { Self::VMM_CAP_KEY }>,
        migration_feature: Key<MigrationFeature, { Self::MIGRATION_FEATURE_KEY }, u32>,
        hypervisor_type: Key<HypervisorType, { Self::HYPERVISOR_TYPE_KEY }, u32>,
        cpu_arch: Key<CpuArch, { Self::CPU_ARCH_KEY }, u32>,
        page_size: Key<u64, { Self::PAGE_SIZE_KEY }>,
        feature_flags: Key<FeatureFlags, { Self::FEATURE_FLAGS_KEY }, u64>,
    }
}

impl PluginConfigParamsRequest {
    const UUID_KEY: KeyId = 0x0001;
    const DBDF_KEY: KeyId = 0x0002;
    const DEV_INST_KEY: KeyId = 0x0004;
    const VGPU_TYPE_KEY: KeyId = 0x0005;
    const VM_PID_KEY: KeyId = 0x0006;
    const SWIZZ_ID_KEY: KeyId = 0x0010;
    const NUM_CHANNELS_KEY: KeyId = 0x0011;
    const NUM_PLUGIN_CHANNELS_KEY: KeyId = 0x0012;
    const VMM_CAP_KEY: KeyId = 0x0020;
    const MIGRATION_FEATURE_KEY: KeyId = 0x0021;
    const HYPERVISOR_TYPE_KEY: KeyId = 0x0022;
    const CPU_ARCH_KEY: KeyId = 0x0023;
    const PAGE_SIZE_KEY: KeyId = 0x0024;
    const FEATURE_FLAGS_KEY: KeyId = 0x0030;
}

/// Encodes plugin configuration parameters using the typed NVKV schema.
pub(in crate::vgpu) fn encode_plugin_config_params(
    uuid: [u8; 16],
    dbdf: Dbdf,
    vgpu_type: u32,
    vm_pid: u32,
    num_channels: u32,
    num_plugin_channels: u32,
) -> Result<EncodedStream> {
    let request = PluginConfigParamsRequest {
        uuid: uuid.into(),
        dbdf: dbdf.into(),
        dev_inst: 0.into(),
        vgpu_type: vgpu_type.into(),
        vm_pid: vm_pid.into(),
        swizz_id: SwizzId::WHOLE_GPU.into(),
        num_channels: num_channels.into(),
        num_plugin_channels: num_plugin_channels.into(),
        vmm_cap: 0.into(),
        migration_feature: MigrationFeature::PRESERVE_CTX_BUF.into(),
        hypervisor_type: HypervisorType::Unknown.into(),
        cpu_arch: CpuArch::host()?.into(),
        page_size: u64::try_from(kernel::page::PAGE_SIZE)?.into(),
        feature_flags: FeatureFlags::zeroed()
            .with_enable_uvm(false)
            .with_vmm_migration(true)
            .into(),
    };

    let mut encoder = Encoder::new();
    request.encode(&mut encoder)?;
    Ok(encoder.finish())
}

nvkv_encode! {
    struct PluginSetBmeRequest {
        bme_enable: Key<bool, { Self::BME_ENABLE_KEY }, u32>,
    }
}

impl PluginSetBmeRequest {
    const BME_ENABLE_KEY: KeyId = 0x0100;
}

/// Encodes a plugin BME state update using the typed NVKV schema.
pub(in crate::vgpu) fn encode_plugin_set_bme(enable: bool) -> Result<EncodedStream> {
    let request = PluginSetBmeRequest {
        bme_enable: enable.into(),
    };

    let mut encoder = Encoder::new();
    request.encode(&mut encoder)?;
    Ok(encoder.finish())
}

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
pub(in crate::vgpu) struct AllocCeutilsRequest {
    pub(in crate::vgpu) gfid: u32,
    pub(in crate::vgpu) fixed_chid: u32,
    pub(in crate::vgpu) force_ceid: u32,
    pub(in crate::vgpu) swizz_id: u32,
}

static_assert!(size_of::<AllocCeutilsRequest>() == 16);

#[repr(C)]
#[derive(FromBytes)]
pub(in crate::vgpu) struct AllocCeutilsResponse {
    pub(in crate::vgpu) semaphore_address: u64,
    pub(in crate::vgpu) semaphore_aperture: u32,
    _reserved: u32,
}

static_assert!(size_of::<AllocCeutilsResponse>() == 16);

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
pub(in crate::vgpu) struct FreeCeutilsRequest {
    pub(in crate::vgpu) gfid: u32,
}

static_assert!(size_of::<FreeCeutilsRequest>() == 4);

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
pub(in crate::vgpu) struct ScrubGuestFbRequest {
    pub(in crate::vgpu) gfid: u32,
    pub(in crate::vgpu) reserved: u32,
    pub(in crate::vgpu) fb_offset: u64,
    pub(in crate::vgpu) fb_size: u64,
}

static_assert!(size_of::<ScrubGuestFbRequest>() == 24);

#[repr(C)]
#[derive(FromBytes)]
pub(in crate::vgpu) struct ScrubGuestFbResponse {
    pub(in crate::vgpu) work_id: u64,
}

static_assert!(size_of::<ScrubGuestFbResponse>() == 8);
