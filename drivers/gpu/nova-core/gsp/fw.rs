// SPDX-License-Identifier: GPL-2.0

pub(crate) mod commands;
mod r000_00;
mod r570_144;

use r570_144 as r570;
use r000_00 as r000;

use core::ops::Range;

use kernel::{
    dma::Coherent,
    prelude::*,
    ptr::{
        Alignable,
        Alignment,
        KnownSize, //
    },
    sizes::{
        SZ_128K,
        SZ_1M, //
    },
    transmute::{
        AsBytes,
        FromBytes, //
    },
};

use crate::{
    fb::{
        FbLayout,
        PMU_RESERVED_SIZE, //
    },
    firmware::gsp::GspFirmware,
    gpu::Chipset,
    gsp::{
        cmdq::Cmdq,
        GSP_PAGE_SHIFT,
        GSP_PAGE_SIZE, //
    },
    num::{
        self,
        FromSafeCast, //
    },
};

pub(super) mod gsp_mem {
    use core::sync::atomic::{
        fence,
        Ordering, //
    };

    use kernel::dma::Coherent;

    use crate::gsp::cmdq::{
        GspMem,
        MSGQ_NUM_PAGES, //
    };

    pub(in crate::gsp) fn gsp_write_ptr(qs: &Coherent<GspMem>) -> u32 {
        kernel::io_read!(qs, .gspq.tx.0.writePtr) % MSGQ_NUM_PAGES
    }

    pub(in crate::gsp) fn gsp_read_ptr(qs: &Coherent<GspMem>) -> u32 {
        kernel::io_read!(qs, .gspq.rx.0.readPtr) % MSGQ_NUM_PAGES
    }

    pub(in crate::gsp) fn cpu_read_ptr(qs: &Coherent<GspMem>) -> u32 {
        kernel::io_read!(qs, .cpuq.rx.0.readPtr) % MSGQ_NUM_PAGES
    }

    pub(in crate::gsp) fn advance_cpu_read_ptr(qs: &Coherent<GspMem>, count: u32) {
        let rptr = cpu_read_ptr(qs).wrapping_add(count) % MSGQ_NUM_PAGES;

        // Ensure read pointer is properly ordered.
        fence(Ordering::SeqCst);

        kernel::io_write!(qs, .cpuq.rx.0.readPtr, rptr);
    }

    pub(in crate::gsp) fn cpu_write_ptr(qs: &Coherent<GspMem>) -> u32 {
        kernel::io_read!(qs, .cpuq.tx.0.writePtr) % MSGQ_NUM_PAGES
    }

    pub(in crate::gsp) fn advance_cpu_write_ptr(qs: &Coherent<GspMem>, count: u32) {
        let wptr = cpu_write_ptr(qs).wrapping_add(count) % MSGQ_NUM_PAGES;

        kernel::io_write!(qs, .cpuq.tx.0.writePtr, wptr);

        // Ensure all command data is visible before triggering the GSP read.
        fence(Ordering::SeqCst);
    }
}

/// Maximum size of a single GSP message queue element in bytes.
pub(crate) const GSP_MSG_QUEUE_ELEMENT_SIZE_MAX: usize =
    num::u32_as_usize(r570::GSP_MSG_QUEUE_ELEMENT_SIZE_MAX);

/// Status code returned by GSP-RM RPCs.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NvStatus {
    Ok,
    AlreadySignalled,
    BrokenFb,
    BufferTooSmall,
    BusyRetry,
    CallbackNotScheduled,
    CardNotPresent,
    CycleDetected,
    DmaInUse,
    DmaMemNotLocked,
    DmaMemNotUnlocked,
    DualLinkInuse,
    EccError,
    FabricManagerNotPresent,
    FatalError,
    FeatureNotEnabled,
    FifoBadAccess,
    FlcnError,
    FreqNotSupported,
    Generic,
    GpuDmaNotInitialized,
    GpuInDebugMode,
    GpuInFullchipReset,
    GpuIsLost,
    GpuMemoryOnliningFailure,
    GpuNotFullPower,
    GpuUuidNotFound,
    HotSwitch,
    I2cError,
    I2cSpeedTooHigh,
    IllegalAction,
    InUse,
    InflateCompressedDataFailed,
    InsertDuplicateName,
    InsufficientPermissions,
    InsufficientPower,
    InsufficientResources,
    InsufficientZbcEntry,
    InvalidAccessType,
    InvalidAddress,
    InvalidArgument,
    InvalidBase,
    InvalidChannel,
    InvalidClass,
    InvalidClient,
    InvalidCommand,
    InvalidData,
    InvalidDevice,
    InvalidDmaSpecifier,
    InvalidEvent,
    InvalidFlags,
    InvalidFunction,
    InvalidHeap,
    InvalidIndex,
    InvalidIrqLevel,
    InvalidLicense,
    InvalidLimit,
    InvalidLockState,
    InvalidMethod,
    InvalidObject,
    InvalidObjectBuffer,
    InvalidObjectHandle,
    InvalidObjectNew,
    InvalidObjectOld,
    InvalidObjectParent,
    InvalidOffset,
    InvalidOperation,
    InvalidOwner,
    InvalidParamStruct,
    InvalidParameter,
    InvalidPath,
    InvalidPointer,
    InvalidRead,
    InvalidRegistryKey,
    InvalidRequest,
    InvalidState,
    InvalidStringLength,
    InvalidWrite,
    InvalidXlate,
    IrqEdgeTriggered,
    IrqNotFiring,
    KeyRotationInProgress,
    LibRmVersionMismatch,
    MaxSessionLimitReached,
    MemoryError,
    MemoryTrainingFailed,
    MismatchedSlave,
    MismatchedTarget,
    MissingTableEntry,
    ModuleLoadFailed,
    MoreDataAvailable,
    MoreProcessingRequired,
    MultipleMemoryTypes,
    NoFreeFifos,
    NoIntrPending,
    NoMemory,
    NoSuchDomain,
    NoValidPath,
    NotCompatible,
    NotReady,
    NotSupported,
    NvlinkClockError,
    NvlinkConfigurationError,
    NvlinkFabricFailure,
    NvlinkFabricNotReady,
    NvlinkInitError,
    NvlinkMinionError,
    NvlinkTrainingError,
    ObjectNotFound,
    ObjectTypeMismatch,
    OperatingSystem,
    OtherDeviceFound,
    OutOfRange,
    OverlappingUvmCommit,
    PageTableNotAvail,
    PidNotFound,
    PmuNotReady,
    PrivSecViolation,
    ProtectionFault,
    QueueTaskSlotNotAvailable,
    RcError,
    ReductionManagerNotAvailable,
    RejectedVbios,
    ResetRequired,
    ResourceLost,
    ResourceRetirementError,
    RiscvError,
    SecureBootFailed,
    SignalPending,
    StateInUse,
    TestOnlyCodeNotEnabled,
    Timeout,
    TimeoutRetry,
    TooManyPrimaries,
    UvmAddressInUse,
    Unknown(u32),
}

