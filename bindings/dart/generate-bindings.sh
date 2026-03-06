#!/usr/bin/env bash
set -euo pipefail

DART_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUST_DIR="$DART_DIR/rust"
OS=$(uname -s)

echo "🚀 CDK Dart Bindings Generator"
echo "===================================="
echo "Running on: $OS"
echo ""

cd "$DART_DIR"
echo "📦 Checking Dart version..."
dart --version
echo ""

echo "📥 Getting Dart dependencies..."
dart pub get
echo ""

if [[ "$OS" == "Darwin" ]]; then
    LIB_EXT="dylib"
elif [[ "$OS" == "Linux" ]]; then
    LIB_EXT="so"
else
    echo "❌ Unsupported OS: $OS"
    exit 1
fi

cd "$RUST_DIR"
echo "🔨 Building cdk_ffi_dart..."
cargo build --release

CDK_FFI_LIB=$(find "$DART_DIR/../../target/release/deps" -name "libcdk_ffi*.$LIB_EXT" -type f | head -1)

if [ -z "$CDK_FFI_LIB" ]; then
    echo "❌ Error: Could not find cdk-ffi library in target/release/deps"
    exit 1
fi

echo "📚 Using cdk-ffi library: $CDK_FFI_LIB"
echo ""

echo "🔧 Generating Dart bindings..."
cargo run --release --bin uniffi-bindgen -- "$CDK_FFI_LIB" --out-dir "$DART_DIR/lib/src/generated"

echo ""
echo "✅ Dart bindings generated successfully!"
echo "📄 Output: $DART_DIR/lib/src/generated/cdk.dart"
