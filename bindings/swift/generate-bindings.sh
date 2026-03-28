#!/bin/bash
set -euo pipefail

# Build script for generating Swift bindings used by local SwiftPM tests.

SWIFT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$SWIFT_DIR/rust"
BUILD_DIR="$SWIFT_DIR/.build"
TARGET_DIR="$SWIFT_DIR/../../target"

echo "🔨 Building CDK Swift bindings..."
echo "Rust dir: $RUST_DIR"
echo "Build dir: $BUILD_DIR"

# Clean previous build
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR/macos"
rm -rf "$SWIFT_DIR/cdkFFI"
mkdir -p "$SWIFT_DIR/cdkFFI"
mkdir -p "$SWIFT_DIR/Sources/Cdk"

cd "$RUST_DIR"

# Install the macOS targets needed for local SwiftPM validation.
echo "📦 Ensuring Rust targets are installed..."
rustup target add aarch64-apple-darwin
rustup target add x86_64-apple-darwin

# Set cross-compilation toolchain for macOS targets when building on Linux.
# cc-rs (used by build scripts like secp256k1-sys) picks CC_<target> to find the C compiler.
# Without these it falls back to the host "cc" (Linux GCC) which doesn't understand
# -arch or -mmacosx-version-min flags.
if [[ "$(uname)" == "Linux" ]]; then
    OSXCROSS_BIN=/usr/local/osxcross/bin
    DARWIN_TRIPLE=aarch64-apple-darwin24.4
    DARWIN_X86_TRIPLE=x86_64-apple-darwin24.4

    export CC_aarch64_apple_darwin=$OSXCROSS_BIN/$DARWIN_TRIPLE-clang
    export AR_aarch64_apple_darwin=$OSXCROSS_BIN/$DARWIN_TRIPLE-ar
    export CC_x86_64_apple_darwin=$OSXCROSS_BIN/$DARWIN_X86_TRIPLE-clang
    export AR_x86_64_apple_darwin=$OSXCROSS_BIN/$DARWIN_X86_TRIPLE-ar
fi

# On macOS, point DEVELOPER_DIR at the real Xcode installation so xcrun can
# resolve the active developer toolchain consistently.
if [[ "$(uname)" == "Darwin" ]]; then
    XCODE_DEV_DIR=$(/usr/bin/xcrun xcode-select -p 2>/dev/null || echo "/Applications/Xcode.app/Contents/Developer")
    export DEVELOPER_DIR="$XCODE_DEV_DIR"
fi

echo "💻 Building for macOS (arm64)..."
cargo build --release --target aarch64-apple-darwin

echo "💻 Building for macOS (x86_64)..."
cargo build --release --target x86_64-apple-darwin

echo "🔗 Creating universal macOS binary..."
lipo -create \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$TARGET_DIR/x86_64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    -output "$BUILD_DIR/macos/libcdk_ffi_swift.dylib"

echo "🦀 Generating Swift bindings..."
echo "📝 Generating Swift source files..."
SOURCES_DIR="$SWIFT_DIR/Sources/Cdk"
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$SOURCES_DIR" \
    --swift-sources

echo "📚 Generating Swift C header..."
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$SWIFT_DIR/cdkFFI" \
    --headers

cp "$BUILD_DIR/macos/libcdk_ffi_swift.dylib" "$SWIFT_DIR/.build/libcdk_ffi_swift.dylib"

HEADER_PATHS=("$SWIFT_DIR"/cdkFFI/*.h)
if [[ ${#HEADER_PATHS[@]} -ne 1 || ! -f "${HEADER_PATHS[0]}" ]]; then
    echo "❌ Expected exactly one generated Swift header in $SWIFT_DIR/cdkFFI" >&2
    exit 1
fi

mv "${HEADER_PATHS[0]}" "$SWIFT_DIR/cdkFFI/cdkFFI.h"

echo ""
echo "✅ Build complete!"
echo ""
echo "📦 Swift package: $SWIFT_DIR/Package.swift"
echo "📚 Swift C module: $SWIFT_DIR/cdkFFI"
echo "📝 Swift sources: $SOURCES_DIR"
echo "🔗 macOS library: $SWIFT_DIR/.build/libcdk_ffi_swift.dylib"
