// SPDX-License-Identifier: GPL-2.0

use crate::driver::{NovaDevice, NovaDriver};
use crate::gem::NovaObject;
use crate::uapi::{GemCreate, GemInfo, Getparam};
use kernel::{
    alloc::flags::*,
    drm::{self, gem::BaseObject},
    pci,
    prelude::*,
    types::Opaque,
    uapi,
};

pub(crate) struct File;

impl drm::file::DriverFile for File {
    type Driver = NovaDriver;

    fn open(dev: &NovaDevice) -> Result<Pin<KBox<Self>>> {
        dev_dbg!(dev.as_ref(), "Opening DRM device file\n");
        Ok(KBox::new(Self, GFP_KERNEL)?.into())
    }
}

impl File {
    /// IOCTL: get_param: Query GPU / driver metadata.
    pub(crate) fn get_param(
        dev: &NovaDevice,
        getparam: &Opaque<uapi::drm_nova_getparam>,
        _file: &drm::File<File>,
    ) -> Result<u32> {
        dev_dbg!(dev.as_ref(), "get_param called\n");

        let adev = &dev.adev;
        let parent = adev.parent().ok_or(ENOENT)?;
        let pdev: &pci::Device = parent.try_into()?;
        let getparam: &Getparam = getparam.into();

        let param = getparam.param() as u32;
        dev_dbg!(dev.as_ref(), "get_param param={}\n", param);

        let value = match param {
            uapi::NOVA_GETPARAM_VRAM_BAR_SIZE => pdev.resource_len(1)?,
            _ => return Err(EINVAL),
        };

        getparam.set_value(value);
        dev_dbg!(dev.as_ref(), "get_param success, value={}\n", value);

        Ok(0)
    }

    /// IOCTL: gem_create: Create a new DRM GEM object.
    pub(crate) fn gem_create(
        dev: &NovaDevice,
        req: &Opaque<uapi::drm_nova_gem_create>,
        file: &drm::File<File>,
    ) -> Result<u32> {
        dev_dbg!(dev.as_ref(), "gem_create called\n");

        let req: &GemCreate = req.into();
        let size = req.size();
        dev_dbg!(dev.as_ref(), "gem_create size={}\n", size);

        let obj = NovaObject::new(dev, req.size().try_into()?)?;
        let handle = obj.create_handle(file)?;
        req.set_handle(handle);

        dev_dbg!(dev.as_ref(), "gem_create success, handle={}\n", handle);
        Ok(0)
    }

    /// IOCTL: gem_info: Query GEM metadata.
    pub(crate) fn gem_info(
        _dev: &NovaDevice,
        req: &Opaque<uapi::drm_nova_gem_info>,
        file: &drm::File<File>,
    ) -> Result<u32> {
        let req: &GemInfo = req.into();
        let bo = NovaObject::lookup_handle(file, req.handle())?;

        req.set_size(bo.size().try_into()?);

        Ok(0)
    }
}
