#!/usr/bin/env bash
# Push the closures of flake dev shells (toolchains, helper scripts) to the
# self-hosted Attic cache so CI runners substitute them instead of rebuilding
# or refetching them.
#
# Usage: attic_push_devshells.sh <shell>...
#
# A plain `nix build .#devShells.<system>.<shell>` output does not reference the
# shell's inputs, so this realises each shell through `nix develop --profile`.
# The resulting `nix-shell-env` path references everything the shell provides.
#
# Env: same as attic_push.sh. Without ATTIC_TOKEN this is a no-op.
set -euo pipefail

if [ "$#" -eq 0 ]; then
    echo "usage: attic_push_devshells.sh <shell>..." >&2
    exit 2
fi

if [ -z "${ATTIC_TOKEN:-}" ]; then
    echo "ATTIC_TOKEN not configured, skipping push"
    exit 0
fi

script_dir=$(cd "$(dirname "$0")" && pwd)
work_dir=$(mktemp -d)
cleanup() {
    rm -rf "$work_dir"
}
trap cleanup EXIT HUP INT TERM

for shell in "$@"; do
    # Keep shell startup messages out of the store-path list.
    nix develop --fallback -i --profile "$work_dir/$shell" ".#$shell" --command true >&2
    readlink -f "$work_dir/$shell"
done > "$work_dir/paths.txt"

nix shell .#attic-client --command "$script_dir/attic_push.sh" "$work_dir/paths.txt"
