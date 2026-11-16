/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Copyright (c) 2024-2025, NVIDIA CORPORATION & AFFILIATES
 */
#ifndef __LINUX_FWCTL_H
#define __LINUX_FWCTL_H
#include <linux/device.h>
#include <linux/cdev.h>
#include <linux/cleanup.h>
#include <uapi/fwctl/fwctl.h>
#include <linux/scatterlist.h>

struct fwctl_device;
struct fwctl_uctx;

/**
 * struct fwctl_umem_reg - Passed to driver for user memory registration
 * @sgt: Scatter gather table of the pinned and dma mapped user memory.
 * @len: Length of the user memory.
 * @offset: Offset into the first page of the user memory.
 * @flags: One of enum fwctl_umem_flags.
 *
 * This struct is passed to the driver's umem_reg() operation to register
 * user memory for use with device operations. The driver fills in umem_id
 * and mkey_id if applicable.
 */
struct fwctl_umem_reg {
	struct sg_table *sgt; /* dma information */
	size_t len; /* length of the user memory */
	size_t offset; /* offset into the first page */
	u64 flags; /* enum fwctl_umem_flags */

	/* filled by the driver */
	u32 umem_id;
	u32 mkey_id;
};

/**
 * struct fwctl_ops - Driver provided operations
 *
 * fwctl_unregister() will wait until all excuting ops are completed before it
 * returns. Drivers should be mindful to not let their ops run for too long as
 * it will block device hot unplug and module unloading.
 */
struct fwctl_ops {
	/**
	 * @device_type: The drivers assigned device_type number. This is uABI.
	 */
	enum fwctl_device_type device_type;
	/**
	 * @uctx_size: The size of the fwctl_uctx struct to allocate. The first
	 * bytes of this memory will be a fwctl_uctx. The driver can use the
	 * remaining bytes as its private memory.
	 */
	size_t uctx_size;
	/**
	 * @open_uctx: Called when a file descriptor is opened before the uctx
	 * is ever used.
	 */
	int (*open_uctx)(struct fwctl_uctx *uctx);
	/**
	 * @close_uctx: Called when the uctx is destroyed, usually when the FD
	 * is closed.
	 */
	void (*close_uctx)(struct fwctl_uctx *uctx);
	/**
	 * @info: Implement FWCTL_INFO. Return a kmalloc() memory that is copied
	 * to out_device_data. On input length indicates the size of the user
	 * buffer on output it indicates the size of the memory. The driver can
	 * ignore length on input, the core code will handle everything.
	 */
	void *(*info)(struct fwctl_uctx *uctx, size_t *length);
	/**
	 * @fw_rpc: Implement FWCTL_RPC. Deliver rpc_in/in_len to the FW and
	 * return the response and set out_len. rpc_in can be returned as the
	 * response pointer. Otherwise the returned pointer is freed with
	 * kvfree().
	 */
	void *(*fw_rpc)(struct fwctl_uctx *uctx, enum fwctl_rpc_scope scope,
			void *rpc_in, size_t in_len, size_t *out_len);

	/**
	 * @umem_reg: Implement FWCTL_RESOURCE_UMEM_REG. Register a user memory
	 * range for use with device operations. The memory must be page
	 * aligned, and the length must be a multiple of the page size. The
	 * memory is pinned until unregistered with FWCTL_RESOURCE_DESTROY or
	 * the file descriptor is closed.
	 */
	void *(*umem_reg)(struct fwctl_uctx *uctx, struct fwctl_umem_reg *umem);

	/**
	 * @umem_unreg: Implement FWCTL_RESOURCE_DESTROY for
	 * FWCTL_RESOURCE_UMEM_REG. Unregister a user memory range previously
	 * registered with FWCTL_RESOURCE_UMEM_REG.
	 */
	int (*umem_unreg)(struct fwctl_uctx *uctx, void *drv_data);
};

/**
 * struct fwctl_device - Per-driver registration struct
 * @dev: The sysfs (class/fwctl/fwctlXX) device
 *
 * Each driver instance will have one of these structs with the driver private
 * data following immediately after. This struct is refcounted, it is freed by
 * calling fwctl_put().
 */
struct fwctl_device {
	struct device dev;
	/* private: */
	struct cdev cdev;

	/* Protect uctx_list */
	struct mutex uctx_list_lock;
	struct list_head uctx_list;
	/*
	 * Protect ops, held for write when ops becomes NULL during unregister,
	 * held for read whenever ops is loaded or an ops function is running.
	 */
	struct rw_semaphore registration_lock;
	const struct fwctl_ops *ops;
};

struct fwctl_device *_fwctl_alloc_device(struct device *parent,
					 const struct fwctl_ops *ops,
					 size_t size);
/**
 * fwctl_alloc_device - Allocate a fwctl
 * @parent: Physical device that provides the FW interface
 * @ops: Driver ops to register
 * @drv_struct: 'struct driver_fwctl' that holds the struct fwctl_device
 * @member: Name of the struct fwctl_device in @drv_struct
 *
 * This allocates and initializes the fwctl_device embedded in the drv_struct.
 * Upon success the pointer must be freed via fwctl_put(). Returns a 'drv_struct
 * \*' on success, NULL on error.
 */
#define fwctl_alloc_device(parent, ops, drv_struct, member)               \
	({                                                                \
		static_assert(__same_type(struct fwctl_device,            \
					  ((drv_struct *)NULL)->member)); \
		static_assert(offsetof(drv_struct, member) == 0);         \
		(drv_struct *)_fwctl_alloc_device(parent, ops,            \
						  sizeof(drv_struct));    \
	})

static inline struct fwctl_device *fwctl_get(struct fwctl_device *fwctl)
{
	get_device(&fwctl->dev);
	return fwctl;
}
static inline void fwctl_put(struct fwctl_device *fwctl)
{
	put_device(&fwctl->dev);
}
DEFINE_FREE(fwctl, struct fwctl_device *, if (_T) fwctl_put(_T));

int fwctl_register(struct fwctl_device *fwctl);
void fwctl_unregister(struct fwctl_device *fwctl);

/**
 * struct fwctl_uctx - Per user FD context
 * @fwctl: fwctl instance that owns the context
 *
 * Every FD opened by userspace will get a unique context allocation. Any driver
 * private data will follow immediately after.
 */
struct fwctl_uctx {
	struct fwctl_device *fwctl;
	/* private: */
	struct xarray umems;
	struct ida umem_ida;
	/* Head at fwctl_device::uctx_list */
	struct list_head uctx_list_entry;
};

#endif
