// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::NVGMC_ENGINE_TYPE_COUNT,
        nvkv, //
    },
    vgpu::{
        consts::{
            bootload_keys,
            config_keys,
            gmcapi,
            plugin_rpc as rpc_consts, //
        },
        Gfid,
        GmcEngineMasks, //
    },
};

use super::instance::VgpuInstance;

/// Build a 32-bit GMC engine ID from type and instance index.
///
/// Format: `type[15:0] | index[31:16]` per `gmcapi_engine_types.h`.
fn gmc_engine_id(engine_type: usize, index: usize) -> u64 {
    (engine_type as u64) | ((index as u64) << 16)
}

fn blackwell_vgpu_channel_mapping_masks() -> [u64; NVGMC_ENGINE_TYPE_COUNT] {
    const GMC_ENGINE_TYPE_GR: usize = 0x01;
    const GMC_ENGINE_TYPE_COPY: usize = 0x02;
    const GMC_ENGINE_TYPE_NVDEC: usize = 0x03;
    const GMC_ENGINE_TYPE_NVENC: usize = 0x04;
    const GMC_ENGINE_TYPE_SEC2: usize = 0x0d;
    const GMC_ENGINE_TYPE_NVJPEG: usize = 0x12;
    const GMC_ENGINE_TYPE_OFA: usize = 0x13;

    let mut masks = [0u64; NVGMC_ENGINE_TYPE_COUNT];

    masks[GMC_ENGINE_TYPE_GR] = 0x000f; // GR0-3
    masks[GMC_ENGINE_TYPE_COPY] = 0x00ff; // COPY0-7
    masks[GMC_ENGINE_TYPE_NVDEC] = 0x000f; // NVDEC0-3
    masks[GMC_ENGINE_TYPE_NVENC] = 0x000f; // NVENC0-3
    masks[GMC_ENGINE_TYPE_SEC2] = 0x0001; // SEC2
    masks[GMC_ENGINE_TYPE_NVJPEG] = 0x000f; // NVJPEG0-3
    masks[GMC_ENGINE_TYPE_OFA] = 0x0001; // OFA0

    masks
}

