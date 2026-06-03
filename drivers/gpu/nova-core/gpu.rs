// SPDX-License-Identifier: GPL-2.0

use core::{
    num::NonZero,
    ops::Range, //
};

use kernel::{
    device,
    dma::Device,
    fmt,
    gpu::buddy::GpuBuddyParams,
    io::Io,
    num::Bounded,
    pci,
    prelude::*,
    ptr::Alignment,
    sizes::{
        SizeConstants,
        SZ_4K, //
    },
    sync::Arc, //
};

use crate::{
    bounded_enum,
    driver::Bar0,
    falcon::{
        gsp::Gsp as GspFalcon,
        sec2::Sec2 as Sec2Falcon,
        Falcon, //
    },
    fb::SysmemFlush,
    firmware,
    fsp::Fsp,
    gsp::{
        self,
        cmdq::Cmdq,
        Gsp,
        GspBootContext, //
    },
    mm::{
        bar_user::BarUser,
        pagetable::MmuVersion,
        GpuMm,
        IntoVramRange,
        VramAddress, //
    },
    regs,
    vgpu::VgpuManager, //
};

mod channel;
mod hal;

pub(crate) use self::channel::{
    ChannelIdArea,
    ChannelIdPool,
    TOTAL_CHANNELS, //
};

macro_rules! define_chipset {
    ({ $($variant:ident = $value:expr),* $(,)* }) =>
    {
        /// Enum representation of the GPU chipset.
        #[derive(fmt::Debug, Copy, Clone, PartialOrd, Ord, PartialEq, Eq)]
        pub(crate) enum Chipset {
            $($variant = $value),*,
        }

        impl Chipset {
            pub(crate) const ALL: &'static [Chipset] = &[
                $( Chipset::$variant, )*
            ];

            ::kernel::macros::paste!(
            /// Returns the name of this chipset, in lowercase.
            ///
            /// # Examples
            ///
            /// ```
            /// let chipset = Chipset::GA102;
            /// assert_eq!(chipset.name(), "ga102");
            /// ```
            pub(crate) const fn name(&self) -> &'static str {
                match *self {
                $(
                    Chipset::$variant => stringify!([<$variant:lower>]),
                )*
                }
            }
            );
        }

        // TODO[FPRI]: replace with something like derive(FromPrimitive)
        impl TryFrom<u32> for Chipset {
            type Error = kernel::error::Error;

            fn try_from(value: u32) -> Result<Self, Self::Error> {
                match value {
                    $( $value => Ok(Chipset::$variant), )*
                    _ => Err(ENODEV),
                }
            }
        }
    }
}

define_chipset!({
    // Turing
    TU102 = 0x162,
    TU104 = 0x164,
    TU106 = 0x166,
    TU117 = 0x167,
    TU116 = 0x168,
    // Ampere
    GA100 = 0x170,
    GA102 = 0x172,
    GA103 = 0x173,
    GA104 = 0x174,
    GA106 = 0x176,
    GA107 = 0x177,
    // Hopper
    GH100 = 0x180,
    // Ada
    AD102 = 0x192,
    AD103 = 0x193,
    AD104 = 0x194,
    AD106 = 0x196,
    AD107 = 0x197,
    // Blackwell GB10x
    GB100 = 0x1a0,
    GB102 = 0x1a2,
    // Blackwell GB20x
    GB202 = 0x1b2,
    GB203 = 0x1b3,
    GB205 = 0x1b5,
    GB206 = 0x1b6,
    GB207 = 0x1b7,
});

impl Chipset {
    pub(crate) const fn arch(self) -> Architecture {
        match self {
            Self::TU102 | Self::TU104 | Self::TU106 | Self::TU117 | Self::TU116 => {
                Architecture::Turing
            }
            Self::GA100 | Self::GA102 | Self::GA103 | Self::GA104 | Self::GA106 | Self::GA107 => {
                Architecture::Ampere
            }
            Self::GH100 => Architecture::Hopper,
            Self::AD102 | Self::AD103 | Self::AD104 | Self::AD106 | Self::AD107 => {
                Architecture::Ada
            }
            Self::GB100 | Self::GB102 => Architecture::BlackwellGB10x,
            Self::GB202 | Self::GB203 | Self::GB205 | Self::GB206 | Self::GB207 => {
                Architecture::BlackwellGB20x
            }
        }
    }

