// SPDX-License-Identifier: GPL-2.0

//! GPU buddy allocator bindings.
//!
//! C header: [`include/linux/gpu_buddy.h`](srctree/include/linux/gpu_buddy.h)
//!
//! This module provides Rust abstractions over the Linux kernel's GPU buddy
//! allocator, which implements a binary buddy memory allocator.
//!
//! The buddy allocator manages a contiguous address space and allocates blocks
//! in power-of-two sizes, useful for GPU physical memory management.
//!
//! # Examples
//!
//! Create a buddy allocator and perform a basic range allocation:
//!
//! ```
//! use kernel::{
//!     gpu::buddy::{BuddyFlag, GpuBuddy, GpuBuddyAllocParams, GpuBuddyParams},
//!     prelude::*,
//!     sizes::*, //
//! };
//!
//! // Create a 1GB buddy allocator with 4KB minimum chunk size.
//! let buddy = GpuBuddy::new(GpuBuddyParams {
//!     base_offset: 0,
//!     physical_memory_size: SZ_1G as u64,
//!     chunk_size: SZ_4K as u64,
//! })?;
//!
//! assert_eq!(buddy.size(), SZ_1G as u64);
//! assert_eq!(buddy.chunk_size(), SZ_4K as u64);
//! let initial_free = buddy.free_memory();
//!
//! // Allocate 16MB - should result in a single 16MB block at offset 0.
//! let params = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: 0,
//!     size: SZ_16M as u64,
//!     min_block_size: SZ_16M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let allocated = KBox::pin_init(buddy.alloc_blocks(params), GFP_KERNEL)?;
//! assert_eq!(buddy.free_memory(), initial_free - SZ_16M as u64);
//!
//! let block = allocated.iter().next().expect("expected one block");
//! assert_eq!(block.offset(), 0);
//! assert_eq!(block.order(), 12); // 2^12 pages = 16MB
//! assert_eq!(block.size(), SZ_16M as u64);
//!
//! // Dropping the allocation returns the memory to the buddy allocator.
//! drop(allocated);
//! assert_eq!(buddy.free_memory(), initial_free);
//! # Ok::<(), Error>(())
//! ```
//!
//! Top-down allocation allocates from the highest addresses:
//!
//! ```
//! # use kernel::{
//! #     gpu::buddy::{BuddyFlags, GpuBuddy, GpuBuddyAllocParams, GpuBuddyParams},
//! #     prelude::*,
//! #     sizes::*, //
//! # };
//! # let buddy = GpuBuddy::new(GpuBuddyParams {
//! #     base_offset: 0,
//! #     physical_memory_size: SZ_1G as u64,
//! #     chunk_size: SZ_4K as u64,
//! # })?;
//! # let initial_free = buddy.free_memory();
//! let params = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: 0,
//!     size: SZ_16M as u64,
//!     min_block_size: SZ_16M as u64,
//!     buddy_flags: BuddyFlag::TopdownAllocation.into(),
//! };
//! let topdown = KBox::pin_init(buddy.alloc_blocks(params), GFP_KERNEL)?;
//! assert_eq!(buddy.free_memory(), initial_free - SZ_16M as u64);
//!
//! let block = topdown.iter().next().expect("expected one block");
//! assert_eq!(block.offset(), (SZ_1G - SZ_16M) as u64);
//! assert_eq!(block.order(), 12);
//! assert_eq!(block.size(), SZ_16M as u64);
//!
//! drop(topdown);
//! assert_eq!(buddy.free_memory(), initial_free);
//! # Ok::<(), Error>(())
//! ```
//!
//! Non-contiguous allocation can fill fragmented memory by returning multiple
//! blocks:
//!
//! ```
//! # use kernel::{
//! #     gpu::buddy::{BuddyFlags, GpuBuddy, GpuBuddyAllocParams, GpuBuddyParams},
//! #     prelude::*,
//! #     sizes::*, //
//! # };
//! # let buddy = GpuBuddy::new(GpuBuddyParams {
//! #     base_offset: 0,
//! #     physical_memory_size: SZ_1G as u64,
//! #     chunk_size: SZ_4K as u64,
//! # })?;
//! # let initial_free = buddy.free_memory();
//! // Create fragmentation by allocating 4MB blocks at [0,4M) and [8M,12M).
//! let params_frag1 = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: SZ_4M as u64,
//!     size: SZ_4M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let frag1 = KBox::pin_init(buddy.alloc_blocks(params_frag1), GFP_KERNEL)?;
//! assert_eq!(buddy.free_memory(), initial_free - SZ_4M as u64);
//!
//! let params_frag2 = GpuBuddyAllocParams {
//!     start_range_address: SZ_8M as u64,
//!     end_range_address: (SZ_8M + SZ_4M) as u64,
//!     size: SZ_4M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let frag2 = KBox::pin_init(buddy.alloc_blocks(params_frag2), GFP_KERNEL)?;
//! assert_eq!(buddy.free_memory(), initial_free - SZ_8M as u64);
//!
//! // Allocate 8MB - returns 2 blocks from the holes.
//! let params = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: SZ_16M as u64,
//!     size: SZ_8M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let fragmented = KBox::pin_init(buddy.alloc_blocks(params), GFP_KERNEL)?;
//! assert_eq!(buddy.free_memory(), initial_free - SZ_16M as u64);
//!
//! let (mut count, mut total) = (0u32, 0u64);
//! for block in fragmented.iter() {
//!     assert_eq!(block.size(), SZ_4M as u64);
//!     total += block.size();
//!     count += 1;
//! }
//! assert_eq!(total, SZ_8M as u64);
//! assert_eq!(count, 2);
//! # Ok::<(), Error>(())
//! ```
//!
//! Contiguous allocation fails when only fragmented space is available:
//!
//! ```
//! # use kernel::{
//! #     gpu::buddy::{BuddyFlags, GpuBuddy, GpuBuddyAllocParams, GpuBuddyParams},
//! #     prelude::*,
//! #     sizes::*, //
//! # };
//! // Create a small 16MB buddy allocator with fragmented memory.
//! let small = GpuBuddy::new(GpuBuddyParams {
//!     base_offset: 0,
//!     physical_memory_size: SZ_16M as u64,
//!     chunk_size: SZ_4K as u64,
//! })?;
//!
//! let params1 = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: SZ_4M as u64,
//!     size: SZ_4M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let _hole1 = KBox::pin_init(small.alloc_blocks(params1), GFP_KERNEL)?;
//!
//! let params2 = GpuBuddyAllocParams {
//!     start_range_address: SZ_8M as u64,
//!     end_range_address: (SZ_8M + SZ_4M) as u64,
//!     size: SZ_4M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::RangeAllocation.into(),
//! };
//! let _hole2 = KBox::pin_init(small.alloc_blocks(params2), GFP_KERNEL)?;
//!
//! // 8MB contiguous should fail - only two non-contiguous 4MB holes exist.
//! let params = GpuBuddyAllocParams {
//!     start_range_address: 0,
//!     end_range_address: 0,
//!     size: SZ_8M as u64,
//!     min_block_size: SZ_4M as u64,
//!     buddy_flags: BuddyFlag::ContiguousAllocation.into(),
//! };
//! let result = KBox::pin_init(small.alloc_blocks(params), GFP_KERNEL);
//! assert!(result.is_err());
//! # Ok::<(), Error>(())
//! ```

