#!/bin/bash
set -e

# Build script for creating Swift bindings and XCFramework
# This script builds the Rust library for iOS and macOS, generates Swift bindings,
# and packages everything into an XCFramework suitable for distribution.

SWIFT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$SWIFT_DIR/rust"
BUILD_DIR="$SWIFT_DIR/build"
TARGET_DIR="$SWIFT_DIR/../../target"
XCFRAMEWORK_DIR="$BUILD_DIR/xcframework"

echo "🔨 Building CDK Swift bindings..."
echo "Rust dir: $RUST_DIR"
echo "Build dir: $BUILD_DIR"

# Clean previous build
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"
mkdir -p "$XCFRAMEWORK_DIR"

cd "$RUST_DIR"

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

# Build for iOS device (arm64)
echo "🍎 Building for iOS device (arm64)..."
if [[ "$(uname)" == "Linux" ]]; then
    env -u MACOSX_DEPLOYMENT_TARGET SDKROOT="$IOS_SDK" cargo build --release --target aarch64-apple-ios
else
    env -u MACOSX_DEPLOYMENT_TARGET -u SDKROOT cargo build --release --target aarch64-apple-ios
fi

# Build for iOS simulator (arm64 + x86_64)
echo "📱 Building for iOS simulator (arm64)..."
if [[ "$(uname)" == "Linux" ]]; then
    env -u MACOSX_DEPLOYMENT_TARGET SDKROOT="$IOS_SIM_SDK" cargo build --release --target aarch64-apple-ios-sim
else
    env -u MACOSX_DEPLOYMENT_TARGET -u SDKROOT cargo build --release --target aarch64-apple-ios-sim
fi

echo "📱 Building for iOS simulator (x86_64)..."
if [[ "$(uname)" == "Linux" ]]; then
    env -u MACOSX_DEPLOYMENT_TARGET SDKROOT="$IOS_SIM_SDK" cargo build --release --target x86_64-apple-ios
else
    env -u MACOSX_DEPLOYMENT_TARGET -u SDKROOT cargo build --release --target x86_64-apple-ios
fi

# Build for macOS (arm64 + x86_64)
echo "💻 Building for macOS (arm64)..."
cargo build --release --target aarch64-apple-darwin

echo "💻 Building for macOS (x86_64)..."
cargo build --release --target x86_64-apple-darwin

# Create universal binaries
echo "🔗 Creating universal binaries..."

# iOS simulator universal binary
mkdir -p "$BUILD_DIR/ios-simulator"
lipo -create \
    "$TARGET_DIR/aarch64-apple-ios-sim/release/libcdk_ffi_swift.a" \
    "$TARGET_DIR/x86_64-apple-ios/release/libcdk_ffi_swift.a" \
    -output "$BUILD_DIR/ios-simulator/libcdk_ffi_swift.a"

# macOS universal binary
mkdir -p "$BUILD_DIR/macos"
lipo -create \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$TARGET_DIR/x86_64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    -output "$BUILD_DIR/macos/libcdk_ffi_swift.dylib"

# Create framework structure for each platform
echo "📦 Creating framework structures..."

# iOS device framework
IOS_DEVICE_FRAMEWORK="$BUILD_DIR/ios-device/CashuDevKitFFI.framework"
mkdir -p "$IOS_DEVICE_FRAMEWORK"
cp "$TARGET_DIR/aarch64-apple-ios/release/libcdk_ffi_swift.a" "$IOS_DEVICE_FRAMEWORK/CashuDevKitFFI"
cp "$SWIFT_DIR/resources/Info-iOS.plist" "$IOS_DEVICE_FRAMEWORK/Info.plist"

# iOS simulator framework
IOS_SIM_FRAMEWORK="$BUILD_DIR/ios-simulator-framework/CashuDevKitFFI.framework"
mkdir -p "$IOS_SIM_FRAMEWORK"
cp "$BUILD_DIR/ios-simulator/libcdk_ffi_swift.a" "$IOS_SIM_FRAMEWORK/CashuDevKitFFI"
cp "$SWIFT_DIR/resources/Info-iOSSimulator.plist" "$IOS_SIM_FRAMEWORK/Info.plist"

# macOS framework (versioned bundle layout)
MACOS_FRAMEWORK="$BUILD_DIR/macos-framework/CashuDevKitFFI.framework"
mkdir -p "$MACOS_FRAMEWORK/Versions/A/Resources"
cp "$BUILD_DIR/macos/libcdk_ffi_swift.dylib" "$MACOS_FRAMEWORK/Versions/A/CashuDevKitFFI"
cp "$SWIFT_DIR/resources/Info-macOS.plist" "$MACOS_FRAMEWORK/Versions/A/Resources/Info.plist"

