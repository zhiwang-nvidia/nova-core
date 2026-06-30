.. SPDX-License-Identifier: GPL-2.0
.. SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

=================================================
GPU interrupt handling: GIN and the GSP event
=================================================

This document describes how nova-core receives interrupts from the GPU. It
covers the GIN interrupt controller and the first interrupt nova-core services,
the GSP event. nova-core supports Turing and later GPUs.

Throughout, *host* means the CPU and the nova-core driver running on it. It
stands in contrast to the GPU's own on-chip processors, which run their own
firmware: the GSP (GPU System Processor), the PMU, and the system-firmware
processor. Those on-chip processors can receive their own interrupts, as
described below.

The register names are the names from the GPU hardware reference headers.
``NV_GIN`` is the register namespace for the controller. Older material calls it
``NV_CTRL`` or ``INTR_CTRL``.

The GIN controller
==================

A GPU has many interrupt sources: the GSP, copy engines, the graphics
engine, video decode and encode, the MMU fault path, timers, and others. The
host has one MSI vector for the whole GPU. GIN combines every source onto
that one vector and records which sources are pending so the host can tell them
apart.

Each source has a vector number. GIN records pending vectors in a two-level tree
of registers and raises the MSI when an enabled vector becomes pending. The
host's interrupt handler then reads the tree to find which vectors are pending,
acknowledges them, and runs the work for each.

Interrupt trees
===============

GIN keeps a separate interrupt tree for each place an interrupt can be sent to:

* One tree per PCIe function. The Physical Function (PF) has a tree, and each
  Virtual Function (VF) has a tree.
* One tree per on-chip microcontroller that receives interrupts: the GSP, the
  PMU, and the system-firmware processor.

Each destination reaches its own tree through its own BAR0 and cannot reach any
other tree. GSP firmware decides which tree each engine's interrupt is sent to.

nova-core services only the PF CPU tree. The VF trees and the microcontroller
trees belong to firmware or to virtual functions.

The two-level tree
==================

Each tree has two levels. The bottom level is the LEAF registers, which hold one
pending bit per vector. The top level is the single TOP register, which
summarizes the leaves so the handler can find pending work by reading one
register.

* Each ``LEAF(i)`` is a 32-bit register holding the pending bits for vectors
  ``i * 32`` through ``i * 32 + 31``. A set bit means that vector is pending.
* ``TOP`` is a single 32-bit read-only register. Each of its bits summarizes one
  *subtree*, which is a pair of adjacent leaves. TOP bit ``N`` reads 1 when any
  bit is set in ``LEAF[2N]`` or ``LEAF[2N + 1]``.

A subtree is two leaves, so a part with L leaves has L / 2 subtrees and uses
that many TOP bits. An 8-leaf part uses TOP bits 0 through 3, and the other 28
bits always read 0. A 16-leaf part uses TOP bits 0 through 7. The handler reads
TOP first so that one read tells it which leaves can have pending bits, and it
then reads only those leaves instead of all of them::

    TOP  (one 32-bit register; an 8-leaf part uses only bits 0..3)

      bit 0  ->  subtree 0  ->  LEAF[0], LEAF[1]   vectors   0..63
      bit 1  ->  subtree 1  ->  LEAF[2], LEAF[3]   vectors  64..127
      bit 2  ->  subtree 2  ->  LEAF[4], LEAF[5]   vectors 128..191
      bit 3  ->  subtree 3  ->  LEAF[6], LEAF[7]   vectors 192..255
      bits 4..31: always 0 on an 8-leaf part (a 16-leaf part uses bits 0..7)

    A LEAF is one 32-bit register, one bit per vector. For example, LEAF[4]
    holds vectors 128..159:

      bit 1  = vector 129  (CPU doorbell)
      bit 27 = vector 155  (GSP event)

Registers
---------

All the registers are 32 bits, defined in ``regs.rs`` under the
``NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_*`` names. The leaf registers are arrays
indexed by leaf number:

* ``LEAF(i)`` holds the pending bits for the vectors in leaf ``i``. Reading
  returns the pending mask, and writing a 1 to a bit acknowledges that vector
  (write-1-to-clear).
* ``LEAF_EN_SET(i)`` and ``LEAF_EN_CLEAR(i)`` enable and disable individual
  vectors in leaf ``i``.
* ``TOP`` is the read-only summary: bit N is set when any bit is set in
  ``LEAF[2N]`` or ``LEAF[2N + 1]``.
