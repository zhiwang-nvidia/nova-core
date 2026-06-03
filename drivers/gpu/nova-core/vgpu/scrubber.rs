// SPDX-License-Identifier: GPL-2.0

//! Per-VM CeUtils guest framebuffer scrubbing.
//!
//! Sends GMCAPI commands to GSP-RM to manage a per-VM Copy Engine utility
//! channel and scrub guest framebuffer memory during vGPU create, destroy,
//! and reset.
//!
//! Wire types match `gmcapi_vgpu.h` (`GmcapiVgpuMgr*` structs).

use kernel::{
    device,
    prelude::*,
    sync::Arc,
    time::{delay::fsleep, Delta},
};

use crate::{
    driver::Bar0,
    gsp::cmdq::Cmdq,
    mm::bar_user::{Bar1Map, BarUser},
    vgpu::{
        consts::gmcapi,
        Gfid, //
    },
};

/// Reinterpret a `repr(C)` struct as a byte slice for GMCAPI payloads.
///
/// # Safety
///
/// `val` must be a `repr(C)` POD type with no padding that carries
/// security-sensitive data. The returned slice borrows `val`.
unsafe fn as_bytes<T>(val: &T) -> &[u8] {
    // SAFETY: Caller guarantees `T` is repr(C) POD with no padding.
    // `from_ref` yields a valid, aligned pointer; size_of::<T>() bytes are in bounds.
    unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(val).cast::<u8>(),
            core::mem::size_of::<T>(),
        )
    }
}

/// Allocate a per-VM CeUtils scrubber channel on GSP-RM.
///
/// `chid` is the fixed channel ID reserved for CeUtils within the VM's
/// channel range (pass `u32::MAX` to let RM choose).
/// `swizz_id` is the MIG GPU instance ID (0 for non-MIG).
///
/// Returns `(sema_phys_addr, sema_aperture)` from GSP.
pub(crate) fn alloc_ceutils(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    gfid: Gfid,
    chid: u32,
    swizz_id: u32,
) -> Result<(u64, u32)> {
    #[repr(C)]
    struct InParams {
        gfid: u32,
        fixed_ch_id: u32,
        force_ce_id: u32,
        swizz_id: u32,
    }

    let req = InParams {
        gfid: gfid.0,
        fixed_ch_id: chid,
        force_ce_id: u32::MAX,
        swizz_id,
    };

    dev_dbg!(
        dev,
        "alloc_ceutils: gfid={} chid={} swizz_id={}\n",
        gfid.0, chid, swizz_id
    );

    // SAFETY: `InParams` is repr(C) POD matching the wire layout.
    let resp = cmdq.send_gmc_and_receive(
        bar,
        gmcapi::VGPU_MGR_ALLOC_GSP_CEUTILS,
        unsafe { as_bytes(&req) },
        16,
    )?;

    if resp.status != 0 {
        dev_warn!(
            dev,
            "alloc_ceutils: gfid={} GSP returned status {:#x}\n",
            gfid.0, resp.status
        );
        return Err(EIO);
    }

    dev_dbg!(
        dev,
        "alloc_ceutils: resp status={:#x} payload len={}\n",
        resp.status, resp.payload.len()
    );

    let sema_phys = if resp.payload.len() >= 8 {
        u64::from_le_bytes(resp.payload[..8].try_into().map_err(|_| EINVAL)?)
    } else {
        0
    };
    let sema_aperture = if resp.payload.len() >= 12 {
        u32::from_le_bytes(resp.payload[8..12].try_into().map_err(|_| EINVAL)?)
    } else {
        0
    };

    Ok((sema_phys, sema_aperture))
}

/// Destroy the per-VM CeUtils scrubber channel on GSP-RM.
pub(crate) fn free_ceutils(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    gfid: Gfid,
) -> Result {
    #[repr(C)]
    struct InParams {
        gfid: u32,
    }

    let req = InParams { gfid: gfid.0 };

    dev_dbg!(dev, "free_ceutils: gfid={}\n", gfid.0);

    // SAFETY: `InParams` is repr(C) POD matching the wire layout.
    cmdq.send_gmc_no_response(bar, gmcapi::VGPU_MGR_FREE_GSP_CEUTILS, unsafe {
        as_bytes(&req)
    })
}

