.. SPDX-License-Identifier: (GPL-2.0+ OR MIT)

==============================
Firmware Parameters TLV Format
==============================

This document specifies the binary format of the firmware parameters files
that ship in linux-firmware for NVIDIA GSP-based GPUs and are loaded by the
nova-core driver via ``request_firmware()``. The format is a small
Type-Length-Value (TLV) container that pairs a raw firmware blob with the
metadata required to load and verify it (signature, version, build ID, heap
parameters, and similar). Files using this format have the magic bytes
``"FWPM"`` (Firmware Parameters) at offset 0 and conventionally use the
``-params.bin`` suffix (for example ``gsp-params.bin``, ``fmc-params.bin``).

The format exists so that container parsing (currently ELF) does not have to
happen in the kernel. The userspace firmware extraction script reads the
upstream ELF blobs once, splits them into a raw firmware blob plus a
firmware parameters file, and ships both to linux-firmware. The kernel
driver then loads the parameters file, walks its TLV entries, and
optionally issues a second ``request_firmware()`` for the referenced
firmware blob.

Although the format originated for nova-core, it is not nova-core specific.
It is intended to be shared between nova-core and any other consumer of the
same linux-firmware files (for example NVIDIA's open-source GPU kernel
modules).

Design properties
=================

The format has a few deliberate properties that drive everything else:

- **Unknown tags are silently skipped.** A reader iterates the entries it
  understands and ignores the rest. This gives forward compatibility
  without a version field in the header.

- **Missing expected tags are detectable after iteration.** A reader
  collects the tags it cares about during the walk and checks at the end
  whether the required ones were present. This gives backward compatibility:
  a new reader can run against an old file.

- **Entry order is not significant.** The producer may emit entries in any
  order. Readers must not rely on order.

- **Values are 4-byte aligned on disk.** The length field stores the actual
  value size; padding is implicit and not counted in the length.

- **The format has no recursion and no nesting.** It is a flat sequence of
  records, structurally identical to iterating an array of variable-sized
  entries. The full reader is on the order of twenty lines of code.

Structured tags (where the value is a packed struct rather than a single
scalar or byte string) are extended by appending fields and growing the
length, with old readers ignoring trailing bytes they do not understand.
This is described in detail in the section on the ``HEAP`` tag below.

Wire format
===========

Overall file layout::

    Byte offset
    0x00  +------------------------------------------------------+
          |  magic: u32 = "FWPM" (0x4657_504d, little-endian)    |
          +------------------------------------------------------+
    0x04  |  tag: u32       |  len: u32                          |  entry 0
    0x0C  |  value: [u8; len]  (padded to 4-byte alignment)      |
          +------------------------------------------------------+
          |  tag: u32       |  len: u32                          |  entry 1
          |  value: [u8; len]  (padded to 4-byte alignment)      |
          +------------------------------------------------------+
          |  ...repeat until EOF...                              |
          +------------------------------------------------------+

Encoding rules:

- The magic value is ``0x4657_504d`` (the ASCII string ``"FWPM"`` interpreted
  as a little-endian u32). Files that do not start with this magic are not
  firmware parameters files and must be rejected.

- All multi-byte integers (magic, tag, length, and any integer-typed values)
  are stored little-endian.

- ``tag`` and ``len`` are both u32. The tag is conventionally the
  little-endian u32 reinterpretation of a four-character ASCII identifier
  (see the tag table below), but readers must treat tags as opaque 32-bit
  values and dispatch on numeric equality.

- ``len`` is the value size in bytes. Values are padded with zero bytes on
  disk to the next 4-byte boundary. Padding bytes are not included in
  ``len``.

- The file ends at the byte following the last entry's padding. There is
  no explicit terminator and no entry count.

- Entries may appear in any order. A reader must walk all entries before
  concluding that a tag is absent.

- Readers must silently skip tags they do not recognise and continue
  iterating.

Tag assignments
===============

The following tags are currently defined. ASCII names are shown as four
characters; the corresponding hexadecimal value is the little-endian u32
reinterpretation of those characters.

