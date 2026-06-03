// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2025-2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use core::ops::Range;

use kernel::{
    alloc::ArrayVec,
    bitfield,
    device,
    pci,
    prelude::*,
    transmute::AsBytes, //
};

use crate::{
    gpu::Chipset,
    num, //
};

use crate::gsp::nvkv::{
    nvkv_decode,
    nvkv_encode,
    Accumulated,
    Array,
    DecoderValue,
    Encodable,
    Encoder,
    Indexed,
    Key,
    KeyId,
    Required, //
};

use super::bindings;

/// Power level the GSP is asked to enter, which [`GspSuspend`] maps to its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
#[expect(unused)]
pub(crate) enum PowerStateLevel {
    /// Full unload.
    Level0 = bindings::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_0,
    /// S3 (suspend to RAM).
    Level3 = bindings::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_3,
    /// Hibernate (suspend to disk).
    Level7 = bindings::NV2080_CTRL_GPU_SET_POWER_STATE_GPU_LEVEL_7,
}

impl PowerStateLevel {
    /// Returns `true` if this state represents a power management transition, i.e. some GPU state
    /// must survive it (as opposed to a full unload).
    pub(crate) fn is_power_transition(self) -> bool {
        self != PowerStateLevel::Level0
    }
}

/// Set in [`GspSuspend::flags`] when some GPU state must survive the suspend.
const GMCAPI_GSP_SUSPEND_FLAGS_PM_TRANSITION: u64 = 1 << 0;

/// Payload of the `GSP_SUSPEND` GMC command.
#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable)]
pub(crate) struct GspSuspend {
    flags: u64,
}

impl GspSuspend {
    /// Creates a `GSP_SUSPEND` payload for the given [`PowerStateLevel`].
    pub(crate) fn new(level: PowerStateLevel) -> Self {
        Self {
            flags: if level.is_power_transition() {
                GMCAPI_GSP_SUSPEND_FLAGS_PM_TRANSITION
            } else {
                0
            },
        }
    }
}

// SAFETY: The single field is an integer type, and the struct has no padding.
unsafe impl AsBytes for GspSuspend {}

/// The host CPU architecture.
#[derive(Clone, Copy)]
pub(crate) enum HostArch {
    None = 0,
    X86_64 = 1,
    Ppc64le = 2,
    Arm = 3,
    Aarch64 = 4,
    Riscv64 = 5,
}

impl HostArch {
    /// Returns the variant naming the architecture this kernel is built for.
    fn host() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else if cfg!(target_arch = "powerpc64") {
            Self::Ppc64le
        } else if cfg!(target_arch = "arm") {
            Self::Arm
        } else if cfg!(target_arch = "riscv64") {
            Self::Riscv64
        } else {
            Self::None
        }
    }
}

// TODO[FPRI]: This is a temporary solution to be replaced with the corresponding derive macros once
// they land.
impl TryFrom<u32> for HostArch {
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

impl From<HostArch> for u32 {
    fn from(value: HostArch) -> Self {
        value as u32
    }
}

nvkv_encode! {
    /// A GSP registry entry.
    pub(crate) struct RegKey {
        key_name: Key<&'static [u8], { Self::REGKEY_NAME_KEY }>,
        key_value: Key<u32, { Self::REGKEY_VALUE_U32_KEY }>,
    }
}

impl RegKey {
    // Define the Key IDs read/written by GSP.
    const REGKEY_NAME_KEY: KeyId = 0x3070;
    const REGKEY_VALUE_U32_KEY: KeyId = 0x3071;

