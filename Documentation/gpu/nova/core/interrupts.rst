.. SPDX-License-Identifier: GPL-2.0
.. SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.

=================================================
GPU interrupt handling: GIN and the GSP event
=================================================

This document describes how nova-core receives interrupts from the GPU. It
covers the GIN interrupt controller and the first interrupt nova-core services,
the GSP event. nova-core supports Turing and later GPUs.

Throughout, *CPU* means the CPU and the nova-core driver running on it. It stands
in contrast to the GPU's own on-chip processors, which run their own firmware:
the GSP (GPU System Processor), the PMU, and the system-firmware processor. Those
on-chip processors can receive their own interrupts, as described below.

The register names are the names from the GPU hardware reference headers.
``NV_GIN`` is the register namespace for the controller. Older material calls it
``NV_CTRL`` or ``INTR_CTRL``.

The GIN controller
==================

A GPU has many interrupt sources: the GSP, copy engines, the graphics engine,
video decode and encode, the MMU fault path, timers, and others. GIN records
which sources are pending in its own register tree, so a handler can tell them
apart.

Each source has a GIN vector number, which is internal to the controller and is
not a PCI vector index. GIN records pending vectors in its own two-level
register tree and raises the PCI interrupt when an enabled vector becomes
pending. The CPU's interrupt handler then reads the tree to find which vectors
are pending, acknowledges them, and runs the work for each.

How the tree reaches the CPU over PCI
-------------------------------------

How many PCI interrupts the tree needs depends on the interrupt type Linux
grants.

MSI has a single message, and every subtree raises that one message. One
allocated vector serves the whole tree.

MSI-X raises a separate table entry per subtree, so a subtree's interrupts
arrive on the table entry whose index is the subtree number. Linux masks each
table entry a driver did not allocate, and a masked entry sends no message: the
request sets a bit in the pending-bit array and waits for an unmask that never
comes. A driver that leaves out the entry its subtree raises loses every
interrupt on that subtree, and loses it silently, with the GIN leaf and TOP
registers showing the vector pending and enabled while no handler runs.

The armed-subtree invariant
---------------------------

Every subtree armed at TOP must have an allocated PCI vector with a registered
handler.

MSI satisfies this with one message that every subtree raises. MSI-X needs one
allocated, unmasked entry per armed subtree, which means allocating entries 0
through the highest armed subtree, registering a handler on each armed subtree's
entry, and rearming only that subtree. Entries allocated below an armed subtree
that the driver does not service cost nothing: Linux unmasks an entry only when
its interrupt is requested, and an unarmed subtree raises nothing.

nova-core arms exactly one subtree, subtree 2, because both the vectors it uses
live in leaf 4: the GSP event (155) and the self-test doorbell (129).

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
summarizes the leaves.

* Each ``LEAF(i)`` is a 32-bit register holding the pending bits for vectors
  ``i * 32`` through ``i * 32 + 31``. A set bit means that vector is pending.
* ``TOP`` is a single 32-bit read-only register. Each of its bits summarizes one
  *subtree*, which is a pair of adjacent leaves. TOP bit ``N`` reflects
  ``LEAF[2N]`` and ``LEAF[2N + 1]`` gated by their leaf enables, so a vector
  that latched while masked does not appear in TOP.

A subtree is two leaves, so a part with L leaves has L / 2 subtrees and uses
that many TOP bits. An 8-leaf part uses TOP bits 0 through 3, and the other 28
bits always read 0. A 16-leaf part uses TOP bits 0 through 7::

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

Mapping a vector to the tree
----------------------------

Each vector occupies one bit of one leaf, and each leaf belongs to one
subtree::

    leaf    = v / 32
    bit     = v % 32
    subtree = leaf / 2

Both of the vectors nova-core names by number fall in leaf 4: vector 129 at bit
1 and vector 155 at bit 27, so both arrive under subtree 2.

Enabling and acknowledging
--------------------------

Every set/clear register is write-1-to-act: writing a 1 performs the action, and
writing a 0 does nothing.

* ``LEAF(i)`` is write-1-to-clear. Reading returns the pending mask. Writing a 1
  to a bit acknowledges that vector.
* ``LEAF_EN_SET(i)`` and ``LEAF_EN_CLEAR(i)`` enable and disable individual
  vectors in a leaf.
* ``TOP_EN_SET`` and ``TOP_EN_CLEAR`` arm and disarm whole subtrees.