use crate::{
    bindings,
    clist_create,
    error::to_result,
    ffi::clist::CListHead,
    new_mutex,
    prelude::*,
    sync::{
        lock::mutex::MutexGuard,
        Arc,
        Mutex, //
    },
    types::Opaque, //
};

crate::impl_flags!(
    /// Flags for GPU buddy allocator operations.
    ///
    /// These flags control the allocation behavior of the buddy allocator.
    /// Combine individual [`BuddyFlag`] values using `|` to form a [`BuddyFlags`] set.
    #[derive(Clone, Copy, Default, PartialEq, Eq)]
    pub struct BuddyFlags(u32);

    /// Individual flag for GPU buddy allocator operations.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum BuddyFlag {
        /// Range-based allocation from start to end addresses.
        RangeAllocation = bindings::GPU_BUDDY_RANGE_ALLOCATION as u32,

        /// Allocate from top of address space downward.
        TopdownAllocation = bindings::GPU_BUDDY_TOPDOWN_ALLOCATION as u32,

        /// Allocate physically contiguous blocks.
        ContiguousAllocation = bindings::GPU_BUDDY_CONTIGUOUS_ALLOCATION as u32,

        /// Request allocation from the cleared (zeroed) memory. The zero'ing is not
        /// done by the allocator, but by the caller before freeing old blocks.
        ClearAllocation = bindings::GPU_BUDDY_CLEAR_ALLOCATION as u32,

        /// Mark blocks as cleared (zeroed) when freeing. When set during free,
        /// indicates that the caller has already zeroed the memory.
        Cleared = bindings::GPU_BUDDY_CLEARED as u32,

        /// Disable trimming of partially used blocks.
        TrimDisable = bindings::GPU_BUDDY_TRIM_DISABLE as u32,
    }
);

