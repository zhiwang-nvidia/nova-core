use kernel::{
    bitfield,
    prelude::*, //
};

#[macro_use]
mod encode;
pub(crate) use encode::*;

mod types;
pub(crate) use types::*;

#[macro_use]
mod decode;
pub(crate) use decode::*;

// Stability assumed:
//  1. Fixed struct serialized calls are append only.
//  2. Max response size for fixed struct serialized calls will not be broken by future firmware versions.
//  3. Multipart MCTP support declared by the caller (or we implement multipart from the beginning).
//  4. New unknown keys for NVKV encoded calls should be an error or ignored, decided per call.
//  5. NVKV calls cannot remove existing required keys (unless they were defined as optional previously).

// Semantics assumed:
// 1. Overflowing a key via SEQ* is an error.

#[derive(Clone, Copy)]
pub(crate) enum OorArch {
    None = 0,
    X86_64 = 1,
    Ppc64le = 2,
    Arm = 3,
    Aarch64 = 4,
    Riscv64 = 5,
}

// TODO[FPRI]: This is a temporary solution to be replaced with the corresponding derive macros once
// they land.
impl TryFrom<u32> for OorArch {
    type Error = Error;

    fn try_from(value: u32) -> Result<Self> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::X86_64),
            2 => Ok(Self::Ppc64le),
            3 => Ok(Self::Arm),
            4 => Ok(Self::Aarch64),
            5 => Ok(Self::Riscv64),
            _ => Err(EINVAL),
        }
    }
}

impl From<OorArch> for u32 {
    fn from(value: OorArch) -> Self {
        value as u32
    }
}

nvkv_encode! {
    pub(super) struct RegKey {
        key_name: Key<&'static [u8], { Self::REGKEY_NAME_KEY }>,
        key_value: Key<u32, { Self::REGKEY_VALUE_U32_KEY }>,
    }
}

impl RegKey {
    const REGKEY_NAME_KEY: KeyId = 0x3070;
    const REGKEY_VALUE_U32_KEY: KeyId = 0x3071;

    pub(super) fn new(key_name: &'static [u8], key_value: u32) -> Self {
        Self {
            key_name: key_name.into(),
            key_value: key_value.into(),
        }
    }
}

impl Encodeable for KVVec<RegKey> {
    fn encode(&self, encoder: &mut Encoder) -> Result {
        for regkey in self {
            regkey.encode(encoder)?;
        }
        Ok(())
    }
}

nvkv_encode! {
    struct VfInfo {
        total_vfs: Key<u32, { Self::VF_TOTAL_VFS_KEY }>,
        first_vf_offset: Key<u32, { Self::VF_FIRST_VF_OFFSET_KEY }>,
        flags: Key<u64, { Self::VF_FLAGS_KEY }>,
        first_bar0_address: Key<u64, { Self::VF_FIRST_BAR0_ADDRESS_KEY }>,
        first_bar1_address: Key<u64, { Self::VF_FIRST_BAR1_ADDRESS_KEY }>,
        first_bar2_address: Key<u64, { Self::VF_FIRST_BAR2_ADDRESS_KEY }>,
    }
}

impl VfInfo {
    const VF_TOTAL_VFS_KEY: KeyId = 0x0080;
    const VF_FIRST_VF_OFFSET_KEY: KeyId = 0x0081;
    const VF_FLAGS_KEY: KeyId = 0x1003;
    const VF_FIRST_BAR0_ADDRESS_KEY: KeyId = 0x1050;
    const VF_FIRST_BAR1_ADDRESS_KEY: KeyId = 0x1051;
    const VF_FIRST_BAR2_ADDRESS_KEY: KeyId = 0x1052;
}

