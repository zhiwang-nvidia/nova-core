// SPDX-License-Identifier: GPL-2.0-only
#include <linux/compiler.h>
#include <linux/completion.h>
#include <linux/minmax.h>
#include <linux/mm.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/overflow.h>
#include <linux/pci.h>
#include <linux/pid.h>
#include <linux/uaccess.h>
#include <linux/unaligned.h>
#include <linux/vfio_pci_core.h>
#include <drm/nvidia_vgpu.h>

#define NVIDIA_VGPU_DRIVER_NAME "nvidia-vgpu-vfio-pci"

static bool enable_sriov;
module_param(enable_sriov, bool, 0644);
MODULE_PARM_DESC(enable_sriov, "Enable SR-IOV configuration for PFs bound to this driver");

static int nvidia_vgpu_fb_bar_index(struct pci_dev *pdev)
{
	if (pci_resource_flags(pdev, 0) & IORESOURCE_MEM_64)
		return 2;
	return 1;
}

struct nvidia_vgpu_pci_core_device {
	struct vfio_pci_core_device core_device;
	struct nvidia_vgpu_type_info type_info;
	/* Protects lifecycle calls and the active/resetting state. */
	struct mutex instance_lock;
	struct completion reset_completion;
	unsigned int gfid;
	int reset_error;
	bool instance_active;
	bool resetting;
};

/* Encode the VF's PCI segment:bus:device.function as a 32-bit address. */
static inline unsigned int nvidia_vgpu_vf_sbdf(struct pci_dev *vf)
{
	return ((u32)pci_domain_nr(vf->bus) << 16) | pci_dev_id(vf);
}

/* Keep open and close outside the complete prepare/PCI-reset/done window. */
static void nvidia_vgpu_lock_instance(struct nvidia_vgpu_pci_core_device *nvdev)
{
	for (;;) {
		mutex_lock(&nvdev->instance_lock);
		if (!nvdev->resetting)
			return;
		mutex_unlock(&nvdev->instance_lock);
		wait_for_completion(&nvdev->reset_completion);
	}
}

static int nvidia_vgpu_open_device(struct vfio_device *core_vdev)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);
	struct pci_dev *vf = to_pci_dev(core_vdev->dev);
	struct nvidia_vgpu_type_info type_info;
	int ret;

	ret = vfio_pci_core_enable(&nvdev->core_device);
	if (ret)
		return ret;

	nvidia_vgpu_lock_instance(nvdev);
	ret = nvidia_vgpu_open(vf, nvdev->gfid, nvidia_vgpu_vf_sbdf(vf),
			       task_tgid_nr(current), &type_info);
	if (!ret) {
		nvdev->instance_active = true;
		WRITE_ONCE(nvdev->reset_error, 0);
	}
	mutex_unlock(&nvdev->instance_lock);
	if (ret) {
		vfio_pci_core_disable(&nvdev->core_device);
		return ret;
	}

	nvdev->type_info = type_info;
	put_unaligned_le16(type_info.pci_dev_id,
			   nvdev->core_device.vconfig + PCI_DEVICE_ID);
	pci_dbg(vf, "vgpu open: dev_id=0x%x subsys_id=0x%x bar1_length=0x%llx\n",
		type_info.pci_dev_id, type_info.pci_subsys_id,
		type_info.bar1_length);
	vfio_pci_core_finish_enable(&nvdev->core_device);
	return 0;
}

static void nvidia_vgpu_close_device(struct vfio_device *core_vdev)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);

	nvidia_vgpu_lock_instance(nvdev);
	nvdev->instance_active = false;
	nvidia_vgpu_close(nvdev->core_device.pdev, nvdev->gfid);
	mutex_unlock(&nvdev->instance_lock);
	vfio_pci_core_close_device(core_vdev);
}

static int nvidia_vgpu_bar1_size(struct nvidia_vgpu_pci_core_device *nvdev,
				 u64 *size)
{
	struct pci_dev *pdev = nvdev->core_device.pdev;
	u64 physical_size = pci_resource_len(pdev, nvidia_vgpu_fb_bar_index(pdev));

	/* The assigned vGPU type reports its VRAM aperture in MiB. */
	if (check_shl_overflow(nvdev->type_info.bar1_length, 20, size))
		return -EOVERFLOW;

	/* An unspecified or larger vGPU type aperture uses the physical limit. */
	if (!*size || *size > physical_size)
		*size = physical_size;

	return 0;
}