impl From<NvStatus> for Result {
    fn from(status: NvStatus) -> Self {
        match status {
            NvStatus::Ok => Ok(()),

            NvStatus::BufferTooSmall | NvStatus::MoreDataAvailable => Err(EMSGSIZE),

            NvStatus::BusyRetry
            | NvStatus::DmaInUse
            | NvStatus::DualLinkInuse
            | NvStatus::GpuInFullchipReset
            | NvStatus::InUse
            | NvStatus::KeyRotationInProgress
            | NvStatus::NotReady
            | NvStatus::NvlinkFabricNotReady
            | NvStatus::PmuNotReady
            | NvStatus::StateInUse
            | NvStatus::UvmAddressInUse => Err(EBUSY),

            NvStatus::CardNotPresent
            | NvStatus::FabricManagerNotPresent
            | NvStatus::InvalidDevice
            | NvStatus::ReductionManagerNotAvailable => Err(ENODEV),

            NvStatus::FeatureNotEnabled
            | NvStatus::FreqNotSupported
            | NvStatus::NotSupported
            | NvStatus::TestOnlyCodeNotEnabled => Err(ENOTSUPP),

            NvStatus::GpuUuidNotFound
            | NvStatus::MissingTableEntry
            | NvStatus::NoSuchDomain
            | NvStatus::NoValidPath
            | NvStatus::ObjectNotFound => Err(ENOENT),

            NvStatus::I2cSpeedTooHigh
            | NvStatus::InvalidAccessType
            | NvStatus::InvalidArgument
            | NvStatus::InvalidBase
            | NvStatus::InvalidChannel
            | NvStatus::InvalidClass
            | NvStatus::InvalidClient
            | NvStatus::InvalidCommand
            | NvStatus::InvalidData
            | NvStatus::InvalidDmaSpecifier
            | NvStatus::InvalidEvent
            | NvStatus::InvalidFlags
            | NvStatus::InvalidFunction
            | NvStatus::InvalidHeap
            | NvStatus::InvalidIndex
            | NvStatus::InvalidIrqLevel
            | NvStatus::InvalidLimit
            | NvStatus::InvalidLockState
            | NvStatus::InvalidMethod
            | NvStatus::InvalidObject
            | NvStatus::InvalidObjectBuffer
            | NvStatus::InvalidObjectHandle
            | NvStatus::InvalidObjectNew
            | NvStatus::InvalidObjectOld
            | NvStatus::InvalidObjectParent
            | NvStatus::InvalidOffset
            | NvStatus::InvalidOperation
            | NvStatus::InvalidOwner
            | NvStatus::InvalidParamStruct
            | NvStatus::InvalidParameter
            | NvStatus::InvalidPath
            | NvStatus::InvalidRegistryKey
            | NvStatus::InvalidRequest
            | NvStatus::InvalidState
            | NvStatus::InvalidStringLength
            | NvStatus::InvalidXlate
            | NvStatus::LibRmVersionMismatch
            | NvStatus::MismatchedSlave
            | NvStatus::MismatchedTarget
            | NvStatus::MultipleMemoryTypes
            | NvStatus::NotCompatible
            | NvStatus::ObjectTypeMismatch
            | NvStatus::OverlappingUvmCommit
            | NvStatus::RejectedVbios => Err(EINVAL),

            NvStatus::IllegalAction => Err(EPERM),

            NvStatus::InsertDuplicateName => Err(EEXIST),

            NvStatus::InsufficientPermissions
            | NvStatus::InvalidLicense
            | NvStatus::PrivSecViolation => Err(EACCES),

            NvStatus::InsufficientResources | NvStatus::NoMemory | NvStatus::PageTableNotAvail => {
                Err(ENOMEM)
            }

            NvStatus::InsufficientZbcEntry
            | NvStatus::MaxSessionLimitReached
            | NvStatus::NoFreeFifos
            | NvStatus::QueueTaskSlotNotAvailable
            | NvStatus::TooManyPrimaries => Err(ENOSPC),

            NvStatus::InvalidAddress | NvStatus::InvalidPointer | NvStatus::ProtectionFault => {
                Err(EFAULT)
            }

            NvStatus::MoreProcessingRequired | NvStatus::TimeoutRetry => Err(EAGAIN),

            NvStatus::OutOfRange => Err(ERANGE),

            NvStatus::PidNotFound => Err(ESRCH),

            NvStatus::SignalPending => Err(EINTR),

            NvStatus::Timeout => Err(ETIMEDOUT),

            _ => Err(EIO),
        }
    }
}

