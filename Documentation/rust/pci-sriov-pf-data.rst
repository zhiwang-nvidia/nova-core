.. SPDX-License-Identifier: GPL-2.0

.. _pci_rust_sriov_pf_data:

===========================================
Sharing Rust PF data with SR-IOV VF drivers
===========================================

An SR-IOV Physical Function (PF) and its Virtual Functions (VFs) are
independent PCI devices.  Their drivers may live in different modules, and a
VF driver may be written in either Rust or C.  Nevertheless, a VF often needs
to invoke a small PF-owned interface for coordination with the physical
device.

This document describes how a Rust PF driver can publish pinned data for its
VFs without exposing its complete private ``drvdata``.  It supplements the
general SR-IOV description in :doc:`../PCI/pci-iov-howto`.

The idea
========

The PF publishes one deliberately chosen data object through
``SriovPfRegistration`` before enabling VFs.  The object is owned by the PF
registration and remains at a stable address.  A Rust VF receives a typed
``Pin<&T>``.  A C VF receives a checked ``struct rust_ffi`` descriptor whose
operations call the same object through generated C ABI trampolines.

The published object is an explicit PF/VF contract, not a replacement for PF
``drvdata``.  PCI supplies the route to the correct PF and orders driver
teardown; the chosen object supplies only the data and operations that the PF
intends to share.  VFs borrow that object and never own it.

There is no global interface registry.  PCI topology selects the provider:
the consumer is a VF and its ``physfn`` identifies the PF.  The consumer path
then checks that the selected PF published the expected Rust type or C ABI.

::

        PF driver data
        +-----------------------------+
        | SriovPfRegistration --------+-- owns ------------------+
        +-----------------------------+                          |
                                                                 v
        PF struct pci_dev                 registration allocation
        +-----------------------------+  +--------------------------+
        | sriov_registration_data_rust+->| rust_ffi (at offset 0)   |
        +-----------------------------+  | TypeId | pinned T        |
                                         +----+-------------+-------+
                                              ^             ^
                                              |             |
        C VF: token/ABI/size -> ops/context --+             |
        Rust VF: TypeId check -> Pin<&T> -------------------+

The design separates four concerns:

* PCI topology selects the actual PF for a VF.
* Rust ``TypeId`` or the C FFI token and version check the requested
  interface.
* A managed device link and the PF registration determine the lifetime of
  the borrow.
* The published data type supplies any synchronization needed by concurrent
  callers.

The FFI descriptor does not hold a second copy of the PF data.  Its
``context`` points at the same pinned object that a Rust VF borrows directly.
For example, both Rust and C VFs in the SR-IOV sample eventually call
``PfApi::submit()``.  The C path adds only an ABI trampoline and return-value
conversion.

Lifetime foundation
===================

The published pointer is borrowed; it is not a reference-counted handle.
Its lifetime is based on managed SR-IOV and a persistent managed device link
from each VF consumer to its PF supplier.

The PCI core creates that link before the VF is allowed to probe.  The link
remains after a failed probe or a normal driver unbind so it also protects a
later bind.  The driver core consequently waits for an in-progress VF probe
and unbinds every bound VF consumer before unbinding the PF supplier.  On PF
removal, managed SR-IOV also invokes ``sriov_configure(0)`` before the PF
driver's remove callback if VFs are still enabled.  VF unbind, including
destruction of its driver data, completes before PF removal proceeds.

::

        PF probe
           |
           +-- initialize and pin the published data
           |
           +-- publish SriovPfRegistration
           |
        PF probe returns and installs all PF driver data
           |
        sriov_configure(n) enables VFs
           |
           +-- PCI creates each VF
           |
           +-- PCI adds a managed link: VF consumer -> PF supplier
           |
           +-- VF probe borrows and uses the PF data
           |
        PF unbind is requested
           |
           +-- driver core unbinds every VF consumer
           |      |
           |      +-- VF remove stops and drains all PF calls
           |      |
           |      +-- VF driver data is destroyed
           |
           +-- if VFs remain, PCI invokes sriov_configure(0)
           |
           +-- disabling SR-IOV destroys VF devices and links
           |
           +-- PF remove runs
           |
           +-- PF driver data is destroyed
                  |
                  +-- registration unpublishes and frees the data

Disabling VFs through ``sriov_numvfs`` follows the shorter part of the same
ordering: VF drivers are removed before their VF devices disappear, while the
PF driver remains bound and its registration remains published.

