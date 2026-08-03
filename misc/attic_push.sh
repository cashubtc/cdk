#!/usr/bin/env bash
# Push nix store paths to the self-hosted attic cache (unlimited storage).
#
# Usage: attic_push.sh <paths-file>
#
# <paths-file> contains one store path per line (e.g. produced by
# `nix build --no-link --print-out-paths | tee <paths-file>`).
#
# Env:
#   ATTIC_TOKEN  - attic push token (repo secret). If unset, this is a no-op.
#   ATTIC_CACHE  - attic cache name, e.g. "cashudevkit" (repo variable).
#
# Skipping is silent-ish (one line) so fork PRs and missing config are cheap.
set -euo pipefail

paths_file="${1:?usage: attic_push.sh <paths-file>}"

if [ -z "${ATTIC_TOKEN:-}" ] || [ -z "${ATTIC_CACHE:-}" ]; then
    echo "ATTIC_TOKEN/ATTIC_CACHE not configured, skipping push"
    exit 0
fi

if [ ! -s "$paths_file" ]; then
    echo "No store paths in $paths_file, nothing to push"
    exit 0
fi

# Prefer the attic client already on the (self-hosted) runner; only fall back
# to installing one when it is missing.
if ! command -v attic >/dev/null 2>&1; then
    nix profile install nixpkgs#attic-client
fi

attic login cashudevkit https://cache.cashudevkit.org "$ATTIC_TOKEN"
# -r: don't invoke attic at all on empty input.
xargs -r -a "$paths_file" attic push "cashudevkit:$ATTIC_CACHE"
