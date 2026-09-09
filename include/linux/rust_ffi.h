/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _LINUX_RUST_FFI_H
#define _LINUX_RUST_FFI_H

#include <linux/err.h>
#include <linux/types.h>

/**
 * struct rust_ffi_token - Stable identifier for a Rust FFI ABI
 * @high: Most significant half of the identifier
 * @low: Least significant half of the identifier
 *
 * A token is the ABI type tag for the opaque operations table. It tells a
 * consumer which C type and semantics may be used to access @ops. It is not a
 * device identifier, secret, permission check, or lifetime handle. Providers
 * and consumers must use the same pair of constants.
 */
struct rust_ffi_token {
	u64 high;
	u64 low;
};

/**
 * struct rust_ffi - C ABI descriptor for calls into Rust
 * @token: Stable identifier for the FFI ABI
 * @abi_major: ABI major version
 * @abi_minor: ABI minor version
 * @ops_size: Size of the operations table in bytes
 * @ops: C ABI operations table
 * @context: Immutable provider context passed to operations
 *
 * Providers must fully initialize this descriptor before publishing it and
 * must keep the descriptor, operations table, and context alive and immutable
 * while it is published. A published callable descriptor has non-NULL @ops
 * and @context pointers. A NULL @ops indicates that no C-callable FFI is
 * available.
 *
 * Minor versions may only append operations to the table. Consumers request
 * an ABI major version, a minimum ABI minor version, and the size of the table
 * prefix they use.
 */
struct rust_ffi {
	struct rust_ffi_token token;
	u16 abi_major;
	u16 abi_minor;
	size_t ops_size;
	const void *ops;
	const void *context;
};

/**
 * rust_ffi_borrow - Validate and borrow a Rust FFI descriptor
 * @ffi: Descriptor to borrow
 * @token: Required FFI ABI token
 * @abi_major: Required ABI major version
 * @min_abi_minor: Minimum required ABI minor version
 * @required_ops_size: Minimum required size of the operations table
 *
 * This validates only the descriptor contents. The caller must arrange for
 * @ffi, its operations table, and its context to remain alive and immutable
 * for the entire borrow.
 *
 * Return: @ffi on success, or an ERR_PTR() value on failure.
 */
static inline const struct rust_ffi *
rust_ffi_borrow(const struct rust_ffi *ffi,
		const struct rust_ffi_token *token,
		u16 abi_major, u16 min_abi_minor, size_t required_ops_size)
{
	if (!token)
		return ERR_PTR(-EINVAL);

	if (!ffi || !ffi->ops || !ffi->context)
		return ERR_PTR(-ENOENT);

	if (ffi->token.high != token->high || ffi->token.low != token->low)
		return ERR_PTR(-ENOENT);

	if (ffi->abi_major != abi_major || ffi->abi_minor < min_abi_minor)
		return ERR_PTR(-EPROTONOSUPPORT);

	if (ffi->ops_size < required_ops_size)
		return ERR_PTR(-EMSGSIZE);

	return ffi;
}

#endif /* _LINUX_RUST_FFI_H */
