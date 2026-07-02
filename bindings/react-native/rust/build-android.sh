#!/bin/bash
set -euo pipefail

# Build the cdk-nitro static library for Android targets
# Requires: Android NDK, cargo-ndk

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
JNILIBS_DIR="$ROOT_DIR/android/src/main/jniLibs"

mkdir -p "$JNILIBS_DIR"/{arm64-v8a,armeabi-v7a,x86_64}

# Build for all Android ABIs
cargo ndk \
  --manifest-path "$SCRIPT_DIR/Cargo.toml" \
  --target aarch64-linux-android \
  --target armv7-linux-androideabi \
  --target x86_64-linux-android \
  --platform 24 \
  -- build --release

TARGET_DIR="$(cargo metadata --manifest-path "$SCRIPT_DIR/Cargo.toml" --format-version 1 | \
  python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])')"

cp "$TARGET_DIR/aarch64-linux-android/release/libcdk_nitro.a" "$JNILIBS_DIR/arm64-v8a/"
cp "$TARGET_DIR/armv7-linux-androideabi/release/libcdk_nitro.a" "$JNILIBS_DIR/armeabi-v7a/"
cp "$TARGET_DIR/x86_64-linux-android/release/libcdk_nitro.a" "$JNILIBS_DIR/x86_64/"

echo "Built Android native libraries:"
ls -la "$JNILIBS_DIR"/*/libcdk_nitro.a