A vector reaches the CPU only when both its leaf enable bit and its subtree's TOP
enable bit are set. The leaf enable gates delivery and the TOP summary, but not
the latch: a masked vector still latches its LEAF bit, and that bit is visible
only by reading the leaf directly.

How an engine interrupt reaches the CPU
=======================================

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
    3. If that vector is enabled and its subtree is armed, GIN raises the PCI
       interrupt to the CPU.

Because firmware assigns the vectors, nova-core does not hardcode which vector
belongs to which engine. The one exception nova-core relies on is the GSP event
vector, which firmware pins to a fixed number (see "The GSP event vector").

Edge behavior and rearm
=======================

The pieces behave as follows:

* A LEAF bit is a latch. It is set on the rising edge of its source and stays set
  until the CPU writes a 1 to it. A source that stays high does not set the bit
  again.
* TOP is read-only and reports the subtree's *enabled* pending state. A vector
  that latched while its leaf enable was clear does not appear in TOP.
* LEAF_EN and TOP_EN are CPU-controlled enables that gate delivery.
* GIN raises the PCI interrupt for subtree N when the subtree's enabled pending
  state goes from low to high::

    Per vector, in leaf i at bit b:
        LEAF[i][b] AND LEAF_EN[i][b]

    Per subtree N, across its leaves 2N and 2N + 1:
        OR of every enabled pending bit  ->  TOP[N]

    Delivery for subtree N:
        TOP[N] AND TOP_EN[N]  ->  rising edge  ->  PCI interrupt

    TOP_EN applies below TOP, so unarming a subtree halts delivery and leaves
    what TOP reports unchanged.

Because a masked vector is invisible in TOP, code that must find every pending
bit cannot descend from TOP. It has to read the leaves directly. Open RM does
the same: its stalling-interrupt path never reads TOP, and instead walks every
subtree in its mask reading LEAF registers.

Because delivery is edge-triggered, writing ``TOP_EN_SET`` while an enabled leaf
bit is still set produces a new edge. A full tree walk uses this: after it
acknowledges the leaves, it writes ``TOP_EN_SET`` so an interrupt that arrived
during servicing is still delivered.

An engine that holds an internal level signal high does not produce a new leaf
edge after the CPU acknowledges the bit, so rearming alone does not re-deliver
it. Such engines have an ``INTR_RETRIGGER`` register that forces a new edge.

Retriggering a falcon
---------------------

A falcon signals the tree on a transition of its enabled interrupt causes.
Clearing the tree leaf while a cause is still latched leaves no transition, so
the vector stays clear however many further causes arrive. Both clear orders
have that window, which is why a handler on a falcon vector writes
``INTR_RETRIGGER`` on every path that services the vector.

An unconditional re-emit only terminates if nothing can raise it forever. A
cause the handler does not service is therefore masked at the falcon with
``IRQMCLR`` and cleared with ``IRQSCLR`` before the re-emit, so the re-emit
cannot raise that cause again.

``INTR_RETRIGGER`` is absent on Turing falcons and present from GA100 onward, so
the write is gated on the architecture.

Rearming PCI interrupt delivery
-------------------------------

Acknowledging the GIN state is not enough. A message-signaled interrupt is
delivered once per edge, and the PCI side delivers no further interrupt until the
CPU rearms it. Which operation does that depends on the GPU family and on the
interrupt type Linux granted:

==================  =====  ===========================================
Architecture        Type   Rearm operation
==================  =====  ===========================================
Turing / Ampere     MSI    write the configuration-mirror EOI register
Ada                 MSI    write the configuration-mirror EOI register
Hopper / Blackwell  MSI    clear then set the armed TOP enables
Any                 MSI-X  clear then set the handler's own TOP enable
==================  =====  ===========================================

The MSI forms cover every armed subtree, because one message serves all of them.
The MSI-X form covers one subtree, because each armed subtree has its own table
entry and its own handler.

INTx is level-triggered and needs no rearm write. nova-core does not allocate it,
so it never reaches a handler.

This rearm is owed once per delivered interrupt, on every path that services one,
and is separate from the TOP restore at the end of a full tree walk even though
one of its forms writes the same registers. The walk clears TOP_EN on entry so
that it can read and acknowledge without new interrupts arriving, and sets it
again on exit. A handler that skips the rearm receives no further interrupts at
all.

Servicing an interrupt
======================

nova-core services the tree in one of two ways, depending on which code handles
the interrupt.

