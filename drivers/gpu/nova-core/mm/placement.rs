// SPDX-License-Identifier: GPL-2.0

//! Allocates framebuffer placements from an ID-to-ranges map.
//!
//! Each map entry binds one ID to one or more absolute framebuffer ranges:
//!
//! ```text
//! ID 0 -> guest FB [F0, F1), management heap [H0, H1)
//! ID 1 -> guest FB [F1, F2), management heap [H1, H2)
//! ```
//!
//! The backing ranges are reserved before the allocator is created. Every map
//! range must be inside that backing, and a placement reference keeps the
//! backing reserved while any of its ranges remain in use.
//!
//! Entries may overlap when they describe choices that cannot be active
//! together. Allocation fails when an entry overlaps a placement already in
//! use.
//!
//! # vGPU
//!
//! A homogeneous vGPU configuration creates one entry for every instance slot.
//! Each entry contains that slot's guest framebuffer and management heap. The
//! entries do not overlap, so any free slot can be allocated.
//!
//! A heterogeneous configuration can put entries for different vGPU types in
//! one map. Their ranges may overlap, and each `(vGPU type, placement)` pair
//! must have a unique ID.
//!
//! Future MIG support can use the same model by assigning one ID to each
//! concrete profile and placement choice. MIG topology validation remains in
//! the MIG code.

use core::ops::Range;

use kernel::{
    bitmap::BitmapVec,
    new_mutex,
    prelude::*,
    sync::{
        Arc,
        Mutex, //
    }, //
};

use super::{
    range_contains,
    ranges_overlap,
    vram::VramBlock,
    AllocatorBacking,
    GpuMmAllocator, //
};

const PLACEMENT_MIN_ALIGN: u64 = 4096;

/// One placement ID and its framebuffer ranges.
pub(crate) struct PlacementEntry {
    id: u32,
    ranges: KVec<Range<u64>>,
}

impl PlacementEntry {
    /// Create a placement with at least one non-empty, 4-KiB-aligned range.
    ///
    /// The ranges must not overlap. The allocator checks that they are inside
    /// its backing when it is created.
    pub(crate) fn new(placement_id: u32, ranges: KVec<Range<u64>>) -> Result<Self> {
        validate_ranges(&ranges)?;
        Ok(Self {
            id: placement_id,
            ranges,
        })
    }

    pub(crate) const fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn range_count(&self) -> usize {
        self.ranges.len()
    }

    pub(crate) fn range(&self, index: usize) -> Option<&Range<u64>> {
        self.ranges.get(index)
    }
}

/// Shared reference that keeps a placement and its backing in use.
pub(crate) struct PlacementRef {
    index: usize,
    entry: Arc<PlacementEntry>,
    backing: Arc<AllocatorBacking>,
}

impl PlacementRef {
    pub(crate) fn id(&self) -> u32 {
        self.entry.id()
    }

    pub(crate) fn range_count(&self) -> usize {
        self.entry.range_count()
    }

    pub(crate) fn range(&self, index: usize) -> Option<&Range<u64>> {
        self.entry.range(index)
    }

    /// Return whether this placement includes the full range.
    pub(crate) fn contains_range(&self, range: &Range<u64>) -> bool {
        range.start < range.end
            && self
                .entry
                .ranges
                .iter()
                .any(|placement_range| range_contains(placement_range, range))
    }
}

/// Handle returned for an allocated placement.
pub(crate) struct PlacementAllocation {
    placement_ref: Arc<PlacementRef>,
}

impl PlacementAllocation {
    pub(crate) fn id(&self) -> u32 {
        self.placement_ref.id()
    }

    pub(crate) fn range_count(&self) -> usize {
        self.placement_ref.range_count()
    }

    pub(crate) fn range(&self, index: usize) -> Option<&Range<u64>> {
        self.placement_ref.range(index)
    }

    /// Return a shared reference for a VRAM region using this placement.
    pub(crate) fn placement_ref(&self) -> Arc<PlacementRef> {
        self.placement_ref.clone()
    }
}

struct PlacementState {
    entries: KVec<Arc<PlacementEntry>>,
    in_use: BitmapVec,
}

impl PlacementState {
    fn is_empty(&self) -> bool {
        self.in_use.last_bit().is_none()
    }

    fn is_in_use(&self, index: usize) -> bool {
        index < self.in_use.len() && self.in_use.next_bit(index) == Some(index)
    }
}

/// Allocates map entries by ID and prevents overlapping ranges from being used
/// together.
pub(crate) struct Placement {
    state: Pin<KBox<Mutex<PlacementState>>>,
}

