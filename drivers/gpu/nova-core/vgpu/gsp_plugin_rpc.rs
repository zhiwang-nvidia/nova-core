// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

//! GSP plugin RPC.
//!
//! ```text
//! Host (PluginRpc)       Shared RPC buffer (VRAM)       GSP plugin
//!       |                           |                       |
//!       |-- BAR1: payload --------->| Message               |
//!       |-- BAR1: type, sequence -->| Control               |
//!       |                           |                       |
//!       |-- BAR0: VF doorbell ----------------------------->|
//!       |                           |<-- read request ------|
//!       |                           |                       | process RPC
//!       |                           |<-- completion --------|
//!       |-- poll processed seq ---->| Response              |
//!       |<-- matching seq, status --|                       |
//! ```

use kernel::{
    device,
    prelude::*,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
    transmute::AsBytes, //
};

use crate::{
    driver::Bar0,
    mm::GpuMm,
    regs::NV_VIRTUAL_FUNCTION_PRIV_DOORBELL, //
};

use super::{
    fw::{
        RpcMessage,
        RpcResponse, //
    },
    gsp_plugin_comm::CommBufferRegion,
    instance::Gfid, //
};

/// BAR1-backed channel used to communicate with one GSP plugin.
pub(super) struct PluginRpc<'map, 'gpu> {
    comm: CommBufferRegion<'map, 'gpu>,
    message_sequence: u32,
}

impl<'map, 'gpu> PluginRpc<'map, 'gpu> {
    pub(super) fn new(comm: CommBufferRegion<'map, 'gpu>) -> Self {
        Self {
            comm,
            message_sequence: 0,
        }
    }

    pub(super) fn comm(&self) -> &CommBufferRegion<'map, 'gpu> {
        &self.comm
    }

    /// Initialize the control and response buffers for the first RPC.
    pub(super) fn init_rpc(&mut self) -> Result {
        self.comm.initialize()?;
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
    pub(super) fn rpc_call(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        message_type: RpcMessage,
        data: &[u8],
    ) -> Result {
        let sequence = self.next_sequence();
        self.comm.submit(message_type, sequence, data)?;
        self.message_sequence = sequence;

        dev_dbg!(
            dev,
            "vGPU RPC: gfid={} type={} bytes={} sequence={}\n",
            gfid.0,
            message_type as u32,
            data.len(),
            sequence,
        );

        NV_VIRTUAL_FUNCTION_PRIV_DOORBELL::ring_gsp_plugin(bar0, gfid.0)?;
        self.wait_response(dev, sequence)
    }

    /// Send an NVKV stream prefixed by its word count.
    pub(super) fn rpc_call_nvkv(
        &mut self,
        dev: &device::Device<device::Bound>,
        bar0: Bar0<'_>,
        gfid: Gfid,
        message_type: RpcMessage,
        encoded: &[u64],
    ) -> Result {
        let word_count = u64::try_from(encoded.len()).map_err(|_| EOVERFLOW)?;
        let mut payload = KVec::new();
        payload.extend_from_slice(&word_count.to_le_bytes(), GFP_KERNEL)?;
        payload.extend_from_slice(AsBytes::as_bytes(encoded), GFP_KERNEL)?;

        self.rpc_call(dev, bar0, gfid, message_type, &payload)
    }

    fn wait_response(&self, dev: &device::Device<device::Bound>, expected_sequence: u32) -> Result {
        let start = Instant::<Monotonic>::now();
        let timeout = Delta::from_secs(120);

        loop {
            match self.comm.response(expected_sequence)? {
                RpcResponse::Complete { status } => {
                    if status != 0 {
                        dev_dbg!(
                            dev,
                            "vGPU RPC: sequence {} failed with status {}\n",
                            expected_sequence,
                            status,
                        );
                        return Err(EIO);
                    }

                    dev_dbg!(
                        dev,
                        "vGPU RPC: sequence {} completed after {:?}\n",
                        expected_sequence,
                        start.elapsed(),
                    );
                    return Ok(());
                }
                RpcResponse::Pending { sequence } => {
                    if start.elapsed() >= timeout {
                        dev_dbg!(
                            dev,
                            "vGPU RPC: sequence {} timed out; last response was {}\n",
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

    /// Release the BAR1 mapping.
    pub(super) fn destroy(self, mm: &mut GpuMm<'_>) -> Result {
        self.comm.destroy(mm)
    }
}
