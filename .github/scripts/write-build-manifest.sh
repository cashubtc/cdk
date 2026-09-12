#!/usr/bin/env bash
#
# Records what a binding release was built from, so the dependency graph and
# toolchain can be checked after the fact without re-running the release.
#
# Usage: write-build-manifest.sh <language> <downstream-dir> <output-path>
#   run from the monorepo checkout root. Reads TAG, CDK_REF, CDK_VER, NIGHTLY
#   and GITHUB_REPOSITORY from the environment.

set -euo pipefail

LANGUAGE="${1:?usage: write-build-manifest.sh <language> <downstream-dir> <output-path>}"
DOWNSTREAM_DIR="${2:?missing downstream dir}"
OUTPUT="${3:?missing output path}"

sha256() {
  if [[ ! -f "$1" ]]; then
    echo "null"
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

CDK_VER="${CDK_VER:-${TAG:-}}"
CDK_VER="${CDK_VER#v}"

# The binding lockfile is seeded from the tree that published the pinned
# cdk-ffi: its tag for a release, the exact commit for a nightly.
if [[ "${NIGHTLY:-false}" == "true" ]]; then
  BINDING_LOCK_REF="${CDK_REF:-}"
else
  BINDING_LOCK_REF="refs/tags/v${CDK_VER}"
fi

RUST_CHANNEL="$(grep '^channel' rust-toolchain.toml | cut -d'"' -f2)"
WORKSPACE_LOCK="$(sha256 Cargo.lock)"
BINDING_LOCK="$(sha256 "${DOWNSTREAM_DIR}/rust/Cargo.lock")"
FLAKE_LOCK="$(sha256 flake.lock)"

# binding_cargo_lock_sha256 stays null for a binding with no downstream Rust
# crate to seed. Every language currently released has one.
jq -n \
  --arg language "${LANGUAGE}" \
  --arg tag "${TAG:-}" \
  --arg repo "${GITHUB_REPOSITORY:-cashubtc/cdk}" \
  --arg commit "${CDK_REF:-}" \
  --arg cdk_version "${CDK_VER}" \
  --arg binding_lock_ref "${BINDING_LOCK_REF}" \
  --arg rust_channel "${RUST_CHANNEL}" \
  --arg workspace_lock "${WORKSPACE_LOCK}" \
  --arg binding_lock "${BINDING_LOCK}" \
  --arg flake_lock "${FLAKE_LOCK}" \
  '{
     schema_version: 2,
     language: $language,
     release_tag: $tag,
     source: { repo: $repo, commit: $commit, cdk_version: $cdk_version },
     toolchain: { rust_channel: $rust_channel },
     lockfiles: {
       workspace_cargo_lock_sha256: $workspace_lock,
       binding_cargo_lock_sha256: (if $binding_lock == "null" then null else $binding_lock end),
       binding_cargo_lock_source_ref: $binding_lock_ref,
       flake_lock_sha256: $flake_lock
     }
   }' > "${OUTPUT}"

cat "${OUTPUT}"