/// Scrub the guest framebuffer: write magic markers, submit CE scrub,
/// wait for HW completion, then verify the FB is zeroed.
///
/// All sub-steps are best-effort — failures are logged but do not abort
/// the overall operation.
#[expect(clippy::too_many_arguments)]
pub(crate) fn scrub_guest_fb(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    bar_user: &Arc<BarUser>,
    gfid: Gfid,
    fb_addr: u64,
    fb_size: u64,
    sema_phys_addr: u64,
) {
    if let Err(e) = write_magic(bar_user, dev, fb_addr, fb_size) {
        dev_warn!(dev, "scrub_guest_fb: gfid={} write_magic failed: {:?}\n", gfid.0, e);
    }

    match submit_scrub(dev, cmdq, bar, gfid, fb_addr, fb_size) {
        Ok(work_id) => {
            if let Err(e) = wait_scrub_complete(bar_user, dev, sema_phys_addr, work_id) {
                dev_warn!(
                    dev,
                    "scrub_guest_fb: gfid={} wait_scrub timed out: {:?}\n",
                    gfid.0, e
                );
            }
        }
        Err(e) => {
            dev_warn!(dev, "scrub_guest_fb: gfid={} submit_scrub failed: {:?}\n", gfid.0, e);
        }
    }

    if let Err(e) = verify_scrub(bar_user, dev, fb_addr, fb_size) {
        dev_warn!(dev, "scrub_guest_fb: gfid={} verify_scrub failed: {:?}\n", gfid.0, e);
    }
}

/// Submit a guest FB scrub command and return the `submitted_work_id`.
fn submit_scrub(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    gfid: Gfid,
    fb_offset: u64,
    fb_size: u64,
) -> Result<u64> {
    #[repr(C)]
    struct InParams {
        gfid: u32,
        reserved: u32,
        fb_offset: u64,
        fb_size: u64,
    }

    let req = InParams {
        gfid: gfid.0,
        reserved: 0,
        fb_offset,
        fb_size,
    };

    dev_dbg!(
        dev,
        "submit_scrub: gfid={} offset={:#x} size={:#x}\n",
        gfid.0, fb_offset, fb_size
    );

    // SAFETY: `InParams` is repr(C) POD matching the wire layout.
    let resp = cmdq.send_gmc_and_receive(
        bar,
        gmcapi::VGPU_MGR_SCRUB_GUEST_FB,
        unsafe { as_bytes(&req) },
        8,
    )?;

    if resp.status != 0 {
        dev_warn!(
            dev,
            "submit_scrub: gfid={} GSP returned status {:#x}\n",
            gfid.0, resp.status
        );
        return Err(EIO);
    }

    if resp.payload.len() < 8 {
        dev_warn!(
            dev,
            "submit_scrub: gfid={} response too short ({})\n",
            gfid.0,
            resp.payload.len()
        );
        return Err(EIO);
    }

    Ok(u64::from_le_bytes(
        resp.payload[..8].try_into().map_err(|_| EINVAL)?,
    ))
}

const SEMA_PAGE_MAGIC: u32 = 0xCE5E_5EA0;
const SEMA_PAGE_MAGIC_OFFSET: u64 = 0x0;
const SEMA_PAGE_PAYLOAD_OFFSET: u64 = 0x4;

