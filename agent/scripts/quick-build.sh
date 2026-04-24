#!/bin/bash
# quick-build.sh — Compile kernel + Clippy, report errors/warnings
#
# Usage: quick-build.sh [--no-clippy]
#   Run from kernel tree root, or it auto-detects via git.
#   --no-clippy   Skip Clippy (plain compile only)
#
# Exit: 0 = clean build, 1 = failure or warnings in changed files
set -uo pipefail

CLIPPY=1
for arg in "$@"; do
  case "$arg" in
    --no-clippy) CLIPPY=0 ;;
  esac
done

cd "$(git rev-parse --show-toplevel)"

LOG=$(mktemp)
trap 'rm -f "$LOG"' EXIT

if [ "$CLIPPY" -eq 1 ]; then
  printf '\e[36m=== Building with LLVM=1 CLIPPY=1 -j%s ===\e[0m\n' "$(nproc)" >&2
  MAKE_ARGS="LLVM=1 CLIPPY=1"
else
  printf '\e[36m=== Building with LLVM=1 -j%s ===\e[0m\n' "$(nproc)" >&2
  MAKE_ARGS="LLVM=1"
fi

set +o pipefail
make $MAKE_ARGS -j"$(nproc)" 2>&1 | tee "$LOG" >&2
RC=${PIPESTATUS[0]}
set -o pipefail

ERRORS=$(grep 'error\[' "$LOG" | head -20 || true)
WARNINGS=$(grep 'warning:' "$LOG" | grep -v 'generated.*warning' | sort -u || true)

echo "" >&2
echo "────────────────────────────────" >&2
if [ "$RC" -ne 0 ]; then
  printf '\e[31m✗ BUILD FAILED\e[0m\n' >&2
  [ -n "$ERRORS" ] && { echo "First errors:" >&2; echo "$ERRORS" >&2; }
  exit 1
fi

if [ -n "$WARNINGS" ]; then
  WCOUNT=$(echo "$WARNINGS" | wc -l)
  printf '\e[33m⚠ BUILD OK with %d warning(s)\e[0m\n' "$WCOUNT" >&2
  echo "$WARNINGS" >&2
else
  printf '\e[32m✓ BUILD OK — zero warnings\e[0m\n' >&2
fi

echo "BUILD_DONE rc=$RC" >&2
