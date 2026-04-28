// SPDX-License-Identifier: GPL-2.0

use kernel::{
    prelude::*,
    transmute::{
        AsBytes,
        FromBytes, //
    }, //
};

use super::r000;

/// Payload of the `GetGspStaticInfo` command and message.
#[expect(dead_code)]
#[repr(transparent)]
#[derive(Zeroable)]
pub(crate) struct GspStaticConfigInfo(r000::GspStaticConfigInfo_t);

// SAFETY: Padding is explicit and will not contain uninitialized data.
unsafe impl AsBytes for GspStaticConfigInfo {}

// SAFETY: This struct only contains integer types for which all bit patterns
// are valid.
unsafe impl FromBytes for GspStaticConfigInfo {}
