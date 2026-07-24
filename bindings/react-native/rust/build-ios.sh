#!/bin/bash
set -euo pipefail

# Build the cdk-nitro static library for iOS and package it as an XCFramework.
# Output: ios/Frameworks/CdkNitro.xcframework
#
# This matches the podspec's vendored_frameworks entry and the layout the
# release packaging produces in CI, so a local source build is consumable by
# the same Pod without further manual steps.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
OUT_DIR="$ROOT_DIR/ios/Frameworks"

mkdir -p "$OUT_DIR"

# Build for device (arm64) and simulator (arm64)
cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --release --target aarch64-apple-ios
cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --release --target aarch64-apple-ios-sim

TARGET_DIR="$(cargo metadata --manifest-path "$SCRIPT_DIR/Cargo.toml" --format-version 1 | \
  python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')"

# xcodebuild refuses to write to an existing output path.
rm -rf "$OUT_DIR/CdkNitro.xcframework"

xcodebuild -create-xcframework \
  -library "$TARGET_DIR/aarch64-apple-ios/release/libcdk_nitro.a" \
  -library "$TARGET_DIR/aarch64-apple-ios-sim/release/libcdk_nitro.a" \
  -output "$OUT_DIR/CdkNitro.xcframework"

echo "Built: $OUT_DIR/CdkNitro.xcframework"