impl From<u32> for NvStatus {
    fn from(value: u32) -> Self {
        match value {
            r570::NV_OK => Self::Ok,
            r570::NV_ERR_ALREADY_SIGNALLED => Self::AlreadySignalled,
            r570::NV_ERR_BROKEN_FB => Self::BrokenFb,
            r570::NV_ERR_BUFFER_TOO_SMALL => Self::BufferTooSmall,
            r570::NV_ERR_BUSY_RETRY => Self::BusyRetry,
            r570::NV_ERR_CALLBACK_NOT_SCHEDULED => Self::CallbackNotScheduled,
            r570::NV_ERR_CARD_NOT_PRESENT => Self::CardNotPresent,
            r570::NV_ERR_CYCLE_DETECTED => Self::CycleDetected,
            r570::NV_ERR_DMA_IN_USE => Self::DmaInUse,
            r570::NV_ERR_DMA_MEM_NOT_LOCKED => Self::DmaMemNotLocked,
            r570::NV_ERR_DMA_MEM_NOT_UNLOCKED => Self::DmaMemNotUnlocked,
            r570::NV_ERR_DUAL_LINK_INUSE => Self::DualLinkInuse,
            r570::NV_ERR_ECC_ERROR => Self::EccError,
            r570::NV_ERR_FABRIC_MANAGER_NOT_PRESENT => Self::FabricManagerNotPresent,
            r570::NV_ERR_FATAL_ERROR => Self::FatalError,
            r570::NV_ERR_FEATURE_NOT_ENABLED => Self::FeatureNotEnabled,
            r570::NV_ERR_FIFO_BAD_ACCESS => Self::FifoBadAccess,
            r570::NV_ERR_FLCN_ERROR => Self::FlcnError,
            r570::NV_ERR_FREQ_NOT_SUPPORTED => Self::FreqNotSupported,
            r570::NV_ERR_GENERIC => Self::Generic,
            r570::NV_ERR_GPU_DMA_NOT_INITIALIZED => Self::GpuDmaNotInitialized,
            r570::NV_ERR_GPU_IN_DEBUG_MODE => Self::GpuInDebugMode,
            r570::NV_ERR_GPU_IN_FULLCHIP_RESET => Self::GpuInFullchipReset,
            r570::NV_ERR_GPU_IS_LOST => Self::GpuIsLost,
            r570::NV_ERR_GPU_MEMORY_ONLINING_FAILURE => Self::GpuMemoryOnliningFailure,
            r570::NV_ERR_GPU_NOT_FULL_POWER => Self::GpuNotFullPower,
            r570::NV_ERR_GPU_UUID_NOT_FOUND => Self::GpuUuidNotFound,
            r570::NV_ERR_HOT_SWITCH => Self::HotSwitch,
            r570::NV_ERR_I2C_ERROR => Self::I2cError,
            r570::NV_ERR_I2C_SPEED_TOO_HIGH => Self::I2cSpeedTooHigh,
            r570::NV_ERR_ILLEGAL_ACTION => Self::IllegalAction,
            r570::NV_ERR_IN_USE => Self::InUse,
            r570::NV_ERR_INFLATE_COMPRESSED_DATA_FAILED => Self::InflateCompressedDataFailed,
            r570::NV_ERR_INSERT_DUPLICATE_NAME => Self::InsertDuplicateName,
            r570::NV_ERR_INSUFFICIENT_PERMISSIONS => Self::InsufficientPermissions,
            r570::NV_ERR_INSUFFICIENT_POWER => Self::InsufficientPower,
            r570::NV_ERR_INSUFFICIENT_RESOURCES => Self::InsufficientResources,
            r570::NV_ERR_INSUFFICIENT_ZBC_ENTRY => Self::InsufficientZbcEntry,
            r570::NV_ERR_INVALID_ACCESS_TYPE => Self::InvalidAccessType,
            r570::NV_ERR_INVALID_ADDRESS => Self::InvalidAddress,
            r570::NV_ERR_INVALID_ARGUMENT => Self::InvalidArgument,
            r570::NV_ERR_INVALID_BASE => Self::InvalidBase,
            r570::NV_ERR_INVALID_CHANNEL => Self::InvalidChannel,
            r570::NV_ERR_INVALID_CLASS => Self::InvalidClass,
            r570::NV_ERR_INVALID_CLIENT => Self::InvalidClient,
            r570::NV_ERR_INVALID_COMMAND => Self::InvalidCommand,
            r570::NV_ERR_INVALID_DATA => Self::InvalidData,
            r570::NV_ERR_INVALID_DEVICE => Self::InvalidDevice,
            r570::NV_ERR_INVALID_DMA_SPECIFIER => Self::InvalidDmaSpecifier,
            r570::NV_ERR_INVALID_EVENT => Self::InvalidEvent,
            r570::NV_ERR_INVALID_FLAGS => Self::InvalidFlags,
            r570::NV_ERR_INVALID_FUNCTION => Self::InvalidFunction,
            r570::NV_ERR_INVALID_HEAP => Self::InvalidHeap,
            r570::NV_ERR_INVALID_INDEX => Self::InvalidIndex,
            r570::NV_ERR_INVALID_IRQ_LEVEL => Self::InvalidIrqLevel,
            r570::NV_ERR_INVALID_LICENSE => Self::InvalidLicense,
            r570::NV_ERR_INVALID_LIMIT => Self::InvalidLimit,
            r570::NV_ERR_INVALID_LOCK_STATE => Self::InvalidLockState,
            r570::NV_ERR_INVALID_METHOD => Self::InvalidMethod,
            r570::NV_ERR_INVALID_OBJECT => Self::InvalidObject,
            r570::NV_ERR_INVALID_OBJECT_BUFFER => Self::InvalidObjectBuffer,
            r570::NV_ERR_INVALID_OBJECT_HANDLE => Self::InvalidObjectHandle,
            r570::NV_ERR_INVALID_OBJECT_NEW => Self::InvalidObjectNew,
            r570::NV_ERR_INVALID_OBJECT_OLD => Self::InvalidObjectOld,
            r570::NV_ERR_INVALID_OBJECT_PARENT => Self::InvalidObjectParent,
            r570::NV_ERR_INVALID_OFFSET => Self::InvalidOffset,
            r570::NV_ERR_INVALID_OPERATION => Self::InvalidOperation,
            r570::NV_ERR_INVALID_OWNER => Self::InvalidOwner,
            r570::NV_ERR_INVALID_PARAM_STRUCT => Self::InvalidParamStruct,
            r570::NV_ERR_INVALID_PARAMETER => Self::InvalidParameter,
            r570::NV_ERR_INVALID_PATH => Self::InvalidPath,
            r570::NV_ERR_INVALID_POINTER => Self::InvalidPointer,
            r570::NV_ERR_INVALID_READ => Self::InvalidRead,
            r570::NV_ERR_INVALID_REGISTRY_KEY => Self::InvalidRegistryKey,
            r570::NV_ERR_INVALID_REQUEST => Self::InvalidRequest,
            r570::NV_ERR_INVALID_STATE => Self::InvalidState,
            r570::NV_ERR_INVALID_STRING_LENGTH => Self::InvalidStringLength,
            r570::NV_ERR_INVALID_WRITE => Self::InvalidWrite,
            r570::NV_ERR_INVALID_XLATE => Self::InvalidXlate,
            r570::NV_ERR_IRQ_EDGE_TRIGGERED => Self::IrqEdgeTriggered,
            r570::NV_ERR_IRQ_NOT_FIRING => Self::IrqNotFiring,
            r570::NV_ERR_KEY_ROTATION_IN_PROGRESS => Self::KeyRotationInProgress,
            r570::NV_ERR_LIB_RM_VERSION_MISMATCH => Self::LibRmVersionMismatch,
            r570::NV_ERR_MAX_SESSION_LIMIT_REACHED => Self::MaxSessionLimitReached,
            r570::NV_ERR_MEMORY_ERROR => Self::MemoryError,
            r570::NV_ERR_MEMORY_TRAINING_FAILED => Self::MemoryTrainingFailed,
            r570::NV_ERR_MISMATCHED_SLAVE => Self::MismatchedSlave,
            r570::NV_ERR_MISMATCHED_TARGET => Self::MismatchedTarget,
            r570::NV_ERR_MISSING_TABLE_ENTRY => Self::MissingTableEntry,
            r570::NV_ERR_MODULE_LOAD_FAILED => Self::ModuleLoadFailed,
            r570::NV_ERR_MORE_DATA_AVAILABLE => Self::MoreDataAvailable,
            r570::NV_ERR_MORE_PROCESSING_REQUIRED => Self::MoreProcessingRequired,
            r570::NV_ERR_MULTIPLE_MEMORY_TYPES => Self::MultipleMemoryTypes,
            r570::NV_ERR_NO_FREE_FIFOS => Self::NoFreeFifos,
            r570::NV_ERR_NO_INTR_PENDING => Self::NoIntrPending,
            r570::NV_ERR_NO_MEMORY => Self::NoMemory,
            r570::NV_ERR_NO_SUCH_DOMAIN => Self::NoSuchDomain,
            r570::NV_ERR_NO_VALID_PATH => Self::NoValidPath,
            r570::NV_ERR_NOT_COMPATIBLE => Self::NotCompatible,
            r570::NV_ERR_NOT_READY => Self::NotReady,
            r570::NV_ERR_NOT_SUPPORTED => Self::NotSupported,
            r570::NV_ERR_NVLINK_CLOCK_ERROR => Self::NvlinkClockError,
            r570::NV_ERR_NVLINK_CONFIGURATION_ERROR => Self::NvlinkConfigurationError,
            r570::NV_ERR_NVLINK_FABRIC_FAILURE => Self::NvlinkFabricFailure,
            r570::NV_ERR_NVLINK_FABRIC_NOT_READY => Self::NvlinkFabricNotReady,
            r570::NV_ERR_NVLINK_INIT_ERROR => Self::NvlinkInitError,
            r570::NV_ERR_NVLINK_MINION_ERROR => Self::NvlinkMinionError,
            r570::NV_ERR_NVLINK_TRAINING_ERROR => Self::NvlinkTrainingError,
            r570::NV_ERR_OBJECT_NOT_FOUND => Self::ObjectNotFound,
            r570::NV_ERR_OBJECT_TYPE_MISMATCH => Self::ObjectTypeMismatch,
            r570::NV_ERR_OPERATING_SYSTEM => Self::OperatingSystem,
            r570::NV_ERR_OTHER_DEVICE_FOUND => Self::OtherDeviceFound,
            r570::NV_ERR_OUT_OF_RANGE => Self::OutOfRange,
            r570::NV_ERR_OVERLAPPING_UVM_COMMIT => Self::OverlappingUvmCommit,
            r570::NV_ERR_PAGE_TABLE_NOT_AVAIL => Self::PageTableNotAvail,
            r570::NV_ERR_PID_NOT_FOUND => Self::PidNotFound,
            r570::NV_ERR_PMU_NOT_READY => Self::PmuNotReady,
            r570::NV_ERR_PRIV_SEC_VIOLATION => Self::PrivSecViolation,
            r570::NV_ERR_PROTECTION_FAULT => Self::ProtectionFault,
            r570::NV_ERR_QUEUE_TASK_SLOT_NOT_AVAILABLE => Self::QueueTaskSlotNotAvailable,
            r570::NV_ERR_RC_ERROR => Self::RcError,
            r570::NV_ERR_REDUCTION_MANAGER_NOT_AVAILABLE => Self::ReductionManagerNotAvailable,
            r570::NV_ERR_REJECTED_VBIOS => Self::RejectedVbios,
            r570::NV_ERR_RESET_REQUIRED => Self::ResetRequired,
            r570::NV_ERR_RESOURCE_LOST => Self::ResourceLost,
            r570::NV_ERR_RESOURCE_RETIREMENT_ERROR => Self::ResourceRetirementError,
            r570::NV_ERR_RISCV_ERROR => Self::RiscvError,
            r570::NV_ERR_SECURE_BOOT_FAILED => Self::SecureBootFailed,
            r570::NV_ERR_SIGNAL_PENDING => Self::SignalPending,
            r570::NV_ERR_STATE_IN_USE => Self::StateInUse,
            r570::NV_ERR_TEST_ONLY_CODE_NOT_ENABLED => Self::TestOnlyCodeNotEnabled,
            r570::NV_ERR_TIMEOUT => Self::Timeout,
            r570::NV_ERR_TIMEOUT_RETRY => Self::TimeoutRetry,
            r570::NV_ERR_TOO_MANY_PRIMARIES => Self::TooManyPrimaries,
            r570::NV_ERR_UVM_ADDRESS_IN_USE => Self::UvmAddressInUse,
            other => Self::Unknown(other),
        }
    }
}

