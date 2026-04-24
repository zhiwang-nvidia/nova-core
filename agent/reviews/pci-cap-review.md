# Review: `rust: pci: add extended capability and SR-IOV support`

**Commit**: `33291e21708d rust: pci: add extended capability and SR-IOV support`
**Verdict**: **REJECT**

## Summary

The patch adds `ExtCapability<T>` as an `Io` implementor for PCI extended
capability regions, an `ExtCapId` enum for type-safe capability discovery, an
`ExtSriovRegs` struct matching the PCIe SR-IOV register layout, and a C helper
for `PCI_EXT_CAP_NEXT()`. The overall design is sound: the delegation of
`IoCapable` to the underlying `ConfigSpace` is correct, the SR-IOV register
layout matches the PCIe specification (`PCI_EXT_CAP_SRIOV_SIZEOF = 0x40`), and
the `read_vf_bar64` boundary check is correct.

One medium-severity arithmetic bug was found in the capability size
calculation, and one low-severity coding style issue per the Rust subsystem
conventions.

---

## Findings

### Medium

#### M-1: Integer underflow in `calculate_ext_cap_size` when capability chain is not in ascending address order

**File**: `rust/kernel/pci/cap.rs`, lines 170–181

**Code**:

```rust
fn calculate_ext_cap_size(&self, offset: usize) -> usize {
    let header = self.try_read32(offset).unwrap_or(0);
    let next_ptr = unsafe { bindings::pci_ext_cap_next(header) } as usize;

    if next_ptr == 0 {
        KnownSize::size(self.as_ptr()) - offset
    } else {
        next_ptr - offset   // <--- underflow when next_ptr < offset
    }
}
```

**Problem**: The subtraction `next_ptr - offset` assumes the next capability in
the linked list is at a higher address than the current one. The PCIe
specification does not mandate that extended capabilities are linked in
ascending address order. If a device has an out-of-order capability chain
(e.g., capability at 0x300 with next pointer 0x200), this subtraction wraps
around on `usize`.

**Concrete scenario**:

```
Extended capability chain:
  0x100: Cap A  (next → 0x300)
  0x300: SR-IOV (next → 0x200)
  0x200: Cap C  (next → 0)

pci_find_ext_capability(dev, SRIOV) returns 0x300.
calculate_ext_cap_size(0x300):
  header at 0x300 → next_ptr = 0x200
  next_ptr - offset = 0x200 - 0x300 → underflow
```

**Impact**:
- With overflow checks enabled (`CONFIG_RUST_OVERFLOW_CHECKS`): **kernel panic**.
- Without overflow checks: the size wraps to a very large value. `cast_sized()`
  erroneously passes for any type size, defeating the bounds validation that
  the `ExtCapability` abstraction is designed to provide.

**Suggested fix**:

```rust
fn calculate_ext_cap_size(&self, offset: usize) -> usize {
    let header = self.try_read32(offset).unwrap_or(0);
    let next_ptr = unsafe { bindings::pci_ext_cap_next(header) } as usize;

    if next_ptr > offset {
        next_ptr - offset
    } else {
        // Last cap in chain, or chain is not in address order.
        KnownSize::size(self.as_ptr()) - offset
    }
}
```

**TASK POSITIVE.1 verification**:

1. *Path executes*: `ExtSriovCapability::find()` → `config.find_ext_capability()` → `make_ext_capability()` → `calculate_ext_cap_size()`. Always enabled, no CONFIG guard.
2. *Structurally possible*: PCIe spec does not mandate ascending address order in the capability linked list. Hardware with out-of-order chains exists (particularly in virtualized environments).
3. *Full context*: `calculate_ext_cap_size` is only called from `make_ext_capability`; no additional guards on the result.
4. *Actually wrong*: Arithmetic underflow producing a bogus size is always wrong. No commit message or comment documents an assumption about capability ordering.
5. *Commit message*: No mention of ordering assumptions.
6. *Conditions possible*: Single condition (out-of-order chain), achievable with real hardware.
7. *Hallucination check*: Verified at `cap.rs:179`: `next_ptr - offset` with no guard.
8. *Future fixes*: Single commit, no git range.
9. *Implementation vs docs*: CAST comment addresses value range but not ordering relationship.
10. *Debate*:
    - Author: "In practice, vendors lay out capabilities in ascending order."
    - Reviewer: "The spec doesn't guarantee this. Debug builds panic. The fix is one line. Cannot refute with code evidence."

