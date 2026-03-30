#!/bin/bash
set -euo pipefail

KOTLIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$KOTLIN_DIR/rust"
JVM_MODULE="$KOTLIN_DIR/cdk-jvm"

OS=$(uname -s)
ARCH=$(uname -m)

echo "Building CDK Kotlin JVM bindings..."

# Clean previous generated code
rm -rf "$JVM_MODULE/src/main/kotlin/org"
rm -rf "$JVM_MODULE/src/main/resources/libcdk_ffi"*

cd "$RUST_DIR"

# Resolve workspace target directory (crate is a workspace member, so output goes to workspace root)
WORKSPACE_TARGET="$(cargo locate-project --workspace --message-format plain | xargs dirname)/target"

# Build for host platform
case "$OS" in
    Linux)
        TARGET="x86_64-unknown-linux-gnu"
        LIB_EXT="so"
        ;;
    Darwin)
        if [ "$ARCH" = "arm64" ]; then
            TARGET="aarch64-apple-darwin"
        else
            TARGET="x86_64-apple-darwin"
        fi
        LIB_EXT="dylib"
        ;;
    *)
        echo "Unsupported OS: $OS"
        exit 1
        ;;
esac

echo "Building Rust library for $TARGET..."
cargo build --release --target "$TARGET"

LIB_PATH="$WORKSPACE_TARGET/$TARGET/release/libcdk_ffi_kotlin.$LIB_EXT"

if [ ! -f "$LIB_PATH" ]; then
    echo "Library not found at $LIB_PATH, trying default target..."
    cargo build --release
    LIB_PATH="$WORKSPACE_TARGET/release/libcdk_ffi_kotlin.$LIB_EXT"
fi

echo "Library built at: $LIB_PATH"

# Generate Kotlin bindings
echo "Generating Kotlin bindings..."
mkdir -p "$KOTLIN_DIR/build/kotlin"
cargo run --release --bin uniffi-bindgen -- generate \
    --library "$LIB_PATH" \
    --language kotlin \
    --out-dir "$KOTLIN_DIR/build/kotlin" \
    --no-format

# Copy generated Kotlin to JVM module
echo "Setting up JVM module..."
mkdir -p "$JVM_MODULE/src/main/kotlin"
cp -r "$KOTLIN_DIR/build/kotlin/org" "$JVM_MODULE/src/main/kotlin/"

# Copy native library to resources (JNA looks for "cdk_ffi" not "cdk_ffi_kotlin")
mkdir -p "$JVM_MODULE/src/main/resources"
cp "$LIB_PATH" "$JVM_MODULE/src/main/resources/libcdk_ffi.$LIB_EXT"

# Strip debug symbols
if [ "$OS" = "Darwin" ]; then
    strip -x "$JVM_MODULE/src/main/resources/libcdk_ffi.$LIB_EXT" 2>/dev/null || true
else
    strip --strip-all "$JVM_MODULE/src/main/resources/libcdk_ffi.$LIB_EXT" 2>/dev/null || true
fi

echo ""
echo "Done! Generated Kotlin bindings in $JVM_MODULE/src/main/kotlin/org/"
echo "Native library copied to $JVM_MODULE/src/main/resources/"
echo ""
echo "Next: cd $KOTLIN_DIR && ./gradlew :cdk-jvm:build"