/* Enforce the same vGPU type aperture reported by GET_REGION_INFO. */
static int nvidia_vgpu_bar1_access(struct vfio_device *core_vdev,
				   loff_t pos, size_t *count)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);
	unsigned int index = VFIO_PCI_OFFSET_TO_INDEX(pos);
	u64 size;
	int ret;

	if (!*count || index != nvidia_vgpu_fb_bar_index(nvdev->core_device.pdev))
		return 0;

	ret = nvidia_vgpu_bar1_size(nvdev, &size);
	if (ret)
		return ret;

	pos &= VFIO_PCI_OFFSET_MASK;
	if (pos >= size)
		return -EINVAL;

	*count = min_t(u64, *count, size - pos);
	return 0;
}

static ssize_t nvidia_vgpu_read_config(struct vfio_device *core_vdev,
				      char __user *buf, size_t count,
				      loff_t *ppos)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);
	loff_t pos = *ppos & VFIO_PCI_OFFSET_MASK;
	loff_t next = *ppos, copy_offset;
	size_t copy_count, register_offset;
	__le16 value;
	ssize_t ret;

	ret = vfio_pci_core_read(core_vdev, buf, count, &next);
	if (ret <= 0)
		return ret;

	/* The core reads the subsystem ID from physical configuration space. */
	if (vfio_pci_core_range_intersect_range(pos, ret, PCI_SUBSYSTEM_ID,
					      sizeof(value), &copy_offset,
					      &copy_count, &register_offset)) {
		value = cpu_to_le16(nvdev->type_info.pci_subsys_id);
		if (copy_to_user(buf + copy_offset,
				(u8 *)&value + register_offset, copy_count))
			return -EFAULT;
	}

	*ppos = next;
	return ret;
}

static ssize_t nvidia_vgpu_pci_read(struct vfio_device *core_vdev,
				    char __user *buf, size_t count,
				    loff_t *ppos)
{
	int ret;

	if (VFIO_PCI_OFFSET_TO_INDEX(*ppos) == VFIO_PCI_CONFIG_REGION_INDEX)
		return nvidia_vgpu_read_config(core_vdev, buf, count, ppos);

	ret = nvidia_vgpu_bar1_access(core_vdev, *ppos, &count);
	if (ret)
		return ret;

	return vfio_pci_core_read(core_vdev, buf, count, ppos);
}

static ssize_t nvidia_vgpu_pci_write(struct vfio_device *core_vdev,
				     const char __user *buf, size_t count,
				     loff_t *ppos)
{
	int ret;

	ret = nvidia_vgpu_bar1_access(core_vdev, *ppos, &count);
	if (ret)
		return ret;

	/* Reset callbacks report a firmware FLR failure through the error IRQ. */
	return vfio_pci_core_write(core_vdev, buf, count, ppos);
}

static int nvidia_vgpu_pci_mmap(struct vfio_device *core_vdev,
				struct vm_area_struct *vma)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);
	unsigned int index = vma->vm_pgoff >> (VFIO_PCI_OFFSET_SHIFT - PAGE_SHIFT);
	u64 size, req_start, req_len, end;
	int ret;

	if (index != nvidia_vgpu_fb_bar_index(nvdev->core_device.pdev))
		return vfio_pci_core_mmap(core_vdev, vma);

	ret = nvidia_vgpu_bar1_size(nvdev, &size);
	if (ret)
		return ret;

	req_start = (vma->vm_pgoff & (VFIO_PCI_OFFSET_MASK >> PAGE_SHIFT)) << PAGE_SHIFT;
	if (check_sub_overflow(vma->vm_end, vma->vm_start, &req_len) ||
	    check_add_overflow(req_start, req_len, &end))
		return -EOVERFLOW;
	if (end > size)
		return -EINVAL;

	return vfio_pci_core_mmap(core_vdev, vma);
}

static int nvidia_vgpu_get_region_info(struct vfio_device *core_vdev,
				       struct vfio_region_info *info,
				       struct vfio_info_cap *caps)
{
	struct pci_dev *pdev = to_pci_dev(core_vdev->dev);
	int ret;

	ret = vfio_pci_ioctl_get_region_info(core_vdev, info, caps);
	if (ret)
		return ret;

	if (info->index == nvidia_vgpu_fb_bar_index(pdev) && info->size) {
		struct nvidia_vgpu_pci_core_device *nvdev =
			container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
				     core_device.vdev);
		u64 vgpu_bar1;

		ret = nvidia_vgpu_bar1_size(nvdev, &vgpu_bar1);
		if (ret)
			return ret;

		if (vgpu_bar1 && vgpu_bar1 < info->size)
			info->size = vgpu_bar1;
	}

	return 0;
}

