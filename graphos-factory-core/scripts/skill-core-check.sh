#!/usr/bin/env bash
# skill-core-check.sh — every target's SKILL.md carries the core's shared
# body, graphos-factory-core/SKILL-core.md, verbatim.
#
#   skill-core-check.sh [SKILL.md ...]
#
# With no argument it checks every skills/*/SKILL.md in this repository.
#
# SKILL-core.md is a list of sections, each between a `<!-- core:begin -->`
# line and a `<!-- core:end -->` line. A target's SKILL.md is written whole
# (it must stand alone: its frontmatter triggers the skill), and fences the
# same sections, in the same order, with the same two marker lines; what it
# says between them is its own. Each fenced section must be byte-for-byte
# the core's, so a change to the shared text is made in SKILL-core.md and
# every SKILL.md at once. A generated SKILL.md was rejected: it is one more
# file that can be stale on the branch a marketplace installs from.
#
# Exit codes: 0 every file matches · 1 a section drifted, or the files fence
# a different number of sections · 2 a missing file, or markers that do not
# pair (a begin inside a section, an end outside one, an unclosed section).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
CORE="$ROOT/graphos-factory-core/SKILL-core.md"

case "${1:-}" in
  -h|--help) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac

files=("$@")
if [ "${#files[@]}" -eq 0 ]; then
  for f in "$ROOT"/skills/*/SKILL.md; do
    [ -f "$f" ] && files+=("$f")
  done
fi
[ -f "$CORE" ] || { echo "skill-core-check: no $CORE" >&2; exit 2; }
[ "${#files[@]}" -gt 0 ] || { echo "skill-core-check: no skills/*/SKILL.md to check" >&2; exit 2; }

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

# split FILE DIR — write each fenced section to DIR/NNN.md and print the
# count. Exit 2 when the markers do not pair.
split() {
  mkdir -p "$2"
  awk -v dir="$2" '
    /^<!-- core:begin -->$/ {
      if (on) { printf "%s:%d: core:begin inside a section\n", FILENAME, NR > "/dev/stderr"; bad = 1; exit 2 }
      on = 1; n++; out = sprintf("%s/%03d.md", dir, n); printf "" > out; next
    }
    /^<!-- core:end -->$/ {
      if (!on) { printf "%s:%d: core:end outside a section\n", FILENAME, NR > "/dev/stderr"; bad = 1; exit 2 }
      on = 0; close(out); next
    }
    on { print > out }
    END {
      if (bad) exit 2
      if (on) { printf "%s: a core section is never closed\n", FILENAME > "/dev/stderr"; exit 2 }
      print n + 0
    }' "$1"
}

want="$(split "$CORE" "$SCRATCH/core")" || exit 2
[ "$want" -gt 0 ] || { echo "skill-core-check: $CORE fences no section" >&2; exit 2; }

status=0
i=0
for f in "${files[@]}"; do
  [ -f "$f" ] || { echo "skill-core-check: no $f" >&2; exit 2; }
  i=$((i + 1))
  have="$(split "$f" "$SCRATCH/skill-$i")" || exit 2
  rel="${f#"$ROOT"/}"
  if [ "$have" != "$want" ]; then
    echo "skill-core-check: $rel fences $have core section(s); graphos-factory-core/SKILL-core.md has $want" >&2
    status=1
    continue
  fi
  drift=0
  for n in $(seq -f '%03g' 1 "$want"); do
    if ! cmp -s "$SCRATCH/core/$n.md" "$SCRATCH/skill-$i/$n.md"; then
      echo "skill-core-check: $rel core section $((10#$n)) (first line: $(head -1 "$SCRATCH/core/$n.md")) differs from graphos-factory-core/SKILL-core.md:" >&2
      diff -u --label "graphos-factory-core/SKILL-core.md" --label "$rel" "$SCRATCH/core/$n.md" "$SCRATCH/skill-$i/$n.md" >&2 || true
      drift=1
    fi
  done
  if [ "$drift" -eq 0 ]; then
    echo "skill-core-check: $rel carries all $want core section(s) verbatim"
  else
    status=1
  fi
done
exit "$status"
