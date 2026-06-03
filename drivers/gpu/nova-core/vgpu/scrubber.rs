// SPDX-License-Identifier: GPL-2.0

//! Per-VM CeUtils guest framebuffer scrubbing.

use kernel::{
    device,
    prelude::*,
    time::{
        delay::fsleep,
        Delta, //
    }, //
};

use crate::{
    driver::Bar0,
    gsp::cmdq::Cmdq,
    mm::{
        bar_user::{
            Bar1Map,
            BarUser, //
        },
        vram::VramRegion,
        GpuMm,
        Pfn,
        VramAddress,
        PAGE_SIZE, //
    },
    num,
    vgpu::{
        consts::gmc,
        instance::Gfid, //
    }, //
};

const ADDR_FBMEM: u32 = 2;
const SEMA_PAGE_MAGIC: u32 = 0xce5e_5ea0;
const SEMA_PAGE_MAGIC_OFFSET: usize = 0;
const SEMA_PAGE_PAYLOAD_OFFSET: usize = 4;
const SCRUB_TIMEOUT_MS: u32 = 5_000;

const MAGIC_HEAD: u32 = 0xdead_beef;
const MAGIC_TAIL: u32 = 0xcafe_babe;

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
struct AllocCeutilsRequest {
    gfid: u32,
    fixed_chid: u32,
    force_ceid: u32,
    swizz_id: u32,
}

static_assert!(size_of::<AllocCeutilsRequest>() == 16);

#[repr(C)]
#[derive(FromBytes)]
struct AllocCeutilsResponse {
    semaphore_address: u64,
    semaphore_aperture: u32,
    _reserved: u32,
}

static_assert!(size_of::<AllocCeutilsResponse>() == 16);

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
struct FreeCeutilsRequest {
    gfid: u32,
}

static_assert!(size_of::<FreeCeutilsRequest>() == 4);

#[repr(C)]
#[derive(IntoBytes, zerocopy_derive::Immutable)]
struct ScrubGuestFbRequest {
    gfid: u32,
    reserved: u32,
    fb_offset: u64,
    fb_size: u64,
}

static_assert!(size_of::<ScrubGuestFbRequest>() == 24);

#[repr(C)]
#[derive(FromBytes)]
struct ScrubGuestFbResponse {
    work_id: u64,
}

static_assert!(size_of::<ScrubGuestFbResponse>() == 8);

/// A firmware-owned per-VM CeUtils allocation.
///
/// The owner must call [`Self::release`] before returning its CHID or VRAM to
/// their allocators.
pub(crate) struct CeUtils {
    gfid: Gfid,
    chid: u32,
    semaphore_address: u64,
}

impl CeUtils {
    /// Allocate a CeUtils channel and validate its semaphore description.
    pub(crate) fn allocate(
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        gfid: Gfid,
        chid: u32,
        swizz_id: u32,
    ) -> Result<Self> {
        let request = AllocCeutilsRequest {
            gfid: gfid.0.to_le(),
            fixed_chid: chid.to_le(),
            force_ceid: u32::MAX.to_le(),
            swizz_id: swizz_id.to_le(),
        };

        dev_dbg!(
            dev,
            "alloc CeUtils: gfid={} chid={} swizz_id={}\n",
            gfid.0,
            chid,
            swizz_id,
        );

        let response = cmdq.send_gmc_and_receive(
            bar,
            gmc::ALLOC_GSP_CEUTILS,
            <AllocCeutilsRequest as IntoBytes>::as_bytes(&request),
            num::usize_into_u32::<{ size_of::<AllocCeutilsResponse>() }>(),
        )?;
        if response.status != 0 {
            return Err(EIO);
        }

        let bytes = response
            .payload
            .get(..size_of::<AllocCeutilsResponse>())
            .ok_or(EMSGSIZE)?;
        let response = AllocCeutilsResponse::read_from_bytes(bytes).map_err(|_| EINVAL)?;
        let semaphore_address = u64::from_le(response.semaphore_address);
        let semaphore_aperture = u32::from_le(response.semaphore_aperture);
        let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;

        if semaphore_address == 0
            || !semaphore_address.is_multiple_of(page_size)
            || semaphore_aperture != ADDR_FBMEM
        {
            return Err(EINVAL);
        }

        dev_dbg!(
            dev,
            "alloc CeUtils: gfid={} semaphore={:#x}\n",
            gfid.0,
            semaphore_address,
        );
        Ok(Self {
            gfid,
            chid,
            semaphore_address,
        })
    }