/// Parameters for creating a GPU buddy allocator.
pub struct GpuBuddyParams {
    /// Base offset (in bytes) where the managed memory region starts.
    /// Allocations will be offset by this value.
    pub base_offset: u64,
    /// Total physical memory size (in bytes) managed by the allocator.
    pub physical_memory_size: u64,
    /// Minimum allocation unit / chunk size (in bytes), must be >= 4KB.
    pub chunk_size: u64,
}

/// Parameters for allocating blocks from a GPU buddy allocator.
pub struct GpuBuddyAllocParams {
    /// Start of allocation range in bytes. Use 0 for beginning.
    pub start_range_address: u64,
    /// End of allocation range in bytes. Use 0 for entire range.
    pub end_range_address: u64,
    /// Total size (in bytes) to allocate.
    pub size: u64,
    /// Minimum block size (in bytes) for fragmented allocations.
    pub min_block_size: u64,
    /// Buddy allocator behavior flags.
    pub buddy_flags: BuddyFlags,
}

/// Inner structure holding the actual buddy allocator.
///
/// # Synchronization
///
/// The C `gpu_buddy` API requires synchronization (see `include/linux/gpu_buddy.h`).
/// [`GpuBuddyGuard`] ensures that the lock is held for all
/// allocator and free operations, preventing races between concurrent allocations
/// and the freeing that occurs when [`AllocatedBlocks`] is dropped.
///
/// # Invariants
///
/// The inner [`Opaque`] contains a valid, initialized buddy allocator.
#[pin_data(PinnedDrop)]
struct GpuBuddyInner {
    #[pin]
    inner: Opaque<bindings::gpu_buddy>,

    // TODO: Replace `Mutex<()>` with `Mutex<Opaque<..>>` once `Mutex::new()`
    // accepts `impl PinInit<T>`.
    #[pin]
    lock: Mutex<()>,
    /// Base offset for all allocations (does not change after init).
    base_offset: u64,
    /// Cached chunk size (does not change after init).
    chunk_size: u64,
    /// Cached total size (does not change after init).
    size: u64,
}

impl GpuBuddyInner {
    /// Create a pin-initializer for the buddy allocator.
    fn new(params: GpuBuddyParams) -> impl PinInit<Self, Error> {
        let base_offset = params.base_offset;
        let size = params.physical_memory_size;
        let chunk_size = params.chunk_size;

        try_pin_init!(Self {
            inner <- Opaque::try_ffi_init(|ptr| {
                // SAFETY: ptr points to valid uninitialized memory from the pin-init
                // infrastructure. gpu_buddy_init will initialize the structure.
                to_result(unsafe { bindings::gpu_buddy_init(ptr, size, chunk_size) })
            }),
            lock <- new_mutex!(()),
            base_offset: base_offset,
            chunk_size: chunk_size,
            size: size,
        })
    }

    /// Lock the mutex and return a guard for accessing the allocator.
    fn lock(&self) -> GpuBuddyGuard<'_> {
        GpuBuddyGuard {
            inner: self,
            _guard: self.lock.lock(),
        }
    }
}