.. list-table::
   :header-rows: 1
   :widths: 12 18 18 52

   * - Tag
     - Hex value
     - Value type
     - Description
   * - ``FILE``
     - ``0x4649_4c45``
     - string (UTF-8)
     - Relative filename of a separate firmware blob. The driver loads it
       with a second ``request_firmware()`` call, prepending the appropriate
       directory.
   * - ``BLOB``
     - ``0x424c_4f42``
     - bytes
     - Embedded firmware image. The entire firmware blob is contained in
       this entry's value.
   * - ``SIZE``
     - ``0x5349_5a45``
     - u64
     - Size in bytes of the firmware blob referenced by ``FILE``. Allows
       the driver to call ``request_firmware_into_buf()`` with a
       pre-allocated buffer.
   * - ``SIGN``
     - ``0x5349_474e``
     - bytes
     - GSP firmware signature. Appears in ``gsp-params.bin``.
   * - ``VERS``
     - ``0x5645_5253``
     - string (UTF-8)
     - Firmware version string (for example ``"570.144"``).
   * - ``BLID``
     - ``0x424c_4944``
     - bytes
     - Build ID (raw GNU build-id descriptor bytes, up to 32 bytes).
   * - ``HEAP``
     - ``0x4845_4150``
     - struct
     - GSP heap parameters. See `HEAP tag value layout`_ below.
   * - ``HASH``
     - ``0x4841_5348``
     - bytes
     - FMC hash (48 bytes). Appears in ``fmc-params.bin``.
   * - ``PKEY``
     - ``0x504b_4559``
     - bytes
     - FMC public key (up to 384 bytes). Appears in ``fmc-params.bin``.
   * - ``FSIG``
     - ``0x4653_4947``
     - bytes
     - FMC signature (up to 384 bytes). Appears in ``fmc-params.bin``.

Constraints between tags:

- ``FILE`` and ``BLOB`` are mutually exclusive. A file containing both is
  malformed and must be rejected.

- ``SIZE`` is meaningful only when ``FILE`` is present. It is optional but
  recommended when ``FILE`` is used.

- ``SIGN`` and ``FSIG`` are distinct tags for distinct firmware components
  (GSP vs FMC) and never appear in the same file.

New tags may be added at any time. Old readers will skip them. No central
registry coordination is required, but the producer and consumer of a new
tag obviously need to agree on its meaning.Documentation/gpu/nova/core/firmware-params.rst

HEAP tag value layout
---------------------

The ``HEAP`` tag groups GSP heap parameters into a single packed struct
rather than allocating one tag per parameter. The parameters always travel
together (they are needed together or not at all), so combining them keeps
the entry count down and makes the relationship explicit.

Current layout (40 bytes, ``len = 40``)::

    offset  field                   type
    0x00    heap_size_base          u64
    0x08    heap_size_per_fb_gb     u64
    0x10    heap_size_per_vf        u64
    0x18    non_wpr_heap_size       u64
    0x20    pmu_reserved_size       u64

All fields are little-endian.

Extending the struct
~~~~~~~~~~~~~~~~~~~~

The struct is extended by appending fields and growing ``len``. For
example, adding a sixth u64 field would make ``len = 48``. Readers
distinguish layouts by inspecting ``len``:

- An old reader (built when only the five fields above existed) sees
  ``len >= 40``, reads its five fields, and ignores any trailing bytes.

- A new reader (which knows about a sixth field) checks ``len >= 48``
  before reading the sixth field, and falls back to a default if it is
  absent.

This pattern - a struct behind a single TLV tag, extended by appending
fields and growing ``len`` - is the recommended way to introduce any
firmware header that needs to evolve over time. Define the struct, place
it behind a tag, and hand the bytes through with no field-by-field
reassembly.

Concrete example: gsp-params.bin
================================