/// Empty type to group methods related to heap parameters for running the GSP firmware.
enum GspFwHeapParams {}

/// Minimum required alignment for the GSP heap.
const GSP_HEAP_ALIGNMENT: Alignment = Alignment::new::<{ 1 << 20 }>();

// These constants override the generated bindings for architecture-specific heap sizing.
// See Open RM: kgspCalculateGspFwHeapSize and related functions.
//
// 14MB for Hopper/Blackwell+.
const GSP_FW_HEAP_PARAM_BASE_RM_SIZE_GH100: u64 = 14 * num::usize_as_u64(SZ_1M);
// 142MB client alloc for ~188MB total.
const GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE_GH100: u64 = 142 * num::usize_as_u64(SZ_1M);
// Hopper/Blackwell+ minimum heap size: 170MB (88 + 12 + 70).
// See Open RM: GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB for the base 88MB,
// plus Hopper+ additions in kgspCalculateGspFwHeapSize_GH100.
const GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB_HOPPER: u64 = 170;

impl GspFwHeapParams {
    /// Returns the amount of GSP-RM heap memory used during GSP-RM boot and initialization (up to
    /// and including the first client subdevice allocation).
    fn base_rm_size(chipset: Chipset) -> u64 {
        use crate::gpu::Architecture;
        match chipset.arch() {
            Architecture::Hopper | Architecture::Blackwell => {
                GSP_FW_HEAP_PARAM_BASE_RM_SIZE_GH100
            }
            _ => u64::from(r570::GSP_FW_HEAP_PARAM_BASE_RM_SIZE_TU10X),
        }
    }

    /// Returns the amount of heap memory required to support a single channel allocation.
    fn client_alloc_size(chipset: Chipset) -> Result<u64> {
        use crate::gpu::Architecture;
        let size = match chipset.arch() {
            Architecture::Hopper | Architecture::Blackwell => {
                GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE_GH100
            }
            _ => u64::from(r570::GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE),
        };
        size.align_up(GSP_HEAP_ALIGNMENT).ok_or(EINVAL)
    }

    /// Returns the amount of memory to reserve for management purposes for a framebuffer of size
    /// `fb_size`.
    fn management_overhead(fb_size: u64) -> Result<u64> {
        let fb_size_gb = fb_size.div_ceil(u64::from_safe_cast(kernel::sizes::SZ_1G));

        u64::from(r570::GSP_FW_HEAP_PARAM_SIZE_PER_GB_FB)
            .saturating_mul(fb_size_gb)
            .align_up(GSP_HEAP_ALIGNMENT)
            .ok_or(EINVAL)
    }
}