    /// Returns the address range of the PCI config mirror space.
    pub(crate) fn pci_config_mirror_range(self) -> Range<u32> {
        hal::gpu_hal(self).pci_config_mirror_range()
    }

    /// Returns the MMU version for this chipset.
    pub(crate) fn mmu_version(self) -> MmuVersion {
        MmuVersion::from(self.arch())
    }
}

// TODO
//
// The resulting strings are used to generate firmware paths, hence the
// generated strings have to be stable.
//
// Hence, replace with something like strum_macros derive(Display).
//
// For now, redirect to fmt::Debug for convenience.
impl fmt::Display for Chipset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

bounded_enum! {
    /// Enum representation of the GPU generation.
    #[derive(fmt::Debug, Copy, Clone)]
    pub(crate) enum Architecture with TryFrom<Bounded<u32, 6>> {
        Turing = 0x16,
        Ampere = 0x17,
        Hopper = 0x18,
        Ada = 0x19,
        BlackwellGB10x = 0x1a,
        BlackwellGB20x = 0x1b,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Revision {
    major: Bounded<u8, 4>,
    minor: Bounded<u8, 4>,
}

impl From<regs::NV_PMC_BOOT_42> for Revision {
    fn from(boot0: regs::NV_PMC_BOOT_42) -> Self {
        Self {
            major: boot0.major_revision().cast(),
            minor: boot0.minor_revision().cast(),
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:x}.{:x}", self.major, self.minor)
    }
}

/// Structure holding a basic description of the GPU: `Chipset` and `Revision`.
#[derive(Clone, Copy)]
pub(crate) struct Spec {
    chipset: Chipset,
    revision: Revision,
}

impl Spec {
    fn new(dev: &device::Device, bar: Bar0<'_>) -> Result<Spec> {
        // Some brief notes about boot0 and boot42, in chronological order:
        //
        // NV04 through NV50:
        //
        //    Not supported by Nova. boot0 is necessary and sufficient to identify these GPUs.
        //    boot42 may not even exist on some of these GPUs.
        //
        // Fermi through Volta:
        //
        //     Not supported by Nova. boot0 is still sufficient to identify these GPUs, but boot42
        //     is also guaranteed to be both present and accurate.
        //
        // Turing and later:
        //
        //     Supported by Nova. Identified by first checking boot0 to ensure that the GPU is not
        //     from an earlier (pre-Fermi) era, and then using boot42 to precisely identify the GPU.
        //     Somewhere in the Rubin timeframe, boot0 will no longer have space to add new GPU IDs.

        let boot0 = bar.read(regs::NV_PMC_BOOT_0);

        if boot0.is_older_than_fermi() {
            return Err(ENODEV);
        }

        let boot42 = bar.read(regs::NV_PMC_BOOT_42);
        Spec::try_from(boot42).inspect_err(|_| {
            dev_err!(dev, "Unsupported chipset: {}\n", boot42);
        })
    }
}

impl TryFrom<regs::NV_PMC_BOOT_42> for Spec {
    type Error = Error;

    fn try_from(boot42: regs::NV_PMC_BOOT_42) -> Result<Self> {
        Ok(Self {
            chipset: boot42.chipset()?,
            revision: boot42.into(),
        })
    }
}

impl fmt::Display for Spec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_fmt(fmt!(
            "Chipset: {}, Architecture: {:?}, Revision: {}",
            self.chipset,
            self.chipset.arch(),
            self.revision
        ))
    }
}

