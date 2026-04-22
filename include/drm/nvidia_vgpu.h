/* SPDX-License-Identifier: GPL-2.0 */
// SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
#ifndef __DRM_NVIDIA_VGPU_H__
#define __DRM_NVIDIA_VGPU_H__

#include <linux/types.h>

struct pci_dev;

/**
 * struct nvidia_vgpu_type_info - vGPU type descriptor returned by open
 * @pci_dev_id: PCI device ID to present to the guest
 * @pci_subsys_id: PCI subsystem ID to present to the guest
 * @bar1_length: BAR1 aperture size in MiB
 */
struct nvidia_vgpu_type_info {
	u32 pci_dev_id;
	u32 pci_subsys_id;
	u64 bar1_length;
};

/*
 * These driver-local entry points borrow the PF's Rust API for each call.
 * The VF driver must remain bound throughout the call, including probe and
 * remove, and drain all calls before failed probe or remove returns.
 * All calls may sleep and must not acquire the PF device lock.
 *
 * gfid is the one-based VF index. sbdf encodes the VF's PCI address as
 * (segment << 16) | (bus << 8) | devfn, and vm_pid is its VM's thread-group ID.
 * type_info must point to writable storage with no concurrent access.
 * Open and reset return zero on success or a negative errno.
 */
bool nvidia_vgpu_is_available(struct pci_dev *vf);
int nvidia_vgpu_open(struct pci_dev *vf, unsigned int gfid, unsigned int sbdf,
		     unsigned int vm_pid, struct nvidia_vgpu_type_info *type_info);
void nvidia_vgpu_close(struct pci_dev *vf, unsigned int gfid);
int nvidia_vgpu_reset(struct pci_dev *vf, unsigned int gfid);

#endif /* __DRM_NVIDIA_VGPU_H__ */
