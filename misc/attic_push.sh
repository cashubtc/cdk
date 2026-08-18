#!/usr/bin/env bash
# Push Nix store paths to the self-hosted Attic cache.
#
# Usage: attic_push.sh <paths-file>
#
# <paths-file> contains one store path per line (e.g. produced by
# `nix build --no-link --print-out-paths | tee <paths-file>`).
#
# Env:
#   ATTIC_TOKEN       - Attic push token (repository secret). If unset, this is
#                       a no-op.
#   ATTIC_CACHE       - Cache name (default: "cashudevkit").
#   ATTIC_ENDPOINT    - Server endpoint (default: CDK's Attic server).
#   ATTIC_SERVER_NAME - Temporary server alias (default: "cashudevkit-cache").
#
# Skipping is silent-ish (one line) so fork PRs and missing config are cheap.
set -euo pipefail

paths_file="${1:?usage: attic_push.sh <paths-file>}"

if [ -z "${ATTIC_TOKEN:-}" ]; then
    echo "ATTIC_TOKEN not configured, skipping push"
    exit 0
fi

if [ ! -s "$paths_file" ]; then
    echo "No store paths in $paths_file, nothing to push"
    exit 0
fi

if ! command -v attic >/dev/null 2>&1; then
    echo "Attic client is not available; run this script through 'nix shell .#attic-client'" >&2
    exit 1
fi

attic_config_dir=$(mktemp -d)
cleanup() {
    rm -rf "$attic_config_dir"
}
trap cleanup EXIT HUP INT TERM

# Keep the write token out of the persistent HOME used by self-hosted runners.
export XDG_CONFIG_HOME="$attic_config_dir"
server_name="${ATTIC_SERVER_NAME:-cashudevkit-cache}"
endpoint="${ATTIC_ENDPOINT:-https://cache.cashudevkit.org}"
cache_name="${ATTIC_CACHE:-cashudevkit}"

attic login "$server_name" "$endpoint" "$ATTIC_TOKEN"
# -r: don't invoke attic at all on empty input.
xargs -r -a "$paths_file" attic push "$server_name:$cache_name"
