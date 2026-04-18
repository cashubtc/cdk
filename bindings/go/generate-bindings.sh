#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"

UNIFFI_BINDGEN_GO_TAG="${UNIFFI_BINDGEN_GO_TAG:-v0.6.0+v0.30.0}"
UNIFFI_BINDGEN_GO_REPO="${UNIFFI_BINDGEN_GO_REPO:-https://github.com/NordSecurity/uniffi-bindgen-go}"

# Install uniffi-bindgen-go if not already available at the right version
if command -v uniffi-bindgen-go >/dev/null 2>&1; then
    INSTALLED_VERSION="$(uniffi-bindgen-go --version 2>/dev/null | awk '{print $2}')"
    REQUESTED_VERSION="${UNIFFI_BINDGEN_GO_TAG#v}"

    if [ "${INSTALLED_VERSION}" != "${REQUESTED_VERSION}" ]; then
        echo "Updating uniffi-bindgen-go from ${INSTALLED_VERSION:-unknown} to ${UNIFFI_BINDGEN_GO_TAG}"
        cargo install uniffi-bindgen-go \
            --git "${UNIFFI_BINDGEN_GO_REPO}" \
            --tag "${UNIFFI_BINDGEN_GO_TAG}" \
            --locked --force
    fi
else
    echo "Installing uniffi-bindgen-go ${UNIFFI_BINDGEN_GO_TAG}..."
    cargo install uniffi-bindgen-go \
        --git "${UNIFFI_BINDGEN_GO_REPO}" \
        --tag "${UNIFFI_BINDGEN_GO_TAG}" \
        --locked
fi

# Platform detection
if [[ "${OSTYPE:-}" == darwin* ]]; then
    LIB_EXT="dylib"
    PLATFORM_OS="darwin"
else
    LIB_EXT="so"
    PLATFORM_OS="linux"
fi

UNAME_ARCH="$(uname -m)"
case "${UNAME_ARCH}" in
    x86_64)         PLATFORM_ARCH="amd64" ;;
    aarch64|arm64)  PLATFORM_ARCH="arm64" ;;
    *)
        echo "Unsupported architecture: ${UNAME_ARCH}" >&2
        exit 1
        ;;
esac

PLATFORM_KEY="${PLATFORM_OS}_${PLATFORM_ARCH}"
PACKAGE_DIR="${SCRIPT_DIR}/cdkffi"

# Build the cdk-ffi-go cdylib
pushd "${ROOT_DIR}" >/dev/null
cargo build --release -p cdk-ffi-go
popd >/dev/null

LIB_FILE="${ROOT_DIR}/target/release/libcdk_ffi_go.${LIB_EXT}"
if [ ! -f "${LIB_FILE}" ]; then
    echo "ERROR: Could not find ${LIB_FILE}"
    ls -la "${ROOT_DIR}/target/release/libcdk_ffi_go"* || true
    exit 1
fi

# Clean previous generation
rm -rf "${PACKAGE_DIR}"

# Generate Go bindings using uniffi-bindgen-go
pushd "${ROOT_DIR}" >/dev/null
uniffi-bindgen-go "${LIB_FILE}" \
    --library \
    --config bindings/go/rust/uniffi.toml \
    --out-dir bindings/go
popd >/dev/null

# Copy native library to platform-specific directory
NATIVE_DIR="${PACKAGE_DIR}/native/${PLATFORM_KEY}"
mkdir -p "${NATIVE_DIR}"
cp "${LIB_FILE}" "${NATIVE_DIR}/libcdk_ffi_go.${LIB_EXT}"

if [[ "${PLATFORM_OS}" == "darwin" ]]; then
    install_name_tool -id "@rpath/libcdk_ffi_go.dylib" "${NATIVE_DIR}/libcdk_ffi_go.dylib"
fi

# Create CGO link file for the current platform
if [[ "${PLATFORM_OS}" == "linux" ]]; then
    EXTRA_LIBS="-lm -ldl"
else
    EXTRA_LIBS="-lm"
fi

cat > "${PACKAGE_DIR}/link_${PLATFORM_KEY}.go" <<GOEOF
//go:build ${PLATFORM_OS} && ${PLATFORM_ARCH}

package cdk_ffi

// #cgo LDFLAGS: -L\${SRCDIR}/native/${PLATFORM_KEY} -lcdk_ffi_go -Wl,-rpath,\${SRCDIR}/native/${PLATFORM_KEY} ${EXTRA_LIBS}
import "C"
GOEOF

echo "Generated Go bindings in ${PACKAGE_DIR} (${PLATFORM_KEY})"