static long nvidia_vgpu_pci_ioctl(struct vfio_device *core_vdev,
				  unsigned int cmd, unsigned long arg)
{
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_vdev, struct nvidia_vgpu_pci_core_device,
			     core_device.vdev);
	bool reset = cmd == VFIO_DEVICE_RESET || cmd == VFIO_DEVICE_PCI_HOT_RESET;
	long ret;

	/* A failed firmware reset requires closing and reopening the instance. */
	if (reset && READ_ONCE(nvdev->reset_error))
		return READ_ONCE(nvdev->reset_error);

	ret = vfio_pci_core_ioctl(core_vdev, cmd, arg);
	if (ret)
		return ret;

	if (reset)
		return READ_ONCE(nvdev->reset_error);

	return 0;
}

static void nvidia_vgpu_reset_prepare(struct pci_dev *pdev)
{
	struct vfio_pci_core_device *core_device = dev_get_drvdata(&pdev->dev);
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_device, struct nvidia_vgpu_pci_core_device,
			     core_device);
	int ret = 0;

	/*
	 * PCI holds the VF device lock, and VFIO may hold its memory lock.
	 * Open and close release instance_lock before entering vfio-pci-core;
	 * PF operations take only GPU locks, never the PF or VF device lock.
	 */
	mutex_lock(&nvdev->instance_lock);
	nvdev->resetting = true;
	reinit_completion(&nvdev->reset_completion);
	if (nvdev->instance_active)
		ret = nvidia_vgpu_reset(pdev, nvdev->gfid);
	if (ret && !nvdev->reset_error)
		WRITE_ONCE(nvdev->reset_error, ret);
	mutex_unlock(&nvdev->instance_lock);
}

static void nvidia_vgpu_reset_done(struct pci_dev *pdev)
{
	struct vfio_pci_core_device *core_device = dev_get_drvdata(&pdev->dev);
	struct nvidia_vgpu_pci_core_device *nvdev =
		container_of(core_device, struct nvidia_vgpu_pci_core_device,
			     core_device);
	int ret;

	mutex_lock(&nvdev->instance_lock);
	ret = nvdev->reset_error;
	/* PCI reset callbacks cannot return an error or abort the PCI reset. */
	if (ret) {
		pci_err(pdev, "vGPU instance has a failed firmware reset: %d\n", ret);
		vfio_pci_core_aer_err_detected(pdev, pci_channel_io_normal);
	}
	nvdev->resetting = false;
	complete_all(&nvdev->reset_completion);
	mutex_unlock(&nvdev->instance_lock);
}

static const struct pci_error_handlers nvidia_vgpu_err_handlers = {
	.error_detected = vfio_pci_core_aer_err_detected,
	.reset_prepare = nvidia_vgpu_reset_prepare,
	.reset_done = nvidia_vgpu_reset_done,
};

static const struct vfio_device_ops nvidia_vgpu_pci_ops = {
	.name		= NVIDIA_VGPU_DRIVER_NAME,
	.init		= vfio_pci_core_init_dev,
	.release	= vfio_pci_core_release_dev,
	.open_device	= nvidia_vgpu_open_device,
	.close_device	= nvidia_vgpu_close_device,
	.ioctl		= nvidia_vgpu_pci_ioctl,
	.get_region_info_caps = nvidia_vgpu_get_region_info,
	.device_feature	= vfio_pci_core_ioctl_feature,
	.read		= nvidia_vgpu_pci_read,
	.write		= nvidia_vgpu_pci_write,
	.mmap		= nvidia_vgpu_pci_mmap,
	.request	= vfio_pci_core_request,
	.match		= vfio_pci_core_match,
	.match_token_uuid = vfio_pci_core_match_token_uuid,
	.bind_iommufd	= vfio_iommufd_physical_bind,
	.unbind_iommufd	= vfio_iommufd_physical_unbind,
	.attach_ioas	= vfio_iommufd_physical_attach_ioas,
	.detach_ioas	= vfio_iommufd_physical_detach_ioas,
	.pasid_attach_ioas = vfio_iommufd_physical_pasid_attach_ioas,
	.pasid_detach_ioas = vfio_iommufd_physical_pasid_detach_ioas,
};

static int nvidia_vgpu_passthrough_open(struct vfio_device *core_vdev)
{
	struct vfio_pci_core_device *vdev =
		container_of(core_vdev, struct vfio_pci_core_device, vdev);
	int ret;

	ret = vfio_pci_core_enable(vdev);
	if (ret)
		return ret;

	vfio_pci_core_finish_enable(vdev);
	return 0;
}