nvkv_encode! {
    pub(super) struct GspInitRequest {
        pci_device_id: Key<u32, { Self::PCI_DEVICE_ID_KEY }>,
        pci_sub_device_id: Key<u32, { Self::PCI_SUBDEVICE_ID_KEY }>,
        pci_revision_id: Key<u32, { Self::PCI_REVISION_ID_KEY }>,
        pci_config_mirror_base: Key<u32, { Self::PCI_CONFIG_MIRROR_BASE_KEY }>,
        pci_config_mirror_size: Key<u32, { Self::PCI_CONFIG_MIRROR_SIZE_KEY }>,
        oor_arch: Key<OorArch, { Self::OOR_ARCH_KEY }, u32>,
        bus_device_func: Key<u64, { Self::NV_DOMAIN_BUS_DEVICE_FUNC_KEY }>,
        regkeys: KVVec<RegKey>,
        vf_info: Option<VfInfo>,
    }
}

impl GspInitRequest {
    const PCI_DEVICE_ID_KEY: KeyId = 0x0001;
    const PCI_SUBDEVICE_ID_KEY: KeyId = 0x0002;
    const PCI_REVISION_ID_KEY: KeyId = 0x0003;
    const PCI_CONFIG_MIRROR_BASE_KEY: KeyId = 0x0010;
    const PCI_CONFIG_MIRROR_SIZE_KEY: KeyId = 0x0011;
    const OOR_ARCH_KEY: KeyId = 0x0070;
    const NV_DOMAIN_BUS_DEVICE_FUNC_KEY: KeyId = 0x1020;

    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        pci_device_id: u32,
        pci_sub_device_id: u32,
        pci_revision_id: u32,
        pci_config_mirror_base: u32,
        pci_config_mirror_size: u32,
        oor_arch: OorArch,
        bus_device_func: u64,
        regkeys: KVVec<RegKey>,
    ) -> Self {
        Self {
            pci_device_id: pci_device_id.into(),
            pci_sub_device_id: pci_sub_device_id.into(),
            pci_revision_id: pci_revision_id.into(),
            pci_config_mirror_base: pci_config_mirror_base.into(),
            pci_config_mirror_size: pci_config_mirror_size.into(),
            oor_arch: oor_arch.into(),
            bus_device_func: bus_device_func.into(),
            regkeys,
            vf_info: None,
        }
    }
}

// vGPU related:

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum HypervisorType {
    Xen = 0,
    Vmware = 1,
    HyperV = 2,
    Kvm = 3,
    Unknown = 4,
}