/// Send GMCAPI VGPU_BOOTLOAD with NVKV-encoded parameters, then wait for
/// the plugin to signal readiness via the ctrl_buf magic value.
pub(crate) fn bootload(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    instance: &VgpuInstance,
    _engine_masks: &GmcEngineMasks,
) -> Result {
    let mut kvs: KVec<u64> = KVec::new();

    // SEQ32: DBDF, GFID, VGPU_TYPE, VM_PID (keys 0x0001..0x0004)
    nvkv::nvkv_push_seq32(
        &mut kvs,
        bootload_keys::DBDF,
        &[
            instance.dbdf.0,
            instance.gfid.0,
            instance.vgpu_type.vgpu_type_id,
            instance.vm_pid,
        ],
    )?;

    // SEQ32: SWIZZ_ID, NUM_CHANNELS, NUM_PLUGIN_CHANNELS (keys 0x0005..0x0007)
    nvkv::nvkv_push_seq32(
        &mut kvs,
        bootload_keys::SWIZZ_ID,
        &[
            config_keys::SWIZZ_ID_NONE,
            instance.num_chid,
            instance.num_plugin_channels,
        ],
    )?;

    // ARRAY64: CHANNEL_MAPPING
    let mut channel_map: KVec<u64> = KVec::new();
    let channel_mapping_masks = blackwell_vgpu_channel_mapping_masks();
    for (engine_type, &mask) in channel_mapping_masks.iter().enumerate().skip(1) {
        let mut bit = 0u32;
        let mut remaining = mask;
        while remaining != 0 {
            if remaining & 1 != 0 {
                let engine_id = gmc_engine_id(engine_type, bit as usize);
                let entry = engine_id | (u64::from(instance.chid_offset) << 32);
                channel_map.push(entry, GFP_KERNEL)?;
            }
            remaining >>= 1;
            bit += 1;
        }
    }
    nvkv::nvkv_push_array64(
        &mut kvs,
        bootload_keys::CHANNEL_MAPPING,
        channel_map.as_slice(),
    )?;

    let fb = instance.fbmem_heap.as_ref().ok_or(EINVAL)?;

    // SEQ32: GUEST_FB_SEGMENT_COUNT (key 0x0008)
    nvkv::nvkv_push_seq32(&mut kvs, bootload_keys::GUEST_FB_SEGMENT_COUNT, &[1])?;

    // ARRAY64: GUEST_FB_SEGMENT_PHYS_ADDR_LIST, GUEST_FB_SEGMENT_LENGTH_LIST
    nvkv::nvkv_push_array64(
        &mut kvs,
        bootload_keys::GUEST_FB_SEGMENT_PHYS_ADDR,
        &[fb.addr],
    )?;
    nvkv::nvkv_push_array64(
        &mut kvs,
        bootload_keys::GUEST_FB_SEGMENT_LENGTH,
        &[fb.size],
    )?;

    let mgmt = instance.mgmt_heap.as_ref().ok_or(EINVAL)?;

    // SEQ64: PLUGIN_HEAP_PHYS_ADDR, PLUGIN_HEAP_LENGTH, CTRL_BUFF_OFFSET (keys 0x1004..0x1006)
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::PLUGIN_HEAP_PHYS_ADDR,
        &[mgmt.addr, mgmt.size, 0],
    )?;

    // SEQ64: INIT_LOG_OFFSET, INIT_LOG_SIZE, VGPU_LOG_OFFSET, VGPU_LOG_SIZE (keys 0x1007..0x100A)
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::INIT_TASK_LOG_OFFSET,
        &[
            mgmt.addr + rpc_consts::INIT_TASK_LOG_OFFSET,
            rpc_consts::INIT_LOG_SIZE,
            mgmt.addr + rpc_consts::VGPU_TASK_LOG_OFFSET,
            rpc_consts::VGPU_LOG_SIZE,
        ],
    )?;

    // SEQ64: KERNEL_LOG_OFFSET, KERNEL_LOG_SIZE, MIG_RM_HEAP_PHYS_ADDR, MIG_RM_HEAP_LENGTH
    // (keys 0x100B..0x100E)
    nvkv::nvkv_push_seq64(
        &mut kvs,
        bootload_keys::KERNEL_LOG_OFFSET,
        &[
            mgmt.addr + rpc_consts::KERNEL_LOG_OFFSET,
            rpc_consts::KERNEL_LOG_SIZE,
            0,
            0,
        ],
    )?;

    // SEQ64: OPTIONS (key 0x1000)
    nvkv::nvkv_push_seq64(&mut kvs, bootload_keys::OPTIONS, &[0])?;

    // SAFETY: `kvs` is a valid `KVec<u64>` and we reinterpret it as a byte
    // slice of the same total size. The pointer is valid for `len * 8` bytes.
    let payload: &[u8] =
        unsafe { core::slice::from_raw_parts(kvs.as_ptr().cast::<u8>(), kvs.len() * 8) };

    dev_dbg!(
        dev,
        "bootload: gfid={} sending VGPU_BOOTLOAD ({} kvs entries, {} bytes)\n",
        instance.gfid.0,
        kvs.len(),
        payload.len()
    );

    cmdq.send_gmc_fire_and_forget(bar, gmcapi::VGPU_BOOTLOAD, payload)?;

    dev_dbg!(
        dev,
        "bootload: gfid={} waiting for plugin ready\n",
        instance.gfid.0
    );

    // Wait for the plugin to signal readiness via ctrl_buf while concurrently draining
    // the GSP→CPU queue.  GSP may post GMCAPI events during NVDEC RISC-V bootstrap and
    // then stall in GspMsgQueueSyncStatus_GSP until the host consumes them before ACR
    // lockdown.  fire-and-forget VGPU_BOOTLOAD means nova-core is not inside an RPC wait
    // loop, so the drain must happen here.
    let rpc = instance.plugin_rpc.as_ref().ok_or(EINVAL)?;
    let start = Instant::<Monotonic>::now();
    let timeout = Delta::from_millis(rpc_consts::PLUGIN_BOOT_TIMEOUT_MS as i64);
    loop {
        if rpc.is_ready(dev)? {
            break;
        }
        if start.elapsed() > timeout {
            dev_dbg!(
                dev,
                "bootload: gfid={} plugin ready timeout\n",
                instance.gfid.0
            );
            return Err(ETIMEDOUT);
        }
        cmdq.drain_gsp_tx_events(bar);
        fsleep(Delta::from_millis(1));
    }

    dev_dbg!(dev, "bootload: gfid={} plugin ready\n", instance.gfid.0);

    Ok(())
}

/// Send GMCAPI VGPU_SHUTDOWN, wait for VGPU_SHUTDOWN_COMPLETE, then send VGPU_CLEANUP.
pub(crate) fn shutdown(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: &Bar0,
    gfid: Gfid,
) -> Result {
    dev_dbg!(dev, "shutdown: gfid={} sending VGPU_SHUTDOWN\n", gfid.0);

    let mut payload = KVec::<u8>::new();
    payload.extend_from_slice(&gfid.0.to_le_bytes(), GFP_KERNEL)?;

    cmdq.send_gmc_fire_and_forget(bar, gmcapi::VGPU_SHUTDOWN, payload.as_slice())?;

    dev_dbg!(
        dev,
        "shutdown: gfid={} waiting for SHUTDOWN_COMPLETE\n",
        gfid.0
    );

    cmdq.wait_gmc_event(bar, Delta::from_secs(10), |cmd_id, _data| {
        cmd_id == gmcapi::VGPU_SHUTDOWN_COMPLETE
    })?;

    cmdq.send_gmc_no_response(bar, gmcapi::VGPU_CLEANUP, payload.as_slice())?;

    dev_dbg!(dev, "shutdown: gfid={} done\n", gfid.0);

    Ok(())
}
