/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _SAMPLES_RUST_DRIVER_SRIOV_H
#define _SAMPLES_RUST_DRIVER_SRIOV_H

#include <linux/rust_ffi.h>

#define RUST_DRIVER_SRIOV_FFI_TOKEN_HIGH	0x6c8686c04f7a4ba1ULL
#define RUST_DRIVER_SRIOV_FFI_TOKEN_LOW		0x77ab9a3c4d3cfb34ULL
#define RUST_DRIVER_SRIOV_FFI_ABI_MAJOR		1U
#define RUST_DRIVER_SRIOV_FFI_ABI_MINOR		0U

/**
 * struct rust_driver_sriov_ops - Operations published by the Rust PF sample
 * @submit: Submit one request for a requester ID and return 0 or a negative
 *          errno
 *
 * The context passed to each operation must be the context from the borrowed
 * struct rust_ffi. It remains valid until the VF driver is fully unbound,
 * including the return of its remove() callback when present.
 *
 * @submit may sleep and must not be called from atomic context.
 */
struct rust_driver_sriov_ops {
	int (*submit)(const void *context, u16 requester_id);
};

#endif /* _SAMPLES_RUST_DRIVER_SRIOV_H */
