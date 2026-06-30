// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use super::GinHal;

/// GIN parameters for Hopper and Blackwell, which implement a 16-leaf PF CPU
/// tree. Only 12 leaves are currently used, but arming the unused subtrees is
/// harmless because they read back zero.
struct Gh100;

impl GinHal for Gh100 {
    fn num_leaves(&self) -> usize {
        16
    }
}

const GH100: Gh100 = Gh100;
pub(super) const GH100_HAL: &dyn GinHal = &GH100;