#[pinned_drop]
impl PinnedDrop for GpuBuddyInner {
    fn drop(self: Pin<&mut Self>) {
        let guard = self.lock();

        // SAFETY: guard provides exclusive access to the allocator.
        unsafe {
            bindings::gpu_buddy_fini(guard.as_raw());
        }
    }
}

// SAFETY: GpuBuddyInner can be sent between threads.
unsafe impl Send for GpuBuddyInner {}

// SAFETY: GpuBuddyInner is `Sync` because the internal GpuBuddyGuard
// serializes all access to the C allocator, preventing data races.
unsafe impl Sync for GpuBuddyInner {}

// Guard that proves the lock is held, enabling access to the allocator.
// The `_guard` holds the lock for the duration of this guard's lifetime.
struct GpuBuddyGuard<'a> {
    inner: &'a GpuBuddyInner,
    _guard: MutexGuard<'a, ()>,
}

impl GpuBuddyGuard<'_> {
    /// Get a raw pointer to the underlying C `gpu_buddy` structure.
    fn as_raw(&self) -> *mut bindings::gpu_buddy {
        self.inner.inner.get()
    }
}

/// GPU buddy allocator instance.
///
/// This structure wraps the C `gpu_buddy` allocator using reference counting.
/// The allocator is automatically cleaned up when all references are dropped.
///
/// Refer to the module-level documentation for usage examples.
///
/// # Invariants
///
/// The inner [`Arc`] points to a valid, initialized GPU buddy allocator.
pub struct GpuBuddy(Arc<GpuBuddyInner>);

impl GpuBuddy {
    /// Create a new buddy allocator.
    ///
    /// Creates a buddy allocator that manages a contiguous address space of the given
    /// size, with the specified minimum allocation unit (chunk_size must be at least 4KB).
    pub fn new(params: GpuBuddyParams) -> Result<Self> {
        Ok(Self(Arc::pin_init(GpuBuddyInner::new(params), GFP_KERNEL)?))
    }

    /// Get the base offset for allocations.
    pub fn base_offset(&self) -> u64 {
        self.0.base_offset
    }

    /// Get the chunk size (minimum allocation unit).
    pub fn chunk_size(&self) -> u64 {
        self.0.chunk_size
    }

    /// Get the total managed size.
    pub fn size(&self) -> u64 {
        self.0.size
    }

    /// Get the available (free) memory in bytes.
    pub fn free_memory(&self) -> u64 {
        let guard = self.0.lock();

        // SAFETY: guard provides exclusive access to the allocator.
        unsafe { (*guard.as_raw()).avail }
    }

    /// Allocate blocks from the buddy allocator.
    ///
    /// Returns a pin-initializer for [`AllocatedBlocks`].
    ///
    /// Takes `&self` instead of `&mut self` because the internal [`Mutex`] provides
    /// synchronization - no external `&mut` exclusivity needed.
    pub fn alloc_blocks(
        &self,
        params: GpuBuddyAllocParams,
    ) -> impl PinInit<AllocatedBlocks, Error> {
        let buddy_arc = Arc::clone(&self.0);

        // Create pin-initializer that initializes list and allocates blocks.
        try_pin_init!(AllocatedBlocks {
            buddy: buddy_arc,
            list <- CListHead::new(),
            flags: params.buddy_flags,
            _: {
                // Lock while allocating to serialize with concurrent frees.
                let guard = buddy.lock();

                // SAFETY: `guard` provides exclusive access to the buddy allocator.
                to_result(unsafe {
                    bindings::gpu_buddy_alloc_blocks(
                        guard.as_raw(),
                        params.start_range_address,
                        params.end_range_address,
                        params.size,
                        params.min_block_size,
                        list.as_raw(),
                        // CAST: u32 to usize is lossless.
                        u32::from(params.buddy_flags) as usize,
                    )
                })?
            }
        })
    }
}