* ``TOP_EN_SET`` and ``TOP_EN_CLEAR`` arm and disarm subtrees.
* ``LEAF_TRIGGER`` makes a vector pending in software. The self-test uses it.

Vector encoding
---------------

A vector number ``v`` maps to a leaf and a bit::

    leaf = v / 32
    bit  = v % 32

The subtree that covers a leaf is ``leaf / 2``. So vector 129 is leaf 4, bit 1,
subtree 2, and vector 155 is leaf 4, bit 27, subtree 2.

Enabling and acknowledging
--------------------------

Every set/clear register is write-1-to-act: writing a 1 performs the action, and
writing a 0 does nothing.

* ``LEAF(i)`` is write-1-to-clear. Reading returns the pending mask. Writing a 1
  to a bit acknowledges that vector.
* ``LEAF_EN_SET(i)`` and ``LEAF_EN_CLEAR(i)`` enable and disable individual
  vectors in a leaf.
* ``TOP_EN_SET`` and ``TOP_EN_CLEAR`` arm and disarm whole subtrees.

A vector reaches the MSI only when both its leaf enable bit and its subtree's TOP
enable bit are set.

How an engine interrupt reaches the host
========================================

An engine does not write a LEAF register itself. Each engine has an interrupt
routing register, and GSP firmware programs it once at boot. Firmware writes
three things into it: the engine's VECTOR (which leaf bit it uses), its GFID
(which tree to post to: the PF or a specific VF), and its destination flags
(which on-chip consumers get it: CPU, GSP, PMU, or system firmware).

Later, when an engine has an event, three things happen in turn::

    1. The engine sends an interrupt message to GIN, carrying the VECTOR, GFID,
       and destination flags from its routing register.
    2. GIN sets bit (VECTOR % 32) in LEAF[VECTOR / 32], in the tree that the
       GFID and destination flags select.
    3. If that subtree is armed, GIN raises the MSI to the host.

Because firmware assigns the vectors, the host does not hardcode which vector
belongs to which engine. The one exception nova-core relies on is the GSP event
vector, which firmware pins to a fixed number (see "The GSP event vector").

Edge behavior and rearm
=======================

The pieces behave as follows:

* A LEAF bit is a latch. It is set on the rising edge of its source and stays set
  until the host writes a 1 to it. A source that stays high does not set the bit
  again.
* TOP is read-only. TOP bit N reads 1 while any bit is set in either leaf of
  subtree N.
* TOP_EN is a host-controlled enable bit per subtree, set through ``TOP_EN_SET``
  and cleared through ``TOP_EN_CLEAR``.
* GIN raises the MSI for subtree N on the rising edge of
  ``TOP[N] AND TOP_EN[N]``::

    LEAF[2N], LEAF[2N+1]  (latches, write-1-to-clear)
        -> OR -> TOP[N] --+
                          AND -> rising edge -> MSI for subtree N
              TOP_EN[N] ---+

Because the MSI is edge-triggered, writing ``TOP_EN_SET`` while a leaf bit is
still set produces a new edge and a new MSI. The handler uses this to rearm:
after it acknowledges the leaves, it writes ``TOP_EN_SET`` so an interrupt that
arrived during servicing is still delivered.

An engine that holds an internal level signal high does not produce a new leaf
edge after the host acknowledges the bit, so rearming alone does not re-deliver
it. Such engines have an ``INTR_RETRIGGER`` register that forces a new edge.
nova-core does not use it. The GSP event produces a new edge on each SWGEN0
assertion, and the handler drains the whole queue on each interrupt.

Servicing an interrupt
======================

nova-core services the tree in one of two ways, depending on which code handles
the interrupt.

The GSP event handler is nova-core's only real interrupt handler so far. It
knows that one specific vector fired, the GSP event, so it leaves its subtree
armed and just reads and acknowledges its own leaf bit. It never touches TOP or
TOP_EN, because it has no reason to scan the tree.

The self-test and the one-time startup drain instead walk the whole tree,
because they must handle whatever is pending across every subtree rather than one
known vector. They disarm the subtrees, read TOP, acknowledge every pending leaf,
then rearm.