If ``sriov_configure(0)`` does not disable all VFs during PF unbind, the PCI
core warns and forcibly disables SR-IOV.  The lifetime guarantee therefore
does not depend on a successful driver callback.

A successful VF probe may retain the borrow in its driver data for the
duration of that binding.  A failed probe must discard the borrow before
returning.  A VF remove callback must stop and drain all work that could use
the PF data before the callback returns.  These rules also apply to raw
descriptor and context pointers retained by a C VF.

Publishing PF data
==================

A PF may publish data from either a ``SriovPfDriver`` implementation or a
regular Rust ``pci::Driver`` implementation.  The ``SriovPfDriver`` adapter
rejects VFs but allows the same ID table to cover conventional PCI functions.
Such a driver can publish conditionally, as nova-core does for devices that
expose an SR-IOV capability.  A driver that handles PFs and VFs in one
implementation can instead use ``pci::Driver`` directly.

Use one of these constructors during PF probe:

* ``SriovPfRegistration::new_with_lt()`` publishes ``ForLt``-encoded data
  that is ``Send + Sync`` to Rust consumers.
* ``SriovPfRegistration::new_ffi_with_lt()`` publishes ``Send + Sync`` data
  encoded by ``CovariantForLt`` to Rust consumers and adds a C-callable FFI
  descriptor.

Both constructors are unsafe because the PF driver establishes the lifetime
conditions that cannot be expressed entirely in the type system.  A provider
must obey all of the following rules:

* Call the constructor during PF probe, before any VF is enabled.
* Publish at most one registration for a PF.
* Keep the registration in immutable PF driver data until removal.
* Enable VFs only after PF probe has returned and installed that driver data.
* Use managed SR-IOV so VF consumers are unbound before the registration is
  dropped.
* If the published object refers to other PF driver fields, declare the
  registration before those fields so it is dropped first.

The published type must be ``Send + Sync`` for every lifetime because VFs may
call it from different threads.

The Rust PCI adapter opts drivers into managed SR-IOV.  Its
``sriov_configure`` callback receives a checked ``pci::sriov::Device`` and a
pinned reference to the PF driver data.  It enables or disables VFs with
``enable_sriov()`` and ``disable_sriov()``.

Rust VF consumers
=================

``SriovVfDriver`` declares the expected covariant PF data as its ``PfData``
associated type.  Before invoking the VF driver's probe callback, its
adapter:

#. verifies that the PCI device is a VF;
#. follows the VF's PF relationship;
#. verifies that the PF has published data;
#. compares the stored ``TypeId`` with ``PfData``; and
#. returns the data as a pinned shared reference.

The VF callback therefore receives ``Pin<&PfData>`` directly.  It does not
need the PF's ``pci::Device``, the PF driver object, or a cast from an untyped
pointer.

PF and VF drivers may be registered by separate modules.  They must share the
exact ``ForLt`` type that identifies the PF data.  Defining look-alike types
independently does not work because they have different ``TypeId`` values.
When separate Rust crates are used, put the shared definition in a crate that
both can import.

``pf_registration_data()`` is the direct accessor for data encoded by
``CovariantForLt`` and is the path used by ``SriovVfDriver``.  A regular
``pci::Driver`` implementation can use ``pf_registration_data_with()`` for
invariant data; its higher-ranked closure prevents that data from escaping
with a shortened lifetime.

If one module registers both drivers, register the VF driver first.  This
ensures that it is ready before the PF can enable VFs.  PF-only and VF-only
modules can instead use ``module_pci_sriov_pf_driver!`` and
``module_pci_sriov_vf_driver!`` independently.

C VF consumers
==============

The common C descriptor is declared in ``include/linux/rust_ffi.h``::

        struct rust_ffi
        +---------------------------------------------------+
        | token | ABI version | ops size | ops | context    |
        +---------------------------------------------------+

The token identifies the type and semantics of an operations table.  It is
not a PCI device identifier, a secret, an authorization check, a registry
key, or a lifetime handle.  PCI locates the PF before comparing the token.

A driver-specific header defines the stable token, ABI version, and C
operations structure shared by the Rust provider and C consumers.  ABI
compatibility follows these rules:

* The major version must match exactly.
* A provider's minor version must be at least the consumer's requested minor
  version.