    pub(crate) const fn chid(&self) -> u32 {
        self.chid
    }

    pub(crate) const fn semaphore_address(&self) -> u64 {
        self.semaphore_address
    }

    /// Scrub the complete guest framebuffer and verify its boundary markers.
    pub(crate) fn scrub_guest_fb<'gpu>(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
        bar_user: &BarUser<'gpu>,
        mm: &GpuMm<'gpu>,
        fb: &VramRegion,
    ) -> Result {
        write_markers(bar_user, mm, dev, fb)?;
        let work_id = submit_scrub(dev, cmdq, bar, self.gfid, fb.address(), fb.size())?;
        wait_scrub_complete(bar_user, mm, dev, self.semaphore_address, work_id)?;
        verify_markers_zeroed(bar_user, mm, dev, fb)
    }

    /// Release the firmware allocation.
    pub(crate) fn release(
        &self,
        dev: &device::Device<device::Bound>,
        cmdq: &Cmdq,
        bar: Bar0<'_>,
    ) -> Result {
        let request = FreeCeutilsRequest {
            gfid: self.gfid.0.to_le(),
        };

        dev_dbg!(
            dev,
            "free CeUtils: gfid={} chid={}\n",
            self.gfid.0,
            self.chid,
        );
        cmdq.send_gmc_no_response(
            bar,
            gmc::FREE_GSP_CEUTILS,
            <FreeCeutilsRequest as IntoBytes>::as_bytes(&request),
        )
    }
}

/// Submit an asynchronous guest FB scrub and return its work identifier.
fn submit_scrub(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    gfid: Gfid,
    fb_offset: u64,
    fb_size: u64,
) -> Result<u32> {
    let request = ScrubGuestFbRequest {
        gfid: gfid.0.to_le(),
        reserved: 0,
        fb_offset: fb_offset.to_le(),
        fb_size: fb_size.to_le(),
    };

    dev_dbg!(
        dev,
        "submit scrub: gfid={} offset={:#x} size={:#x}\n",
        gfid.0,
        fb_offset,
        fb_size,
    );

    let response = cmdq.send_gmc_and_receive(
        bar,
        gmc::SCRUB_GUEST_FB,
        <ScrubGuestFbRequest as IntoBytes>::as_bytes(&request),
        num::usize_into_u32::<{ size_of::<ScrubGuestFbResponse>() }>(),
    )?;
    if response.status != 0 {
        return Err(EIO);
    }

    let bytes = response
        .payload
        .get(..size_of::<ScrubGuestFbResponse>())
        .ok_or(EMSGSIZE)?;
    let response = ScrubGuestFbResponse::read_from_bytes(bytes).map_err(|_| EINVAL)?;
    let work_id = u32::try_from(u64::from_le(response.work_id)).map_err(|_| EOVERFLOW)?;
    if work_id == 0 {
        return Err(EIO);
    }

    Ok(work_id)
}

