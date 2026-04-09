/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Copyright © 2025 NVIDIA Corporation
 *
 * PF-side vGPU lifecycle interface for the VFIO variant driver.
 *
 * The PF driver (nova-core) implements these ops and exports a
 * getter function.  The VFIO variant driver obtains the ops table
 * via the PF's driver data and calls open/close/reset to manage
 * vGPU instance lifecycles without touching PF-internal resources.
 */
#ifndef __DRM_NOVA_VGPU_VFIO_H__
#define __DRM_NOVA_VGPU_VFIO_H__

#include <linux/types.h>

struct pci_dev;

/**
 * struct nova_vgpu_vfio_ops - PF-exported vGPU lifecycle operations
 * @open:  Create a vGPU instance on the given VF.
 *         Allocates channels, framebuffer, management heap, bootloads
 *         the GSP plugin, and sets up the host RPC channel.
 *         @pf_drvdata: PF driver data (opaque to VFIO side)
 *         @vf_id:      VF index (0-based)
 *         @vgpu_type_id: vGPU type identifier
 *         @vm_pid:     VM process ID
 *         Returns 0 on success, negative errno on failure.
 *
 * @close: Destroy the vGPU instance on the given VF.
 *         Shuts down the GSP plugin, releases all resources.
 *         @pf_drvdata: PF driver data
 *         @vf_id:      VF index (0-based)
 *         Returns 0 on success, negative errno on failure.
 *
 * @reset: Hot-reset the vGPU instance on the given VF.
 *         Sends a reset RPC to the GSP plugin without tearing
 *         down allocated resources (channels, FB memory, etc.).
 *         @pf_drvdata: PF driver data
 *         @vf_id:      VF index (0-based)
 *         Returns 0 on success, negative errno on failure.
 */
struct nova_vgpu_vfio_ops {
	int (*open)(void *pf_drvdata, int vf_id, u32 vgpu_type_id, u32 vm_pid);
	int (*close)(void *pf_drvdata, int vf_id);
	int (*reset)(void *pf_drvdata, int vf_id);
};

struct nova_vgpu_vfio_ops *nova_vgpu_get_vfio_ops(void *pf_drvdata);

#endif /* __DRM_NOVA_VGPU_VFIO_H__ */
