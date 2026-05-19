#!/bin/bash
set -euo pipefail

# Build the cdk-nitro static library for iOS targets
# Outputs: ios/Frameworks/libcdk_nitro.a (universal fat binary)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$ROOT_DIR/ios/Frameworks"

mkdir -p "$OUT_DIR"

# Build for each iOS target
cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --release --target aarch64-apple-ios
cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --release --target aarch64-apple-ios-sim

# For device (arm64 only)
cp "$(cargo metadata --manifest-path "$SCRIPT_DIR/Cargo.toml" --format-version 1 | \
  python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')/aarch64-apple-ios/release/libcdk_nitro.a" \
  "$OUT_DIR/libcdk_nitro.a"

echo "Built: $OUT_DIR/libcdk_nitro.a"
echo ""
echo "For simulator, the library is at:"
echo "  target/aarch64-apple-ios-sim/release/libcdk_nitro.a"
echo ""
echo "To create an XCFramework:"
echo "  xcodebuild -create-xcframework \\"
echo "    -library target/aarch64-apple-ios/release/libcdk_nitro.a \\"
echo "    -library target/aarch64-apple-ios-sim/release/libcdk_nitro.a \\"
echo "    -output ios/Frameworks/CdkNitro.xcframework"