impl From<HypervisorType> for u32 {
    fn from(value: HypervisorType) -> Self {
        value as u32
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum CpuArch {
    X86_64 = 2,
}

impl From<CpuArch> for u32 {
    fn from(value: CpuArch) -> Self {
        value as u32
    }
}

bitfield! {
    pub(crate) struct Dbdf(u32) {
        2:0 function;
        7:3 device;
        15:8 bus;
        31:16 domain;
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SwizzId(pub(crate) u32);

impl SwizzId {
    pub(crate) const WHOLE_GPU: Self = Self(0xFFFF_FFFF);
}

impl Default for SwizzId {
    fn default() -> Self {
        Self::WHOLE_GPU
    }
}

impl From<SwizzId> for u32 {
    fn from(value: SwizzId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy)]
pub(crate) struct MigrationFeature(pub(crate) u32);

impl MigrationFeature {
    const KVM: Self = Self(0x4000);
}

impl From<MigrationFeature> for u32 {
    fn from(value: MigrationFeature) -> Self {
        value.0
    }
}

bitfield! {
    pub(crate) struct FeatureFlags(u64) {
        3:3 enable_uvm => bool;
        5:5 vmm_migration => bool;
    }
}

bitfield! {
    pub(crate) struct ChannelMapEntry(u64) {
        15:0 engine_type;
        31:16 index;
        63:32 chid_offset;
    }
}

impl ChannelMapEntry {
    const KEY: KeyId = 0x1001;
}

impl Encodeable for KVVec<ChannelMapEntry> {
    fn encode(&self, encoder: &mut Encoder) -> Result {
        // SAFETY: `ChannelMapEntry` is a `bitfield!` over `u64`, i.e.
        // `#[repr(transparent)]` around a `u64`, so the entries are
        // layout-compatible with `u64` and can be viewed as a `u64` slice.
        let slice = unsafe { core::slice::from_raw_parts(self.as_ptr().cast::<u64>(), self.len()) };
        encoder.encode_array64(ChannelMapEntry::KEY, Index::new::<0>(), slice)
    }
}

bitfield! {
    pub(crate) struct VgpuBootloadOptions(u64) {
    }
}

// VGPU_BOOTLAOD

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

// VGPU_MGMT_QUERY_PROPERTIES

nvkv_decode! {
    #[derive(Default)]
    struct VgpuPropertiesSchema => VgpuProperties {
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

struct VgpuProperties {
    name: ArrayVec<u8, { Self::STRING_LEN }>,
    class: ArrayVec<u8, { Self::STRING_LEN }>,
    type_id: u32,
    bar1_length: u64,
    max_instance: u32,
    ecc: u32,
    profile_size: u64,
    max_fps: u32,
    num_heads: u32,
    max_res_x: u32,
    max_res_y: u32,
    dev_id: u32,
    subsystem_id: u32,
    fb_length: u64,
    gsp_heap_size: u64,
    fb_reservation: u64,
}

impl VgpuProperties {
    const STRING_LEN: usize = 64;
}

// SETUP_CONFIG_PARAMS_AND_INIT

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

// UPDATE_BME_STATE

nvkv_encode! {
    struct PluginSetBmeRequest {
        bme_enable: Key<bool, { Self::BME_ENABLE_KEY }, u32>,
    }
}

impl PluginSetBmeRequest {
    const BME_ENABLE_KEY: KeyId = 0x0100;
}

// Decode:

// Should decode with UnknownKeyPolicy::Ignore.
nvkv_decode! {
    #[derive(Default)]
    pub(super) struct GspInitResponseSchema => GspInitResponse {
        gpu_name:
            Array<u8, { GspInitResponse::MAX_GPU_NAME_LEN }, { Self::GPU_NAME_STRING_KEY }>,
        fb_regions: Accumulated<FbRegionSchema>,
        bar1_pde_base: Required<u64, { Self::BAR1_PDE_BASE_KEY }>,
        vmmu_segment_size: Key<u64, { Self::VMMU_SEGMENT_SIZE_KEY }>,
        gmc_engine_masks:
            Indexed<u64, { GspInitResponse::NVGMC_ENGINE_TYPE_COUNT }, { Self::ENGINE_MASK_KEY }>,
    }
}

impl GspInitResponseSchema {
    #[expect(dead_code)]
    const FB_REGION_COUNT_KEY: KeyId = 0x0010;
    const GPU_NAME_STRING_KEY: KeyId = 0x2000;
    const BAR1_PDE_BASE_KEY: KeyId = 0x1020;
    const VMMU_SEGMENT_SIZE_KEY: KeyId = 0x1050;
    const ENGINE_MASK_KEY: KeyId = 0x1100;
}

pub(super) struct GspInitResponse {
    gpu_name: ArrayVec<u8, { Self::MAX_GPU_NAME_LEN }>,
    fb_regions: KVVec<FbRegion>,
    bar1_pde_base: u64,
    vmmu_segment_size: u64,
    gmc_engine_masks: [u64; Self::NVGMC_ENGINE_TYPE_COUNT],
}

impl GspInitResponse {
    const MAX_GPU_NAME_LEN: usize = 64;
    const NVGMC_ENGINE_TYPE_COUNT: usize = 20;
    const FB_REGION_TAG_NONE: u32 = 0;

    /// Returns the BAR1 Page Directory Entry base address.
    ///
    /// This is the root page table address for BAR1 virtual memory,
    /// set up by GSP-RM firmware.
    pub(super) fn bar1_pde_base(&self) -> u64 {
        self.bar1_pde_base
    }

    pub(super) fn gpu_name(&self) -> &[u8] {
        self.gpu_name.as_slice()
    }

    /// Iterates over FB regions that the driver may use for memory allocation.
    pub(super) fn usable_fb_regions(&self) -> impl Iterator<Item = core::ops::Range<u64>> + '_ {
        self.fb_regions.iter().filter_map(|region| {
            if region.limit >= region.base
                && region.tag == Self::FB_REGION_TAG_NONE
                && !region.flags.protected()
                && region.flags.support_compressed()
                && region.flags.support_iso()
            {
                region.limit.checked_add(1).map(|end| region.base..end)
            } else {
                None
            }
        })
    }

    /// Compute the end of physical VRAM from all valid FB regions.
    pub(super) fn total_fb_end(&self) -> Option<u64> {
        self.fb_regions
            .iter()
            .filter(|region| region.limit >= region.base)
            .filter_map(|region| region.limit.checked_add(1))
            .max()
    }
}

nvkv_decode! {
    #[derive(Default)]
    struct FbRegionSchema => FbRegion {
        base: Required<u64, { Self::BASE_KEY }>,
        limit: Required<u64, { Self::LIMIT_KEY }>,
        flags: Required<FbRegionFlags, { Self::FLAGS_KEY }>,
        tag: Required<u32, { Self::TAG_KEY }>,
    }
}

impl FbRegionSchema {
    const BASE_KEY: KeyId = 0x1011;
    const LIMIT_KEY: KeyId = 0x1012;
    const FLAGS_KEY: KeyId = 0x0012;
    const TAG_KEY: KeyId = 0x0013;
}

bitfield! {
    struct FbRegionFlags(u32) {
        0:0 support_compressed => bool;
        1:1 support_iso => bool;
        2:2 protected => bool;
    }
}

impl TryFrom<DecoderValue<'_>> for FbRegionFlags {
    type Error = Error;

    fn try_from(value: DecoderValue<'_>) -> Result<Self> {
        if let DecoderValue::Scalar32(v) = value {
            Ok(v.into())
        } else {
            Err(EINVAL)
        }
    }
}

struct FbRegion {
    base: u64,
    limit: u64,
    flags: FbRegionFlags,
    tag: u32,
}

#[kunit_tests(nova_core_nvkv)]
mod tests {
    use pin_init::stack_try_pin_init;

    use super::*;

    #[test]
    fn gsp_init_request() -> Result {
        let mut encoder = Encoder::new();

        let mut regkeys = KVVec::new();
        regkeys.push(
            RegKey {
                key_name: b"test_key\0".into(),
                key_value: 0xdead_beef.into(),
            },
            GFP_KERNEL,
        )?;
        regkeys.push(
            RegKey {
                key_name: b"test_key2\0".into(),
                key_value: 0xc0ff_ee00.into(),
            },
            GFP_KERNEL,
        )?;

        let gsp_init = GspInitRequest {
            pci_device_id: 45.into(),
            pci_sub_device_id: 67.into(),
            pci_revision_id: 3.into(),
            pci_config_mirror_base: 0x1234_5678.into(),
            pci_config_mirror_size: 0x1000.into(),
            oor_arch: OorArch::Aarch64.into(),
            bus_device_func: 0x0001_0203_0405_0607.into(),
            regkeys,
            vf_info: Some(VfInfo {
                total_vfs: 8.into(),
                first_vf_offset: 1.into(),
                flags: 0x7.into(),
                first_bar0_address: 0x1000_0000.into(),
                first_bar1_address: 0x2000_0000.into(),
                first_bar2_address: 0x3000_0000.into(),
            }),
        };

        gsp_init.encode(&mut encoder)?;
        let _encoded = encoder.finish();
        Ok(())
    }

    #[test]
    fn encode_vgpu_bootload_request() -> Result {
        let mut encoder = Encoder::new();

        let mut channel_mapping = KVVec::new();
        channel_mapping.push(
            ChannelMapEntry::zeroed().with_const_engine_type::<1>(),
            GFP_KERNEL,
        )?;
        channel_mapping.push(
            ChannelMapEntry::zeroed()
                .with_const_engine_type::<2>()
                .with_const_chid_offset::<8>(),
            GFP_KERNEL,
        )?;

        let request = VgpuBootloadRequest {
            dbdf: Dbdf::zeroed()
                .with_const_domain::<1>()
                .with_const_bus::<2>()
                .with_const_device::<0>()
                .with_const_function::<3>()
                .into(),
            gfid: 1.into(),
            vgpu_type: 0x42.into(),
            vm_pid: 1234.into(),
            swizz_id: SwizzId(3).into(),
            num_channels: 8.into(),
            num_plugin_channels: 2.into(),
            guest_fb_segment_count: 1.into(),
            options: VgpuBootloadOptions::zeroed().into(),
            channel_mapping,
            guest_fb_segment_phys_addr: Array::new(&[0x1_0000_0000])?,
            guest_fb_segment_length: Array::new(&[0x4000_0000])?,
            plugin_heap_phys_addr: 0x2000_0000.into(),
            plugin_heap_length: 0x10_0000.into(),
            ctrl_buff_offset: 0.into(),
            init_task_log_offset: 0x100.into(),
            init_task_log_size: 0x200.into(),
            vgpu_task_log_offset: 0x300.into(),
            vgpu_task_log_size: 0x400.into(),
            kernel_log_offset: 0x500.into(),
            kernel_log_size: 0x600.into(),
            mig_rm_heap_phys_addr: 0x3000_0000.into(),
            mig_rm_heap_length: 0x40_0000.into(),
        };

        request.encode(&mut encoder)?;
        let _encoded = encoder.finish();
        Ok(())
    }

    #[test]
    fn encode_plugin_config_params() -> Result {
        let mut encoder = Encoder::new();

        let request = PluginConfigParamsRequest {
            uuid: [
                0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                0x77, 0x88,
            ]
            .into(),
            dbdf: Dbdf::zeroed()
                .with_const_domain::<1>()
                .with_const_bus::<2>()
                .with_const_device::<0>()
                .with_const_function::<3>()
                .into(),
            dev_inst: 0.into(),
            vgpu_type: 0x42.into(),
            vm_pid: 1234.into(),
            swizz_id: SwizzId(3).into(),
            num_channels: 8.into(),
            num_plugin_channels: 2.into(),
            vmm_cap: 0.into(),
            migration_feature: MigrationFeature::KVM.into(),
            hypervisor_type: HypervisorType::Unknown.into(),
            cpu_arch: CpuArch::X86_64.into(),
            page_size: 4096.into(),
            feature_flags: FeatureFlags::zeroed()
                .with_enable_uvm(true)
                .with_vmm_migration(true)
                .into(),
        };

        request.encode(&mut encoder)?;
        let _encoded = encoder.finish();
        Ok(())
    }

    #[test]
    fn encode_plugin_set_bme() -> Result {
        let mut encoder = Encoder::new();

        let request = PluginSetBmeRequest {
            bme_enable: true.into(),
        };

        request.encode(&mut encoder)?;
        let _encoded = encoder.finish();
        Ok(())
    }

    #[test]
    fn encode_all_value_kinds() -> Result {
        const KEY: KeyId = 0x1234;

        let mut encoder = Encoder::new();
        let index = Index::new::<0>();

        encoder.encode_u32(KEY, index, 0x89ab_cdef)?;
        encoder.encode_u64(KEY, index, 0x0123_4567_89ab_cdef)?;
        encoder.encode_array8(KEY, index, &[0x12, 0x34, 0x56])?;
        encoder.encode_array32(KEY, index, &[0x0123_4567, 0x89ab_cdef])?;
        encoder.encode_array64(KEY, index, &[0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210])?;

        Ok(())
    }

    #[test]
    fn decode_test() -> Result {
        const SCALAR32_KEY: KeyId = 0x1234;
        const SCALAR64_KEY: KeyId = 0x1235;
        const ARRAY8_KEY: KeyId = 0x1236;
        const ARRAY32_KEY: KeyId = 0x1237;
        const ARRAY64_KEY: KeyId = 0x1238;

        const SCALAR32_VALUE: u32 = 0x89ab_cdef;
        const SCALAR64_VALUE: u64 = 0x0123_4567_89ab_cdef;
        const ARRAY8_VALUE: &[u8] = &[0x12, 0x34, 0x56];
        const ARRAY32_VALUE: &[u32] = &[0x0123_4567, 0x89ab_cdef];
        const ARRAY64_VALUE: &[u64] = &[0x0123_4567_89ab_cdef, 0xfedc_ba98_7654_3210];

        let mut encoder = Encoder::new();
        let index = Index::new::<0>();
        encoder.encode_u32(SCALAR32_KEY, index, SCALAR32_VALUE)?;
        encoder.encode_u64(SCALAR64_KEY, index, SCALAR64_VALUE)?;
        encoder.encode_array8(ARRAY8_KEY, index, ARRAY8_VALUE)?;
        encoder.encode_array32(ARRAY32_KEY, index, ARRAY32_VALUE)?;
        encoder.encode_array64(ARRAY64_KEY, index, ARRAY64_VALUE)?;
        let serialized = encoder.finish();

        nvkv_decode! {
            #[derive(Default)]
            struct TestSchema => TestDecodeable {
                scalar32: Required<u32, { SCALAR32_KEY }>,
                scalar64: Required<u64, { SCALAR64_KEY }>,
                array8: Array<u8, 64, { ARRAY8_KEY }>,
                array32: Array<u32, 64, { ARRAY32_KEY }>,
                array64: Array<u64, 64, { ARRAY64_KEY }>,
            }
        }

        struct TestDecodeable {
            scalar32: u32,
            scalar64: u64,
            array8: ArrayVec<u8, 64>,
            array32: ArrayVec<u32, 64>,
            array64: ArrayVec<u64, 64>,
        }

        let decoder = Decoder::new(&serialized, UnknownKeyPolicy::Error)?;
        let decoded = KBox::try_init(decoder.decode(TestSchema::default())?, GFP_KERNEL)?;

        assert_eq!(decoded.scalar32, SCALAR32_VALUE);
        assert_eq!(decoded.scalar64, SCALAR64_VALUE);
        assert_eq!(*decoded.array8, *ARRAY8_VALUE);
        assert_eq!(*decoded.array32, *ARRAY32_VALUE);
        assert_eq!(*decoded.array64, *ARRAY64_VALUE);

        Ok(())
    }

    #[test]
    fn gsp_init_response() -> Result {
        let name = b"test name\0";
        const BAR1_PDE_BASE: u64 = 0xdead_0000;

        let index = Index::new::<0>();
        let mut encoder = Encoder::new();
        encoder.encode_array8(GspInitResponseSchema::GPU_NAME_STRING_KEY, index, name)?;
        encoder.encode_u64(
            GspInitResponseSchema::BAR1_PDE_BASE_KEY,
            index,
            BAR1_PDE_BASE,
        )?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore)?;
        let response = KBox::try_init(
            decoder.decode(GspInitResponseSchema::default())?,
            GFP_KERNEL,
        )?;
        assert_eq!(&*response.gpu_name, &name[..]);
        assert_eq!(response.bar1_pde_base, BAR1_PDE_BASE);
        assert!(response.fb_regions.is_empty());

        // A single FB region decoded via its own schema.
        const FB_REGION_BASE: u64 = 0x1000_0000;
        const FB_REGION_LIMIT: u64 = 0x1fff_ffff;
        const FB_REGION_FLAGS: u32 = 0x7;
        const FB_REGION_TAG: u32 = 0;
        let mut encoder = Encoder::new();
        encoder.encode_u64(FbRegionSchema::BASE_KEY, index, FB_REGION_BASE)?;
        encoder.encode_u64(FbRegionSchema::LIMIT_KEY, index, FB_REGION_LIMIT)?;
        encoder.encode_u32(FbRegionSchema::FLAGS_KEY, index, FB_REGION_FLAGS)?;
        encoder.encode_u32(FbRegionSchema::TAG_KEY, index, FB_REGION_TAG)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore)?;
        // Stack allocation for demonstration purposes.
        stack_try_pin_init!(
            let fb_region: FbRegion =? decoder.decode(FbRegionSchema::default())?
        );
        assert_eq!(fb_region.base, FB_REGION_BASE);
        assert_eq!(fb_region.limit, FB_REGION_LIMIT);
        assert_eq!(fb_region.flags.into_raw(), FB_REGION_FLAGS);
        assert!(fb_region.flags.support_compressed());
        assert!(fb_region.flags.support_iso());
        assert!(fb_region.flags.protected());
        assert_eq!(fb_region.tag, FB_REGION_TAG);

        Ok(())
    }

    #[test]
    fn decode_vgpu_properties() -> Result {
        let name = b"test name\0";
        let class = b"test class\0";
        const TYPE_ID: u32 = 0x42;
        const BAR1_LENGTH: u64 = 0x1_0000_0000;
        const MAX_INSTANCE: u32 = 4;
        const ECC: u32 = 1;
        const PROFILE_SIZE: u64 = 0x1_0000_0000;
        const MAX_FPS: u32 = 60;
        const NUM_HEADS: u32 = 4;
        const MAX_RES_X: u32 = 7680;
        const MAX_RES_Y: u32 = 4320;
        const DEV_ID: u32 = 0x1db4;
        const SUBSYSTEM_ID: u32 = 0x1234;
        const FB_LENGTH: u64 = 0x1_0000_0000;
        const GSP_HEAP_SIZE: u64 = 0x10_0000;
        const FB_RESERVATION: u64 = 0x40_0000;

        let index = Index::new::<0>();
        let mut encoder = Encoder::new();
        encoder.encode_array8(VgpuPropertiesSchema::TYPE_NAME_KEY, index, name)?;
        encoder.encode_array8(VgpuPropertiesSchema::CLASS_KEY, index, class)?;
        encoder.encode_u32(VgpuPropertiesSchema::TYPE_ID_KEY, index, TYPE_ID)?;
        encoder.encode_u64(VgpuPropertiesSchema::BAR1_LENGTH_KEY, index, BAR1_LENGTH)?;
        encoder.encode_u32(VgpuPropertiesSchema::MAX_INSTANCE_KEY, index, MAX_INSTANCE)?;
        encoder.encode_u32(VgpuPropertiesSchema::ECC_KEY, index, ECC)?;
        encoder.encode_u64(VgpuPropertiesSchema::PROFILE_SIZE_KEY, index, PROFILE_SIZE)?;
        encoder.encode_u32(VgpuPropertiesSchema::MAX_FPS_KEY, index, MAX_FPS)?;
        encoder.encode_u32(VgpuPropertiesSchema::NUM_HEADS_KEY, index, NUM_HEADS)?;
        encoder.encode_u32(VgpuPropertiesSchema::MAX_RES_X_KEY, index, MAX_RES_X)?;
        encoder.encode_u32(VgpuPropertiesSchema::MAX_RES_Y_KEY, index, MAX_RES_Y)?;
        encoder.encode_u32(VgpuPropertiesSchema::DEV_ID_KEY, index, DEV_ID)?;
        encoder.encode_u32(VgpuPropertiesSchema::SUBSYSTEM_ID_KEY, index, SUBSYSTEM_ID)?;
        encoder.encode_u64(VgpuPropertiesSchema::FB_LENGTH_KEY, index, FB_LENGTH)?;
        encoder.encode_u64(
            VgpuPropertiesSchema::GSP_HEAP_SIZE_KEY,
            index,
            GSP_HEAP_SIZE,
        )?;
        encoder.encode_u64(
            VgpuPropertiesSchema::FB_RESERVATION_KEY,
            index,
            FB_RESERVATION,
        )?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore)?;
        let props = KBox::try_init(decoder.decode(VgpuPropertiesSchema::default())?, GFP_KERNEL)?;

        assert_eq!(&*props.name, &name[..]);
        assert_eq!(&*props.class, &class[..]);
        assert_eq!(props.type_id, TYPE_ID);
        assert_eq!(props.bar1_length, BAR1_LENGTH);
        assert_eq!(props.max_instance, MAX_INSTANCE);
        assert_eq!(props.ecc, ECC);
        assert_eq!(props.profile_size, PROFILE_SIZE);
        assert_eq!(props.max_fps, MAX_FPS);
        assert_eq!(props.num_heads, NUM_HEADS);
        assert_eq!(props.max_res_x, MAX_RES_X);
        assert_eq!(props.max_res_y, MAX_RES_Y);
        assert_eq!(props.dev_id, DEV_ID);
        assert_eq!(props.subsystem_id, SUBSYSTEM_ID);
        assert_eq!(props.fb_length, FB_LENGTH);
        assert_eq!(props.gsp_heap_size, GSP_HEAP_SIZE);
        assert_eq!(props.fb_reservation, FB_RESERVATION);

        Ok(())
    }

    #[test]
    fn decode_vgpu_properties_missing_required_fails() -> Result {
        let index = Index::new::<0>();
        let mut encoder = Encoder::new();
        encoder.encode_u32(VgpuPropertiesSchema::ECC_KEY, index, 1)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore)?;
        let init = decoder.decode(VgpuPropertiesSchema::default())?;
        assert!(KBox::try_init(init, GFP_KERNEL).is_err());

        Ok(())
    }

