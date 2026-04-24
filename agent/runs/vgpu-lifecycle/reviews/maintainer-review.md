# Maintainer Review

## Verdict
PASS

## Summary

Round 3 (final) re-review of 12 vGPU lifecycle patches. All 7 previous findings from rounds 1 and 2 have been correctly fixed — each fix is squashed into its introducing commit with no fixup artifacts. The series is clean: proper commit hygiene, correct logical separation, no fwctl.rs contamination, and no new issues introduced by the fixes. 867 lines added across 7 files, building a well-structured vGPU lifecycle from VRAM allocation through mock bootload verification.

## Comment Status
| # | Comment (short) | Status | Evidence (function / hunk) |
|---|-----------------|--------|----------------------------|
| — | No maintainer comments | — | `comments: []` in manifest |

## Previous Findings — Fix Verification

| # | Round | Original Severity | Issue | Fix Status | Evidence |
|---|-------|-------------------|-------|------------|----------|
| 1 | R1 | High | `mock_bootload` fails on Blackwell (V3 MMU) — `bar_user` is `None` | **FIXED** | `bar_user.is_none()` guard at top of `Gpu::mock_bootload` in commit `1b5732876445` (12/12). Returns `Ok(())` with `dev_dbg!`. Fix is in the correct commit (where `mock_bootload` is introduced). |
| 2 | R1 | Medium | `alloc_vram` ignores alignment parameter | **FIXED** | `_align` renamed to `align`, wired through `Alignment::new_checked(align_val).ok_or(EINVAL)?` into `alloc_blocks` in commit `59948d871733` (01/12). Fix is in the correct commit (where `alloc_vram` is introduced). |
| 3 | R1 | Low | Dead code `query_vmmu_segment_size` | **FIXED** | Method and `CMD_GET_VMMU_SEGMENT_SIZE` constant removed from `vgpu.rs`; VMMU query moved to `gsp/boot.rs` with local constant. Commit `c0e3f3289f53` (11/12). |
| 4 | R1 | Low | Raw magic number `0x2080_017e` in `boot.rs` | **FIXED** | Named constant `const CMD_GET_VMMU_SEGMENT_SIZE: u32 = 0x2080_017e;` defined locally in `gsp/boot.rs` at point of use. |
| 5 | R2 | Low | Commit 02 missing `gpu:` prefix | **FIXED** | Subject is now `gpu: nova-core: persist BAR1 mapping in Gpu struct`. Commit `d0fd75a5c4c0`. |
| 6 | R2 | Low | Commit 03 trailing period | **FIXED** | Subject is now `gpu: nova-core: vgpu: add vGPU preludes` (no period). Commit `fe9bb76115b0`. |
| 7 | R2 | Low | Unrelated fwctl.rs change in commit 12 | **FIXED** | `git diff 9c8fde7ecfcf..HEAD -- drivers/gpu/nova-core/fwctl.rs` returns empty. Commit 12 `--stat` shows only `driver.rs`, `gpu.rs`, `vgpu.rs`. |

## Commit Hygiene Verification

| Check | Result |
|-------|--------|
| No fixup/squash/wip subject lines | Clean — `git log --oneline` shows no artifacts |
| Each fix in correct introducing commit | Verified — fix #1 in 12/12, fix #2 in 01/12, fixes #3–4 in 11/12 |
| Subject lines ≤ 75 chars | All pass (max 60 chars, excluding hash) |
| Imperative mood | All use "add", "persist", "wire", "query", "bootstrap" |
| `gpu: nova-core:` prefix | All 12 commits |
| Unique subjects | All 12 are distinct |
| Body ≤ 75 columns | All pass |
| Signed-off-by present | All 12 commits |
| Single logical change per commit | All pass — each commit touches only related files |
| fwctl.rs clean | Zero diff from baseline `9c8fde7ecfcf` |

## Findings

### Critical / High
(none)

### Medium
(none)

### Low
(none)

## False Positives Eliminated

1. **`ctrl_buff_offset` left at zero in `BootloadParams`** — The ctrl buffer sits at offset 0 within the management heap. GSP computes the absolute address as `plugin_heap_memory_phys_addr + ctrl_buff_offset`, so 0 is correct.

2. **Stack buffer alignment in `build_engine_bitmap`** — The `[u8; ...]` stack array is cast to `&DeviceInfoTableParams` (requires alignment). On x86_64/aarch64, stack alignment guarantees (16-byte minimum) prevent any practical issue. Common kernel Rust pattern.

3. **`build_engine_bitmap` infinite loop** — If GSP returns `b_more != 0` indefinitely, the loop would not terminate. However, `send_vgpu_command` can fail (timeout, RPC error), which breaks the loop via `?`. The RM driver uses the same GSP trust model.

4. **Mock instance resource leak after `mock_bootload`** — The mock vGPU instance remains in `self.vgpu.instances` for the driver's lifetime. Resource leaks in test/mock code are acceptable unless they crash the system.

5. **`create_instance` error paths leaking VRAM** — Traced all 3 failure paths: (a) `alloc_guest_fb` fails: chids released, no VRAM allocated. (b) `alloc_plugin_heap` fails: chids released, `fbmem` dropped by Rust ownership. (c) `bootload_plugin` fails: chids released, `instance` (owning both VramBlocks) dropped. No leak.

6. **`prev_pow2` overflow/correctness** — x=0 handled by early return; for x≥1, `leading_zeros()` ≤ 31, `1u32 << (31 - leading_zeros())` fits in u32. Correct for all u32 inputs.

7. **VMMU alignment underflow** — Guarded by `if seg > 0`. VMMU segment sizes (2–64 MiB) are far smaller than usable VRAM (GiB range). `aligned >= vram_start` always holds. Cannot prove underflow.

8. **`compute_fb_size` division by zero** — Only reachable when `ecc_enabled == true`, which is never set (`GspConfig.ecc_enabled` initialized to `false`, never modified). The only registered type (L40-1Q) has `max_instance = 32`. Cannot prove reachable.

9. **Redundant `bar_user.as_mut().ok_or(ENODEV)?` after `is_none()` guard** — In `mock_bootload`, `this.bar_user.as_mut().ok_or(ENODEV)?` appears after the early-return guard. Since `this` is the same object (via `get_unchecked_mut`), `bar_user` is guaranteed `Some`. Defensive but correct.

10. **VMMU query behavioral change in boot.rs vs old vgpu.rs** — The boot.rs implementation silently skips non-zero status (leaving `vmmu_segment_size` at 0), while the old `query_vmmu_segment_size` in vgpu.rs would propagate EIO. This is correct: on query failure, no VMMU alignment is applied (`if seg > 0` guard in gpu.rs), and `alloc_guest_fb` falls back to PAGE_SIZE alignment via `.max(4096)`. Graceful degradation, not a regression.

## Risks

None. All previous risks have been resolved.