* A minor-version update may only append operations to the table.
* ``ops_size`` must cover the table prefix used by the consumer.

The Rust provider implements ``interop::ffi::Abi`` and applies
``#[ffi_vtable]`` to methods on its PF data.  The macro verifies the complete
bindgen operations-table layout and generates a private static operations
table and private C ABI trampolines.  It does not generate the C header.  A
trampoline recovers ``Pin<&T>`` from ``context`` and invokes the same Rust
method used by Rust VFs.  It converts ``Result<()>`` into zero or a negative
errno, and ``Result<c_int>`` into its successful value or a negative errno.

::

        Rust VF                                  C VF
        VfAdapter + TypeId check                 borrow + ABI checks
                   |                                      |
                   v                                      v
              Pin<&PfApi>                   ops->submit(context, id)
                   |                                      |
                   |                            generated trampoline
                   +------------------+-------------------+
                                      |
                                      v
                             PfApi::submit() -> Result
                                |                 |
                           Rust error         C 0 or -errno

A C VF includes ``linux/rust_ffi.h`` directly or through its driver-specific
header and borrows the interface during probe.  The essential call sequence
is::

        const struct my_pf_ops *ops;
        const struct rust_ffi *ffi;
        int ret;

        ffi = pci_iov_borrow_rust_pf_data(vf, &my_token,
                                          MY_ABI_MAJOR,
                                          MY_ABI_MINOR,
                                          sizeof(*ops));
        if (IS_ERR(ffi))
                return PTR_ERR(ffi);

        ops = ffi->ops;
        if (!ops->submit)
                return -EOPNOTSUPP;

        ret = ops->submit(ffi->context, pci_dev_id(vf));
        if (ret)
                return ret;

The PCI helper verifies that the device is a VF, that its PF is bound to a
managed SR-IOV driver, and that the descriptor satisfies the requested token,
version, and size.  It returns a borrow, so there is no matching ``put``
operation.  The size check does not prove that an individual operation is
implemented, so the consumer must still check each callback it needs.  The
pointers must not be used after VF probe fails or after VF remove returns.

Synchronization and teardown
============================

All VFs of a PF borrow the same object and may call it concurrently.  Pinning
keeps the object's address stable; it does not serialize access.  The PF data
must use interior synchronization appropriate for each operation, such as a
mutex for sleepable methods or an atomic for a simple counter.

The sample uses ``Mutex<u64>`` for its request count to demonstrate shared,
synchronized PF state.  The count is an internal implementation detail and
is not returned through the C ABI.  Both consumers see only whether
``submit()`` succeeded.

Document for every C operation whether it may sleep and which calling
contexts are permitted.  Before VF removal returns, cancel or flush any work
that could still call an operation.  Do not wait for such work while holding
a lock that the operation itself needs.

Disabling SR-IOV through sysfs removes VFs synchronously while holding the PF
device lock.  A VF remove path must not wait for an FFI operation that must
acquire that same lock, or the two paths can deadlock.

What is not provided
====================

This model deliberately does not provide:

* automatic locking for the published data;
* a reference that may outlive the VF driver binding;
* authorization or secrecy through the FFI token;
* runtime-PM coupling between VF and PF devices;
* a stable C layout for arbitrary Rust data;
* a global registry or multiple independent registrations on one PF; or
* a way for one VF driver to call another VF driver directly.

The managed device link currently supplies driver-presence and teardown
ordering, but not runtime-PM integration.  Operations that access powered PF
hardware must arrange runtime PM separately.  C consumers interact only with
the explicitly declared operations table, never with the layout of the Rust
object behind ``context``.

Examples
========

The complete examples are:

* ``samples/rust/rust_driver_sriov.rs``: a Rust PF and Rust VF sharing a
  pinned PF object with a mutex-protected counter;
* ``samples/rust/rust_driver_sriov.h``: the C ABI token, version, and
  operations table;
* ``samples/rust/rust_driver_sriov_c_vf.c``: a C VF borrowing and calling the
  Rust PF object; and
* ``drivers/gpu/nova-core/driver.rs``: a regular Rust PCI driver publishing
  unit data as a typed PF-readiness marker without a C ABI.

The Rust and C sample VF drivers match the same device ID, so only one can
bind to a given VF.  Use the module ordering described by their Kconfig help
or ``driver_override`` to select the C path deterministically.

See also :doc:`../driver-api/device_link` for the general device-link model.