The GSP event handler is nova-core's only production interrupt handler so far. It
knows that one specific vector fired, the GSP event, so it leaves its subtree
armed and reads and acknowledges only its own leaf bit, touching a single leaf
per interrupt.

The startup drain walks the whole tree instead, because it must clear whatever is
pending across every subtree rather than one known vector. It disarms the
subtrees, acknowledges every pending leaf, then rearms.

It reads every implemented leaf rather than descending from TOP. Boot latches
vectors while they are still masked, and those bits do not appear in TOP, so a
TOP-driven walk would skip exactly the state the drain exists to clear.

The two paths as register operations::

    Full tree walk (the one-time startup drain):
        write TOP_EN_CLEAR = armed_mask      disarm; stop new interrupts
        for each implemented subtree N, for i in {2N, 2N+1}:
            mask = read LEAF[i]              pending vectors in this leaf
            write LEAF[i] = mask             ack (write-1-to-clear)
        write TOP_EN_SET = armed_mask        restore TOP_EN

    Notification, subtree stays armed (the GSP event handler, and the
    self-test, which deliberately mirrors it):
        mask = read LEAF[gsp_leaf]           is our vector's bit set?
        write LEAF[gsp_leaf] = GSP_BIT       ack only our bit
        rearm PCI interrupt delivery         see "Rearming PCI interrupt
                                             delivery"

Two rules for the full walk:

* Acknowledge every pending leaf bit, including bits nova-core does not handle.
  An unacknowledged bit holds its subtree in the pending state, and restoring
  TOP_EN over it produces a delivery edge straight away. The walk writes back
  the whole mask it read.
* Restore TOP_EN only after acknowledging every pending leaf. Otherwise a
  still-set bit raises the interrupt again while the walk is still running.

The notification path is safe because a vector that is still pending raises no
second interrupt, and the next edge raises a new one, so nothing is lost. Both
paths still owe the PCI rearm for the interrupt they serviced.

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

The tree is the same on every supported GPU except for its size, and there are
only two sizes, split at Hopper:

===================  ======  ========  ============
GPUs                 Leaves  Subtrees  subtree_mask
===================  ======  ========  ============
Turing, Ampere, Ada  8       4         ``0x0f``
Hopper and later     16      8         ``0xff``
===================  ======  ========  ============

Only the lower eight leaves exist before Hopper, so TOP bits 4 through 31 read
zero there. Hopper and later have 16 leaves, though sources do not populate all
of them.

``subtree_mask`` is the set of subtrees the architecture implements. It bounds
which TOP bits mean anything, and is not the set nova-core arms: nova-core arms
only the subtrees it services, per the armed-subtree invariant. The startup
drain still reads every implemented leaf, because a vector that latched while
masked is invisible in TOP and can sit in any leaf.

The HAL provides the leaf count. The subtree count (leaves / 2) and the
``subtree_mask`` derive from it. If nova-core later services the stall vector
range, that range differs by architecture and would be added to the HAL.

Multi-die parts
===============

On multi-die parts the controller is replicated per die, with an aggregation
level above the per-die TOP registers. nova-core supports single-die parts and
services one PF CPU tree. It does not drive the aggregation level.

The GSP event
=============

The GSP signals the CPU by raising SWGEN0, one of the software-generated
interrupt outputs of the GSP microcontroller (a "falcon" in NVIDIA hardware).
When the GSP has output for the CPU (log records, error records, and other
events), it writes the messages into the GSP-to-CPU queue in shared memory and
raises SWGEN0. SWGEN0 is routed through a GIN vector, so it reaches the CPU as a
PCI interrupt::

    GSP writes messages into the GSP-to-CPU queue
    GSP raises SWGEN0
    GIN sets the GSP leaf bit; the subtree becomes pending
    PCI interrupt -> Linux IRQ -> nova-core hard IRQ handler:
        read the GSP leaf bit and acknowledge it (subtree stays armed)
        read the GSP falcon IRQ status:
            SWGEN0 set   -> wake the threaded handler
            SWGEN0 clear -> report the cause (a halt or other fatal cause on the
                            same vector), then mask and clear it at the falcon
        retrigger the falcon
        rearm PCI interrupt delivery
    threaded handler: take the command-queue lock and drain the GSP-to-CPU
        queue, routing each message

The interrupt is only the trigger to drain the queue. The drain works the same
way whether a poll or an interrupt starts it, so the code is shared (see
"Draining and classifying the GSP-to-CPU queue").

