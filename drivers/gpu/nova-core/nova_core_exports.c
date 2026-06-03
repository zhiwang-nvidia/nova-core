// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

/*
 * Exports Rust symbols from the `nova_core` crate for use by dependent modules.
 *
 * This is a workaround until the build system supports Rust cross-module
 * dependencies natively.
 */

#include <drm/nvidia_vgpu.h>
#include <linux/export.h>

EXPORT_SYMBOL_NS_GPL(nvidia_vgpu_open, "NOVA_CORE_VGPU");
EXPORT_SYMBOL_NS_GPL(nvidia_vgpu_close, "NOVA_CORE_VGPU");
EXPORT_SYMBOL_NS_GPL(nvidia_vgpu_reset, "NOVA_CORE_VGPU");

#define EXPORT_SYMBOL_RUST_GPL(sym) extern int sym; EXPORT_SYMBOL_GPL(sym)

#include "exports_nova_core_generated.h"
