/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * Copyright (c) 2025, NVIDIA CORPORATION & AFFILIATES
 *
 * nova-core fwctl device specific definitions.
 *
 * The device_type for this file is FWCTL_DEVICE_TYPE_NOVA_CORE.
 */
#ifndef _UAPI_FWCTL_NOVA_CORE_H
#define _UAPI_FWCTL_NOVA_CORE_H

#include <linux/types.h>

/**
 * enum fwctl_cmd_nova_core - Firmware command identifiers
 * @FWCTL_CMD_NOVA_CORE_UPLOAD_VGPU_TYPE: Upload vGPU type definitions to GSP.
 *     Payload is NV2080_CTRL_VGPU_MGR_INTERNAL_PGPU_ADD_VGPU_TYPE_PARAMS.
 */
enum fwctl_cmd_nova_core {
	FWCTL_CMD_NOVA_CORE_UPLOAD_VGPU_TYPE = 0,
};

/**
 * struct fwctl_rpc_nova_core_request_hdr - ioctl(FWCTL_RPC) input header
 * @cmd: Command identifier from &enum fwctl_cmd_nova_core.
 * @mctp_header: MCTP transport header (packed u32).
 * @nvdm_header: NVDM vendor-defined message header (packed u32).
 *
 * Placed at &struct fwctl_rpc.in with total length &struct fwctl_rpc.in_len.
 * The access scope is specified through &struct fwctl_rpc.scope.
 * Followed by command-specific input parameters.
 */
struct fwctl_rpc_nova_core_request_hdr {
	__u32 mctp_header;
	__u32 nvdm_header;
	__u32 cmd;
};

/**
 * struct fwctl_rpc_nova_core_resp_hdr - ioctl(FWCTL_RPC) output header
 * @mctp_header: MCTP transport header (packed u32).
 * @nvdm_header: NVDM vendor-defined message header (packed u32).
 *
 * Placed at &struct fwctl_rpc.out with total length &struct fwctl_rpc.out_len.
 * Followed by command-specific output parameters.
 */
struct fwctl_rpc_nova_core_resp_hdr {
	__u32 mctp_header;
	__u32 nvdm_header;
};

#endif