impl GpuMmAllocator<Placement> {
    /// Create an allocator over framebuffer ranges already reserved from a
    /// parent buddy allocator.
    pub(crate) fn new(
        backings: KVec<Arc<VramBlock>>,
        new_entries: KVec<PlacementEntry>,
    ) -> Result<Self> {
        let backing = AllocatorBacking::from_blocks(backings)?;
        let mut entries = KVec::new();
        entries.reserve(new_entries.len(), GFP_KERNEL)?;

        for new_entry in new_entries {
            validate_map_entry(&backing, &new_entry)?;
            if entries
                .iter()
                .any(|entry: &Arc<PlacementEntry>| entry.id() == new_entry.id())
            {
                return Err(EINVAL);
            }
            entries.push_within_capacity(Arc::new(new_entry, GFP_KERNEL)?)?;
        }

        let in_use = BitmapVec::new(entries.len(), GFP_KERNEL)?;
        let state = KBox::pin_init(
            new_mutex!(
                PlacementState { entries, in_use },
                "nova-core::mm-placement"
            ),
            GFP_KERNEL,
        )?;

        Ok(Self {
            backend: Placement { state },
            backing,
        })
    }

    /// Allocate the map entry identified by `placement_id`.
    ///
    /// Allocation fails if its ranges overlap a placement that is already in
    /// use.
    pub(crate) fn alloc(&self, placement_id: u32) -> Result<PlacementAllocation> {
        let mut state = self.backend.state.lock();
        let index = state
            .entries
            .iter()
            .position(|entry| entry.id() == placement_id)
            .ok_or(ENOENT)?;
        let entry = &state.entries[index];

        if state
            .entries
            .iter()
            .enumerate()
            .any(|(in_use_index, in_use_entry)| {
                state.is_in_use(in_use_index) && placements_overlap(entry, in_use_entry)
            })
        {
            return Err(ENOSPC);
        }

        let placement_ref = Arc::new(
            PlacementRef {
                index,
                entry: entry.clone(),
                backing: self.backing.clone(),
            },
            GFP_KERNEL,
        )?;

        // Create every object that can fail before marking this placement in use.
        state.in_use.set_bit(index);
        Ok(PlacementAllocation { placement_ref })
    }

    /// Free a placement after all VRAM regions using it have been dropped.
    ///
    /// On `EBUSY`, the placement stays in use and its memory is never reused.
    pub(crate) fn free(&self, placement: PlacementAllocation) -> Result {
        let PlacementAllocation { placement_ref } = placement;
        let mut state = self.backend.state.lock();
        let index = placement_ref.index;

        if index >= state.entries.len()
            || !Arc::ptr_eq(&self.backing, &placement_ref.backing)
            || !Arc::ptr_eq(&state.entries[index], &placement_ref.entry)
            || !state.is_in_use(index)
        {
            return Err(EINVAL);
        }

        let Some(placement_ref) = Arc::into_unique_or_drop(placement_ref) else {
            return Err(EBUSY);
        };
        state.in_use.clear_bit(index);
        drop(placement_ref);
        Ok(())
    }

    /// Return whether no placement is in use.
    pub(crate) fn is_empty(&self) -> bool {
        self.backend.state.lock().is_empty()
    }
}

fn validate_map_entry(backing: &AllocatorBacking, entry: &PlacementEntry) -> Result {
    validate_ranges(&entry.ranges)?;
    if entry
        .ranges
        .iter()
        .any(|range| !backing.contains_range(range))
    {
        return Err(ERANGE);
    }
    Ok(())
}

fn validate_ranges(ranges: &[Range<u64>]) -> Result {
    if ranges.is_empty() {
        return Err(EINVAL);
    }
    for (index, range) in ranges.iter().enumerate() {
        validate_range(range)?;
        if ranges[index + 1..]
            .iter()
            .any(|other| ranges_overlap(range, other))
        {
            return Err(EINVAL);
        }
    }
    Ok(())
}

fn validate_range(range: &Range<u64>) -> Result {
    if range.start >= range.end
        || !range.start.is_multiple_of(PLACEMENT_MIN_ALIGN)
        || !range.end.is_multiple_of(PLACEMENT_MIN_ALIGN)
    {
        return Err(EINVAL);
    }
    Ok(())
}

fn placements_overlap(left: &PlacementEntry, right: &PlacementEntry) -> bool {
    left.ranges.iter().any(|left_range| {
        right
            .ranges
            .iter()
            .any(|right_range| ranges_overlap(left_range, right_range))
    })
}
