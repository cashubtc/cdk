#!/usr/bin/env bash
#
# Build the cdk-python wheel for the host platform.
#
# Self-contained: it needs only a Rust toolchain and Python. rust/ depends on a
# published cdk-ffi, so no checkout of the cdk monorepo is required.

set -euo pipefail

cd "$(dirname "$0")"

PKG_DIR=src/cdk

case "$(uname -s)" in
  Darwin)          LIB_EXT=dylib; LIB_PREFIX=lib ;;
  MINGW*|MSYS*|CYGWIN*) LIB_EXT=dll; LIB_PREFIX=  ;;
  *)               LIB_EXT=so;    LIB_PREFIX=lib ;;
esac

# uniffi names the library it loads after the cdk-ffi namespace rather than the
# wrapper crate, so the built cdylib is renamed on the way into the package.
INSTALLED="${PKG_DIR}/${LIB_PREFIX}cdk_ffi.${LIB_EXT}"

echo "Building cdk-ffi-python..."
(cd rust && cargo build --release)

# Standalone this is rust/target, but inside the cdk workspace it is the
# workspace target dir, so ask cargo rather than assuming.
TARGET_DIR=$(cd rust && cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
BUILT="${TARGET_DIR}/release/${LIB_PREFIX}cdk_ffi_python.${LIB_EXT}"

if [[ ! -f "$BUILT" ]]; then
  echo "error: expected the built library at $BUILT" >&2
  exit 1
fi

echo "Generating Python bindings..."
rm -rf generated-bindings && mkdir -p generated-bindings
(cd rust && cargo run --release --bin uniffi-bindgen-python -- generate \
  --library "$BUILT" \
  --language python \
  --out-dir ../generated-bindings \
  --no-format)

if [[ ! -s generated-bindings/cdk_ffi.py ]]; then
  echo "error: uniffi-bindgen produced no bindings." >&2
  echo "  It reads metadata from the library's symbol table, so a stripped" >&2
  echo "  build yields nothing. Check that rust/Cargo.toml's [profile.release]" >&2
  echo "  does not set strip = true." >&2
  exit 1
fi

echo "Assembling the package..."
rm -rf build dist
find "$PKG_DIR" -type f \( -name '*.so' -o -name '*.dylib' -o -name '*.dll' \) -delete
cp generated-bindings/cdk_ffi.py "${PKG_DIR}/cdk_ffi.py"
cp "$BUILT" "$INSTALLED"

# Strip only after generating, never through the cargo profile, for the reason
# above. Best effort: a missing strip costs size, not correctness.
if command -v strip >/dev/null 2>&1; then
  case "$(uname -s)" in
    Darwin) strip -S "$INSTALLED" ;;
    *)      strip --strip-all "$INSTALLED" ;;
  esac
else
  echo "note: strip not found, shipping an unstripped library"
fi

# The nix ffi shell already provides build and wheel. Elsewhere the system
# interpreter is often externally managed and refuses installs, so fall back to
# a throwaway venv rather than touching it.
if python3 -c 'import build, wheel' 2>/dev/null; then
  PY=python3
else
  BUILD_VENV=".build-venv"
  # Check that the venv actually works rather than that it merely exists: a
  # copied or half-installed one has absolute paths pointing somewhere else.
  if ! "$BUILD_VENV/bin/python" -c 'import build, wheel' 2>/dev/null; then
    rm -rf "$BUILD_VENV"
    python3 -m venv "$BUILD_VENV"
    "$BUILD_VENV/bin/pip" install --quiet --upgrade pip "build" "wheel>=0.42"
  fi
  PY="$BUILD_VENV/bin/python"
fi

echo "Building wheel..."
"$PY" -m build --wheel

# The library is loaded with ctypes rather than linked as a CPython extension,
# so the wheel is valid for any Python 3 and is retagged to say so.
PLATFORM=$("$PY" -c 'import sysconfig; print(sysconfig.get_platform().replace("-", "_").replace(".", "_"))')
"$PY" -m wheel tags \
  --python-tag py3 \
  --abi-tag none \
  --platform-tag "$PLATFORM" \
  --remove \
  dist/*.whl

echo "Wheel built:"
ls -1 dist/*.whl