/// Poll the CeUtils semaphore page until the HW payload >= `work_id`.
fn wait_scrub_complete(
    bar_user: &Arc<BarUser>,
    dev: &device::Device<device::Bound>,
    sema_phys_addr: u64,
    work_id: u64,
) -> Result {
    if sema_phys_addr == 0 {
        dev_warn!(dev, "wait_scrub_complete: sema_phys_addr is 0, skipping poll\n");
        return Ok(());
    }

    let sema_map = Bar1Map::new(bar_user, dev, sema_phys_addr, 4096)?;

    let magic = sema_map.read32(dev, SEMA_PAGE_MAGIC_OFFSET)?;
    if magic != SEMA_PAGE_MAGIC {
        dev_warn!(
            dev,
            "wait_scrub_complete: bad sema page magic {:#x} (expected {:#x}), skipping poll\n",
            magic, SEMA_PAGE_MAGIC
        );
        sema_map.destroy(dev)?;
        return Err(EIO);
    }

    let target = work_id as u32;
    let mut last_val = 0u32;
    let max_iters = 5000;

    for i in 0..max_iters {
        let val = sema_map.read32(dev, SEMA_PAGE_PAYLOAD_OFFSET)?;
        last_val = val;

        if val.wrapping_sub(target) < 0x8000_0000 || val == target {
            dev_dbg!(
                dev,
                "wait_scrub_complete: done after {} polls, sema={:#x} target={:#x}\n",
                i + 1, val, target
            );
            sema_map.destroy(dev)?;
            return Ok(());
        }

        fsleep(Delta::from_millis(1));
    }

    dev_warn!(
        dev,
        "wait_scrub_complete: timeout after {} ms, sema={:#x} target={:#x}\n",
        max_iters, last_val, target
    );
    sema_map.destroy(dev)?;
    Err(ETIMEDOUT)
}

const MAGIC_HEAD: u32 = 0xDEAD_BEEF;
const MAGIC_TAIL: u32 = 0xCAFE_BABE;

/// Write magic numbers at the beginning and end of the guest FB via BAR1.
fn write_magic(
    bar_user: &Arc<BarUser>,
    dev: &device::Device<device::Bound>,
    fb_addr: u64,
    fb_size: u64,
) -> Result {
    let page_size = 4096u64;

    let head_map = Bar1Map::new(bar_user, dev, fb_addr, page_size)?;
    head_map.write32(dev, 0, MAGIC_HEAD)?;
    let readback = head_map.read32(dev, 0)?;
    dev_dbg!(
        dev,
        "write_magic: wrote MAGIC_HEAD={:#x} at fb+0x0, readback={:#x}\n",
        MAGIC_HEAD, readback
    );
    head_map.destroy(dev)?;

    let tail_offset = (fb_size - page_size) & !(page_size - 1);
    let tail_map = Bar1Map::new(bar_user, dev, fb_addr + tail_offset, page_size)?;
    tail_map.write32(dev, 0, MAGIC_TAIL)?;
    let readback = tail_map.read32(dev, 0)?;
    dev_dbg!(
        dev,
        "write_magic: wrote MAGIC_TAIL={:#x} at fb+{:#x}, readback={:#x}\n",
        MAGIC_TAIL, tail_offset, readback
    );
    tail_map.destroy(dev)?;

    Ok(())
}

/// After scrub, read back the same locations and verify zeroed.
fn verify_scrub(
    bar_user: &Arc<BarUser>,
    dev: &device::Device<device::Bound>,
    fb_addr: u64,
    fb_size: u64,
) -> Result {
    let page_size = 4096u64;

    let head_map = Bar1Map::new(bar_user, dev, fb_addr, page_size)?;
    let head_val = head_map.read32(dev, 0)?;
    head_map.destroy(dev)?;

    let tail_offset = (fb_size - page_size) & !(page_size - 1);
    let tail_map = Bar1Map::new(bar_user, dev, fb_addr + tail_offset, page_size)?;
    let tail_val = tail_map.read32(dev, 0)?;
    tail_map.destroy(dev)?;

    dev_dbg!(
        dev,
        "verify_scrub: head={:#010x} (expect 0), tail={:#010x} (expect 0)\n",
        head_val, tail_val
    );

    if head_val == 0 && tail_val == 0 {
        dev_dbg!(dev, "verify_scrub: PASS — guest FB scrubbed successfully\n");
    } else {
        dev_warn!(
            dev,
            "verify_scrub: FAIL — stale data remains (head={:#x} tail={:#x})\n",
            head_val, tail_val
        );
    }

    Ok(())
}
