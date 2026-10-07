#!/usr/bin/env bash
# .github/scripts/release-to-skills.sh: bring a graphos-factory release into a
# checkout of apollographql/skills, for the release workflow's pull request.
#
#   release-to-skills.sh SOURCE SKILLS
#
#   SOURCE  this repository at the release tag (its skills/graphos-factory/)
#   SKILLS  a checkout of apollographql/skills, changed in place
#
# That repository carries the skill in its apollo-skills plugin, as a
# read-only copy of a release. This makes the two changes its checks expect
# of one:
#   - skills/graphos-factory/ becomes an exact copy of SOURCE's (that
#     repository refuses any other content there);
#   - .claude-plugin/plugin.json's version (the apollo-skills plugin) gets a
#     patch bump, which that repository requires of any change under
#     skills/.
# It prints `unchanged` and changes nothing when SKILLS already holds this
# release, else `changed`. Every check runs before the first edit, so a
# failure (exit 1) leaves SKILLS untouched.
#
# Exit codes: 0 ok, 1 a file is not what it expects, 2 usage error.
set -euo pipefail

[ $# -eq 2 ] || { sed -n '2,21p' "$0" | sed 's/^# \{0,1\}//' >&2; exit 2; }
SOURCE="$1" SKILLS="$2"
SKILL=graphos-factory
MANIFEST="$SKILLS/.claude-plugin/plugin.json"

# Every check before any edit: a failure leaves SKILLS as it was.
[ -f "$SOURCE/skills/$SKILL/SKILL.md" ] || { echo "release-to-skills: no skills/$SKILL/SKILL.md in $SOURCE" >&2; exit 1; }
[ -f "$MANIFEST" ] || { echo "release-to-skills: no $MANIFEST" >&2; exit 1; }
old="$(sed -n 's/^  "version": "\([0-9]*\.[0-9]*\.[0-9]*\)",$/\1/p' "$MANIFEST" | head -1)"
[ -n "$old" ] || { echo "release-to-skills: $MANIFEST has no X.Y.Z \"version\" line" >&2; exit 1; }
new="$(echo "$old" | awk -F. '{ printf "%d.%d.%d", $1, $2, $3 + 1 }')"

if diff -rq "$SOURCE/skills/$SKILL" "$SKILLS/skills/$SKILL" >/dev/null 2>&1; then
  echo unchanged
  exit 0
fi

rm -rf "${SKILLS:?}/skills/$SKILL"
mkdir -p "$SKILLS/skills"
cp -R "$SOURCE/skills/$SKILL" "$SKILLS/skills/$SKILL"

# A line edit, not jq: the manifest keeps its own formatting.
sed -i.bak "s/^  \"version\": \"$old\",\$/  \"version\": \"$new\",/" "$MANIFEST"
rm -f "$MANIFEST.bak"
echo changed
