// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use kernel::{
    device,
    prelude::*,
    time::Delta,
    transmute::AsBytes, //
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::{
            encode_vgpu_bootload,
            ChannelMapEntry,
            FifoEngineList, //
        }, //
    },
    vgpu::consts::gmc, //
};

use super::instance::{
    Gfid,
    VgpuInstance, //
};

/// Build the typed channel mapping from the GSP FIFO engine list.
fn channel_mapping(
    fifo_engine_list: &FifoEngineList,
    chid_offset: u32,
) -> Result<KVVec<ChannelMapEntry>> {
    let mut mapping = KVVec::new();
    for &gmc_id in &fifo_engine_list.gmc_ids[..fifo_engine_list.count] {
        let engine_type = (gmc_id & 0xffff) as usize;
        let index = gmc_id >> 16;
        mapping.push(
            ChannelMapEntry::new(engine_type, index, chid_offset)?,
            GFP_KERNEL,
        )?;
    }
    Ok(mapping)
}

/// Bootload the GSP vGPU plugin and wait for its BAR1 ready indication.
pub(crate) fn bootload(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    instance: &VgpuInstance<'_>,
    fifo_engine_list: &FifoEngineList,
) -> Result {
    let fb = &instance.vram_slot.fbmem;
    let mgmt = &instance.vram_slot.mgmt_heap;
    let logs = instance.plugin_rpc.plugin_logs()?;

    let payload = encode_vgpu_bootload(
        instance.dbdf,
        instance.gfid.0,
        instance.vgpu_type.vgpu_type_id(),
        instance.vm_pid,
        u32::try_from(instance.chids.len()).map_err(|_| EOVERFLOW)?,
        instance.num_plugin_channels,
        channel_mapping(
            fifo_engine_list,
            u32::try_from(instance.chids.start).map_err(|_| EOVERFLOW)?,
        )?,
        fb.address(),
        fb.size(),
        mgmt.address(),
        mgmt.size(),
        0,
        logs.init().address(),
        logs.init().size(),
        logs.vgpu().address(),
        logs.vgpu().size(),
        logs.kernel().address(),
        logs.kernel().size(),
    )?;

    dev_dbg!(
        dev,
        "bootload: gfid={} sending {} typed NVKV bytes\n",
        instance.gfid.0,
        payload.len() * size_of::<u64>(),
    );

    // BOOTLOAD completes synchronously. The receive path dispatches any RM RPC
    // frames that arrive before matching the response by command and sequence.
    let response = cmdq.send_gmc_and_receive_timeout(
        bar,
        gmc::BOOTLOAD,
        AsBytes::as_bytes(payload.as_slice()),
        0,
        Delta::from_secs(10),
    )?;
    if response.status != 0 {
        return Err(EIO);
    }

    instance.plugin_rpc.wait_plugin_ready(dev)?;

    dev_dbg!(dev, "bootload: gfid={} plugin ready\n", instance.gfid.0);
    Ok(())
}

/// Shut down a vGPU plugin task and wait for its completion event.
pub(crate) fn shutdown(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    gfid: Gfid,
) -> Result {
    let payload = gfid.0.to_le_bytes();

    cmdq.send_gmc_and_wait_event(
        bar,
        gmc::SHUTDOWN,
        &payload,
        Delta::from_secs(10),
        |command_id, status, _sequence, payload_0, payload_1| {
            if command_id != gmc::SHUTDOWN_COMPLETE
                || !payload
                    .iter()
                    .copied()
                    .eq(Iterator::chain(payload_0.iter(), payload_1.iter())
                        .take(payload.len())
                        .copied())
            {
                return Ok(false);
            }
            if status != 0 {
                return Err(EIO);
            }
            Ok(true)
        },
        |command_id, status, _sequence, _payload_0, _payload_1| {
            dev_dbg!(
                dev,
                "shutdown: ignoring unrelated event command={:#x} status={:#x}\n",
                command_id,
                status,
            );
            Ok(())
        },
    )?;
    dev_dbg!(dev, "shutdown: gfid={} stopped\n", gfid.0);
    Ok(())
}

/// Release firmware resources after a plugin task has stopped.
pub(crate) fn cleanup(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    gfid: Gfid,
) -> Result {
    cmdq.send_gmc_and_check_status(bar, gmc::CLEANUP, &gfid.0.to_le_bytes())?;
    dev_dbg!(dev, "cleanup: gfid={} done\n", gfid.0);
    Ok(())
}
