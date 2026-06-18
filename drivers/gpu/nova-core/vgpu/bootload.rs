// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    prelude::*,
    time::Delta, //
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::Cmdq,
        commands::NVGMC_ENGINE_TYPE_COUNT,
        nvkv::{
            self,
            ChannelMapEntry, //
        }, //
    },
    vgpu::consts::gmc, //
};

use super::instance::{
    Gfid,
    VgpuInstance, //
};

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

/// Build the typed channel mapping from the hardcoded Blackwell engine masks.
///
/// Keep the reported masks in the signature for when GSP discovery is fixed.
fn channel_mapping(
    _engine_masks: &[u64; NVGMC_ENGINE_TYPE_COUNT],
    chid_offset: u32,
) -> Result<KVVec<ChannelMapEntry>> {
    let mut mapping = KVVec::new();

    let channel_mapping_masks = blackwell_vgpu_channel_mapping_masks();
    for (engine_type, &mask) in channel_mapping_masks.iter().enumerate().skip(1) {
        let mut remaining = mask;
        let mut index = 0u32;
        while remaining != 0 {
            if remaining & 1 != 0 {
                mapping.push(
                    ChannelMapEntry::new(engine_type, index, chid_offset)?,
                    GFP_KERNEL,
                )?;
            }
            remaining >>= 1;
            index = index.checked_add(1).ok_or(EOVERFLOW)?;
        }
    }

    Ok(mapping)
}

/// Bootload the GSP vGPU plugin and wait for its BAR1 ready indication.
pub(crate) fn bootload(
    dev: &device::Device<device::Bound>,
    cmdq: &Cmdq,
    bar: Bar0<'_>,
    instance: &VgpuInstance<'_>,
    engine_masks: &[u64; NVGMC_ENGINE_TYPE_COUNT],
) -> Result {
    let fb = &instance.vram_slot.fbmem;
    let mgmt = &instance.vram_slot.mgmt_heap;
    let logs = instance.plugin_rpc.plugin_logs()?;

    let payload = nvkv::encode_vgpu_bootload(
        instance.dbdf,
        instance.gfid.0,
        instance.vgpu_type.vgpu_type_id,
        instance.vm_pid,
        u32::try_from(instance.chids.len()).map_err(|_| EOVERFLOW)?,
        instance.num_plugin_channels,
        channel_mapping(
            engine_masks,
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
        payload.len(),
    );

    // BOOTLOAD completes synchronously while the GMC receive path consumes
    // interleaved RM RPC frames and matches the response by command and sequence.
    let response =
        cmdq.send_gmc_and_receive_timeout(bar, gmc::BOOTLOAD, &payload, 0, Delta::from_secs(10))?;
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
                "shutdown: interleaved event command={:#x} status={:#x}\n",
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
    cmdq.send_gmc_no_response(bar, gmc::CLEANUP, &gfid.0.to_le_bytes())?;
    dev_dbg!(dev, "cleanup: gfid={} done\n", gfid.0);
    Ok(())
}