The two paths as register operations::

    Full tree walk (self-test, and the one-time startup drain):
        write TOP_EN_CLEAR = subtree_mask    disarm; stop new MSIs
        read  TOP                            which subtrees are pending
        for each pending subtree N, for i in {2N, 2N+1}:
            mask = read LEAF[i]              pending vectors in this leaf
            write LEAF[i] = mask             ack (write-1-to-clear)
            handle each set bit
        write TOP_EN_SET = subtree_mask      rearm; allow new MSIs

    Notification, subtree stays armed (the GSP event handler):
        mask = read LEAF[gsp_leaf]           is our vector's bit set?
        write LEAF[gsp_leaf] = GSP_BIT       ack only our bit
        TOP_EN is never touched

Two rules for the full walk:

* Acknowledge every pending leaf bit, including bits nova-core does not handle. A
  bit left set keeps its subtree pending, so the next rearm raises another MSI at
  once. The handler writes back the full mask it read.
* Rearm only after acknowledging every pending leaf. Otherwise a still-set bit
  raises the MSI again while the handler is still running.

The notification path is safe because GIN does not raise a second MSI for a
vector that is still pending, and raises a new MSI on the next edge, so nothing
is lost. The GSP event handler uses it. The self-test and the one-time startup
drain use the full walk.

Interrupts and notifications
============================

Two kinds of source use the tree:

* An interrupt means a unit needs servicing.
* A notification means a unit is reporting that something happened, such as a log
  record or completed work.

nova-core's only source so far, the GSP event, is a notification. The handler
leaves the subtree armed and acknowledges only the GSP leaf bit.

The hardware manuals also split the vector space into "stall" and "nonstall"
ranges. Those are range names, not a description of behavior. nova-core does not
service the stall range.

Per-architecture differences
============================

The tree is the same on every supported GPU except for its size:

==================  ======  ========  ============
Architecture        Leaves  Subtrees  subtree_mask
==================  ======  ========  ============
Turing / Ampere     8       4         ``0x0f``
Ada                 8       4         ``0x0f``
Hopper / Blackwell  16      8         ``0xff``
==================  ======  ========  ============

Pre-Hopper parts use leaves 0 through 7, and the upper half of TOP reads zero.
Hopper and later have 16 leaves, though not all of them are populated with
sources. nova-core arms every implemented subtree. Unused leaves read zero, so
arming them does nothing.

The HAL provides the leaf count. The subtree count (leaves / 2) and the
``subtree_mask`` derive from it. If nova-core later services the stall vector
range, that range differs by architecture and would be added to the HAL.

Multi-die parts
===============

On multi-die parts the controller is replicated per die, with an aggregation
level above the per-die TOP registers. nova-core supports single-die parts and
services one PF CPU tree. Multi-die aggregation is out of scope.

The GSP event
=============

The GSP signals the host by raising SWGEN0, one of the software-generated
interrupt outputs of the GSP microcontroller (a "falcon" in NVIDIA hardware).
When the GSP has output for the host (log records, error records, and other
events), it writes the messages into the GSP-to-CPU queue in shared memory and
raises SWGEN0. SWGEN0 is routed through a GIN vector, so it reaches the host as
an MSI::

    GSP writes messages into the GSP-to-CPU queue
    GSP raises SWGEN0
    GIN sets the GSP leaf bit; the subtree becomes pending
    MSI -> Linux IRQ -> nova-core hard IRQ handler:
        read the GSP leaf bit and acknowledge it (subtree stays armed)
        read the GSP falcon IRQ status:
            SWGEN0 set   -> wake the threaded handler
            SWGEN0 clear -> report an error (a halt or fatal cause on the same
                            vector)
    threaded handler: take the command-queue lock and drain the GSP-to-CPU
        queue, routing each message

The interrupt is only the trigger to drain the queue. The drain works the same
way whether a poll or an interrupt starts it, so the code is shared (see
"Draining and classifying the GSP-to-CPU queue").

The GSP event vector
--------------------

The GSP event uses a fixed vector, ``GSP_INTR_0_VECTOR`` (155), on Turing
through Blackwell. Vector 155 is leaf 4, bit 27, subtree 2. nova-core enables
that leaf bit and services it, with no runtime vector discovery.

A full engine-to-vector table can be fetched from the GSP by RPC, but nova-core
does not need it for the single GSP event, so it does not fetch it.

Draining and classifying the GSP-to-CPU queue
=============================================

The queue carries both command replies and unsolicited events. The drain reads
each message and routes it by function code:

