#!/bin/bash
# check-commit-msgs.sh — Validate commit messages against kernel conventions
#
# Usage: check-commit-msgs.sh <baseline> [HEAD]
#   Checks all commits in baseline..HEAD range.
#
# Exit: 0 = all OK, 1 = issues found
set -uo pipefail

BASELINE="${1:?Usage: check-commit-msgs.sh <baseline> [HEAD]}"
HEAD="${2:-HEAD}"

ISSUES=0
issue() { printf '\e[31m  ✗ %s: %s\e[0m\n' "$1" "$2" >&2; ISSUES=$((ISSUES + 1)); }
ok()    { printf '\e[32m  ✓ %s\e[0m\n' "$1" >&2; }

COMMITS=$(git rev-list --reverse "$BASELINE".."$HEAD")
TOTAL=$(echo "$COMMITS" | grep -c . || true)

if [ "$TOTAL" -eq 0 ]; then
  echo "No commits in range $BASELINE..$HEAD" >&2
  exit 0
fi

printf '\e[36m=== Checking %d commit message(s) ===\e[0m\n' "$TOTAL" >&2

declare -A SEEN_SUBJECTS

for c in $COMMITS; do
  SHORT=$(git rev-parse --short "$c")
  SUBJECT=$(git log -1 --format='%s' "$c")
  BODY=$(git log -1 --format='%b' "$c")

  printf '\n\e[36m%s %s\e[0m\n' "$SHORT" "$SUBJECT" >&2

  # 1. Subject contains colon (subsystem: prefix)
  if echo "$SUBJECT" | grep -q ':'; then
    ok "subsystem prefix"
  else
    issue "$SHORT" "missing subsystem prefix (expected 'subsystem: summary')"
  fi

  # 2. Subject length ≤ 75
  SLEN=${#SUBJECT}
  if [ "$SLEN" -le 75 ]; then
    ok "subject length ($SLEN chars)"
  else
    issue "$SHORT" "subject too long ($SLEN > 75 chars)"
  fi

  # 3. No trailing period
  if echo "$SUBJECT" | grep -qE '\.$'; then
    issue "$SHORT" "trailing period in subject"
  else
    ok "no trailing period"
  fi

  # 4. Imperative mood (catch common violations)
  SUMMARY=$(echo "$SUBJECT" | sed 's/^[^:]*: *//')
  if echo "$SUMMARY" | grep -qiE '^(adds |added |adding |this patch |this commit |fixed |fixes |changed |changes )'; then
    issue "$SHORT" "non-imperative mood ('$SUMMARY') — use 'add' not 'adds/added'"
  else
    ok "imperative mood"
  fi

  # 5. Signed-off-by present
  if echo "$BODY" | grep -q '^Signed-off-by:'; then
    ok "Signed-off-by"
  else
    issue "$SHORT" "missing Signed-off-by tag"
  fi

  # 6. Unique subject (no duplicates in series)
  NORM_SUBJ=$(echo "$SUBJECT" | tr '[:upper:]' '[:lower:]')
  if [ -n "${SEEN_SUBJECTS[$NORM_SUBJ]+x}" ]; then
    issue "$SHORT" "duplicate subject (same as ${SEEN_SUBJECTS[$NORM_SUBJ]})"
  else
    ok "unique subject"
    SEEN_SUBJECTS[$NORM_SUBJ]="$SHORT"
  fi

  # 7. Body line width ≤ 75 (skip tag lines and URLs)
  LONG_LINES=""
  if [ -n "$BODY" ]; then
    LONG_LINES=$(echo "$BODY" \
      | grep -vE '^(Signed-off-by:|Reviewed-by:|Acked-by:|Tested-by:|Co-developed-by:|Fixes:|Link:|Closes:|Cc:)' \
      | grep -vE 'https?://' \
      | awk 'length > 75 { print NR": "length" chars: "$0 }' \
      | head -3)
  fi
  if [ -n "$LONG_LINES" ]; then
    issue "$SHORT" "body lines > 75 cols:"
    echo "$LONG_LINES" >&2
  else
    ok "body line width"
  fi
done

echo "" >&2
if [ "$ISSUES" -eq 0 ]; then
  printf '\e[32m✓ All %d commit(s) OK\e[0m\n' "$TOTAL" >&2
else
  printf '\e[31m✗ %d issue(s) found in %d commit(s)\e[0m\n' "$ISSUES" "$TOTAL" >&2
fi

exit $([ "$ISSUES" -eq 0 ] && echo 0 || echo 1)