/// Self-contained resources to operate and drop the GSP.
#[pin_data(PinnedDrop)]
struct GspResources<'gpu> {
    /// Device owning the GPU.
    device: &'gpu pci::Device<device::Bound>,
    /// Details about the chipset.
    spec: Spec,
    /// MMIO mapping of PCI BAR 0.
    bar: Bar0<'gpu>,
    /// GSP falcon instance, used for GSP boot up and cleanup.
    gsp_falcon: Falcon<'gpu, GspFalcon>,
    /// SEC2 falcon instance, used for GSP boot up and cleanup.
    sec2_falcon: Falcon<'gpu, Sec2Falcon>,
    /// FSP instance, if on an arch that supports it.
    // TODO: use different resource types for each boot method, and make the relevant Gsp methods
    // generic against them.
    fsp: Option<Fsp<'gpu>>,
    /// GSP runtime data.
    #[pin]
    gsp: Gsp,
    /// Resources returned by the GSP boot sequence.
    boot_result: gsp::BootResult,
}

/// Structure holding the resources required to operate the GPU.
#[pin_data]
pub(crate) struct Gpu<'gpu> {
    spec: Spec,
    /// vGPU state and firmware parameters.
    ///
    /// Declared before `bar_user` and `mm` to preserve vGPU-before-MM teardown ordering.
    #[pin]
    vgpu: VgpuManager<'gpu>,
    /// BAR1 user interface for CPU access to GPU virtual memory.
    #[pin]
    bar_user: BarUser<'gpu>,
    /// GPU memory manager owning memory management resources.
    #[pin]
    mm: GpuMm<'gpu>,
    /// GSP and its resources.
    #[pin]
    gsp_resources: GspResources<'gpu>,
    /// Channel ID pool borrowed by the vGPU manager and its live instances.
    ///
    /// Declared after `vgpu` so the manager is dropped before the pool.
    #[pin]
    chid_pool: ChannelIdPool,
    /// System memory page required for flushing all pending GPU-side memory writes done through
    /// PCIE into system memory, via sysmembar (A GPU-initiated HW memory-barrier operation).
    ///
    /// Must be kept declared *after* `gsp_resources`, as the latter's `PinnedDrop` implementation
    /// requires the sysmem flush page to be in place.
    sysmem_flush: SysmemFlush<'gpu>,
}

#[pinned_drop]
impl PinnedDrop for GspResources<'_> {
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();
        let device = *this.device;
        let bar = *this.bar;
        let bundle = this.boot_result.take_unload_bundle();

        let _ = this
            .gsp
            .as_ref()
            .get_ref()
            .unload(
                GspBootContext {
                    pdev: device,
                    bar,
                    chipset: this.spec.chipset,
                    gsp_falcon: &*this.gsp_falcon,
                    sec2_falcon: &*this.sec2_falcon,
                    fsp: this.fsp.as_mut(),
                },
                bundle,
            )
            .inspect_err(|e| dev_err!(device, "failed to unload GSP: {:?}\n", e));
    }
}

impl<'gpu> Gpu<'gpu> {
    /// Returns the chipset this GPU was identified as.
    pub(crate) fn chipset(&self) -> Chipset {
        self.spec.chipset
    }

    /// Returns a shared handle to the GSP command queue.
    pub(crate) fn cmdq(&self) -> Arc<Cmdq> {
        self.gsp_resources.gsp.cmdq()
    }

    /// Returns the firmware build identifier, if one was reported.
    pub(crate) fn build_id(&self) -> Option<&firmware::BuildId> {
        self.gsp_resources.gsp.build_id()
    }

