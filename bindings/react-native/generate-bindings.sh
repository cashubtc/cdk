#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# CDK FFI React Native Bindings Generator
# ============================================================================
# Requires: Rust toolchain with iOS + Android targets (provided by nix flake),
#           Node.js, and cargo-ndk (for Android builds).

RN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OS=$(uname -s)

echo "🔨 Building CDK FFI React Native Bindings"
echo "Platform: $OS"
echo ""

# Check for required tools
if ! command -v cargo &> /dev/null; then
    echo "❌ Error: cargo not found. Install Rust from https://rustup.rs"
    exit 1
fi

if ! command -v node &> /dev/null; then
    echo "❌ Error: node not found. Install Node.js"
    exit 1
fi

cd "$RN_DIR"

# Install dependencies without running prepare (generated sources don't exist yet)
echo "📦 Installing dependencies..."
npm install --ignore-scripts

BUILT_ANY=false

# Build for iOS (macOS only)
if [ "$OS" = "Darwin" ]; then
    echo ""
    echo "🍎 Building for iOS..."
    npx ubrn build ios --release --config ubrn.config.yaml --and-generate
    BUILT_ANY=true
else
    echo ""
    echo "⏭️  Skipping iOS build (macOS only)"
fi

# Build for Android (requires cargo-ndk and Android NDK)
if command -v cargo-ndk &> /dev/null || cargo ndk --version &> /dev/null; then
    echo ""
    echo "🤖 Building for Android..."
    npx ubrn build android --release --config ubrn.config.yaml --and-generate
    BUILT_ANY=true
else
    echo ""
    echo "⏭️  Skipping Android build (cargo-ndk not found, install with: cargo install cargo-ndk)"
fi

if [ "$BUILT_ANY" = false ]; then
    echo ""
    echo "❌ Error: No platform was built. Need macOS for iOS or cargo-ndk for Android."
    exit 1
fi

echo ""
echo "📦 Building JS bindings..."
npx bob build

echo ""
echo "✅ Build complete!"
