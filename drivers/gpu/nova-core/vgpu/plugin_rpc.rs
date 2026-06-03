// SPDX-License-Identifier: GPL-2.0

use kernel::{
    device,
    io::Io,
    prelude::*,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    }, //
};

use crate::{
    driver::Bar0,
    gsp::nvkv::{
        self,
        Dbdf, //
    },
    mm::{
        bar_user::BarUser,
        GpuMm, //
    },
    vgpu::fw::{
        CommBufferRegion,
        MappedPluginLogBuffers,
        PluginLogRegions,
        RpcMessage,
        RpcResponse, //
    }, //
};

use super::{
    consts::plugin_rpc as consts,
    instance::Gfid, //
};

/// Values sent in the plugin's setup-configuration RPC.
pub(crate) struct PluginConfigParams {
    uuid: [u8; 16],
    dbdf: Dbdf,
    vgpu_type: u32,
    vm_pid: u32,
    num_channels: u32,
    num_plugin_channels: u32,
}

impl PluginConfigParams {
    pub(crate) const fn new(
        uuid: [u8; 16],
        dbdf: Dbdf,
        vgpu_type: u32,
        vm_pid: u32,
        num_channels: u32,
        num_plugin_channels: u32,
    ) -> Self {
        Self {
            uuid,
            dbdf,
            vgpu_type,
            vm_pid,
            num_channels,
            num_plugin_channels,
        }
    }
}

/// BAR1-backed channel used to communicate with one vGPU plugin.
pub(crate) struct PluginRpc<'gpu> {
    comm: Option<CommBufferRegion<'gpu>>,
    message_sequence: u32,
}

impl<'gpu> PluginRpc<'gpu> {
    /// Create a channel over the mapped communication buffer.
    pub(crate) fn new(comm: CommBufferRegion<'gpu>) -> Self {
        Self {
            comm: Some(comm),
            message_sequence: 0,
        }
    }