    #[test]
    fn gsp_init_response_interleaved_indexed_field() -> Result {
        let name = b"test name\0";
        const BAR1_PDE_BASE: u64 = 0xdead_0000;
        const FB_REGION0_BASE: u64 = 0x1000_0000;
        const FB_REGION0_LIMIT: u64 = 0x1fff_ffff;
        const FB_REGION0_FLAGS: u32 = 0x7;
        const FB_REGION0_TAG: u32 = 0;
        const FB_REGION1_BASE: u64 = 0x2000_0000;
        const FB_REGION1_LIMIT: u64 = 0x2fff_ffff;
        const FB_REGION1_FLAGS: u32 = 0x3;
        const FB_REGION1_TAG: u32 = 1;
        const ENGINE_MASK: u64 = 0x1234_5678;

        let index0 = Index::new::<0>();
        let index1 = Index::new::<1>();
        let mut encoder = Encoder::new();
        encoder.encode_array8(GspInitResponseSchema::GPU_NAME_STRING_KEY, index0, name)?;
        encoder.encode_u64(
            GspInitResponseSchema::BAR1_PDE_BASE_KEY,
            index0,
            BAR1_PDE_BASE,
        )?;
        encoder.encode_u64(FbRegionSchema::BASE_KEY, index0, FB_REGION0_BASE)?;
        encoder.encode_u64(GspInitResponseSchema::ENGINE_MASK_KEY, index1, ENGINE_MASK)?;
        encoder.encode_u64(FbRegionSchema::LIMIT_KEY, index0, FB_REGION0_LIMIT)?;
        encoder.encode_u32(FbRegionSchema::FLAGS_KEY, index0, FB_REGION0_FLAGS)?;
        encoder.encode_u32(FbRegionSchema::TAG_KEY, index0, FB_REGION0_TAG)?;
        encoder.encode_u64(FbRegionSchema::BASE_KEY, index1, FB_REGION1_BASE)?;
        encoder.encode_u64(FbRegionSchema::LIMIT_KEY, index1, FB_REGION1_LIMIT)?;
        encoder.encode_u32(FbRegionSchema::FLAGS_KEY, index1, FB_REGION1_FLAGS)?;
        encoder.encode_u32(FbRegionSchema::TAG_KEY, index1, FB_REGION1_TAG)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Error)?;
        let response = KBox::try_init(
            decoder.decode(GspInitResponseSchema::default())?,
            GFP_KERNEL,
        )?;

        assert_eq!(response.fb_regions.len(), 2);
        // PANIC: The assertion above verifies that two FB regions were decoded.
        let fb_region0 = &response.fb_regions[0];
        assert_eq!(fb_region0.base, FB_REGION0_BASE);
        assert_eq!(fb_region0.limit, FB_REGION0_LIMIT);
        assert_eq!(fb_region0.flags.into_raw(), FB_REGION0_FLAGS);
        assert_eq!(fb_region0.tag, FB_REGION0_TAG);
        let fb_region1 = &response.fb_regions[1];
        assert_eq!(fb_region1.base, FB_REGION1_BASE);
        assert_eq!(fb_region1.limit, FB_REGION1_LIMIT);
        assert_eq!(fb_region1.flags.into_raw(), FB_REGION1_FLAGS);
        assert_eq!(fb_region1.tag, FB_REGION1_TAG);
        assert_eq!(response.gmc_engine_masks.get(1).copied(), Some(ENGINE_MASK));

        Ok(())
    }
}
