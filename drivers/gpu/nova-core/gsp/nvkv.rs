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
    struct RegKey {
        key_name: Key<&'static [u8], { Self::REGKEY_NAME_KEY }>,
        key_value: Key<u32, { Self::REGKEY_VALUE_U32_KEY }>,
    }
}

impl RegKey {
    const REGKEY_NAME_KEY: KeyId = 0x3070;
    const REGKEY_VALUE_U32_KEY: KeyId = 0x3071;
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
    struct GspInitRequest {
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

#[kunit_tests(nova_core_nvkv)]
mod tests {
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
}
