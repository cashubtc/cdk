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

# Install targets if needed. iOS targets stay here so the existing nix/rustup
# setup continues to provision them for contributors, even though Swift tests
# now only require the macOS build path.
echo "📦 Ensuring Rust targets are installed..."
rustup target add aarch64-apple-ios
rustup target add x86_64-apple-ios
rustup target add aarch64-apple-ios-sim
rustup target add aarch64-apple-darwin
rustup target add x86_64-apple-darwin

# Set cross-compilation toolchain for Apple targets when building on Linux.
# cc-rs (used by build scripts like secp256k1-sys) picks CC_<target> to find the C compiler.
# Without these it falls back to the host "cc" (Linux GCC) which doesn't understand
# -arch or -mmacosx-version-min flags.
if [[ "$(uname)" == "Linux" ]]; then
    OSXCROSS_BIN=/usr/local/osxcross/bin
    DARWIN_TRIPLE=aarch64-apple-darwin24.4
    DARWIN_X86_TRIPLE=x86_64-apple-darwin24.4
    IOS_SDK=/usr/local/ios-sdk/iPhoneOS18.4.sdk
    IOS_SIM_SDK=/usr/local/ios-sdk/iPhoneSimulator18.4.sdk

    export CC_aarch64_apple_darwin=$OSXCROSS_BIN/$DARWIN_TRIPLE-clang
    export AR_aarch64_apple_darwin=$OSXCROSS_BIN/$DARWIN_TRIPLE-ar
    export CC_x86_64_apple_darwin=$OSXCROSS_BIN/$DARWIN_X86_TRIPLE-clang
    export AR_x86_64_apple_darwin=$OSXCROSS_BIN/$DARWIN_X86_TRIPLE-ar

    # iOS: use system clang, NOT the osxcross darwin wrapper.
    # The osxcross darwin wrapper unconditionally adds -mmacosx-version-min, which
    # conflicts with -miphoneos-version-min that cc-rs adds for iOS targets.
    export CC_aarch64_apple_ios=/usr/bin/clang
    export AR_aarch64_apple_ios=$OSXCROSS_BIN/$DARWIN_TRIPLE-ar
    export CFLAGS_aarch64_apple_ios="-isysroot $IOS_SDK -target arm64-apple-ios14.0"

    export CC_aarch64_apple_ios_sim=/usr/bin/clang
    export AR_aarch64_apple_ios_sim=$OSXCROSS_BIN/$DARWIN_TRIPLE-ar
    export CFLAGS_aarch64_apple_ios_sim="-isysroot $IOS_SIM_SDK -target arm64-apple-ios14.0-simulator"

    export CC_x86_64_apple_ios=/usr/bin/clang
    export AR_x86_64_apple_ios=$OSXCROSS_BIN/$DARWIN_X86_TRIPLE-ar
    export CFLAGS_x86_64_apple_ios="-isysroot $IOS_SIM_SDK -target x86_64-apple-ios14.0-simulator"
fi

# On macOS, point DEVELOPER_DIR at the real Xcode installation so that
# xcrun (including Nix's xcbuild wrapper) can find the iOS SDKs.
if [[ "$(uname)" == "Darwin" ]]; then
    XCODE_DEV_DIR=$(/usr/bin/xcrun xcode-select -p 2>/dev/null || echo "/Applications/Xcode.app/Contents/Developer")
    export DEVELOPER_DIR="$XCODE_DEV_DIR"

    # Use the system clang for iOS targets to avoid the nix-wrapped clang
    # injecting -mmacos-version-min, which conflicts with -miphoneos-version-min.
    export CC_aarch64_apple_ios=/usr/bin/clang
    export CC_aarch64_apple_ios_sim=/usr/bin/clang
    export CC_x86_64_apple_ios=/usr/bin/clang

    # Also override the linker for iOS targets. rustc uses the default `cc`
    # (Nix-wrapped) as linker, which hardcodes the macOS sysroot and causes
    # "building for iOS Simulator, but linking in .tbd built for macOS" errors.
    export CARGO_TARGET_AARCH64_APPLE_IOS_LINKER=/usr/bin/clang
    export CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER=/usr/bin/clang
    export CARGO_TARGET_X86_64_APPLE_IOS_LINKER=/usr/bin/clang
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
