#!/usr/bin/env bash
# bootstrap.sh — install the graphos-factory binary, and the
# graphos-factory-core link beside it, through the core's bootstrap.sh.
#
#   bootstrap.sh            # download the release binary
#   bootstrap.sh --build    # build it from crate/ with cargo
#   bootstrap.sh --check    # exit 0 if the pinned version is installed
#
# It names the product (GRAPHOS_FACTORY_CORE_BIN_NAME=graphos-factory) and
# execs graphos-factory-core/scripts/bootstrap.sh, which documents the
# channels, the cache and the exit codes.
set -euo pipefail
export GRAPHOS_FACTORY_CORE_BIN_NAME=graphos-factory
exec bash "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../../../graphos-factory-core/scripts/bootstrap.sh" "$@"