/// Poll the GSP-owned CeUtils semaphore page through a temporary BAR1 map.
fn wait_scrub_complete<'gpu>(
    bar_user: &BarUser<'gpu>,
    mm: &GpuMm<'gpu>,
    dev: &device::Device<device::Bound>,
    semaphore_address: u64,
    work_id: u32,
) -> Result {
    let pfn = Pfn::from(VramAddress::new(semaphore_address));
    let semaphore_map = bar_user.map(mm, &[pfn], false)?;

    let result = (|| {
        let magic = semaphore_map.try_read32(SEMA_PAGE_MAGIC_OFFSET)?;
        if magic != SEMA_PAGE_MAGIC {
            dev_warn!(
                dev,
                "bad CeUtils semaphore magic {:#x}, expected {:#x}\n",
                magic,
                SEMA_PAGE_MAGIC,
            );
            return Err(EIO);
        }

        let mut last_value = 0;
        for poll in 0..SCRUB_TIMEOUT_MS {
            let value = semaphore_map.try_read32(SEMA_PAGE_PAYLOAD_OFFSET)?;
            last_value = value;
            if value.wrapping_sub(work_id) < 0x8000_0000 {
                dev_dbg!(
                    dev,
                    "scrub completed after {} polls: semaphore={:#x}, target={:#x}\n",
                    poll + 1,
                    value,
                    work_id,
                );
                return Ok(());
            }
            fsleep(Delta::from_millis(1));
        }

        dev_warn!(
            dev,
            "scrub timed out: semaphore={:#x}, target={:#x}\n",
            last_value,
            work_id,
        );
        Err(ETIMEDOUT)
    })();

    let cleanup = semaphore_map.release();
    match result {
        Ok(()) => cleanup,
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                dev_err!(
                    dev,
                    "failed to release semaphore BAR1 mapping after error {:?}: {:?}\n",
                    error,
                    cleanup_error,
                );
            }
            Err(error)
        }
    }
}

fn with_bar1_map<'gpu, T>(
    bar_user: &BarUser<'gpu>,
    mm: &GpuMm<'gpu>,
    dev: &device::Device<device::Bound>,
    region: VramRegion,
    writable: bool,
    operation: impl FnOnce(&Bar1Map<'gpu>) -> Result<T>,
) -> Result<T> {
    let map = Bar1Map::new(bar_user, mm, region, writable)?;
    let result = operation(&map);
    let cleanup = map.destroy(bar_user, mm);

    match result {
        Ok(value) => {
            cleanup?;
            Ok(value)
        }
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                dev_err!(
                    dev,
                    "failed to release temporary BAR1 mapping after error {:?}: {:?}\n",
                    error,
                    cleanup_error,
                );
            }
            Err(error)
        }
    }
}

fn marker_regions(fb: &VramRegion) -> Result<(VramRegion, VramRegion, usize)> {
    let page_size = u64::try_from(PAGE_SIZE).map_err(|_| EOVERFLOW)?;
    let tail_page = fb.size().checked_sub(page_size).ok_or(EINVAL)?;
    let tail_offset = PAGE_SIZE.checked_sub(size_of::<u32>()).ok_or(EOVERFLOW)?;

    Ok((
        fb.subregion(0..page_size)?,
        fb.subregion(tail_page..fb.size())?,
        tail_offset,
    ))
}

/// Write and read back markers at the first and last framebuffer dwords.
fn write_markers<'gpu>(
    bar_user: &BarUser<'gpu>,
    mm: &GpuMm<'gpu>,
    dev: &device::Device<device::Bound>,
    fb: &VramRegion,
) -> Result {
    let (head_region, tail_region, tail_offset) = marker_regions(fb)?;

    with_bar1_map(bar_user, mm, dev, head_region, true, |map| {
        map.try_write32(MAGIC_HEAD, 0)?;
        if map.try_read32(0)? != MAGIC_HEAD {
            return Err(EIO);
        }
        Ok(())
    })?;

    with_bar1_map(bar_user, mm, dev, tail_region, true, |map| {
        map.try_write32(MAGIC_TAIL, tail_offset)?;
        if map.try_read32(tail_offset)? != MAGIC_TAIL {
            return Err(EIO);
        }
        Ok(())
    })
}

/// Verify that the first and last framebuffer dwords were zeroed.
fn verify_markers_zeroed<'gpu>(
    bar_user: &BarUser<'gpu>,
    mm: &GpuMm<'gpu>,
    dev: &device::Device<device::Bound>,
    fb: &VramRegion,
) -> Result {
    let (head_region, tail_region, tail_offset) = marker_regions(fb)?;
    let head = with_bar1_map(bar_user, mm, dev, head_region, false, |map| {
        map.try_read32(0)
    })?;
    let tail = with_bar1_map(bar_user, mm, dev, tail_region, false, |map| {
        map.try_read32(tail_offset)
    })?;

    dev_dbg!(
        dev,
        "scrub markers: head={:#010x}, tail={:#010x}\n",
        head,
        tail,
    );
    if head != 0 || tail != 0 {
        return Err(EIO);
    }

    Ok(())
}
