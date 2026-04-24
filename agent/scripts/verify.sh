#!/bin/bash
# verify.sh — Run all build quality checks, output verify-result.json
#
# Usage: verify.sh [output.json] [num_patches]
#   output.json   — where to write result (default: stdout)
#   num_patches   — how many HEAD commits to feed checkpatch (default: 1)
#
# Exit: 0 = PASS, 1 = FAIL
set -uo pipefail

TREE=$(git rev-parse --show-toplevel)
cd "$TREE"

OUT="${1:-/dev/stdout}"
NP="${2:-1}"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

info() { printf '\n\e[36m=== [%s] %s ===\e[0m\n' "$1" "$2" >&2; }

VERDICT="PASS"
fail() { VERDICT="FAIL"; }

# ── 1. config-flags ──────────────────────────────────────────────
info "1/7" "config-flags"

NEED=(
  CONFIG_CC_IS_CLANG=y CONFIG_RUST=y CONFIG_KUNIT=y
  CONFIG_RUST_KERNEL_DOCTESTS=y CONFIG_DRM=y CONFIG_FWCTL=y
  CONFIG_NOVA_CORE=m
)
: > "$TMP/cfg_missing"
for f in "${NEED[@]}"; do
  grep -q "^${f}$" .config 2>/dev/null || echo "$f" >> "$TMP/cfg_missing"
done
[ -s "$TMP/cfg_missing" ] && fail
echo "$([ -s "$TMP/cfg_missing" ] && echo FAIL || echo PASS)" > "$TMP/cfg_status"

# ── 2. rust-toolchain ───────────────────────────────────────────
info "2/7" "rust-toolchain"

make LLVM=1 rustavailable > "$TMP/rust_out" 2>&1 || true
if grep -q "Rust is available" "$TMP/rust_out"; then
  echo PASS > "$TMP/rt_status"
else
  echo FAIL > "$TMP/rt_status"; fail
fi

# ── 3. clippy build (= full compile + clippy) ───────────────────
info "3/7" "clippy + compile"

set +o pipefail
make LLVM=1 CLIPPY=1 -j"$(nproc)" 2>&1 | tee "$TMP/clippy.log" >&2
echo "${PIPESTATUS[0]}" > "$TMP/make_rc"
set -o pipefail

grep 'warning:' "$TMP/clippy.log" \
  | grep -v 'generated.*warning' > "$TMP/all_warn" || true
grep -E 'unnecessary_safety_comment|unnecessary safety comment|missing a safety comment|lint.*renamed|ptr_cast_constness|changing.*(their|its) constness|missing_safety_doc|useless_deref|deref on an immutable|manual_saturating_arithmetic|warnings emitted' \
  "$TMP/all_warn" > "$TMP/known_warn" || true

if [ -s "$TMP/known_warn" ] && [ -s "$TMP/all_warn" ]; then
  grep -xvF -f "$TMP/known_warn" "$TMP/all_warn" > "$TMP/real_warn" || true
else
  cp "$TMP/all_warn" "$TMP/real_warn"
fi

# Filter real_warn to only warnings in files changed by our patches.
# Warnings in unrelated files (e.g. fs/select.c) are pre-existing and
# should not fail the build.
BASELINE=$(git log --oneline HEAD~"$NP" -1 --format='%H')
git diff --name-only "$BASELINE"..HEAD > "$TMP/changed_files" 2>/dev/null || true
: > "$TMP/changed_warn"
: > "$TMP/other_warn"
if [ -s "$TMP/real_warn" ]; then
  while IFS= read -r line; do
    matched=false
    while IFS= read -r cf; do
      if echo "$line" | grep -qF "$cf"; then
        matched=true; break
      fi
    done < "$TMP/changed_files"
    if $matched; then
      echo "$line" >> "$TMP/changed_warn"
    else
      echo "$line" >> "$TMP/other_warn"
    fi
  done < "$TMP/real_warn"
fi

MAKE_RC=$(cat "$TMP/make_rc")
if [ "$MAKE_RC" -ne 0 ]; then
  echo FAIL > "$TMP/bw_status"; echo FAIL > "$TMP/cl_status"; fail
elif [ -s "$TMP/changed_warn" ]; then
  echo FAIL > "$TMP/bw_status"; echo FAIL > "$TMP/cl_status"; fail
else
  echo PASS > "$TMP/bw_status"; echo PASS > "$TMP/cl_status"
fi

# ── 4. rustfmt ───────────────────────────────────────────────────
info "4/7" "rustfmt"

make LLVM=1 rustfmt > /dev/null 2>&1
git diff --name-only -- '*.rs' > "$TMP/fmt_diff"
git checkout -- '*.rs' 2>/dev/null || true

if [ -s "$TMP/fmt_diff" ]; then
  echo FAIL > "$TMP/fm_status"; fail
else
  echo PASS > "$TMP/fm_status"
fi

# ── 5. checkpatch ────────────────────────────────────────────────
info "5/7" "checkpatch"