    /// Creates a registry entry. `key_name` must include its NULL terminator, which GSP-RM counts
    /// in the encoded name length.
    pub(crate) fn new(key_name: &'static [u8], key_value: u32) -> Self {
        Self {
            key_name: key_name.into(),
            key_value: key_value.into(),
        }
    }
}

impl Encodable for KVVec<RegKey> {
    fn encode(&self, encoder: &mut Encoder) -> Result {
        for regkey in self {
            regkey.encode(encoder)?;
        }
        Ok(())
    }
}

nvkv_encode! {
    /// SR-IOV virtual function information.
    pub(crate) struct VfInfo {
        total_vfs: Key<u32, { Self::VF_TOTAL_VFS_KEY }>,
        first_vf_offset: Key<u32, { Self::VF_FIRST_VF_OFFSET_KEY }>,
        flags: Key<u64, { Self::VF_FLAGS_KEY }>,
        first_bar0_address: Key<u64, { Self::VF_FIRST_BAR0_ADDRESS_KEY }>,
        first_bar1_address: Key<u64, { Self::VF_FIRST_BAR1_ADDRESS_KEY }>,
        first_bar2_address: Key<u64, { Self::VF_FIRST_BAR2_ADDRESS_KEY }>,
    }
}

impl VfInfo {
    // Define the Key IDs read/written by GSP.
    const VF_TOTAL_VFS_KEY: KeyId = 0x0080;
    const VF_FIRST_VF_OFFSET_KEY: KeyId = 0x0081;
    const VF_FLAGS_KEY: KeyId = 0x1003;
    const VF_FIRST_BAR0_ADDRESS_KEY: KeyId = 0x1050;
    const VF_FIRST_BAR1_ADDRESS_KEY: KeyId = 0x1051;
    const VF_FIRST_BAR2_ADDRESS_KEY: KeyId = 0x1052;

    /// Creates the VF topology portion of a `GSP_INIT` request.
    pub(crate) fn new(
        total_vfs: u32,
        first_vf_offset: u32,
        flags: u64,
        first_bar0_address: u64,
        first_bar1_address: u64,
        first_bar2_address: u64,
    ) -> Self {
        Self {
            total_vfs: total_vfs.into(),
            first_vf_offset: first_vf_offset.into(),
            flags: flags.into(),
            first_bar0_address: first_bar0_address.into(),
            first_bar1_address: first_bar1_address.into(),
            first_bar2_address: first_bar2_address.into(),
        }
    }
}

nvkv_encode! {
    /// Payload of the `GSP_INIT` command.
    // TODO: expect() doesn't work here due to Self:: reference, fixed in 1.97.0
    // https://github.com/rust-lang/rust/pull/154377
    #[cfg_attr(not(CONFIG_KUNIT), allow(dead_code))]
    pub(crate) struct GspInitRequest {
        pci_device_id: Key<u32, { Self::PCI_DEVICE_ID_KEY }>,
        pci_sub_device_id: Key<u32, { Self::PCI_SUBDEVICE_ID_KEY }>,
        pci_revision_id: Key<u32, { Self::PCI_REVISION_ID_KEY }>,
        pci_config_mirror_base: Key<u32, { Self::PCI_CONFIG_MIRROR_BASE_KEY }>,
        pci_config_mirror_size: Key<u32, { Self::PCI_CONFIG_MIRROR_SIZE_KEY }>,
        host_arch: Key<HostArch, { Self::HOST_ARCH_KEY }, u32>,
        bus_device_func: Key<u64, { Self::NV_DOMAIN_BUS_DEVICE_FUNC_KEY }>,
        regkeys: KVVec<RegKey>,
        vf_info: Option<VfInfo>,
    }
}

bitfield! {
    /// PCI bus, device and function, packed the way `PCI_DEVID` packs them, which is what
    /// [`pci::Device::dev_id`] returns.
    struct PciDevId(u16) {
        15:8 bus;
        7:3 device;
        2:0 function;
    }
}

bitfield! {
    /// A GPU's PCI location, encoded the way GSP-RM decodes it, matching Open RM's
    /// `gpuEncodeDomainBusDevice`. Despite the name GSP-RM gives the key, the function number
    /// is not part of it.
    struct DomainBusDevice(u64) {
        63:32 domain;
        15:8 bus;
        7:0 device;
    }
}

#[cfg_attr(not(CONFIG_KUNIT), allow(dead_code))]
impl GspInitRequest {
    // Define the Key IDs read/written by GSP.
    const PCI_DEVICE_ID_KEY: KeyId = 0x0001;
    const PCI_SUBDEVICE_ID_KEY: KeyId = 0x0002;
    const PCI_REVISION_ID_KEY: KeyId = 0x0003;
    const PCI_CONFIG_MIRROR_BASE_KEY: KeyId = 0x0010;
    const PCI_CONFIG_MIRROR_SIZE_KEY: KeyId = 0x0011;
    const HOST_ARCH_KEY: KeyId = 0x0070;
    const NV_DOMAIN_BUS_DEVICE_FUNC_KEY: KeyId = 0x1020;