    fn comm(&self) -> Result<&CommBufferRegion<'gpu>> {
        self.comm.as_ref().ok_or(EIO)
    }

    /// Return the physical regions occupied by the plugin logs.
    pub(crate) fn plugin_logs(&self) -> Result<PluginLogRegions> {
        self.comm()?.plugin_logs()
    }

    /// Return revocable BAR1 views of the plugin logs.
    pub(crate) fn mapped_plugin_logs(&self) -> Result<MappedPluginLogBuffers> {
        self.comm()?.mapped_plugin_logs()
    }

    /// Poll the control buffer until the plugin publishes its boot marker.
    pub(crate) fn wait_plugin_ready(&self, dev: &device::Device<device::Bound>) -> Result {
        let start = Instant::<Monotonic>::now();
        let timeout = Delta::from_millis(consts::PLUGIN_BOOT_TIMEOUT_MS);

        loop {
            if self.comm()?.is_plugin_ready()? {
                dev_dbg!(dev, "vGPU plugin ready after {:?}\n", start.elapsed());
                return Ok(());
            }
            if start.elapsed() >= timeout {
                return Err(ETIMEDOUT);
            }
            fsleep(Delta::from_millis(1));
        }
    }

    /// Initialize the control and response buffers for the first RPC.
    pub(crate) fn init_rpc(&mut self) -> Result {
        self.comm()?.initialize()?;
        self.message_sequence = 0;
        Ok(())
    }

    fn next_sequence(&self) -> u32 {
        let sequence = self.message_sequence.wrapping_add(1);
        if sequence == 0 {
            1
        } else {
            sequence
        }
    }

    /// Write one RPC message, ring the VF doorbell, and wait for its response.
    pub(crate) fn rpc_call(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        message_type: RpcMessage,
        data: &[u8],
    ) -> Result {
        let sequence = self.next_sequence();
        self.comm()?.submit(message_type, sequence, data)?;
        self.message_sequence = sequence;

        dev_dbg!(
            dev,
            "plugin RPC: gfid={} type={} bytes={} sequence={}\n",
            gfid.0,
            message_type as u32,
            data.len(),
            sequence,
        );

        ring_doorbell(bar0, gfid)?;
        self.wait_response(dev, sequence)
    }

    fn wait_response(&self, dev: &device::Device<device::Bound>, expected_sequence: u32) -> Result {
        let start = Instant::<Monotonic>::now();
        let timeout = Delta::from_secs(120);

        loop {
            match self.comm()?.response(expected_sequence)? {
                RpcResponse::Complete { status } => {
                    if status != 0 {
                        dev_dbg!(
                            dev,
                            "plugin RPC sequence {} failed with status {}\n",
                            expected_sequence,
                            status,
                        );
                        return Err(EIO);
                    }

                    dev_dbg!(
                        dev,
                        "plugin RPC sequence {} completed after {:?}\n",
                        expected_sequence,
                        start.elapsed(),
                    );
                    return Ok(());
                }
                RpcResponse::Pending { sequence } => {
                    if start.elapsed() >= timeout {
                        dev_dbg!(
                            dev,
                            "plugin RPC sequence {} timed out; last response was {}\n",
                            expected_sequence,
                            sequence,
                        );
                        return Err(ETIMEDOUT);
                    }
                }
            }
            fsleep(Delta::from_millis(1));
        }
    }

    /// Negotiate the plugin RPC protocol version.
    pub(crate) fn negotiate_rpc_version(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
    ) -> Result {
        self.rpc_call(dev, bar0, gfid, RpcMessage::VersionNegotiation, &[])
    }

    /// Send the NVKV-encoded v2 configuration message.
    pub(crate) fn send_config_params(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        params: &PluginConfigParams,
    ) -> Result {
        let encoded = nvkv::encode_plugin_config_params(
            params.uuid,
            params.dbdf,
            params.vgpu_type,
            params.vm_pid,
            params.num_channels,
            params.num_plugin_channels,
        )?;
        let payload = nvkv_rpc_payload(&encoded)?;

        self.rpc_call(
            dev,
            bar0,
            gfid,
            RpcMessage::SetupConfigParamsAndInit,
            &payload,
        )
    }

    /// Send a Bus Master Enable state update.
    pub(crate) fn set_bme(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        enable: bool,
    ) -> Result {
        let encoded = nvkv::encode_plugin_set_bme(enable)?;
        let payload = nvkv_rpc_payload(&encoded)?;

        self.rpc_call(dev, bar0, gfid, RpcMessage::UpdateBmeState, &payload)
    }

    /// Release the BAR1 mapping.
    pub(crate) fn destroy(&mut self, bar_user: &BarUser<'gpu>, mm: &GpuMm<'gpu>) -> Result {
        self.comm.take().ok_or(EIO)?.destroy(bar_user, mm)
    }
}

fn nvkv_rpc_payload(encoded: &[u8]) -> Result<KVec<u8>> {
    if !encoded.len().is_multiple_of(size_of::<u64>()) {
        return Err(EINVAL);
    }

    let word_count = u64::try_from(encoded.len() / size_of::<u64>()).map_err(|_| EOVERFLOW)?;
    let mut payload = KVec::new();
    payload.extend_from_slice(&word_count.to_le_bytes(), GFP_KERNEL)?;
    payload.extend_from_slice(encoded, GFP_KERNEL)?;
    Ok(payload)
}

fn ring_doorbell(bar0: Bar0<'_>, gfid: Gfid) -> Result {
    let value = gfid
        .0
        .checked_mul(consts::DOORBELL_STRIDE)
        .and_then(|value| value.checked_add(consts::DOORBELL_VECTOR))
        .ok_or(EOVERFLOW)?;
    bar0.try_write32(value, consts::NV_VIRTUAL_FUNCTION_PRIV_DOORBELL)?;
    bar0.try_read32(consts::NV_VIRTUAL_FUNCTION_PRIV_DOORBELL)?;
    Ok(())
}
