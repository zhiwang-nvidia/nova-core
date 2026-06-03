// SPDX-License-Identifier: GPL-2.0
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

/*
 * Exports Rust symbols from the `nova_core` crate for use by dependent modules.
 *
 * This is a workaround until the build system supports Rust cross-module
 * dependencies natively.
 */

#include <linux/export.h>

#define EXPORT_SYMBOL_RUST_GPL(sym) extern int sym; EXPORT_SYMBOL_GPL(sym)

#include "exports_nova_core_generated.h"

/* Stable symbol names for the Rust VF API; these functions use the Rust ABI. */
#ifdef CONFIG_PCI_IOV
EXPORT_SYMBOL_RUST_GPL(nova_core_vf_api_is_available);
EXPORT_SYMBOL_RUST_GPL(nova_core_vf_api_open_instance);
EXPORT_SYMBOL_RUST_GPL(nova_core_vf_api_close_instance);
EXPORT_SYMBOL_RUST_GPL(nova_core_vf_api_reset_instance);
#endif
