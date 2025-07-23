#!/usr/bin/env bash

# Simple script to test LDK-Node mint

set -e

# Create temporary directory
TEMP_DIR=$(mktemp -d)
echo "Using temp directory: $TEMP_DIR"

# Cleanup function
cleanup() {
    echo "Cleaning up..."
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

# Set environment variables for LDK-Node mint
export CDK_MINTD_URL="http://127.0.0.1:8089"
export CDK_MINTD_WORK_DIR="$TEMP_DIR/ldk_node_mint"
export CDK_MINTD_LISTEN_HOST="127.0.0.1"
export CDK_MINTD_LISTEN_PORT=8089
export CDK_MINTD_LN_BACKEND="ldk-node"
export CDK_MINTD_MNEMONIC="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"

# LDK Node specific environment variables
export CDK_MINTD_LDK_NODE_BITCOIN_NETWORK="regtest"
export CDK_MINTD_LDK_NODE_CHAIN_SOURCE_TYPE="bitcoinrpc"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_HOST="127.0.0.1"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_PORT=18443
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_USER="testuser"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_PASSWORD="testpass"
export CDK_MINTD_LDK_NODE_STORAGE_DIR_PATH="$TEMP_DIR/ldk_node_mint/ldk_storage"
export CDK_MINTD_LDK_NODE_LDK_NODE_HOST="127.0.0.1"
export CDK_MINTD_LDK_NODE_LDK_NODE_PORT=9090
export CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE="p2p"
export CDK_MINTD_LDK_NODE_FEE_PERCENT=0.02
export CDK_MINTD_LDK_NODE_RESERVE_FEE_MIN=2
export CDK_MINTD_LDK_NODE_WEBSERVER_PORT=9091

# Create storage directory
mkdir -p "$CDK_MINTD_LDK_NODE_STORAGE_DIR_PATH"

echo "Starting LDK-Node mint..."
# Start the mint in background
cargo run --bin cdk-mintd --features ldk-node &
MINT_PID=$!

# Give it some time to start
sleep 5

# Test if it's running
echo "Testing mint info endpoint..."
curl -v http://127.0.0.1:8089/v1/info

# Test creating a mint quote
echo "Testing mint quote creation..."
curl -v -X POST http://127.0.0.1:8089/v1/mint/quote/bolt11 \
  -H "Content-Type: application/json" \
  -d '{"amount": 1000, "unit": "sat"}'

# Kill the mint
echo "Stopping mint..."
kill $MINT_PID
wait $MINT_PID

echo "Test completed successfully!"