/* PFs and VFs without a compatible Nova interface retain generic VFIO behavior. */
static const struct vfio_device_ops nvidia_vgpu_passthrough_ops = {
	.name		= NVIDIA_VGPU_DRIVER_NAME,
	.init		= vfio_pci_core_init_dev,
	.release	= vfio_pci_core_release_dev,
	.open_device	= nvidia_vgpu_passthrough_open,
	.close_device	= vfio_pci_core_close_device,
	.ioctl		= vfio_pci_core_ioctl,
	.get_region_info_caps = vfio_pci_ioctl_get_region_info,
	.device_feature	= vfio_pci_core_ioctl_feature,
	.read		= vfio_pci_core_read,
	.write		= vfio_pci_core_write,
	.mmap		= vfio_pci_core_mmap,
	.request	= vfio_pci_core_request,
	.match		= vfio_pci_core_match,
	.match_token_uuid = vfio_pci_core_match_token_uuid,
	.bind_iommufd	= vfio_iommufd_physical_bind,
	.unbind_iommufd	= vfio_iommufd_physical_unbind,
	.attach_ioas	= vfio_iommufd_physical_attach_ioas,
	.detach_ioas	= vfio_iommufd_physical_detach_ioas,
	.pasid_attach_ioas = vfio_iommufd_physical_pasid_attach_ioas,
	.pasid_detach_ioas = vfio_iommufd_physical_pasid_detach_ioas,
};

static const struct vfio_pci_device_ops nvidia_vgpu_passthrough_dev_ops = {
	.get_dmabuf_phys = vfio_pci_core_get_dmabuf_phys,
};

static int nvidia_vgpu_pci_probe(struct pci_dev *pdev,
				 const struct pci_device_id *id)
{
	const struct vfio_device_ops *device_ops = &nvidia_vgpu_passthrough_ops;
	struct nvidia_vgpu_pci_core_device *nvdev;
	bool has_vgpu = nvidia_vgpu_is_available(pdev);
	int vf_id = -1;
	int ret;

	if (has_vgpu) {
		device_ops = &nvidia_vgpu_pci_ops;
		vf_id = pci_iov_vf_id(pdev);
		if (vf_id < 0)
			return vf_id;
	}

	nvdev = vfio_alloc_device(nvidia_vgpu_pci_core_device, core_device.vdev,
				  &pdev->dev, device_ops);
	if (IS_ERR(nvdev))
		return PTR_ERR(nvdev);

	mutex_init(&nvdev->instance_lock);
	init_completion(&nvdev->reset_completion);
	nvdev->gfid = vf_id + 1;
	if (!has_vgpu)
		nvdev->core_device.pci_ops = &nvidia_vgpu_passthrough_dev_ops;
	dev_set_drvdata(&pdev->dev, &nvdev->core_device);
	ret = vfio_pci_core_register_device(&nvdev->core_device);
	if (ret)
		goto out_put_vdev;

	return 0;

out_put_vdev:
	vfio_put_device(&nvdev->core_device.vdev);
	return ret;
}

static void nvidia_vgpu_pci_remove(struct pci_dev *pdev)
{
	struct vfio_pci_core_device *core_device = dev_get_drvdata(&pdev->dev);

	/* Drain all VFIO callbacks before the PF registration can be withdrawn. */
	vfio_pci_core_unregister_device(core_device);
	vfio_put_device(&core_device->vdev);
}

static const struct pci_device_id nvidia_vgpu_pci_table[] = {
	/* RTX PRO 6000 Blackwell Server Edition PFs and VFs share this ID. */
	{ PCI_DRIVER_OVERRIDE_DEVICE_VFIO(PCI_VENDOR_ID_NVIDIA, 0x2bb5),
	  .class = PCI_CLASS_DISPLAY_3D << 8, .class_mask = 0xffff00 },
	{}
};
MODULE_DEVICE_TABLE(pci, nvidia_vgpu_pci_table);

static int nvidia_vgpu_sriov_configure(struct pci_dev *pdev, int nr_virtfn)
{
	struct vfio_pci_core_device *vdev = dev_get_drvdata(&pdev->dev);

	if (!enable_sriov)
		return -ENOENT;

	return vfio_pci_core_sriov_configure(vdev, nr_virtfn);
}

static struct pci_driver nvidia_vgpu_pci_driver = {
	.name		= NVIDIA_VGPU_DRIVER_NAME,
	.id_table	= nvidia_vgpu_pci_table,
	.probe		= nvidia_vgpu_pci_probe,
	.remove		= nvidia_vgpu_pci_remove,
	.sriov_configure = nvidia_vgpu_sriov_configure,
	.err_handler	= &nvidia_vgpu_err_handlers,
	.driver_managed_dma = true,
};
module_pci_driver(nvidia_vgpu_pci_driver);

MODULE_DESCRIPTION("NVIDIA vGPU vfio-pci driver");
MODULE_LICENSE("GPL");