/// Allocated blocks from the buddy allocator with automatic cleanup.
///
/// This structure owns a list of allocated blocks and ensures they are
/// automatically freed when dropped. Use `iter()` to iterate over all
/// allocated [`Block`] structures.
///
/// # Invariants
///
/// - `list` is an initialized, valid list head containing allocated blocks.
#[pin_data(PinnedDrop)]
pub struct AllocatedBlocks {
    #[pin]
    list: CListHead,
    buddy: Arc<GpuBuddyInner>,
    flags: BuddyFlags,
}

impl AllocatedBlocks {
    /// Check if the block list is empty.
    pub fn is_empty(&self) -> bool {
        // An empty list head points to itself.
        !self.list.is_linked()
    }

    /// Iterate over allocated blocks.
    ///
    /// Returns an iterator yielding [`AllocatedBlock`] values. Each [`AllocatedBlock`]
    /// borrows `self` and is only valid for the duration of that borrow.
    pub fn iter(&self) -> impl Iterator<Item = AllocatedBlock<'_>> + '_ {
        // SAFETY: Per the type invariant, `list` is a valid list head containing
        // allocated gpu_buddy_block items linked via __bindgen_anon_1.link.
        let clist = clist_create!(unsafe {
            self.list.as_raw(),
            Block,
            bindings::gpu_buddy_block,
            __bindgen_anon_1.link
        });

        clist
            .iter()
            .map(|block| AllocatedBlock { block, alloc: self })
    }
}

#[pinned_drop]
impl PinnedDrop for AllocatedBlocks {
    fn drop(self: Pin<&mut Self>) {
        let guard = self.buddy.lock();

        // SAFETY:
        // - list is valid per the type's invariants.
        // - guard provides exclusive access to the allocator.
        unsafe {
            bindings::gpu_buddy_free_list(
                guard.as_raw(),
                self.list.as_raw(),
                u32::from(self.flags),
            );
        }
    }
}

/// A GPU buddy block.
///
/// Transparent wrapper over C `gpu_buddy_block` structure. This type is returned
/// as references during iteration over [`AllocatedBlocks`].
///
/// # Invariants
///
/// The inner [`Opaque`] contains a valid, allocated `gpu_buddy_block`.
#[repr(transparent)]
pub struct Block(Opaque<bindings::gpu_buddy_block>);

impl Block {
    /// Get a raw pointer to the underlying C block.
    fn as_raw(&self) -> *mut bindings::gpu_buddy_block {
        self.0.get()
    }

    /// Get the block's offset in the address space.
    pub(crate) fn offset(&self) -> u64 {
        // SAFETY: self.as_raw() is valid per the type's invariants.
        unsafe { bindings::gpu_buddy_block_offset(self.as_raw()) }
    }

    /// Get the block order.
    pub(crate) fn order(&self) -> u32 {
        // SAFETY: self.as_raw() is valid per the type's invariants.
        unsafe { bindings::gpu_buddy_block_order(self.as_raw()) }
    }
}

// SAFETY: `Block` is a wrapper around `gpu_buddy_block` which can be
// sent across threads safely.
unsafe impl Send for Block {}

// SAFETY: `Block` is only accessed through shared references after
// allocation, and thus safe to access concurrently across threads.
unsafe impl Sync for Block {}

/// An allocated block with access to the GPU buddy allocator.
///
/// It is returned by [`AllocatedBlocks::iter()`] and provides access to the
/// GPU buddy allocator required for some accessors.
///
/// # Invariants
///
/// - `block` is a valid reference to an allocated [`Block`].
/// - `alloc` is a valid reference to the [`AllocatedBlocks`] that owns this block.
pub struct AllocatedBlock<'a> {
    block: &'a Block,
    alloc: &'a AllocatedBlocks,
}

impl AllocatedBlock<'_> {
    /// Get the block's offset in the address space.
    ///
    /// Returns the absolute offset including the allocator's base offset.
    /// This is the actual address to use for accessing the allocated memory.
    pub fn offset(&self) -> u64 {
        self.alloc.buddy.base_offset + self.block.offset()
    }

    /// Get the block order (size = chunk_size << order).
    pub fn order(&self) -> u32 {
        self.block.order()
    }

    /// Get the block's size in bytes.
    pub fn size(&self) -> u64 {
        self.alloc.buddy.chunk_size << self.block.order()
    }
}
