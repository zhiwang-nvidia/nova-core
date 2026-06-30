// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

use super::GinHal;

/// GIN parameters for Turing, Ampere, and Ada, which have an 8-leaf PF CPU tree.
struct Tu102;

impl GinHal for Tu102 {
    fn num_leaves(&self) -> usize {
        8
    }
}

const TU102: Tu102 = Tu102;
pub(super) const TU102_HAL: &dyn GinHal = &TU102;