A ``gsp-params.bin`` file produced by the linux-firmware extraction script
for one chip might look like this::

    Byte offset
    0x00  +------------------------------------------------------+
          |  magic: u32 = "FWPM"                                 |
          +------------------------------------------------------+
    0x04  |  tag: u32 = "FILE"  |  len: u32 = 7                  |
    0x0C  |  value: "gsp.bin"   |  pad: 1 byte                   |
          +------------------------------------------------------+
    0x14  |  tag: u32 = "SIZE"  |  len: u32 = 8                  |
    0x1C  |  value: 137560064 (u64, size of gsp.bin)             |
          +------------------------------------------------------+
    0x24  |  tag: u32 = "SIGN"  |  len: u32 = 4096               |
    0x2C  |  value: [u8; 4096]  (firmware signature)             |
          +------------------------------------------------------+
          |  tag: u32 = "VERS"  |  len: u32 = 7                  |
          |  value: "570.144"   |  pad: 1 byte                   |
          +------------------------------------------------------+
          |  tag: u32 = "BLID"  |  len: u32 = 20                 |
          |  value: [u8; 20]  (build ID)                         |
          +------------------------------------------------------+
          |  tag: u32 = "HEAP"  |  len: u32 = 40                 |
          |  value:                                              |
          |    heap_size_base       (u64)                        |
          |    heap_size_per_fb_gb  (u64)                        |
          |    heap_size_per_vf     (u64)                        |
          |    non_wpr_heap_size    (u64)                        |
          |    pmu_reserved_size    (u64)                        |
          +------------------------------------------------------+

Note the 1-byte zero pad after the 7-byte ``"gsp.bin"`` and ``"570.144"``
strings: ``len`` is 7 in both cases, and the value is followed by one zero
byte to reach the next 4-byte boundary before the next entry header.

Load sequence
=============

A driver consuming a firmware parameters file performs the following
sequence::

    request_firmware("nvidia/<chip>/gsp/gsp-params.bin")
            |
            v
       Walk TLV entries
            |
            +-- found FILE("gsp.bin")
            |       |
            |       v
            |   request_firmware("nvidia/<chip>/gsp/gsp.bin")
            |       |
            |       v
            |   have image + params, proceed
            |
            +-- found BLOB([...])
            |       |
            |       v
            |   have image + params in one file, proceed
            |
            +-- found both FILE and BLOB
                    |
                    v
                  -EINVAL

When ``FILE`` is present, the value is a relative filename. The driver
prepends the appropriate firmware directory (for example
``nvidia/<chip>/gsp/``) to form the full path for the second
``request_firmware()`` call.

When ``SIZE`` is present alongside ``FILE``, the driver may use
``request_firmware_into_buf()`` with a pre-allocated buffer of the
indicated size, avoiding an internal allocation by the firmware loader.

Because the producer of the parameters file decides whether the firmware
image is referenced by ``FILE`` or embedded as ``BLOB``, the producer can
change that choice in a future firmware release without any driver
change. Large blobs are typically referenced via ``FILE``; small blobs may
be embedded via ``BLOB``.

ABI stability
=============

The format provides natural ABI evolution without a version field:

- New parameters are added by defining a new tag. Old drivers skip the
  unknown tag. New drivers act on it.

- Parameters are removed by ceasing to emit the tag. New drivers handle
  the missing tag gracefully (typically by using a default).

- Structured tags such as ``HEAP`` are evolved within their value by
  appending fields and growing ``len``, as described above.

If a future firmware release makes a change that cannot be expressed
within these rules (for example, an existing tag's value type is
redefined incompatibly), the producer should add a new file alongside
the existing one (for example ``gsp-params-v2.bin``) and the driver
should be updated to try the new file first. This is a last resort;
all changes that can be expressed via new tags or by extending an
existing struct should use those mechanisms instead.

For comparison, here is how the same evolution scenarios play out for
this format versus a plain versioned struct:

