.. SPDX-License-Identifier: GPL-2.0

.. _pci_rust_sriov_pf_data:

===========================================
Sharing Rust PF data with SR-IOV VF drivers
===========================================

An SR-IOV Physical Function (PF) and its Virtual Functions (VFs) are
independent PCI devices.  Their Rust drivers may live in different modules.
A VF often needs to invoke a small PF-owned interface for coordination with
the physical device.

This document describes how a Rust PF driver can publish pinned data for its
VFs without exposing its complete private ``drvdata``.  It supplements the
general SR-IOV description in :doc:`../PCI/pci-iov-howto`.

The idea
========

The PF publishes one deliberately chosen data object through
``VfRegistration`` before enabling VFs.  The registration and object are
initialized inline in the PF's pinned driver data.  A VF accesses the object
through a typed pinned shared reference, ``Pin<&T>``.

The published object is an explicit PF/VF contract, not a replacement for PF
``drvdata``.  PCI supplies the route to the correct PF and orders driver
teardown; the chosen object supplies only the data and operations that the PF
intends to share.  VFs borrow that object and never own it.

There is no global interface registry.  PCI topology selects the provider:
the consumer is a VF and its ``physfn`` identifies the PF.  The consumer path
then checks that the selected PF published the expected Rust type.

::

        PF driver data (pinned)
        +--------------------------------+
        | VfRegistration                 |
        |  +--------------------------+  |
        |  | TypeId | pinned T        |<----+
        |  +--------------------------+  |  |
        | remaining PF data              |  |
        +--------------------------------+  |
                                            |
        PF struct pci_dev                   |
        +----------------------------+      |
        | vf_registration_data_rust -+------+
        +----------------------------+
                     ^
                     | physfn
              VF struct pci_dev
                     |
                     +-- TypeId check --> Pin<&T>

The design separates four concerns:

* PCI topology selects the actual PF for a VF.
* Rust ``TypeId`` checks the requested ``ForLt`` type.
* A managed device link and the inline registration determine the lifetime
  of the borrow.
* The published data type supplies any synchronization needed by concurrent
  callers.

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
           +-- initialize and pin VfRegistration and its data
           |
           +-- publish as the registration's final initialization step
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
                  +-- registration disables any remaining VFs
                  |
                  +-- registration withdraws and drops the data

Disabling VFs through ``sriov_numvfs`` follows the shorter part of the same
ordering: VF drivers are removed before their VF devices disappear, while the
PF driver remains bound and its registration remains published.

If ``sriov_configure(0)`` does not disable all VFs during PF unbind, the PCI
core warns and forcibly disables SR-IOV.  The lifetime guarantee therefore
does not depend on a successful driver callback.

A successful VF probe may retain the borrow in its driver data for the
duration of that binding.  A failed probe must discard the borrow before
returning.  A VF remove callback must stop and drain all work that could use
the PF data before the callback returns.

Publishing PF data
==================

PF and VF drivers use the ordinary ``pci::Driver`` abstraction.
``VfRegistration::new()`` publishes ``ForLt``-encoded data.  It returns a
pin-initializer rather than an allocated registration handle.  The PF embeds
it with ``<-`` in a ``#[pin]`` field of its driver data::

        #[pin_data]
        struct PfData<'a> {
            #[pin]
            vf_registration: pci::VfRegistration<'a, MyApiForLt>,
            // Fields borrowed by MyApi follow the registration.
        }

The constructor rejects a VF.  On a conventional PCI function without an
SR-IOV capability it creates an inactive registration, allowing one PF-side
driver to continue supporting devices with and without SR-IOV.

The constructor is unsafe because the PF driver establishes conditions
that cannot be expressed entirely in the type system.  A provider must:

* Call the constructor during PCI probe, before any VF can be enabled.  It
  publishes only when that function is an SR-IOV PF.
* Publish at most one registration for a PF.
* Initialize it in the pinned PF driver data and do not forget that data.
* Enable VFs only after PF probe has returned and installed that driver data.
* Use managed SR-IOV so VF consumers are unbound before the registration is
  dropped.
