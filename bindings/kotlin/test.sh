#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
RUST_DIR="$SCRIPT_DIR/rust"

# Detect host target
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  HOST_TARGET="x86_64-unknown-linux-gnu"   ; LIB="libcdk_ffi.so"   ;;
  Linux-aarch64) HOST_TARGET="aarch64-unknown-linux-gnu"   ; LIB="libcdk_ffi.so"   ;;
  Darwin-arm64)  HOST_TARGET="aarch64-apple-darwin"        ; LIB="libcdk_ffi.dylib" ;;
  Darwin-x86_64) HOST_TARGET="x86_64-apple-darwin"         ; LIB="libcdk_ffi.dylib" ;;
  *)             echo "Unsupported platform"; exit 1 ;;
esac

echo "==> Building native library ($HOST_TARGET)"
cargo build --release --target "$HOST_TARGET" --manifest-path "$RUST_DIR/Cargo.toml"

BUILT_LIB="$RUST_DIR/target/$HOST_TARGET/release/$LIB"

echo "==> Generating Kotlin bindings"
cargo run --release --manifest-path "$RUST_DIR/Cargo.toml" --bin uniffi-bindgen -- generate \
  --library "$BUILT_LIB" \
  --language kotlin \
  --out-dir "$SCRIPT_DIR/cdk-jvm/src/main/kotlin" \
  --no-format

echo "==> Copying native library to resources"
mkdir -p "$SCRIPT_DIR/cdk-jvm/src/main/resources"
cp "$BUILT_LIB" "$SCRIPT_DIR/cdk-jvm/src/main/resources/"

echo "==> Running tests"
cd "$SCRIPT_DIR"
./gradlew :cdk-jvm:test "$@"
