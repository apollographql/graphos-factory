# shellcheck shell=bash
# cache.sh — where this checkout's product binary and toolchain are cached.
# Sourced (never executed) by bootstrap.sh, toolchain.sh, resolve-bin.sh,
# e2e.sh and live.sh; sets GRAPHOS_FACTORY_CORE_RELEASE_REPO and
# GRAPHOS_FACTORY_CORE_CACHE_DIR.
#
#   GRAPHOS_FACTORY_CORE_RELEASE_REPO  $GRAPHOS_FACTORY_CORE_RELEASE_REPO, else
#                                 crate/Cargo.toml's `repository` (owner/repo
#                                 on GitHub), the releases bootstrap.sh
#                                 downloads from
#   GRAPHOS_FACTORY_CORE_CACHE_DIR     $GRAPHOS_FACTORY_CORE_CACHE, else
#                                 ~/.cache/graphos-factory-core/<owner>-<repo>, so
#                                 two products' builds never overwrite each
#                                 other
#
# Its bin/ holds the product binary bootstrap.sh installed and
# `graphos-factory-core`, a link to it.
# Bash 3.2 compatible (macOS /bin/bash).

_fc_here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# The repository root: these scripts live in graphos-factory-core/scripts/.
_fc_root="$(cd "$_fc_here/../.." && pwd)"
GRAPHOS_FACTORY_CORE_RELEASE_REPO="${GRAPHOS_FACTORY_CORE_RELEASE_REPO:-}"
if [ -z "$GRAPHOS_FACTORY_CORE_RELEASE_REPO" ] && [ -f "$_fc_root/crate/Cargo.toml" ]; then
  GRAPHOS_FACTORY_CORE_RELEASE_REPO="$(sed -n 's#^repository = "https://github.com/\([^"]*\)".*#\1#p' "$_fc_root/crate/Cargo.toml" | head -1)"
fi
_fc_slug="${GRAPHOS_FACTORY_CORE_RELEASE_REPO:-local}"
# shellcheck disable=SC2034 # read by the scripts that source this one
GRAPHOS_FACTORY_CORE_CACHE_DIR="${GRAPHOS_FACTORY_CORE_CACHE:-$HOME/.cache/graphos-factory-core/${_fc_slug//\//-}}"
unset _fc_here _fc_root _fc_slug