    /// Describes `dev` to GSP-RM and asks it to apply `regkeys`.
    pub(crate) fn new(
        dev: &pci::Device<device::Bound>,
        chipset: Chipset,
        regkeys: KVVec<RegKey>,
        vf_info: Option<VfInfo>,
    ) -> Self {
        let mirror = chipset.pci_config_mirror_range();
        let dev_id = PciDevId::from(dev.dev_id());
        let bus_device_func = DomainBusDevice::zeroed()
            .with_domain(dev.domain_nr())
            .with_bus(u8::from(dev_id.bus()))
            .with_device(u8::from(dev_id.device()));
        let device_id = (u32::from(dev.device_id()) << 16) | u32::from(dev.vendor_id().as_raw());
        let sub_device_id =
            (u32::from(dev.subsystem_device_id()) << 16) | u32::from(dev.subsystem_vendor_id());

        Self {
            pci_device_id: device_id.into(),
            pci_sub_device_id: sub_device_id.into(),
            pci_revision_id: u32::from(dev.revision_id()).into(),
            pci_config_mirror_base: mirror.start.into(),
            pci_config_mirror_size: (mirror.end - mirror.start).into(),
            host_arch: HostArch::host().into(),
            bus_device_func: u64::from(bus_device_func).into(),
            regkeys,
            vf_info,
        }
    }
}

// Decode:

pub(in crate::gsp) const MAX_FIFO_ENGINES: usize = 64;

// Should decode with UnknownKeyPolicy::Ignore.
nvkv_decode! {
    /// Schema for the `GSP_INIT` response.
    // TODO: expect() doesn't work here due to Self:: reference, fixed in 1.97.0
    // https://github.com/rust-lang/rust/pull/154377
    #[cfg_attr(not(CONFIG_KUNIT), allow(dead_code))]
    pub(crate) struct GspInitResponseSchema => GspInitResponse {
        gpu_name:
            Array<u8, { GspInitResponse::MAX_GPU_NAME_LEN }, { Self::GPU_NAME_STRING_KEY }>,
        fb_regions: Accumulated<FbRegionSchema>,
        bar1_pde_base: Required<u64, { Self::BAR1_PDE_BASE_KEY }>,
        vmmu_segment_size: Key<u64, { Self::VMMU_SEGMENT_SIZE_KEY }>,
        fifo_engine_count: Key<u32, { Self::FIFO_ENGINE_COUNT_KEY }>,
        fifo_engine_gmc_ids: Indexed<u32, MAX_FIFO_ENGINES, { Self::FIFO_ENGINE_GMC_ID_KEY }>,
        fifo_engine_flags: Indexed<u32, MAX_FIFO_ENGINES, { Self::FIFO_ENGINE_FLAGS_KEY }>,
    }
}

impl GspInitResponseSchema {
    // Define the Key IDs read/written by GSP.
    const GPU_NAME_STRING_KEY: KeyId = 0x2000;
    const BAR1_PDE_BASE_KEY: KeyId = 0x1020;
    const VMMU_SEGMENT_SIZE_KEY: KeyId = 0x1050;
    const FIFO_ENGINE_COUNT_KEY: KeyId = 0x0500;
    const FIFO_ENGINE_GMC_ID_KEY: KeyId = 0x0501;
    const FIFO_ENGINE_FLAGS_KEY: KeyId = 0x0502;
}

/// Payload of the `GSP_INIT` response.
#[cfg_attr(not(CONFIG_KUNIT), allow(dead_code))]
pub(crate) struct GspInitResponse {
    gpu_name: ArrayVec<u8, { Self::MAX_GPU_NAME_LEN }>,
    fb_regions: KVVec<FbRegion>,
    bar1_pde_base: u64,
    vmmu_segment_size: u64,
    fifo_engine_count: u32,
    fifo_engine_gmc_ids: [u32; MAX_FIFO_ENGINES],
    fifo_engine_flags: [u32; MAX_FIFO_ENGINES],
}

impl GspInitResponse {
    pub(crate) const MAX_GPU_NAME_LEN: usize = 64;