If the drain fails, the queue cannot advance past the message it could not parse,
so every later notification would repeat the same failure. The threaded handler
masks the GSP vector before reporting the failure, which leaves the queue
unserviced until the device is reset.

Enabling the GSP event
----------------------

SWGEN0 is a latch, and the GSP drives no new edge into the tree while it stays
set. GSP boot consumes its notifications by polling the queue, which leaves the
latch set and leaves stale state in the tree, so the handoff from polling to
interrupts has a required order::

    register the threaded handler       nothing is enabled yet
    mask every implemented leaf         drop enables left by boot or by a
                                        driver that ran before this one
    clear the SWGEN0 latch              so the next assertion makes an edge
    drain the tree (full walk)          clear stale GIN state from boot
    retrigger the falcon                re-emit a cause the drain cleared
    enable the GSP vector at its leaf   deliveries become possible here
    drain the GSP-to-CPU queue          messages posted before the clear

Clearing the latch is what makes the first interrupt possible. Messages the GSP
posted before that clear produce no interrupt, which is why the queue drain
follows.

Masking every leaf first is required because a leaf enable that boot left set
reaches nova-core's handler as soon as its subtree is armed, and that handler
services one vector and has no way to service any other. Open RM clears all leaf
enables at the same point for the same reason.

The latch is cleared before the tree walk, not after. Draining first leaves the
GSP vector pending in the tree with no falcon cause behind it, so enabling the
vector delivers an interrupt immediately with an empty ``IRQSTAT``.

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
(``NV_GIN_CPU_INTR_*``) configures other functions and is not used by the CPU
path.

Type-state tree API
-------------------

Servicing a leaf has a required order: read its pending mask, then acknowledge
it. The code encodes the two stages as distinct types (``Idle`` and
``Pending``) so that acknowledging a leaf before reading it does not compile.
``Top`` carries no type state, because arming and unarming a subtree can happen
in any order.

The types order the calls on a single handle. They are not a lock and they do
not coordinate the tree as a whole. Nothing stops two walks from running against
the tree at once. nova-core does not run concurrent walks: the GSP event handler
touches only its own leaf and never walks the tree, and the only whole-tree
walk, the startup drain, runs once during probe.

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
needs a real GPU and a working PCI interrupt path. It is gated by
``CONFIG_NOVA_CORE_IRQ_SELFTEST``, runs before GSP boot (so it never touches GSP
interrupt state), and fails probe if a delivery does not arrive.

The vector encoding, the subtree and leaf arithmetic, and the per-architecture
rearm policy have no hardware dependency, and KUnit tests cover them without a
GPU.

The test drives ``LEAF_TRIGGER``, a hardware register that every supported part
implements. Writing a vector number to it latches that vector exactly as its
engine would, after which the vector takes the ordinary path to the CPU under
the ordinary enables.

Vector 129, at leaf 4 bit 1, is the vector the test drives. It registers a
handler for that vector and triggers it twice, waiting for the first delivery
before triggering the second. Its handler deliberately mirrors the notification path: it
acknowledges only its own leaf bit and rearms PCI interrupt delivery, rather than
walking the tree. Two properties follow from that. The two interrupts cannot
coalesce into one, because the second is triggered only after the first handler
has finished. And a handler that fails to rearm times out on the second delivery
instead of passing, which a single-delivery test using a full tree walk cannot
detect, because the walk's own TOP_EN restore produces an edge by itself.

The test borrows the vector probe allocated for the GSP event rather than
allocating its own, so the doorbell vector has to lie in the same subtree as the
GSP event vector. A build-time assertion checks that, so moving either vector to
another subtree fails the build instead of silently misrouting the test.

This exercises the interrupt path from the GPU to the handler without GSP
firmware, which is useful when bringing up PCI, MSI, MSI-X, and passthrough
setups. Under MSI-X a pass also shows that the per-subtree table entry routing
works, since the delivery arrives on the entry belonging to the armed subtree.

Virtualization
==============

The per-function trees, the GFID routing, and the central ``NV_GIN`` aperture
support virtualization: each VF gets its own tree, and the PF or firmware routes
an engine's interrupt to the right function. MIG (multi-instance GPU)
partitioning adds more structure. nova-core services only the PF CPU tree, and
implements no VF tree management, GFID routing, or MIG support.

References
==========

* nova-core source: the register definitions in ``regs.rs``, the interrupt HAL
  and tree API in the ``irq`` module, and the GSP command queue in the ``gsp``
  module.
