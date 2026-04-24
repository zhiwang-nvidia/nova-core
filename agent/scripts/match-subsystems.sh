#!/bin/bash
# match-subsystems.sh — Identify relevant subsystem guides from git diff
#
# Usage: match-subsystems.sh <baseline> [HEAD]
#   Outputs filenames of matched subsystem guides, one per line.
#
# Parses the trigger table in subsystems/subsystem.md, matches triggers
# against changed file paths (path-like triggers) and diff content
# (symbol triggers).  Typically reduces 50+ guides to 2-3.
#
# Exit: 0 always (empty output = no matches)
set -uo pipefail

TREE=$(git rev-parse --show-toplevel)
BASELINE="${1:?Usage: match-subsystems.sh <baseline> [HEAD]}"
HEAD="${2:-HEAD}"
INDEX="$TREE/agent/roles/reviewer-refs/subsystems/subsystem.md"

[ -f "$INDEX" ] || { echo "ERROR: $INDEX not found" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

git diff --name-only "$BASELINE".."$HEAD" > "$TMP/files" 2>/dev/null
git diff "$BASELINE".."$HEAD" > "$TMP/diff" 2>/dev/null

python3 - "$INDEX" "$TMP/files" "$TMP/diff" <<'PYEOF'
import sys, re

index_file, files_path, diff_path = sys.argv[1], sys.argv[2], sys.argv[3]

with open(index_file) as f:
    index_lines = f.readlines()
with open(files_path) as f:
    files_corpus = f.read()
with open(diff_path) as f:
    diff_corpus = f.read()

if not files_corpus.strip() and not diff_corpus.strip():
    sys.exit(0)

def looks_like_path(t):
    return '/' in t or t.startswith('.') or t.endswith('.c') or t.endswith('.h') or t.endswith('.rs')

matched = set()
for line in index_lines:
    m = re.match(r'^\|\s*(.+?)\s*\|\s*(.+?)\s*\|\s*(\S+\.md)\s*\|', line)
    if not m:
        continue
    name, triggers_raw, guide = m.groups()
    if name.strip().startswith(('-', 'Subsystem')):
        continue

    triggers_raw = triggers_raw.replace('`', '')
    for trigger in triggers_raw.split(','):
        trigger = trigger.strip().strip('*').strip()
        if not trigger or len(trigger) < 3:
            continue

        if looks_like_path(trigger):
            if trigger in files_corpus:
                matched.add(guide)
                break
        else:
            if trigger in diff_corpus:
                matched.add(guide)
                break

for g in sorted(matched):
    print(g)
PYEOF