---

### Low

#### L-1: Missing `// INVARIANT:` comments at `ExtCapability` construction sites

**File**: `rust/kernel/pci/cap.rs`

`ExtCapability` has a documented `# Invariants` section:

```rust
/// # Invariants
///
/// `ptr` is within the device's extended configuration space at a valid
/// capability. For sized `T`, the region is at least `size_of::<T>()` bytes.
```

Per the Rust subsystem coding guidelines (`Documentation/rust/coding-guidelines.rst`
and `agent/roles/reviewer-refs/subsystems/rust.md`): "When a struct with an
`# Invariants` documentation section is constructed, the code should have an
`// INVARIANT:` comment explaining why the invariants are satisfied."

The struct is constructed in two places without such comments:

1. `make_ext_capability` (line 167):
   ```rust
   ExtCapability { config: self, ptr }
   ```
2. `cast_sized` (lines 121–124):
   ```rust
   Ok(ExtCapability {
       config: self.config,
       ptr: core::ptr::without_provenance_mut(self.offset()),
   })
   ```

**Suggested fix**: Add `// INVARIANT:` comments at each construction site
explaining why `ptr` is within extended config space and why the region is
large enough.

---

## False Positives Eliminated

1. **`read_vf_bar64` does not enforce even BAR index for 64-bit BARs**: The PCIe
   spec requires 64-bit BARs to start at even indices, but this is a hardware
   layout constraint, not a software invariant. The function is a utility; the
   caller knows which BARs are 64-bit. Misuse reads wrong data but doesn't
   corrupt memory. Not a concrete bug.

2. **`read_vf_bar64` boundary check `>= 5` appears off-by-one**: Verified
   correct. With `bar_index = 4`, both `vf_bar[4]` and `vf_bar[5]` are within
   the `[u32; 6]` array. The check ensures both the low and high reads are
   in bounds.

3. **`read_vf_bar64` does not validate BAR type bit**: Standard pattern in
   kernel PCI code; the caller is responsible for knowing BAR types. Not a
   regression.

4. **`ExtSriovRegs` missing `FromBytes`/`AsBytes` implementations**: Not
   needed. The `io_read!`/`io_write!` macros use pointer projections to
   individual primitive fields, not whole-struct reads requiring these traits.

5. **`ExtCapability` is not `Send`/`Sync`**: Appropriate for the use pattern.
   The struct borrows `ConfigSpace` and is used within a single function scope.
   The raw pointer makes it `!Send`/`!Sync` by default, which is conservative
   and correct.

6. **SAFETY comment "Pure bit manipulation, no preconditions" on
   `pci_ext_cap_next` call**: Verified correct. `PCI_EXT_CAP_NEXT(header)` =
   `((header >> 20) & 0xffc)` is indeed pure bit manipulation. The `unsafe` is
   only required because it's an FFI call.

7. **`IoCapable` delegation SAFETY comments**: The comments state "The caller
   guarantees `address` is within bounds of this capability, which is within
   the config space." This correctly documents the safety contract: bounds
   within capability ⊆ bounds within config space, so delegating to
   `ConfigSpace::io_read`/`io_write` is sound.

---

## Risks

- **M-1 in debug builds**: If a driver encounters a device with out-of-order
  extended capabilities and `CONFIG_RUST_OVERFLOW_CHECKS` is enabled, the
  kernel will panic in `calculate_ext_cap_size`. This is the most immediate
  risk.
- **SR-IOV in virtualized environments**: SR-IOV is heavily used in
  virtualization. Virtual PCI devices may have unusual capability layouts
  depending on the hypervisor implementation, increasing exposure to M-1.
