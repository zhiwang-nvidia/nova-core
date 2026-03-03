// SPDX-License-Identifier: GPL-2.0

use core::{
    array,
    convert::Infallible,
    mem::size_of, //
};

use kernel::{
    prelude::*,
    transmute::FromBytes, //
};

use crate::{
    driver::Bar0,
    gsp::{
        cmdq::{
            Cmdq,
            CommandToGsp,
            MessageFromGsp, //
        },
        fw::{
            rm::*,
            MsgFunction,
            NvStatus, //
        },
    },
    sbuffer::SBufferIter,
};

/// Command for sending an RM control message to the GSP.
struct RmControl<'a> {
    h_client: u32,
    h_object: u32,
    cmd: u32,
    params: &'a [u8],
}

impl<'a> RmControl<'a> {
    /// Creates a new RM control command.
    fn new(h_client: u32, h_object: u32, cmd: impl Into<u32>, params: &'a [u8]) -> Self {
        Self {
            h_client,
            h_object,
            cmd: cmd.into(),
            params,
        }
    }
}

impl CommandToGsp for RmControl<'_> {
    const FUNCTION: MsgFunction = MsgFunction::GspRmControl;
    type Command = GspRmControl;
    type Reply = RmControlReply;
    type InitError = Infallible;

    fn init(&self) -> impl Init<Self::Command, Self::InitError> {
        GspRmControl::new(self.h_client, self.h_object, self.cmd, self.params.len() as u32)
    }

    fn variable_payload_len(&self) -> usize {
        self.params.len()
    }

    fn init_variable_payload(
        &self,
        dst: &mut SBufferIter<array::IntoIter<&mut [u8], 2>>,
    ) -> Result {
        dst.write_all(self.params)
    }
}

/// Response from an RM control message.
pub(crate) struct RmControlReply {
    status: NvStatus,
    params: KVVec<u8>,
}

impl MessageFromGsp for RmControlReply {
    const FUNCTION: MsgFunction = MsgFunction::GspRmControl;
    type Message = GspRmControl;
    type InitError = Error;

    fn read(
        msg: &Self::Message,
        sbuffer: &mut SBufferIter<array::IntoIter<&[u8], 2>>,
    ) -> Result<Self, Self::InitError> {
        Ok(RmControlReply {
            status: msg.status(),
            params: sbuffer.flush_into_kvvec(GFP_KERNEL)?,
        })
    }
}

/// Sends an RM control command, checks the reply status, and returns the raw parameter bytes.
pub(crate) fn send_rm_control(
    cmdq: &Cmdq,
    bar: &Bar0,
    h_client: u32,
    h_object: u32,
    cmd: RmControlMsgFunction,
    params: &[u8],
) -> Result<KVVec<u8>> {
    let reply = cmdq.send_command(bar, RmControl::new(h_client, h_object, cmd, params))?;

    Result::from(reply.status)?;

    Ok(reply.params)
}

/// Sends a raw RM control command (by u32 cmd) and copies the reply back into `params`.
///
/// Returns the raw NV_STATUS value from the firmware response as u32.
/// Callers should check this value themselves — unlike [`send_rm_control`],
/// this function does NOT treat non-zero status as error.
pub(crate) fn send_rmcontrol_with_reply(
    cmdq: &Cmdq,
    bar: &Bar0,
    cmd: u32,
    params: &mut [u8],
    h_client: u32,
    h_subdevice: u32,
) -> Result<u32> {
    let reply = cmdq.send_command(bar, RmControl::new(h_client, h_subdevice, cmd, &*params))?;

    let copy_len = params.len().min(reply.params.len());
    params[..copy_len].copy_from_slice(&reply.params[..copy_len]);

    // Return 0 for Ok, non-zero for error status.
    Ok(if reply.status == NvStatus::Ok { 0 } else { 1 })
}

/// Sends the `CeGetFaultMethodBufferSize` RM control command and waits for its reply.
///
/// Returns the CE fault method buffer size in bytes.
#[expect(dead_code)]
pub(crate) fn get_ce_fault_method_buffer_size(
    cmdq: &Cmdq,
    bar: &Bar0,
    h_client: u32,
    h_subdevice: u32,
) -> Result<u32> {
    // Stack-allocate the request; CeGetFaultMethodBufferSizeParams is small (4 bytes).
    let req = [0u8; size_of::<CeGetFaultMethodBufferSizeParams>()];

    let reply = send_rm_control(
        cmdq,
        bar,
        h_client,
        h_subdevice,
        RmControlMsgFunction::CeGetFaultMethodBufferSize,
        &req,
    )?;

    let params = CeGetFaultMethodBufferSizeParams::from_bytes(&reply).ok_or(EINVAL)?;

    Ok(params.size())
}