* A message whose function code and RPC sequence match the awaited reply is
  decoded and returned to the caller that sent the command. Matching the sequence
  as well as the function stops a late reply to a timed-out command from
  satisfying a later command that reused the same function code.
* Any other message is an unsolicited event. OS-error and robust-channel records
  are logged at error level. An unrecognized function code is logged at warning
  level. Other known events (GSP logs, libos prints, assertion records, lifecycle
  notices) need no action and are not logged again, because the RPC receive trace
  already records their arrival.

The classifier is a fixed set of function codes, not a handler registry.
nova-core has a small set of events, and the two that need action are handled
directly.

Both the polling path and the threaded interrupt handler drain through this same
code under the command-queue lock. Replies and events share one queue and one set
of read pointers, so one lock covers the whole drain. A thread waiting for a
reply dispatches any event it reads first and keeps waiting.

The cost of one lock is serialization. A drain waits for an in-flight command's
receive to finish or time out. For the current events (logs and error records)
that delay does not matter.

Design notes
============

Register naming
---------------

nova-core uses the ``NV_VIRTUAL_FUNCTION_PRIV_CPU_INTR_*`` names for the PF CPU
tree on both pre-Hopper and Hopper-plus parts. The Hopper-plus central aperture
(``NV_GIN_CPU_INTR_*``) configures other functions and is not used by the host
path.

Type-state tree API
-------------------

The tree walk has a required order: disarm a subtree, read its pending mask,
acknowledge the leaves, then rearm. The code encodes the stages as distinct types
(``Idle``, ``Unarmed``, ``Pending``) so the wrong order does not compile. There
is no ``ack`` before a ``read_pending`` has produced the mask, and ``rearm``
consumes the mask so it cannot be used twice. Acknowledging before reading
pending, which causes an interrupt storm, becomes a compile error.

The types order the calls within a single walk. They are not a lock, and nothing
stops two walks from running against the tree at once. nova-core does not run
concurrent walks: the GSP event handler touches only its own leaf and never walks
the tree, and the only whole-tree walks, the self-test and the startup drain, run
once during probe.

Threaded handler
----------------

The drain sleeps: it takes the command-queue mutex and walks shared memory, so it
cannot run in hard-IRQ context. nova-core uses a threaded IRQ handler. The hard
half acknowledges the GIN leaf, checks and clears the SWGEN0 latch, and wakes the
thread. The threaded half takes the lock and drains the queue. The self-test does
no sleeping work and uses a plain hard handler with a completion.

Shared BAR0 mapping
-------------------

The GPU, the self-test, and the GSP event handler read the same BAR0 registers.
nova-core keeps one BAR0 mapping and lets each of them borrow it. An interrupt
handler is torn down when the device unbinds, so it only runs while the mapping
is alive.

Self-test
=========

The self-test is a runtime check that runs during driver probe on real hardware.
It is not a KUnit test: it registers a real interrupt handler and confirms that
an interrupt injected at the GPU is delivered all the way to that handler, so it
needs a real GPU and a working MSI path. It is gated by
``CONFIG_NOVA_CORE_IRQ_SELFTEST``, runs before GSP boot (so it never touches GSP
interrupt state), and fails probe if the interrupt does not arrive.

The vector encoding and the subtree and leaf arithmetic have no hardware
dependency, and KUnit tests cover them without a GPU.

The test relies on ``LEAF_TRIGGER``, which makes a vector pending in software:
the leaf bit latches, TOP updates, and GIN raises the MSI if the subtree is
armed and the vector is enabled. This is a real hardware register, present on
all supported parts.

The test uses vector 129 (leaf 4, bit 1). It registers a handler for vector 129,
writes 129 to ``LEAF_TRIGGER``, and confirms the handler runs within a timeout.
This exercises the MSI path from the GPU to the handler without GSP firmware,
which is useful when bringing up PCI, MSI, and passthrough setups.

Virtualization
==============

The per-function trees, the GFID routing, and the central ``NV_GIN`` aperture
support virtualization: each VF gets its own tree, and the PF or firmware routes
an engine's interrupt to the right function. MIG (multi-instance GPU)
partitioning adds more structure. nova-core services only the PF CPU tree, so VF
tree management, GFID routing, and MIG are out of scope.

References
==========

* nova-core source: the register definitions in ``regs.rs``, the interrupt HAL
  and tree API in the ``irq`` module, and the GSP command queue in the ``gsp``
  module.