* Declare it before any PF driver fields borrowed by the published object, so
  the registration is dropped first.

The published type must be ``Send + Sync`` for every lifetime because VFs may
call it from different threads.

The Rust PCI adapter opts drivers into managed SR-IOV.  Its
``sriov_configure`` callback receives a checked ``pci::sriov::Device`` and a
pinned reference to the PF driver data.  It enables or disables VFs with
``enable_sriov()`` and ``disable_sriov()``.

Rust VF consumers
=================

A Rust VF implements ``pci::Driver`` and explicitly requests PF data during
probe.  The accessor verifies that the PCI device is a VF, follows its PF
relationship, checks that data was published, compares its ``TypeId``, and
returns a pinned shared reference.  The VF does not receive the PF's
``pci::Device``, the PF driver object, or an untyped pointer.

PF and VF drivers may be registered by separate modules.  They must share the
exact ``ForLt`` type that identifies the PF data.  Defining look-alike types
independently does not work because they have different ``TypeId`` values.
When separate Rust crates are used, put the shared definition in a crate that
both can import.

If one module registers both drivers, register the VF driver first.  This
ensures that it is ready before the PF can enable VFs.  PF-only and VF-only
modules register their ordinary PCI drivers independently.

Data and borrow lifetimes
-------------------------

``ForLt`` describes a family of data types, ``F::Of<'data>``, whose encoded
lifetime may refer to resources owned by the PF driver.  The lifetime of a
VF's borrow of that data is separate: ``Pin<&'borrow F::Of<'data>>`` borrows
the published object for ``'borrow`` without changing ``'data``.

``vf_registration_data()`` is the direct accessor for data encoded by
``CovariantForLt``.  Covariance permits the encoded data lifetime to be
shortened to the VF's borrow lifetime, so this accessor can return a pinned
shared reference tied to the VF device borrow.

``vf_registration_data_with()`` also supports data that is invariant in its
encoded lifetime.  Its closure is higher-ranked over two independent
lifetimes::

        f: impl for<'borrow, 'data> FnOnce(Pin<&'borrow F::Of<'data>>) -> R

The closure must work without assuming that ``'borrow`` and ``'data`` are
the same lifetime.  This prevents a reference obtained through the temporary
borrow from being stored in invariant registration data as if it had the
PF data's lifetime.  Using one lifetime for both positions would lose that
guarantee.  The return type ``R`` is independent of both lifetimes, so the
borrowed PF reference cannot escape through the closure's result.

A domain-specific VF handle may store only the VF device and use the closure
accessor for each operation.  It can return an owned result from the closure
without retaining a separate raw PF pointer.

Synchronization and teardown
============================

All VFs of a PF borrow the same object and may call it concurrently.  Pinning
keeps the object's address stable; it does not serialize access.  The PF data
must use interior synchronization appropriate for each operation, such as a
mutex for sleepable methods or an atomic for a simple counter.

The sample uses ``Mutex<u64>`` for its request count to demonstrate shared,
synchronized PF state.  ``PfApi::submit()`` returns the request number as a
``Result<u64>``, and the VF records that number in its log.

Document for every PF operation whether it may sleep and which calling
contexts are permitted.  Before VF removal returns, cancel or flush any work
that could still call an operation.  Do not wait for such work while holding
a lock that the operation itself needs.

Disabling SR-IOV through sysfs removes VFs synchronously while holding the PF
device lock.  A VF remove path must not wait for a PF operation that must
acquire that same lock, or the two paths can deadlock.

The managed device link supplies driver-presence and teardown ordering, but
not runtime-PM integration.  Operations that access powered PF hardware must
arrange runtime PM separately.

Examples
========

The complete examples are:

* ``samples/rust/rust_driver_sriov.rs``: a Rust PF and Rust VF sharing a
  pinned PF object with a mutex-protected counter; and
* ``drivers/gpu/nova-core/driver.rs``: a regular Rust PCI driver embedding a
  typed PF registration in its pinned driver data.

See also :doc:`../driver-api/device_link` for the general device-link model.
