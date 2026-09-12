#!/usr/bin/env bash
#
# Seeds a downstream binding crate's Cargo.lock from the workspace lockfile, so
# every release leg builds against the dependency versions pinned at the tagged
# commit instead of re-resolving. Fails if any dependency the binding resolves
# to was not pinned by the workspace lock.
#
# The reference is the tree that published the cdk-ffi the binding pins, which
# is an earlier tag whenever a release pins an older cdk-ffi than its own tag.
# Pass that checkout's manifest as the second argument.
#
# Usage: seed-binding-lockfile.sh <downstream-rust-dir> [root-manifest]
#   run from the monorepo checkout root.

set -euo pipefail

BINDING_DIR="${1:?usage: seed-binding-lockfile.sh <downstream-rust-dir> [root-manifest]}"
ROOT_MANIFEST="${2:-Cargo.toml}"
ROOT_LOCK="$(dirname "${ROOT_MANIFEST}")/Cargo.lock"

if [[ ! -f "${ROOT_LOCK}" ]]; then
  echo "::error::${ROOT_LOCK} not found; the workspace lockfile must be committed"
  exit 1
fi

cp "${ROOT_LOCK}" "${BINDING_DIR}/Cargo.lock"

# `cargo fetch` resolves minimally against the lock it is handed, changing only
# the entries the rewritten manifest forces (cdk-ffi moves from a path dep to a
# registry or git source). `cargo update` and `cargo generate-lockfile` both
# re-resolve to latest and would silently undo the seeding.
( cd "${BINDING_DIR}" && cargo fetch )

# The wrapper crate and cdk-ffi itself are excluded: their source and version
# legitimately differ downstream, especially for nightlies.
resolved_versions() {
  cargo metadata --format-version 1 --locked --manifest-path "$1" \
    | jq -r '.packages[] | select(.name | startswith("cdk-ffi") | not)
             | "\(.name) \(.version)"' \
    | sort -u
}

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

resolved_versions "${ROOT_MANIFEST}" > "${workdir}/workspace.txt"
resolved_versions "${BINDING_DIR}/Cargo.toml" > "${workdir}/binding.txt"

# Anything the binding resolves that the workspace did not pin is drift. The
# reverse is expected: the workspace has members the binding never pulls in.
drift="$(comm -13 "${workdir}/workspace.txt" "${workdir}/binding.txt")"

if [[ -n "${drift}" ]]; then
  echo "::error::binding dependencies drifted from ${ROOT_LOCK}"
  echo "${drift}" | sed 's/^/  /'
  echo
  echo "Re-run with the workspace lock updated, or pin the offending crate."
  echo "A binding whose dependencies moved ahead of the pinned cdk-ffi needs a"
  echo "matching cdk-ffi released first."
  exit 1
fi

echo "Dependency graph matches the workspace lockfile ($(wc -l < "${workdir}/binding.txt") packages)."