/// Heap memory requirements and constraints for a given version of the GSP LIBOS.
pub(crate) struct LibosParams {
    /// The base amount of heap required by the GSP operating system, in bytes.
    carveout_size: u64,
    /// The minimum and maximum sizes allowed for the GSP FW heap, in bytes.
    allowed_heap_size: Range<u64>,
}

impl LibosParams {
    /// Version 2 of the GSP LIBOS (Turing and GA100)
    const LIBOS2: LibosParams = LibosParams {
        carveout_size: num::u32_as_u64(r570::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS2),
        allowed_heap_size: num::u32_as_u64(r570::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS2_MIN_MB)
            * num::usize_as_u64(SZ_1M)
            ..num::u32_as_u64(r570::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS2_MAX_MB)
                * num::usize_as_u64(SZ_1M),
    };

    /// Version 3 of the GSP LIBOS (GA102+)
    const LIBOS3: LibosParams = LibosParams {
        carveout_size: num::u32_as_u64(r570::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS3_BAREMETAL),
        allowed_heap_size: num::u32_as_u64(
            r570::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB,
        ) * num::usize_as_u64(SZ_1M)
            ..num::u32_as_u64(r570::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MAX_MB)
                * num::usize_as_u64(SZ_1M),
    };

    /// Hopper/Blackwell+ GPUs need a larger minimum heap size than the bindings specify.
    /// The r570 bindings set LIBOS3_BAREMETAL_MIN_MB to 88MB, but Hopper/Blackwell+ actually
    /// requires 170MB (88 + 12 + 70).
    const LIBOS_HOPPER: LibosParams = LibosParams {
        carveout_size: num::u32_as_u64(r570::GSP_FW_HEAP_PARAM_OS_SIZE_LIBOS3_BAREMETAL),
        allowed_heap_size: GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MIN_MB_HOPPER
            * num::usize_as_u64(SZ_1M)
            ..num::u32_as_u64(r570::GSP_FW_HEAP_SIZE_OVERRIDE_LIBOS3_BAREMETAL_MAX_MB)
                * num::usize_as_u64(SZ_1M),
    };

    /// Returns the libos parameters corresponding to `chipset`.
    pub(crate) fn from_chipset(chipset: Chipset) -> &'static LibosParams {
        use crate::gpu::Architecture;
        match chipset.arch() {
            Architecture::Turing => &Self::LIBOS2,
            Architecture::Ampere if chipset == Chipset::GA100 => &Self::LIBOS2,
            Architecture::Ampere | Architecture::Ada => &Self::LIBOS3,
            Architecture::Hopper | Architecture::Blackwell => &Self::LIBOS_HOPPER,
        }
    }

    /// Returns the amount of memory (in bytes) to allocate for the WPR heap for a framebuffer size
    /// of `fb_size` (in bytes) for `chipset`.
    pub(crate) fn wpr_heap_size(&self, chipset: Chipset, fb_size: u64) -> Result<u64> {
        // The WPR heap will contain the following:
        // LIBOS carveout,
        Ok(self
            .carveout_size
            // RM boot working memory,
            .saturating_add(GspFwHeapParams::base_rm_size(chipset))
            // One RM client,
            .saturating_add(GspFwHeapParams::client_alloc_size(chipset)?)
            // Overhead for memory management.
            .saturating_add(GspFwHeapParams::management_overhead(fb_size)?)
            // Clamp to the supported heap sizes.
            .clamp(self.allowed_heap_size.start, self.allowed_heap_size.end - 1))
    }
}

/// Structure passed to the GSP bootloader, containing the framebuffer layout as well as the DMA
/// addresses of the GSP bootloader and firmware.
#[repr(transparent)]
pub(crate) struct GspFwWprMeta(r570::GspFwWprMeta);

// SAFETY: Padding is explicit and does not contain uninitialized data.
unsafe impl AsBytes for GspFwWprMeta {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for GspFwWprMeta {}

type GspFwWprMetaBootResumeInfo = r570::GspFwWprMeta__bindgen_ty_1;
type GspFwWprMetaBootInfo = r570::GspFwWprMeta__bindgen_ty_1__bindgen_ty_1;

impl GspFwWprMeta {
    /// Fill in and return a `GspFwWprMeta` suitable for booting `gsp_firmware` using the
    /// `fb_layout` layout.
    pub(crate) fn new(gsp_firmware: &GspFirmware, fb_layout: &FbLayout) -> Self {
        Self(r570::GspFwWprMeta {
            // CAST: we want to store the bits of `GSP_FW_WPR_META_MAGIC` unmodified.
            magic: r570::GSP_FW_WPR_META_MAGIC as u64,
            revision: u64::from(r570::GSP_FW_WPR_META_REVISION),
            sysmemAddrOfRadix3Elf: gsp_firmware.radix3_dma_handle(),
            sizeOfRadix3Elf: u64::from_safe_cast(gsp_firmware.size()),
            sysmemAddrOfBootloader: gsp_firmware.bootloader.ucode.dma_handle(),
            sizeOfBootloader: u64::from_safe_cast(gsp_firmware.bootloader.ucode.size()),
            bootloaderCodeOffset: u64::from(gsp_firmware.bootloader.code_offset),
            bootloaderDataOffset: u64::from(gsp_firmware.bootloader.data_offset),
            bootloaderManifestOffset: u64::from(gsp_firmware.bootloader.manifest_offset),
            __bindgen_anon_1: GspFwWprMetaBootResumeInfo {
                __bindgen_anon_1: GspFwWprMetaBootInfo {
                    sysmemAddrOfSignature: gsp_firmware.signatures.dma_handle(),
                    sizeOfSignature: u64::from_safe_cast(gsp_firmware.signatures.size()),
                },
            },
            gspFwRsvdStart: fb_layout.heap.start,
            nonWprHeapOffset: fb_layout.heap.start,
            nonWprHeapSize: fb_layout.heap.end - fb_layout.heap.start,
            gspFwWprStart: fb_layout.wpr2.start,
            gspFwHeapOffset: fb_layout.wpr2_heap.start,
            gspFwHeapSize: fb_layout.wpr2_heap.end - fb_layout.wpr2_heap.start,
            gspFwOffset: fb_layout.elf.start,
            bootBinOffset: fb_layout.boot.start,
            frtsOffset: fb_layout.frts.start,
            frtsSize: fb_layout.frts.end - fb_layout.frts.start,
            gspFwWprEnd: fb_layout
                .vga_workspace
                .start
                .align_down(Alignment::new::<SZ_128K>()),
            gspFwHeapVfPartitionCount: fb_layout.vf_partition_count,
            fbSize: fb_layout.fb.end - fb_layout.fb.start,
            vgaWorkspaceOffset: fb_layout.vga_workspace.start,
            vgaWorkspaceSize: fb_layout.vga_workspace.end - fb_layout.vga_workspace.start,
            pmuReservedSize: PMU_RESERVED_SIZE,
            ..Default::default()
        })
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u32)]
pub(crate) enum MsgFunction {
    // Common function codes
    AllocChannelDma = r570::NV_VGPU_MSG_FUNCTION_ALLOC_CHANNEL_DMA,
    AllocCtxDma = r570::NV_VGPU_MSG_FUNCTION_ALLOC_CTX_DMA,
    AllocDevice = r570::NV_VGPU_MSG_FUNCTION_ALLOC_DEVICE,
    AllocMemory = r570::NV_VGPU_MSG_FUNCTION_ALLOC_MEMORY,
    AllocObject = r570::NV_VGPU_MSG_FUNCTION_ALLOC_OBJECT,
    AllocRoot = r570::NV_VGPU_MSG_FUNCTION_ALLOC_ROOT,
    BindCtxDma = r570::NV_VGPU_MSG_FUNCTION_BIND_CTX_DMA,
    ContinuationRecord = r570::NV_VGPU_MSG_FUNCTION_CONTINUATION_RECORD,
    Free = r570::NV_VGPU_MSG_FUNCTION_FREE,
    GetGspStaticInfo = r570::NV_VGPU_MSG_FUNCTION_GET_GSP_STATIC_INFO,
    GetStaticInfo = r570::NV_VGPU_MSG_FUNCTION_GET_STATIC_INFO,
    GspInitPostObjGpu = r570::NV_VGPU_MSG_FUNCTION_GSP_INIT_POST_OBJGPU,
    GspRmControl = r570::NV_VGPU_MSG_FUNCTION_GSP_RM_CONTROL,
    GspSetSystemInfo = r570::NV_VGPU_MSG_FUNCTION_GSP_SET_SYSTEM_INFO,
    Log = r570::NV_VGPU_MSG_FUNCTION_LOG,
    MapMemory = r570::NV_VGPU_MSG_FUNCTION_MAP_MEMORY,
    Nop = r570::NV_VGPU_MSG_FUNCTION_NOP,
    SetGuestSystemInfo = r570::NV_VGPU_MSG_FUNCTION_SET_GUEST_SYSTEM_INFO,
    SetRegistry = r570::NV_VGPU_MSG_FUNCTION_SET_REGISTRY,