    /// A region with no tag is general-purpose memory. A tagged region is reserved for a
    /// firmware-internal use that the tag identifies.
    const FB_REGION_TAG_NONE: u32 = 0;

    /// Returns the GPU name, which GSP-RM sends with its NULL terminator.
    pub(crate) fn gpu_name(&self) -> &[u8] {
        self.gpu_name.as_slice()
    }

    /// Returns the BAR1 root page table address set up by GSP-RM firmware.
    pub(crate) fn bar1_pde_base(&self) -> u64 {
        self.bar1_pde_base
    }

    /// Iterates over the FB regions the driver may allocate from.
    ///
    /// A region qualifies when it is untagged, unprotected, and supports both compression and
    /// isochronous access.
    pub(crate) fn usable_fb_regions(&self) -> impl Iterator<Item = Range<u64>> + '_ {
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

    /// Returns the exclusive end of the FB physical address space.
    ///
    /// Every region counts here, including the tagged and protected ones that
    /// [`Self::usable_fb_regions`] rejects, because the address space has to span them all.
    ///
    /// Returns `None` if GSP-RM reported no region with a limit at or above its base.
    pub(crate) fn total_fb_end(&self) -> Option<u64> {
        self.fb_regions
            .iter()
            .filter(|region| region.limit >= region.base)
            .map(|region| region.limit)
            .max()?
            .checked_add(1)
    }

    /// Returns the VMMU segment size in bytes, or zero if GSP-RM omitted it.
    pub(in crate::gsp) const fn vmmu_segment_size(&self) -> u64 {
        self.vmmu_segment_size
    }

    /// Returns the FIFO engine count, limited to the supported table capacity.
    ///
    /// An omitted count is zero. Indexed entries outside the capacity are rejected during decode.
    pub(in crate::gsp) fn fifo_engine_count(&self) -> usize {
        num::u32_as_usize(self.fifo_engine_count).min(MAX_FIFO_ENGINES)
    }

    /// Returns GMC engine IDs by hardware FIFO order; omitted slots contain zero.
    pub(in crate::gsp) fn fifo_engine_gmc_ids(&self) -> &[u32; MAX_FIFO_ENGINES] {
        &self.fifo_engine_gmc_ids
    }

