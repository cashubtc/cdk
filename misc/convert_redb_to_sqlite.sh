#!/usr/bin/env bash

set -euo pipefail

# Configuration
BINARY_URL="https://github.com/thesimplekid/cdk-convert-redb-to-sqlite/releases/download/v0.1.1/cdk-convert-redb-to-sqlite"
BINARY_NAME="cdk-convert-redb-to-sqlite"
EXPECTED_SHA256="5af74b7fa1d20a8e53694bb31c6234dfdaae9d8222047e361785cc8420496709"

# Create temporary directory for downloading
TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading binary to temporary directory: $TMP_DIR"
cd "$TMP_DIR" || exit 1

echo "Downloading binary from $BINARY_URL..."
if ! curl -L -o "$BINARY_NAME" "$BINARY_URL"; then
    echo "Failed to download binary"
    exit 1
fi

# Verify SHA256 checksum
echo "Verifying SHA256 checksum..."
ACTUAL_SHA256=$(sha256sum "$BINARY_NAME" | cut -d ' ' -f 1)

if [ "$ACTUAL_SHA256" != "$EXPECTED_SHA256" ]; then
    echo "Checksum verification failed!"
    echo "Expected: $EXPECTED_SHA256"
    echo "Got:      $ACTUAL_SHA256"
    exit 1
fi

echo "Checksum verified successfully!"

echo "Making binary executable..."
if ! chmod +x "$BINARY_NAME"; then
    echo "Failed to make binary executable"
    exit 1
fi

echo "Running binary from temporary directory: $TMP_DIR"
"$TMP_DIR/$BINARY_NAME"