if [ -f scripts/checkpatch.pl ]; then
  git diff HEAD~"$NP" \
    | perl scripts/checkpatch.pl --no-tree - > "$TMP/cp.log" 2>&1 || true
  grep '^ERROR:'   "$TMP/cp.log" > "$TMP/cp_err"  || true
  grep '^WARNING:' "$TMP/cp.log" > "$TMP/cp_warn_all" || true

  grep -E 'Possible repeated word|MAINTAINERS' \
    "$TMP/cp_warn_all" > "$TMP/cp_warn_known" || true

  if [ -s "$TMP/cp_warn_known" ] && [ -s "$TMP/cp_warn_all" ]; then
    grep -xvF -f "$TMP/cp_warn_known" "$TMP/cp_warn_all" \
      > "$TMP/cp_warn_real" || true
  else
    cp "$TMP/cp_warn_all" "$TMP/cp_warn_real"
  fi

  if [ -s "$TMP/cp_err" ] || [ -s "$TMP/cp_warn_real" ]; then
    echo FAIL > "$TMP/cp_status"; fail
  else
    echo PASS > "$TMP/cp_status"
  fi
else
  echo SKIP > "$TMP/cp_status"
fi

# ── 6. per-commit build (git bisect safety) ─────────────────────
if [ "$NP" -gt 1 ]; then
  info "6/7" "per-commit build ($NP commits)"

  ORIG_HEAD=$(git rev-parse HEAD)
  ORIG_BRANCH=$(git symbolic-ref --short HEAD 2>/dev/null || echo "")

  : > "$TMP/pc_failures"
  COMMITS=$(git rev-list --reverse HEAD~"$NP"..HEAD)
  PC_TOTAL=$(echo "$COMMITS" | wc -l)
  PC_IDX=0

  for c in $COMMITS; do
    PC_IDX=$((PC_IDX + 1))
    SHORT=$(git rev-parse --short "$c")
    SUBJ=$(git log -1 --format='%s' "$c")
    printf '  [%d/%d] %s %s ... ' "$PC_IDX" "$PC_TOTAL" "$SHORT" "$SUBJ" >&2

    git checkout --quiet "$c"
    if make LLVM=1 CLIPPY=1 -j"$(nproc)" > "$TMP/pc_build.log" 2>&1; then
      printf '\e[32mOK\e[0m\n' >&2
    else
      printf '\e[31mFAIL\e[0m\n' >&2
      tail -10 "$TMP/pc_build.log" >&2
      echo "$SHORT $SUBJ" >> "$TMP/pc_failures"
    fi
  done

  if [ -n "$ORIG_BRANCH" ]; then
    git checkout --quiet "$ORIG_BRANCH"
  else
    git checkout --quiet "$ORIG_HEAD"
  fi

  if [ -s "$TMP/pc_failures" ]; then
    echo FAIL > "$TMP/pc_status"; fail
  else
    echo PASS > "$TMP/pc_status"
  fi
else
  info "6/7" "per-commit build (single commit, skipping)"
  echo SKIP > "$TMP/pc_status"
fi

# ── 7. compose JSON ─────────────────────────────────────────────
info "7/7" "writing result"

python3 - "$VERDICT" "$TMP" "$OUT" <<'PYEOF'
import sys, json, os
from datetime import datetime

verdict, tmp, out = sys.argv[1], sys.argv[2], sys.argv[3]

def read(name):
    p = os.path.join(tmp, name)
    if not os.path.exists(p):
        return ""
    with open(p) as f:
        return f.read().strip()

def lines(name):
    t = read(name)
    return [l for l in t.split("\n") if l] if t else []

result = {
    "verdict": verdict,
    "timestamp": datetime.now().astimezone().isoformat(),
    "checks": [
        {
            "name": "config-flags",
            "status": read("cfg_status"),
            "detail": "All flags present"
            if not lines("cfg_missing")
            else "Missing: " + ", ".join(lines("cfg_missing")),
            "missing": lines("cfg_missing"),
        },
        {
            "name": "rust-toolchain",
            "status": read("rt_status"),
            "detail": read("rust_out").split("\n")[0]
            if read("rust_out")
            else "",
        },
        {
            "name": "build-warnings",
            "status": read("bw_status"),
            "warnings_in_changed_files": [
                w for w in lines("changed_warn") if "clippy::" not in w
            ],
            "other_warnings": lines("other_warn"),
            "known_warnings": lines("known_warn"),
        },
        {
            "name": "clippy",
            "status": read("cl_status"),
            "warnings": [w for w in lines("changed_warn") if "clippy::" in w],
            "known_warnings": lines("known_warn"),
        },
        {
            "name": "rustfmt",
            "status": read("fm_status"),
            "unformatted_files": lines("fmt_diff"),
        },
        {
            "name": "checkpatch",
            "status": read("cp_status"),
            "errors": lines("cp_err"),
            "warnings": lines("cp_warn_real"),
            "known_warnings": lines("cp_warn_known"),
        },
        {
            "name": "per-commit-build",
            "status": read("pc_status"),
            "failed_commits": lines("pc_failures"),
        },
    ],
}

if out == "/dev/stdout":
    json.dump(result, sys.stdout, indent=2, ensure_ascii=False)
    sys.stdout.write("\n")
else:
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    with open(out, "w") as f:
        json.dump(result, f, indent=2, ensure_ascii=False)
        f.write("\n")
    print(f"Wrote: {out}", file=sys.stderr)
PYEOF

echo "" >&2
if [ "$VERDICT" = "PASS" ]; then
  printf '\e[32m✓ VERDICT: PASS\e[0m\n' >&2
else
  printf '\e[31m✗ VERDICT: FAIL\e[0m\n' >&2
fi

echo "VERIFY_DONE rc=$([ "$VERDICT" = "PASS" ] && echo 0 || echo 1)" >&2
[ "$VERDICT" = "PASS" ]
