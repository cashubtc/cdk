#!/usr/bin/env bash
#
# Checks, after the fact, that a published binding release resolved only
# dependency versions that the monorepo's Cargo.lock pinned at the commit the
# release claims to come from.
#
# Usage: verify-binding-lockfile.sh <language> <release-tag>
#   run from the monorepo checkout root. Needs `gh` and a checkout at the
#   source commit recorded in the release's build-manifest.json.

set -euo pipefail

LANGUAGE="${1:?usage: verify-binding-lockfile.sh <language> <release-tag>}"
TAG="${2:?missing release tag}"

case "${LANGUAGE}" in
  dart|swift|kotlin|go) ;;
  *) echo "unknown language: ${LANGUAGE}" >&2; exit 2 ;;
esac

REPO="cashubtc/cdk-${LANGUAGE}"
workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT

git clone --quiet --depth 1 --branch "${TAG}" \
  "https://github.com/${REPO}.git" "${workdir}/downstream"

MANIFEST="${workdir}/downstream/build-manifest.json"
if [[ ! -f "${MANIFEST}" ]]; then
  echo "::error::${REPO}@${TAG} has no build-manifest.json; it predates provenance recording"
  exit 1
fi

CLAIMED_COMMIT="$(jq -r '.source.commit' "${MANIFEST}")"
HEAD_COMMIT="$(git rev-parse HEAD)"
if [[ "${CLAIMED_COMMIT}" != "${HEAD_COMMIT}" ]]; then
  echo "::error::checkout mismatch: release was built from ${CLAIMED_COMMIT}, this tree is ${HEAD_COMMIT}"
  echo "Run: git checkout ${CLAIMED_COMMIT}"
  exit 1
fi

CLAIMED_WS_LOCK="$(jq -r '.lockfiles.workspace_cargo_lock_sha256' "${MANIFEST}")"
ACTUAL_WS_LOCK="$(shasum -a 256 Cargo.lock 2>/dev/null | cut -d' ' -f1)"
if [[ "${CLAIMED_WS_LOCK}" != "${ACTUAL_WS_LOCK}" ]]; then
  echo "::error::workspace Cargo.lock hash mismatch"
  echo "  manifest: ${CLAIMED_WS_LOCK}"
  echo "  checkout: ${ACTUAL_WS_LOCK}"
  exit 1
fi

BINDING_LOCK="${workdir}/downstream/rust/Cargo.lock"
if [[ ! -f "${BINDING_LOCK}" ]]; then
  # Releases cut before a language started seeding its lockfile have none, and
  # the workspace lock hash checked above is then the whole guarantee.
  echo "${LANGUAGE}: no downstream lockfile; verified against the workspace lock."
  exit 0
fi

lock_versions() {
  awk -F'"' '/^name = /{n=$2} /^version = /{print n" "$2}' "$1" \
    | grep -v '^cdk-ffi' \
    | sort -u
}

# Releases that pin an older cdk-ffi than their own tag were seeded from that
# cdk-ffi's tag, so that is what the binding lock has to be compared against.
LOCK_REF="$(jq -r '.lockfiles.binding_cargo_lock_source_ref // empty' "${MANIFEST}")"
REFERENCE_LOCK="${workdir}/reference.lock"
if [[ -n "${LOCK_REF}" && "${LOCK_REF}" != "${HEAD_COMMIT}" ]]; then
  if ! git fetch --quiet --depth 1 origin "${LOCK_REF}"; then
    echo "::error::cannot fetch ${LOCK_REF}, the ref the binding lockfile was seeded from"
    exit 1
  fi
  git show FETCH_HEAD:Cargo.lock > "${REFERENCE_LOCK}"
else
  LOCK_REF="${CLAIMED_COMMIT}"
  cp Cargo.lock "${REFERENCE_LOCK}"
fi

lock_versions "${REFERENCE_LOCK}" > "${workdir}/workspace.txt"
lock_versions "${BINDING_LOCK}" > "${workdir}/binding.txt"

drift="$(comm -13 "${workdir}/workspace.txt" "${workdir}/binding.txt")"
if [[ -n "${drift}" ]]; then
  echo "::error::${REPO}@${TAG} used dependencies the lockfile at ${LOCK_REF} did not pin"
  echo "${drift}" | sed 's/^/  /'
  exit 1
fi

echo "${LANGUAGE} ${TAG}: all $(wc -l < "${workdir}/binding.txt" | tr -d ' ') dependencies match the lockfile at ${LOCK_REF}."
