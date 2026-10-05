#!/usr/bin/env bash
# skill-doc-check.sh — the skill never cites the repository's design
# documents.
#
#   skill-doc-check.sh
#
# Design records, the plan and proposals (the repository's docs directory)
# are written for the people and agents developing the skill. An agent using the skill reads only what the skill ships, so a
# citation of one points at nothing: every rule the skill needs is stated in
# its own files. This greps what an agent using the skill reads, in every
# tree that carries it: graphos-factory-core/ (SKILL-core.md, references/,
# scripts/), each skills/*/ present (SKILL.md, references/, scripts/), the
# schemas/ the binary embeds, and the JSON files under crate/*/src. A path
# that is absent is skipped, so a tree that carries one target passes.
#
# The binary's messages are checked by a test rather than here: the core
# suite's skill_doc module lexes each crate's src/ for string literals, which
# a line grep cannot do for a literal spanning lines without catching the
# comments beside it. That test's `citation()` mirrors the pattern set below
# arm for arm: change the two together.
#
# The pattern set (ERE, one per line of text), the only one:
#   ADR[ -]?[0-9]{3,4}            not glued to a word on its left
#   docs/(decisions|plan|proposals)
#                                 as a path: not preceded by [A-Za-z0-9/-],
#                                 except through "./" or "../"
#   [Pp]hase [0-9]+[a-z]*         as a reference, not as prose: not preceded
#                                 by a word character or "-", and followed by
#                                 "(", ",", ")", ":" (a space may come first),
#                                 " as built", " amendment" or the end of the
#                                 line; or preceded by "(" and not followed by
#                                 a word character. "Phase 7as (c)" and
#                                 "(Phase 4d)" hit; "phase 2 of the rollout"
#                                 and "two-phase 2" do not
#   [0-9]+[a-z]? amendment\)      "(ADR 0114, 8e amendment)" style
#   not yet on `main`             with or without the backticks
#
# Output: one `path:line: text` per hit. Exit codes: 0 no hit · 1 a hit ·
# 2 run outside a checkout (no graphos-factory-core/).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SELF="graphos-factory-core/scripts/$(basename "${BASH_SOURCE[0]}")"

case "${1:-}" in
  -h|--help) sed -n '2,39p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
esac

[ -d "$ROOT/graphos-factory-core" ] || {
  echo "skill-doc-check: no graphos-factory-core/ under $ROOT" >&2
  exit 2
}

# The pattern set documented in the header, as one ERE (so GNU and BSD grep
# read it the same way).
PATTERN='(^|[^[:alnum:]_])ADR[ -]?[0-9]{3,4}'
PATTERN+='|(^|[^[:alnum:]/-]|\.\.?/)docs/(decisions|plan|proposals)'
PATTERN+='|(^|[^[:alnum:]_-])[Pp]hase [0-9]+[a-z]*( ?[(,):]| as built| amendment|$)'
PATTERN+='|\([Pp]hase [0-9]+[a-z]*([^[:alnum:]_]|$)'
PATTERN+='|(^|[^[:alnum:]_])[0-9]+[a-z]? amendment\)'
PATTERN+='|not yet on .?main'

files=()
add() {
  local f
  for f in "$@"; do
    [ -f "$ROOT/$f" ] || continue
    [ "$f" = "$SELF" ] && continue
    files+=("$f")
  done
}

cd "$ROOT"
add graphos-factory-core/SKILL-core.md graphos-factory-core/references/*.md graphos-factory-core/scripts/*.sh
for d in skills/*/; do
  [ -d "$d" ] || continue
  add "${d}SKILL.md" "${d}"references/*.md "${d}"scripts/*.sh
done
add schemas/*.json schemas/*/*.json schemas/README.md
while IFS= read -r f; do add "$f"; done < <(find crate -path '*/src/*' -name '*.json' 2>/dev/null | LC_ALL=C sort)

[ "${#files[@]}" -gt 0 ] || { echo "skill-doc-check: nothing to check" >&2; exit 2; }

if grep -nE "$PATTERN" "${files[@]}"; then
  echo "skill-doc-check: the lines above cite design documents the skill does not ship; state the rule instead" >&2
  exit 1
fi
echo "skill-doc-check: ${#files[@]} files, no citation of a design document"
