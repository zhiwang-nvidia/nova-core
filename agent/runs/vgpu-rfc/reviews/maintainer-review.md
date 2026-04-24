# Maintainer Review

## Verdict
PASS

## Summary
Re-review after developer addressed all 4 findings from the previous round. All fixes are correctly applied: (1) PRC query is now guarded with `if ctx.vgpu_requested`, preventing unnecessary FSP messages on Hopper; (2) `vf_partition_count` is reset to 0 when PRC disables vGPU, ensuring consistent firmware metadata; (3) redundant `wait_secure_boot` removed from `boot_via_fsp()`; (4) `bar` field changed to `pub(crate)` for consistency. No new issues introduced by the fixes.

## Comment Status
| # | Comment (short) | Status | Evidence (function / hunk) |
|---|-----------------|--------|----------------------------|
| (none) | No maintainer comments | N/A | N/A |

## Previous Findings — Resolution Status

### Finding 1 (High): PRC query guarded with `if ctx.vgpu_requested`
**Status: RESOLVED**

The PRC query `Fsp::read_vgpu_mode()` is now inside `if ctx.vgpu_requested` (boot.rs:434). On Hopper, `supports_vgpu()` returns false, so `Vgpu::new()` sets `vgpu_requested = false`, and the PRC message is never sent. The FSP falcon is still created and `wait_secure_boot` still runs (both are needed for FSP boot regardless of vGPU), but the PRC knob read is correctly skipped.

```
if !arch.uses_sec2_boot() {
    let fsp_falcon = Falcon::<FspEngine>::new(ctx.dev(), chipset)?;
    Fsp::wait_secure_boot(ctx.dev(), bar, arch)?;
    if ctx.vgpu_requested {                                          // ← guard added
        let vgpu_mode = Fsp::read_vgpu_mode(ctx.dev(), bar, &fsp_falcon)?;
        ctx.vgpu_requested &= vgpu_mode == VgpuMode::Enabled;
        ...
    }
    ctx.fsp_falcon = Some(fsp_falcon);
}
```

### Finding 2 (Medium): `vf_partition_count` reset to 0 when PRC disables vGPU
**Status: RESOLVED**

Immediately after PRC clears `vgpu_requested`, `vf_partition_count` is reset to 0 (boot.rs:437-439). This ensures `FbLayout::new()` at boot.rs:472 receives the correct count, producing a correctly-sized WPR2 heap and a zero `gspFwHeapVfPartitionCount` in firmware metadata.

```
ctx.vgpu_requested &= vgpu_mode == VgpuMode::Enabled;
if !ctx.vgpu_requested {
    ctx.vf_partition_count = 0;                                      // ← reset added
}
```

### Finding 3 (Low): Redundant `wait_secure_boot` removed from `boot_via_fsp()`
**Status: RESOLVED**

The diff confirms `boot_via_fsp()` (boot.rs:345-375) no longer contains `Fsp::wait_secure_boot()`. The call was moved to `boot()` (boot.rs:433), which runs before `boot_via_fsp()` is invoked. No duplicate call exists.

### Finding 4 (Low): `pub bar` changed to `pub(crate) bar`
**Status: RESOLVED**

The `bar` field on `Gpu` is now `pub(crate) bar: Arc<Devres<Bar0>>` (gpu.rs:316), consistent with the adjacent `pub(crate) gsp: Gsp` (gpu.rs:329).

## New Findings
None.

## False Positives Eliminated

1. **`sriov_get_totalvfs` negative return value**: (Carried from previous review) `pci_sriov_get_totalvfs()` reads from `sriov->driver_max_VFs` (u16) or `sriov->total_VFs` (u16), both non-negative. The `#ifndef CONFIG_PCI_IOV` fallback returns 0. The u16 conversion is always safe.

2. **`ctx.fsp_falcon.as_ref().ok_or(ENODEV)` can panic**: (Carried) The `ok_or(ENODEV)` on the FSP path cannot fail. The guard `!arch.uses_sec2_boot()` is the exact complement of `if arch.uses_sec2_boot()`. If we reach the FSP else-branch, we executed the block that sets `ctx.fsp_falcon = Some(fsp_falcon)`.

3. **`vgpu_enabled` field is never read**: (Carried) `Vgpu::vgpu_enabled` is set after boot but never consumed in this series. It captures state for future use (e.g., vGPU manager VFIO driver). Does not affect correctness.

4. **`AsBytes` soundness for `GspVfInfo`**: (Carried) `GspVfInfo::new()` uses `..Zeroable::zeroed()` to zero-fill all padding. The `AsBytes` impl is sound.

5. **Scrubber only on SEC2 boot path, not FSP**: (Carried) On FSP architectures, FSP/FMC manages WPR and scrubbing internally. Noted as a risk rather than a bug.

6. **`NvdmPayloadCot` comment wording**: (Carried) "NVIDIA Vendor Defined Message" is correct per MCTP spec for message type 0x7e.

7. **Patch series ordering**: (Carried) `GpuacctPerfmonUtilSamples` and `Display for MsgFunction` bundling is acceptable.

## Risks

1. **Scrubber on FSP path**: If a future FSP architecture requires host-driven FB scrubbing, the current code has no scrubber invocation in `boot_via_fsp()`. The assumption that FSP/FMC handles scrubbing should be documented.

2. **PRC protocol error handling**: `read_vgpu_mode()` treats all FSP errors as fatal (returns `Err`). If a future architecture supports FSP but not the vGPU PRC knob, this would need a fallback path. The new `if ctx.vgpu_requested` guard mitigates this for Hopper, but the concern remains for any future FSP architecture that supports vGPU but lacks the PRC knob.