    // Event codes
    GspInitDone = r570::NV_VGPU_MSG_EVENT_GSP_INIT_DONE,
    GspLockdownNotice = r570::NV_VGPU_MSG_EVENT_GSP_LOCKDOWN_NOTICE,
    GspLoadExecGenericBootloader = r000::NV_VGPU_MSG_EVENT_GSP_LOAD_EXEC_GENERIC_BOOTLOADER,
    GspLoadExecHsBinary = r000::NV_VGPU_MSG_EVENT_GSP_LOAD_EXEC_HS_BINARY,
    GspPostNoCat = r570::NV_VGPU_MSG_EVENT_GSP_POST_NOCAT_RECORD,
    MmuFaultQueued = r570::NV_VGPU_MSG_EVENT_MMU_FAULT_QUEUED,
    OsErrorLog = r570::NV_VGPU_MSG_EVENT_OS_ERROR_LOG,
    PostEvent = r570::NV_VGPU_MSG_EVENT_POST_EVENT,
    RcTriggered = r570::NV_VGPU_MSG_EVENT_RC_TRIGGERED,
    UcodeLibOsPrint = r570::NV_VGPU_MSG_EVENT_UCODE_LIBOS_PRINT,
}

impl TryFrom<u32> for MsgFunction {
    type Error = kernel::error::Error;

