#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
MANIFEST="${SCRIPT_DIR}/Cargo.toml"
DIST_DIR="${SCRIPT_DIR}/dist"
VERSION="$(awk -F '"' '/^version = / { print $2; exit }' "${MANIFEST}")"
RUST_VERSION="$(rustc --version | awk '{ print $2 }')"
LINUX_IMAGE="rust:${RUST_VERSION}-bullseye"
BUILD_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/fed-cdk-tui-dist.XXXXXX")"

cleanup() {
    rm -rf "${BUILD_ROOT}"
}
trap cleanup EXIT

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: required command not found: $1" >&2
        exit 1
    fi
}

for command in awk cargo codesign cp docker file grep install mkdir mktemp rm rustc shasum strings strip uname unzip zip; do
    require_command "${command}"
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: this script must run on macOS to produce the macOS ARM64 artifact" >&2
    exit 1
fi

if [[ -z "${VERSION}" ]]; then
    echo "error: could not determine the fed-cdk-tui version" >&2
    exit 1
fi

echo "==> Checking fed-cdk-tui ${VERSION}"
cargo fmt --manifest-path "${MANIFEST}" -- --check
cargo test --manifest-path "${MANIFEST}" --locked
cargo clippy --manifest-path "${MANIFEST}" --all-targets --locked -- -D warnings

MACOS_TARGET_DIR="${BUILD_ROOT}/macos-arm64"
LINUX_CACHE_DIR="${BUILD_ROOT}/cargo-registry"
mkdir -p "${MACOS_TARGET_DIR}" "${LINUX_CACHE_DIR}"

echo "==> Building macOS ARM64 with Rust ${RUST_VERSION}"
env \
    CARGO_TARGET_DIR="${MACOS_TARGET_DIR}" \
    RUSTFLAGS="--remap-path-prefix=${HOME}=/build-home --remap-path-prefix=${REPO_ROOT}=/src" \
    cargo build \
        --manifest-path "${MANIFEST}" \
        --release \
        --target aarch64-apple-darwin \
        --locked

build_linux() {
    local platform="$1"
    local artifact_name="$2"
    local target_dir="${BUILD_ROOT}/${artifact_name}"

    mkdir -p "${target_dir}"
    echo "==> Building ${artifact_name} with ${LINUX_IMAGE}"
    docker run --rm \
        --platform "${platform}" \
        -e "RUSTUP_TOOLCHAIN=${RUST_VERSION}" \
        -e CARGO_TARGET_DIR=/target \
        -v "${REPO_ROOT}:/src:ro" \
        -v "${target_dir}:/target" \
        -v "${LINUX_CACHE_DIR}:/usr/local/cargo/registry" \
        -w /src \
        "${LINUX_IMAGE}" \
        sh -c 'cargo build --manifest-path scripts/fed-cdk-tui/Cargo.toml --release --locked && strip /target/release/fed-cdk-tui'
}

# Build sequentially. This avoids overcommitting Docker Desktop while the
# emulated x86-64 compiler and the native ARM64 compiler produce proc macros.
build_linux linux/arm64 linux-arm64
build_linux linux/amd64 linux-x86_64

echo "==> Packaging release archives"
rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}"

package_artifact() {
    local artifact_name="$1"
    local binary="$2"
    local package_name="fed-cdk-tui-${VERSION}-${artifact_name}"
    local package_dir="${DIST_DIR}/${package_name}"

    mkdir -p "${package_dir}"
    install -m 0755 "${binary}" "${package_dir}/fed-cdk-tui"
    cp "${SCRIPT_DIR}/README.md" "${REPO_ROOT}/LICENSE" "${package_dir}/"
    (
        cd "${DIST_DIR}"
        COPYFILE_DISABLE=1 zip -Xqr "${package_name}.zip" "${package_name}"
    )
}

MACOS_BINARY="${MACOS_TARGET_DIR}/aarch64-apple-darwin/release/fed-cdk-tui"
strip "${MACOS_BINARY}"
codesign --force --sign - "${MACOS_BINARY}"

package_artifact macos-arm64 "${MACOS_BINARY}"
package_artifact linux-arm64 "${BUILD_ROOT}/linux-arm64/release/fed-cdk-tui"
package_artifact linux-x86_64 "${BUILD_ROOT}/linux-x86_64/release/fed-cdk-tui"

echo "==> Verifying binaries"
"${DIST_DIR}/fed-cdk-tui-${VERSION}-macos-arm64/fed-cdk-tui" --version
codesign --verify --verbose=2 \
    "${DIST_DIR}/fed-cdk-tui-${VERSION}-macos-arm64/fed-cdk-tui"

verify_linux() {
    local platform="$1"
    local artifact_name="$2"

    docker run --rm \
        --platform "${platform}" \
        -v "${DIST_DIR}:/dist:ro" \
        "${LINUX_IMAGE}" \
        "/dist/fed-cdk-tui-${VERSION}-${artifact_name}/fed-cdk-tui" \
        --version
}

verify_linux linux/arm64 linux-arm64
verify_linux linux/amd64 linux-x86_64

if strings "${DIST_DIR}/fed-cdk-tui-${VERSION}-macos-arm64/fed-cdk-tui" \
    | grep -F "${HOME}" >/dev/null; then
    echo "error: macOS binary contains a user-specific filesystem path" >&2
    exit 1
fi

file "${DIST_DIR}"/*/fed-cdk-tui
for archive in "${DIST_DIR}"/*.zip; do
    unzip -tq "${archive}"
done

echo "==> SHA-256"
shasum -a 256 "${DIST_DIR}"/*.zip
echo "==> Release archives written to ${DIST_DIR}"
echo "note: the macOS binary is ad-hoc signed, not Apple-notarized"