    /// Returns per-engine flags by hardware FIFO order; omitted slots contain zero.
    pub(in crate::gsp) fn fifo_engine_flags(&self) -> &[u32; MAX_FIFO_ENGINES] {
        &self.fifo_engine_flags
    }
}

nvkv_decode! {
    /// Schema for one FB region of the `GSP_INIT` response.
    struct FbRegionSchema => FbRegion {
        base: Required<u64, { Self::BASE_KEY }>,
        limit: Required<u64, { Self::LIMIT_KEY }>,
        flags: Required<FbRegionFlags, { Self::FLAGS_KEY }>,
        tag: Required<u32, { Self::TAG_KEY }>,
    }
}

impl FbRegionSchema {
    // Define the Key IDs read/written by GSP.
    const BASE_KEY: KeyId = 0x1011;
    const LIMIT_KEY: KeyId = 0x1012;
    const FLAGS_KEY: KeyId = 0x0012;
    const TAG_KEY: KeyId = 0x0013;
}

bitfield! {
    /// FB region attribute flags.
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

/// One FB memory region.
struct FbRegion {
    base: u64,
    limit: u64,
    flags: FbRegionFlags,
    tag: u32,
}

#[kunit_tests(nova_core_fw_commands)]
mod tests {
    use crate::gsp::nvkv::{
        Decoder,
        Index,
        UnknownKeyPolicy, //
    };

    use super::*;

    // Tests that `GspInitRequest` encodes correctly.
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

        let gsp_init = GspInitRequest {
            pci_device_id: 45.into(),
            pci_sub_device_id: 67.into(),
            pci_revision_id: 3.into(),
            pci_config_mirror_base: 0x1234_5678.into(),
            pci_config_mirror_size: 0x1000.into(),
            host_arch: HostArch::Aarch64.into(),
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
        let encoded = encoder.finish();
        assert_eq!(encoded.len(), 22);

        Ok(())
    }

    // Tests that FB region decoding fails when required keys are missing.
    #[test]
    fn decode_fb_region_missing_required_fails() -> Result {
        let mut encoder = Encoder::new();
        encoder.encode_u64(FbRegionSchema::BASE_KEY, Index::new::<0>(), 0x1000_0000)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore);
        let mut schema = FbRegionSchema::default();
        let init = decoder.decode(&mut schema)?;
        assert!(KBox::try_init(init, GFP_KERNEL).is_err());

        Ok(())
    }

    // Tests that a minimal and a full `GSP_INIT` response decode correctly.
    #[test]
    fn gsp_init_response() -> Result {
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
        const VMMU_SEGMENT_SIZE: u64 = 0x0200_0000;

        type Resp = GspInitResponseSchema;

        let index0 = Index::new::<0>();
        let index1 = Index::new::<1>();

        // A minimal response: only the BAR1 PDE base, so the FB region list stays empty.
        let mut encoder = Encoder::new();
        encoder.encode_u64(Resp::BAR1_PDE_BASE_KEY, index0, BAR1_PDE_BASE)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Ignore);
        let mut schema = Resp::default();
        let response = KBox::try_init(decoder.decode(&mut schema)?, GFP_KERNEL)?;
        assert_eq!(response.bar1_pde_base, BAR1_PDE_BASE);
        assert!(response.fb_regions.is_empty());

        // A full response.
        let mut encoder = Encoder::new();
        encoder.encode_array8(Resp::GPU_NAME_STRING_KEY, index0, name)?;
        encoder.encode_u64(Resp::BAR1_PDE_BASE_KEY, index0, BAR1_PDE_BASE)?;
        encoder.encode_u64(FbRegionSchema::BASE_KEY, index0, FB_REGION0_BASE)?;
        encoder.encode_u64(FbRegionSchema::LIMIT_KEY, index0, FB_REGION0_LIMIT)?;
        encoder.encode_u32(FbRegionSchema::FLAGS_KEY, index0, FB_REGION0_FLAGS)?;
        encoder.encode_u32(FbRegionSchema::TAG_KEY, index0, FB_REGION0_TAG)?;

        // Test that this unrelated key can safely interleave.
        encoder.encode_u64(Resp::VMMU_SEGMENT_SIZE_KEY, index0, VMMU_SEGMENT_SIZE)?;

        encoder.encode_u64(FbRegionSchema::BASE_KEY, index1, FB_REGION1_BASE)?;
        encoder.encode_u64(FbRegionSchema::LIMIT_KEY, index1, FB_REGION1_LIMIT)?;
        encoder.encode_u32(FbRegionSchema::FLAGS_KEY, index1, FB_REGION1_FLAGS)?;
        encoder.encode_u32(FbRegionSchema::TAG_KEY, index1, FB_REGION1_TAG)?;
        let data = encoder.finish();

        let decoder = Decoder::new(&data, UnknownKeyPolicy::Error);
        let mut schema = Resp::default();
        let response = KBox::try_init(decoder.decode(&mut schema)?, GFP_KERNEL)?;

        assert_eq!(&*response.gpu_name, &name[..]);
        assert_eq!(response.bar1_pde_base, BAR1_PDE_BASE);
        assert_eq!(response.fb_regions.len(), 2);

        let fb_region0 = &response.fb_regions[0];
        assert_eq!(fb_region0.base, FB_REGION0_BASE);
        assert_eq!(fb_region0.limit, FB_REGION0_LIMIT);
        assert_eq!(fb_region0.flags.into_raw(), FB_REGION0_FLAGS);
        assert!(fb_region0.flags.support_compressed());
        assert!(fb_region0.flags.support_iso());
        assert!(fb_region0.flags.protected());
        assert_eq!(fb_region0.tag, FB_REGION0_TAG);

        let fb_region1 = &response.fb_regions[1];
        assert_eq!(fb_region1.base, FB_REGION1_BASE);
        assert_eq!(fb_region1.limit, FB_REGION1_LIMIT);
        assert_eq!(fb_region1.flags.into_raw(), FB_REGION1_FLAGS);
        assert_eq!(fb_region1.tag, FB_REGION1_TAG);

        assert_eq!(response.vmmu_segment_size, VMMU_SEGMENT_SIZE);

        Ok(())
    }
}