    fn try_from(value: u32) -> Result<MsgFunction> {
        match value {
            // Common function codes
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_CHANNEL_DMA => Ok(MsgFunction::AllocChannelDma),
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_CTX_DMA => Ok(MsgFunction::AllocCtxDma),
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_DEVICE => Ok(MsgFunction::AllocDevice),
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_MEMORY => Ok(MsgFunction::AllocMemory),
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_OBJECT => Ok(MsgFunction::AllocObject),
            r570::NV_VGPU_MSG_FUNCTION_ALLOC_ROOT => Ok(MsgFunction::AllocRoot),
            r570::NV_VGPU_MSG_FUNCTION_BIND_CTX_DMA => Ok(MsgFunction::BindCtxDma),
            r570::NV_VGPU_MSG_FUNCTION_CONTINUATION_RECORD => {
                Ok(MsgFunction::ContinuationRecord)
            }
            r570::NV_VGPU_MSG_FUNCTION_FREE => Ok(MsgFunction::Free),
            r570::NV_VGPU_MSG_FUNCTION_GET_GSP_STATIC_INFO => Ok(MsgFunction::GetGspStaticInfo),
            r570::NV_VGPU_MSG_FUNCTION_GET_STATIC_INFO => Ok(MsgFunction::GetStaticInfo),
            r570::NV_VGPU_MSG_FUNCTION_GSP_INIT_POST_OBJGPU => {
                Ok(MsgFunction::GspInitPostObjGpu)
            }
            r570::NV_VGPU_MSG_FUNCTION_GSP_RM_CONTROL => Ok(MsgFunction::GspRmControl),
            r570::NV_VGPU_MSG_FUNCTION_GSP_SET_SYSTEM_INFO => Ok(MsgFunction::GspSetSystemInfo),
            r570::NV_VGPU_MSG_FUNCTION_LOG => Ok(MsgFunction::Log),
            r570::NV_VGPU_MSG_FUNCTION_MAP_MEMORY => Ok(MsgFunction::MapMemory),
            r570::NV_VGPU_MSG_FUNCTION_NOP => Ok(MsgFunction::Nop),
            r570::NV_VGPU_MSG_FUNCTION_SET_GUEST_SYSTEM_INFO => {
                Ok(MsgFunction::SetGuestSystemInfo)
            }
            r570::NV_VGPU_MSG_FUNCTION_SET_REGISTRY => Ok(MsgFunction::SetRegistry),

            // Event codes
            r570::NV_VGPU_MSG_EVENT_GSP_INIT_DONE => Ok(MsgFunction::GspInitDone),
            r570::NV_VGPU_MSG_EVENT_GSP_LOCKDOWN_NOTICE => Ok(MsgFunction::GspLockdownNotice),
            r000::NV_VGPU_MSG_EVENT_GSP_LOAD_EXEC_GENERIC_BOOTLOADER => {
                Ok(MsgFunction::GspLoadExecGenericBootloader)
            }
            r000::NV_VGPU_MSG_EVENT_GSP_LOAD_EXEC_HS_BINARY => {
                Ok(MsgFunction::GspLoadExecHsBinary)
            }
            r570::NV_VGPU_MSG_EVENT_GSP_POST_NOCAT_RECORD => Ok(MsgFunction::GspPostNoCat),
            r570::NV_VGPU_MSG_EVENT_MMU_FAULT_QUEUED => Ok(MsgFunction::MmuFaultQueued),
            r570::NV_VGPU_MSG_EVENT_OS_ERROR_LOG => Ok(MsgFunction::OsErrorLog),
            r570::NV_VGPU_MSG_EVENT_POST_EVENT => Ok(MsgFunction::PostEvent),
            r570::NV_VGPU_MSG_EVENT_RC_TRIGGERED => Ok(MsgFunction::RcTriggered),
            r570::NV_VGPU_MSG_EVENT_UCODE_LIBOS_PRINT => Ok(MsgFunction::UcodeLibOsPrint),
            _ => Err(EINVAL),
        }
    }
}

impl MsgFunction {
    /// Returns true if this is a GSP-initiated async event (NV_VGPU_MSG_EVENT_*), as opposed to
    /// a command response (NV_VGPU_MSG_FUNCTION_*).
    pub(crate) fn is_event(&self) -> bool {
        matches!(
            self,
            Self::GspInitDone
                | Self::GspLockdownNotice
                | Self::GspLoadExecGenericBootloader
                | Self::GspLoadExecHsBinary
                | Self::PostEvent
                | Self::RcTriggered
                | Self::MmuFaultQueued
                | Self::OsErrorLog
                | Self::GspPostNoCat
                | Self::UcodeLibOsPrint //
        )
    }
}

impl From<MsgFunction> for u32 {
    fn from(value: MsgFunction) -> Self {
        // CAST: `MsgFunction` is `repr(u32)` and can thus be cast losslessly.
        value as u32
    }
}

/// Struct containing the arguments required to pass a memory buffer to the GSP
/// for use during initialisation.
///
/// The GSP only understands 4K pages (GSP_PAGE_SIZE), so even if the kernel is
/// configured for a larger page size (e.g. 64K pages), we need to give
/// the GSP an array of 4K pages. Since we only create physically contiguous
/// buffers the math to calculate the addresses is simple.
///
/// The buffers must be a multiple of GSP_PAGE_SIZE.  GSP-RM also currently
/// ignores the @kind field for LOGINIT, LOGINTR, and LOGRM, but expects the
/// buffers to be physically contiguous anyway.
///
/// The memory allocated for the arguments must remain until the GSP sends the
/// init_done RPC.
#[repr(transparent)]
pub(crate) struct LibosMemoryRegionInitArgument(r570::LibosMemoryRegionInitArgument);

// SAFETY: Padding is explicit and does not contain uninitialized data.
unsafe impl AsBytes for LibosMemoryRegionInitArgument {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for LibosMemoryRegionInitArgument {}

impl LibosMemoryRegionInitArgument {
    pub(crate) fn new<A: AsBytes + FromBytes + KnownSize + ?Sized>(
        name: &'static str,
        obj: &Coherent<A>,
    ) -> Self {
        /// Generates the `ID8` identifier required for some GSP objects.
        fn id8(name: &str) -> u64 {
            let mut bytes = [0u8; core::mem::size_of::<u64>()];

            for (c, b) in name.bytes().rev().zip(&mut bytes) {
                *b = c;
            }

            u64::from_ne_bytes(bytes)
        }

        Self(r570::LibosMemoryRegionInitArgument {
            id8: id8(name),
            pa: obj.dma_handle(),
            size: num::usize_as_u64(obj.size()),
            kind: num::u32_into_u8::<
                { r570::LibosMemoryRegionKind_LIBOS_MEMORY_REGION_CONTIGUOUS },
            >(),
            loc: num::u32_into_u8::<
                { r570::LibosMemoryRegionLoc_LIBOS_MEMORY_REGION_LOC_SYSMEM },
            >(),
            ..Default::default()
        })
    }
}

/// TX header for setting up a message queue with the GSP.
#[repr(transparent)]
pub(crate) struct MsgqTxHeader(pub(crate) r570::msgqTxHeader);

impl MsgqTxHeader {
    /// Create a new TX queue header.
    ///
    /// # Arguments
    ///
    /// * `msgq_size` - Total size of the message queue structure, in bytes.
    /// * `rx_hdr_offset` - Offset, in bytes, of the start of the RX header in the message queue
    ///   structure.
    /// * `msg_count` - Number of messages that can be sent, i.e. the number of memory pages
    ///   allocated for the message queue in the message queue structure.
    pub(crate) fn new(msgq_size: u32, rx_hdr_offset: u32, msg_count: u32) -> Self {
        Self(r570::msgqTxHeader {
            version: 0,
            size: msgq_size,
            msgSize: num::usize_into_u32::<GSP_PAGE_SIZE>(),
            msgCount: msg_count,
            writePtr: 0,
            flags: 1,
            rxHdrOff: rx_hdr_offset,
            entryOff: num::usize_into_u32::<GSP_PAGE_SIZE>(),
        })
    }
}

// SAFETY: Padding is explicit and does not contain uninitialized data.
unsafe impl AsBytes for MsgqTxHeader {}

// SAFETY: All bit-patterns are valid for this type.
unsafe impl FromBytes for MsgqTxHeader {}

/// RX header for setting up a message queue with the GSP.
#[repr(transparent)]
pub(crate) struct MsgqRxHeader(pub(crate) r570::msgqRxHeader);

/// Header for the message RX queue.
impl MsgqRxHeader {
    /// Creates a new RX queue header.
    pub(crate) fn new() -> Self {
        Self(Default::default())
    }
}

// SAFETY: Padding is explicit and does not contain uninitialized data.
unsafe impl AsBytes for MsgqRxHeader {}

// SAFETY: All bit-patterns are valid for this type.
unsafe impl FromBytes for MsgqRxHeader {}

bitfield! {
    struct MsgHeaderVersion(u32) {
        31:24 major as u8;
        23:16 minor as u8;
    }
}

impl MsgHeaderVersion {
    const MAJOR_TOT: u8 = 3;
    const MINOR_TOT: u8 = 0;

