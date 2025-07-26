#!/bin/bash

# CDK Wallet Simulator Script

# Default values
MINT_URL="${MINT_URL:-http://127.0.0.1:8085}"
CURRENCY_UNIT="${CURRENCY_UNIT:-Sat}"
TRANSACTION_COUNT="${TRANSACTION_COUNT:-10}"

# Set environment variables
export MINT_URL
export CURRENCY_UNIT
export TRANSACTION_COUNT

echo "Running CDK Wallet Simulator"
echo "============================="
echo "MINT_URL: $MINT_URL"
echo "CURRENCY_UNIT: $CURRENCY_UNIT"
echo "TRANSACTION_COUNT: $TRANSACTION_COUNT"
echo "============================="

# Run the wallet simulator
cd "$(dirname "$0")"
cargo run --bin wallet_simulator
