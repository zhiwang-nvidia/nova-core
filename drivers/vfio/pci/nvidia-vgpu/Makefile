# SPDX-License-Identifier: GPL-2.0-only
obj-$(CONFIG_NVIDIA_VGPU_VFIO_PCI) += nvidia-vgpu-vfio-pci.o
nvidia-vgpu-vfio-pci-y := main.o ffi.o

# drivers/Makefile completes the GPU build before entering the VFIO subtree.
# Depend on the provider object so changes to its metadata rebuild this crate.
$(obj)/ffi.o: $(objtree)/drivers/gpu/nova-core/nova_core.o
RUSTFLAGS_ffi.o += --crate-name nvidia_vgpu_ffi \
	--extern nova_core=$(objtree)/drivers/gpu/nova-core/libnova_core.rmeta