    pub(crate) fn vgpu_manager(&self) -> &VgpuManager<'gpu> {
        &self.vgpu
    }

    pub(crate) fn vgpu_total_vfs(&self) -> Option<NonZero<u16>> {
        self.vgpu.total_vfs()
    }

    pub(crate) fn mm(&self) -> &GpuMm<'gpu> {
        &self.mm
    }

    pub(crate) fn bar_user(&self) -> &BarUser<'gpu> {
        &self.bar_user
    }

    pub(crate) fn bar0(&self) -> Bar0<'gpu> {
        self.gsp_resources.bar
    }

    pub(crate) fn new(
        pdev: &'gpu pci::Device<device::Core<'_>>,
        bar: Bar0<'gpu>,
        vector: pci::IrqVector<'gpu>,
    ) -> impl PinInit<Self, Error> + 'gpu {
        let dev = pdev.as_ref();

        // `vector` is shared with the permanent GSP handler in probe and used here only by the
        // interrupt self-test.
        #[cfg(not(CONFIG_NOVA_CORE_IRQ_SELFTEST))]
        let _ = vector;

        try_pin_init!(&this in Self {
            spec: Spec::new(dev, bar).inspect(|spec| {
                dev_info!(dev,"NVIDIA ({})\n", spec);
            })?,

            // We must wait for GFW_BOOT completion before doing any significant setup on the GPU.
            _: {
                let hal = hal::gpu_hal(spec.chipset);
                let dma_mask = hal.dma_mask();

                // SAFETY: `Gpu` owns all DMA allocations for this device, and we are
                // still constructing it, so no concurrent DMA allocations can exist.
                unsafe { pdev.dma_set_mask_and_coherent(dma_mask)? };

                hal.wait_gfw_boot_completion(bar)
                    .inspect_err(|_| dev_err!(dev, "GFW boot did not complete\n"))?;
            },

            // Validate the MSI interrupt path before booting GSP, when the self-test is
            // enabled. This runs on a quiesced interrupt tree with no GSP state present, so it
            // never observes or acknowledges GSP or PRIV_RING interrupts.
            _: {
                #[cfg(CONFIG_NOVA_CORE_IRQ_SELFTEST)]
                crate::irq::doorbell_test::run_selftest(pdev, bar, spec.chipset, vector)?;
            },

            // Initialize this early because `gsp_resources` depends on it.
            sysmem_flush: SysmemFlush::register(dev, bar, spec.chipset)?,

            chid_pool <- ChannelIdPool::new(
                usize::try_from(TOTAL_CHANNELS).map_err(|_| EOVERFLOW)?,
            ),

            // TODO: Use `&chid_pool` self-referential pin-init syntax once available.
            //
            // SAFETY: `chid_pool` is initialized before this expression is evaluated and
            // lives at a pinned stable address. On successful construction the declaration
            // order drops `vgpu` before `chid_pool`; on initializer failure the later `vgpu`
            // guard is dropped before the earlier `chid_pool` guard.
            vgpu <- VgpuManager::new(
                // SAFETY: The lifetime and drop-order rationale above covers this borrow.
                unsafe { &*core::ptr::from_ref(chid_pool.as_ref().get_ref()) },
            ),

            gsp_resources <- try_pin_init!(GspResources {
                device: pdev,

                spec: *spec,

                bar,

                gsp_falcon: Falcon::new(
                    dev,
                    spec.chipset,
                    bar
                )
                .inspect(|falcon| falcon.clear_swgen0_intr())?,

                sec2_falcon: Falcon::new(dev, spec.chipset, bar)?,

                fsp: Fsp::try_new(dev, bar, spec.chipset)?,

                _: {
                    // SAFETY: `vgpu` was initialized above at its stable address in this
                    // pinned `Gpu`. Construction is single-threaded, and no other reference
                    // to the manager is live while this temporary mutable pin is used.
                    let vgpu = unsafe {
                        Pin::new_unchecked(&mut *core::ptr::addr_of_mut!((*this.as_ptr()).vgpu))
                    };
                    vgpu.detect_state(pdev, spec.chipset, fsp.as_mut());
                },

                gsp <- Gsp::new(pdev, spec.chipset),

                // This member must be initialized last, so the unload bundle can never be dropped
                // from outside of the constructed `GspResources`, ensuring that the unload
                // sequence is properly run in case of failure.
                boot_result: {
                    // SAFETY: `vgpu` was initialized above at its stable address in this
                    // pinned `Gpu`. The previous temporary reference has expired, and
                    // construction is still single-threaded.
                    let vgpu = unsafe {
                        Pin::new_unchecked(&mut *core::ptr::addr_of_mut!((*this.as_ptr()).vgpu))
                    };
                    gsp.boot(
                        GspBootContext {
                            pdev,
                            bar,
                            chipset: spec.chipset,
                            gsp_falcon,
                            sec2_falcon,
                            fsp: fsp.as_mut(),
                        },
                        vgpu,
                    )?
                },
            }),

            // Create GPU memory manager owning memory management resources.
            mm <- {
                let info = &gsp_resources.boot_result.static_info;
                let mut buddy_params = KVec::new();
                for region in &info.usable_fb_regions {
                    dev_info!(
                        dev,
                        "Using FB region: {:#x}..{:#x}\n",
                        region.start,
                        region.end
                    );
                    buddy_params.push(
                        GpuBuddyParams {
                            base_offset: region.start,
                            size: region.end - region.start,
                            chunk_size: Alignment::new::<SZ_4K>(),
                        },
                        GFP_KERNEL,
                    )?;
                }

                // SAFETY: `vgpu` was initialized above and remains pinned for the lifetime of
                // `Gpu`. GSP boot has returned, so no mutable manager reference remains live.
                let vgpu = unsafe { &*core::ptr::addr_of!((*this.as_ptr()).vgpu) };
                let buddy_base_alignment = match vgpu.vmmu_segment_size() {
                    Some(size) => Alignment::new_checked(
                        usize::try_from(size).map_err(|_| EOVERFLOW)?,
                    )
                    .ok_or(EINVAL)?,
                    None => Alignment::new::<SZ_4K>(),
                };

                // PRAMIN covers all physical VRAM (including GSP-reserved areas
                // above the usable region, e.g. the BAR1 page directory).
                let pramin_vram_region = (0..info.total_fb_end).into_vram_range();
                GpuMm::new(
                    bar,
                    gsp_resources.spec.chipset,
                    buddy_params,
                    buddy_base_alignment,
                    pramin_vram_region,
                )?
            },

            // Create BAR1 user interface for CPU access to GPU virtual memory.
            bar_user <- {
                let info = &gsp_resources.boot_result.static_info;
                let bar1_idx = crate::driver::bar1_resource_index(pdev)?;
                let bar1_size = pdev.resource_len(bar1_idx)?;
                let bar1 = Arc::new(
                    pdev.iomap_region(bar1_idx, c"nova-core/bar1")?
                        .into_devres()?,
                    GFP_KERNEL,
                )?;
                BarUser::new(
                    VramAddress::new(info.bar1_pde_base),
                    gsp_resources.spec.chipset,
                    bar1_size,
                    bar1,
                )?
            },

            _: {
                // GSP_INIT has already returned this information as its boot-completion reply.
                let info = &gsp_resources.boot_result.static_info;
                match info.gpu_name() {
                    Ok(name) => dev_info!(dev, "GPU name: {}\n", name),
                    Err(e) => dev_warn!(dev, "GPU name unavailable: {:?}\n", e),
                }
                dev_info!(
                    dev,
                    "Total physical VRAM: {} MiB\n",
                    info.total_fb_end / u64::SZ_1M
                );

                if !info.usable_fb_regions.is_empty() {
                    dev_dbg!(dev, "Usable FB regions:\n");
                    for region in &info.usable_fb_regions {
                        dev_dbg!(dev, "  - {:#x?}\n", region);
                    }

                    dev_dbg!(
                        dev,
                        "Total usable VRAM: {} MiB\n",
                        info.usable_fb_regions.iter().fold(0u64, |res, region| res
                            .saturating_add(region.end - region.start))
                            / u64::SZ_1M
                        );
                }
            }
        })
    }
}