# Create symlinks for versioned framework structure
ln -s A "$MACOS_FRAMEWORK/Versions/Current"
ln -s Versions/Current/CashuDevKitFFI "$MACOS_FRAMEWORK/CashuDevKitFFI"
ln -s Versions/Current/Headers "$MACOS_FRAMEWORK/Headers"
ln -s Versions/Current/Modules "$MACOS_FRAMEWORK/Modules"
ln -s Versions/Current/Resources "$MACOS_FRAMEWORK/Resources"

# Generate Swift bindings using uniffi-bindgen-swift
echo "🦀 Generating Swift bindings..."

# Generate headers and modulemaps for iOS device
echo "📱 Generating iOS device bindings..."
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-ios/release/libcdk_ffi_swift.a" \
    "$IOS_DEVICE_FRAMEWORK/Headers" \
    --headers

cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-ios/release/libcdk_ffi_swift.a" \
    "$IOS_DEVICE_FRAMEWORK/Modules" \
    --xcframework \
    --modulemap \
    --module-name CashuDevKitFFI \
    --modulemap-filename module.modulemap

# Generate headers and modulemaps for iOS simulator
echo "📱 Generating iOS simulator bindings..."
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-ios-sim/release/libcdk_ffi_swift.a" \
    "$IOS_SIM_FRAMEWORK/Headers" \
    --headers

cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-ios-sim/release/libcdk_ffi_swift.a" \
    "$IOS_SIM_FRAMEWORK/Modules" \
    --xcframework \
    --modulemap \
    --module-name CashuDevKitFFI \
    --modulemap-filename module.modulemap

# Generate headers and modulemaps for macOS
echo "💻 Generating macOS bindings..."
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$MACOS_FRAMEWORK/Versions/A/Headers" \
    --headers

cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$MACOS_FRAMEWORK/Versions/A/Modules" \
    --xcframework \
    --modulemap \
    --module-name CashuDevKitFFI \
    --modulemap-filename module.modulemap

# Generate Swift source files (only need once)
echo "📝 Generating Swift source files..."
SOURCES_DIR="$SWIFT_DIR/Sources/Cdk"
mkdir -p "$SOURCES_DIR"
cargo run --bin uniffi-bindgen-swift -- \
    "$TARGET_DIR/aarch64-apple-darwin/release/libcdk_ffi_swift.dylib" \
    "$SOURCES_DIR" \
    --swift-sources

# Re-sign the macOS framework. lipo invalidates the per-arch linker signatures,
# and macOS will SIGKILL the process if the signature is invalid.
# Sign the entire framework bundle so CodeResources is properly generated.
if [[ "$(uname)" == "Linux" ]]; then
    rcodesign sign "$MACOS_FRAMEWORK"
else
    codesign --force --sign - "$MACOS_FRAMEWORK"
fi

# Create XCFramework
echo "📦 Creating XCFramework..."
echo "xcodebuild resolved to: $(command -v xcodebuild)"
xcodebuild -version || true
xcodebuild -create-xcframework \
    -framework "$IOS_DEVICE_FRAMEWORK" \
    -framework "$IOS_SIM_FRAMEWORK" \
    -framework "$MACOS_FRAMEWORK" \
    -output "$XCFRAMEWORK_DIR/CashuDevKitFFI.xcframework"

# Generate Package.swift at repository root for SPM
REPO_ROOT="$SWIFT_DIR/../.."
cat > "$REPO_ROOT/Package.swift" << 'PKGSWIFT'
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "cdk-swift",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "Cdk", targets: ["Cdk"]),
    ],
    targets: [
        .binaryTarget(
            name: "CashuDevKitFFI",
            path: "bindings/swift/build/xcframework/CashuDevKitFFI.xcframework"
        ),
        .target(
            name: "Cdk",
            dependencies: ["CashuDevKitFFI"],
            path: "bindings/swift/Sources/Cdk"
        ),
        .testTarget(
            name: "CdkTests",
            dependencies: ["Cdk"],
            path: "bindings/swift/Tests"
        ),
    ]
)
PKGSWIFT
echo "📦 Generated Package.swift at repository root"

# Create zip for distribution
echo "📦 Creating distribution zip..."
cd "$XCFRAMEWORK_DIR"
zip -r CashuDevKitFFI.xcframework.zip CashuDevKitFFI.xcframework
CHECKSUM=$(sha256sum CashuDevKitFFI.xcframework.zip | cut -d' ' -f1)

echo ""
echo "✅ Build complete!"
echo ""
echo "📦 XCFramework: $XCFRAMEWORK_DIR/CashuDevKitFFI.xcframework.zip"
echo "📝 Swift sources: $SOURCES_DIR"
echo ""
echo "📊 Checksum for Package.swift:"
echo "   $CHECKSUM"
echo ""
echo "Update Package.swift with this checksum!"