    fn new() -> Self {
        Self::default()
            .set_major(Self::MAJOR_TOT)
            .set_minor(Self::MINOR_TOT)
    }
}

impl r000::rpc_message_header_v {
    fn init(cmd_size: usize, function: MsgFunction, sequence: u32) -> impl Init<Self, Error> {
        type RpcMessageHeader = r000::rpc_message_header_v;

        try_init!(RpcMessageHeader {
            header_version: MsgHeaderVersion::new().into(),
            signature: r570::NV_VGPU_MSG_SIGNATURE_VALID,
            function: function.into(),
            length: size_of::<Self>()
                .checked_add(cmd_size)
                .ok_or(EOVERFLOW)
                .and_then(|v| v.try_into().map_err(|_| EINVAL))?,
            rpc_result: 0xffffffff,
            rpc_result_private: 0xffffffff,
            sequence,
            ..Zeroable::init_zeroed()
        })
    }
}

/// GSP Message Element (r000 MCTP/NVDM format).
///
/// This is the transport-layer header for messages exchanged with GSP-RM.
/// r000 firmware uses MCTP/NVDM framing instead of the r570 `GSP_MSG_QUEUE_ELEMENT`.
#[repr(C)]
pub(crate) struct GspMsgElement {
    mctp_header: u32,
    nvdm_header: u32,
    check_sum: u32,
    seq_num: u32,
    rpc: r000::rpc_message_header_v,
}

impl GspMsgElement {
    /// Creates a new message element.
    ///
    /// # Arguments
    ///
    /// * `transport_seq` - Transport-level sequence number for the outer message header.
    ///   Must be unique per message.
    /// * `rpc_seq` - RPC-level sequence number for the inner RPC header
    ///   (`rpc_message_header_v.sequence`). Set to 0 for async (fire-and-forget) commands,
    ///   or to the sync counter for command/response pairs.
    /// * `cmd_size` - Size of the command (not including the message element), in bytes.
    /// * `function` - Function of the message.
    pub(crate) fn init(
        transport_seq: u32,
        rpc_seq: u32,
        cmd_size: usize,
        function: MsgFunction,
    ) -> impl Init<Self, Error> {
        use crate::mctp;
        type RpcMessageHeader = r000::rpc_message_header_v;

        try_init!(GspMsgElement {
            mctp_header: mctp::TransportHeader::new(true, true, 0, 0, 0).into(),
            nvdm_header: mctp::NvdmHeader::new(mctp::nvdm_type::RM_RPC).into(),
            check_sum: 0u32,
            seq_num: transport_seq,
            rpc <- RpcMessageHeader::init(cmd_size, function, rpc_seq),
        })
    }

    /// Sets the checksum of this message.
    ///
    /// Since the header is also part of the checksum, this is usually called after the whole
    /// message has been written to the shared memory area.
    pub(crate) fn set_checksum(&mut self, checksum: u32) {
        self.check_sum = checksum;
    }

    /// Returns the length of the message's payload (command data after the RPC header).
    pub(crate) fn payload_length(&self) -> usize {
        // `rpc.length` includes the length of the RPC message header.
        num::u32_as_usize(self.rpc.length)
            .saturating_sub(size_of::<r000::rpc_message_header_v>())
    }

    /// Returns the total length of the message, transport and RPC headers included.
    pub(crate) fn length(&self) -> usize {
        size_of::<Self>() + self.payload_length()
    }

    // Returns the sequence number of the message.
    pub(crate) fn sequence(&self) -> u32 {
        self.rpc.sequence
    }

    // Returns the function of the message, if it is valid, or the invalid function number as an
    // error.
    pub(crate) fn function(&self) -> Result<MsgFunction, u32> {
        self.rpc.function.try_into().map_err(|_| self.rpc.function)
    }

    // Returns the number of elements (i.e. memory pages) used by this message.
    pub(crate) fn element_count(&self) -> u32 {
        self.length().div_ceil(GSP_PAGE_SIZE) as u32
    }
}

// SAFETY: All fields are integer types or contain only integer types, with no
// uninitialized padding bytes.
unsafe impl AsBytes for GspMsgElement {}

// SAFETY: All fields are integer types for which all bit patterns are valid.
unsafe impl FromBytes for GspMsgElement {}

/// Optional bindata (ucodes) firmware info for GSP startup arguments.
pub(crate) struct BindataArgs {
    /// DMA address of the radix3 level 0 page table for the bindata firmware.
    pub(crate) radix3: u64,
    /// Size in bytes of the bindata firmware.
    pub(crate) size: u64,
}

/// Arguments for GSP startup.
#[repr(transparent)]
pub(crate) struct GspArgumentsCached(r000::GSP_ARGUMENTS_CACHED);

impl GspArgumentsCached {
    /// Creates the arguments for starting the GSP up using `cmdq` as its command queue.
    ///
    /// If `bindata` is provided, the GSP will be told where to find the ucodes firmware.
    ///
    /// `state_monitor` is the RM state monitor buffer (4KB). GSP-RM maps this buffer
    /// during init for diagnostics.
    pub(crate) fn new(
        cmdq: &Cmdq,
        bindata: Option<&BindataArgs>,
        state_monitor: &Coherent<[u8]>,
    ) -> Self {
        let mut args = r000::GSP_ARGUMENTS_CACHED {
            messageQueueInitArguments: MessageQueueInitArguments::new(cmdq).0,
            bDmemStack: 1,
            ..Default::default()
        };

        if let Some(bindata) = bindata {
            args.bindataArgs.radix3 = bindata.radix3;
            args.bindataArgs.size = bindata.size;
        }

        args.rmStateMonitorBufferArgs.pa = state_monitor.dma_handle();
        args.rmStateMonitorBufferArgs.size = state_monitor.size() as u64;

        Self(args)
    }
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for GspArgumentsCached {}

// SAFETY: All bit patterns are valid for this type.
unsafe impl FromBytes for GspArgumentsCached {}

/// On Turing and GA100, the entries in the `LibosMemoryRegionInitArgument`
/// must all be a multiple of GSP_PAGE_SIZE in size, so add padding to force it
/// to that size.
#[repr(C)]
pub(crate) struct GspArgumentsPadded {
    pub(crate) inner: GspArgumentsCached,
    _padding: [u8; GSP_PAGE_SIZE - core::mem::size_of::<r000::GSP_ARGUMENTS_CACHED>()],
}

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for GspArgumentsPadded {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for GspArgumentsPadded {}

/// Init arguments for the message queue.
#[repr(transparent)]
struct MessageQueueInitArguments(r000::MESSAGE_QUEUE_INIT_ARGUMENTS);

impl MessageQueueInitArguments {
    /// Creates a new init arguments structure for `cmdq`.
    #[allow(non_snake_case)]
    fn new(cmdq: &Cmdq) -> Self {
        Self(r000::MESSAGE_QUEUE_INIT_ARGUMENTS {
            sharedMemPhysAddr: cmdq.dma_handle(),
            pageTableEntryCount: num::usize_into_u32::<{ Cmdq::NUM_PTES }>(),
            cmdQueueOffset: num::usize_as_u64(Cmdq::CMDQ_OFFSET),
            statQueueOffset: num::usize_as_u64(Cmdq::STATQ_OFFSET),

            queueElementHdrSize: core::mem::offset_of!(r000::GSP_MSG_QUEUE_ELEMENT, payload)
                as u64,
            queueElementSizeMin: GSP_PAGE_SIZE as u64,
            queueElementSizeMax: (GSP_PAGE_SIZE * 16) as u64,
            queueHeaderAlign: 4,
            queueElementAlign: GSP_PAGE_SHIFT as u32,

            ..Default::default()
        })
    }
}
