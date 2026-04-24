# Maintainer Review — Final Round

- **Baseline**: `906ed0995a0a`
- **HEAD**: `3647df287b9a` (31 commits)
- **Branch**: `nova-core-blackwell-v000-firmware-CL_37745303`
- **Date**: 2026-04-08
- **Scope**: Verify 3 prior findings fixed, check for regressions

## Prior Findings — Verification

### Critical #1: Placeholder zeros crash GpuBuddy (size==0)

**Status: FIXED ✓**

| Check | File:Line | Evidence |
|-------|-----------|----------|
| `buddy` field type | `mm.rs:39` | `buddy: Option<GpuBuddy>` |
| size==0 guard | `mm.rs:58-62` | `if buddy_params.size > 0 { Some(...) } else { None }` |
| accessor returns Result | `mm.rs:74-76` | `fn buddy(&self) -> Result<&GpuBuddy>` via `ok_or(ENOMEM)` |
| caller: vmm.rs | `vmm.rs:226` | `mm.buddy()?.alloc_blocks(...)` |
| caller: bar_user.rs (×3) | `bar_user.rs:234,311,326` | `mm.buddy()?.alloc_blocks(...)` |

The fix correctly avoids constructing a `GpuBuddy` with `size=0` (which
would panic in the buddy allocator). Downstream callers propagate the
`ENOMEM` error via `?` — no unwrap/expect paths exist.

### Critical #2: V3 MMU rejection for BarUser

**Status: FIXED ✓**

| Check | File:Line | Evidence |
|-------|-----------|----------|
| `bar_user` field type | `gpu.rs:317` | `bar_user: Option<BarUser>` |
| V2-only construction | `gpu.rs:411-419` | `if mmu_version == MmuVersion::V2 { Some(...) } else { None }` |
| V3 comment | `gpu.rs:412` | `// TODO: Extend BarUser/Vmm to support MMU V3` |
| self-test guard | `gpu.rs:482` | `if self.bar_user.is_some() {` |
| Vmm rejects V3 | `vmm.rs:146-148` | `if mmu_version != MmuVersion::V2 { return Err(ENOTSUPP); }` |

The fix provides two layers of protection: (1) `BarUser` is never
constructed on V3/Hopper+, and (2) even if somehow constructed, `Vmm::new`
independently rejects non-V2 MMU versions. The self-test gate ensures
BAR1 tests are skipped when `bar_user` is `None`.

### Medium #3: `#[allow(dead_code)]` → `#[expect(dead_code)]`

**Status: FIXED ✓**

| Check | File:Line | Evidence |
|-------|-----------|----------|
| `#[expect(dead_code)]` on impl | `gsp/fw/commands.rs:139` | Attribute on `impl GspStaticConfigInfo` |
| struct itself is clean | `gsp/fw/commands.rs:136` | No lint attr on struct (type is referenced) |

The attribute was moved from the struct definition to the `impl` block,
which is more precise: the struct type is used (as field in
`GspStaticConfigInfo`), only the methods are currently dead code. This
matches the kernel Rust convention of preferring `#[expect]` over
`#[allow]` for lint suppression.

Note: one `#[allow(dead_code)]` remains in `bitfield.rs:156`, but this is
pre-existing (inside a macro definition, not part of this patch series).

## Regression Check

**No regressions found.**

Checked areas:

1. **Error propagation**: All `mm.buddy()?` call sites use `?` to propagate
   the `ENOMEM` error when buddy is `None`. No panicking unwrap paths.

2. **`Gsp::boot()` return type**: Changed from `Result` to
   `Result<GetGspStaticInfoReply>` (`gsp/boot.rs`). The caller in `gpu.rs`
   correctly captures `let info = gsp.boot(...)?` and stores it in
   `gsp_static_info`.

3. **Self-test probe path**: `driver.rs:84` calls `gpu.run_selftests(pdev)?`
   with `?`. On V3 hardware, both PRAMIN self-tests (skipped for
   Hopper+) and BAR1 self-tests (skipped via `bar_user.is_some()` guard)
   are cleanly bypassed. Self-tests are also fully gated behind
   `CONFIG_NOVA_MM_SELFTESTS` with a no-op fallback (`gpu.rs:498-501`).

4. **Placeholder zeros**: `get_gsp_info()` in `gsp/commands.rs:210-215`
   returns `bar1_pde_base: 0, usable_fb_region: 0..0, total_fb_end: 0`.
   This is a known pre-existing state (TODO at `mm.rs:57-58`). The
   `Option<GpuBuddy>` fix (Finding #1) was specifically designed to handle
   this: `size=0` → `buddy=None` → downstream operations fail gracefully
   with `ENOMEM` rather than crashing.

5. **PRAMIN with `vram_region=0..0`**: When `total_fb_end=0`, PRAMIN
   receives an empty valid region. `compute_window()` will reject any
   access with `EINVAL`. This is correct defensive behavior — no accesses
   should occur until NVKV extraction provides real region values.

## Verdict

```
PASS
```

Zero findings. All three prior issues are properly resolved. The fixes
are defensive, well-structured, and consistent with kernel Rust
conventions. The placeholder-zero paths are explicitly documented with
TODO comments and will resolve once NVKV key extraction is implemented.