.. list-table::
   :header-rows: 1
   :widths: 24 38 38

   * - Property
     - Versioned struct
     - TLV
   * - Add a new parameter
     - New version, new struct
     - Add a new tag constant
   * - Remove a parameter
     - New version
     - Stop emitting the tag
   * - Reorder fields
     - Impossible without a new version
     - Order does not matter
   * - Old reader, new file
     - Must know every version layout
     - Skips unknown tags automatically
   * - New reader, old file
     - Must handle missing fields
     - Checks whether expected tag was present
   * - Version field needed
     - Yes
     - No
   * - Code size
     - One struct per version
     - One iterator, forever

Reference implementation
========================

The wire format above is normative. The Rust code below is informative;
it shows the canonical way to iterate a firmware parameters file from a
kernel driver. Equivalent code in any language is straightforward: read
and verify the magic, then loop reading a (tag, length) header and a
length-bytes value, advancing past the 4-byte-aligned padding.

Tag constants::

    mod fwpm_tags {
        pub const FILE: u32 = 0x4649_4c45;
        pub const BLOB: u32 = 0x424c_4f42;
        pub const SIZE: u32 = 0x5349_5a45;
        pub const SIGN: u32 = 0x5349_474e;
        pub const VERS: u32 = 0x5645_5253;
        pub const BLID: u32 = 0x424c_4944;
        pub const HEAP: u32 = 0x4845_4150;
        pub const HASH: u32 = 0x4841_5348;
        pub const PKEY: u32 = 0x504b_4559;
        pub const FSIG: u32 = 0x4653_4947;
    }

Iterator::

    const FWPM_MAGIC: u32 = 0x4657_504d;

    struct FwParams<'a>(&'a [u8]);

    impl<'a> FwParams<'a> {
        fn new(data: &'a [u8]) -> Option<Self> {
            let (magic, rest) = data.split_at_checked(4)?;
            if u32::from_le_bytes(magic.try_into().ok()?) != FWPM_MAGIC {
                return None;
            }
            Some(Self(rest))
        }
    }

    impl<'a> Iterator for FwParams<'a> {
        type Item = (u32, &'a [u8]);

        fn next(&mut self) -> Option<Self::Item> {
            let tag = u32::from_le_bytes(self.0.get(..4)?.try_into().ok()?);
            let len = u32::from_le_bytes(self.0.get(4..8)?.try_into().ok()?) as usize;
            let val = self.0.get(8..8 + len)?;
            let padded = (len + 3) & !3;
            self.0 = self.0.get(8 + padded..)?;
            Some((tag, val))
        }
    }

HEAP value struct::

    #[repr(C)]
    struct GspHeapParams {
        heap_size_base: u64,
        heap_size_per_fb_gb: u64,
        heap_size_per_vf: u64,
        non_wpr_heap_size: u64,
        pmu_reserved_size: u64,
    }

    // SAFETY: all bit patterns are valid for this type, and it doesn't use
    // interior mutability.
    unsafe impl FromBytes for GspHeapParams {}

A typical consumer pattern (load a parameters file, walk it, dispatch on
known tags, then fail if a required tag was absent)::

    let params_fw = request_firmware(dev, chipset, "gsp-params")?;
    let params = FwParams::new(params_fw.data()).ok_or(EINVAL)?;

    let mut fw_filename = None;
    let mut fw_signature = None;
    let mut fw_version = None;
    let mut build_id = None;
    let mut heap_params = None;

    for (tag, val) in params {
        match tag {
            fwpm_tags::FILE => fw_filename = core::str::from_utf8(val).ok(),
            fwpm_tags::SIGN => fw_signature = Some(val),
            fwpm_tags::VERS => fw_version = core::str::from_utf8(val).ok(),
            fwpm_tags::BLID => build_id = BuildId::from_raw(val),
            fwpm_tags::HEAP => {
                if val.len() >= core::mem::size_of::<GspHeapParams>() {
                    heap_params = GspHeapParams::from_bytes_copy(val);
                }
            }
            _ => {}
        }
    }

    let fw_name = fw_filename.ok_or(EINVAL)?;
    let gsp_fw = request_firmware(dev, chipset, fw_name)?;
