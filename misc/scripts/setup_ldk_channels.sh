#!/usr/bin/env bash
# Helper script to setup LDK channels after the LDK mint is ready

set -e

CDK_ITESTS_DIR=$1
LDK_MINT_PORT=${2:-8089}

echo "⏳ Waiting for LDK mint to be ready on port $LDK_MINT_PORT..."

# Wait for the LDK mint to be ready (up to 120 seconds)
for i in {1..120}; do
    if curl -s "http://127.0.0.1:$LDK_MINT_PORT/v1/info" > /dev/null 2>&1; then
        echo "✅ LDK mint is ready"
        break
    fi
    if [ $i -eq 120 ]; then
        echo "❌ Timeout waiting for LDK mint to be ready"
        exit 1
    fi
    sleep 1
done

# Give it a few more seconds to fully initialize
echo "⏳ Waiting for LDK node to fully initialize..."
sleep 5

# Run the setup_ldk_channels binary
echo "🔧 Setting up LDK channels..."
cargo run -p cdk-integration-tests --bin setup_ldk_channels -- "$CDK_ITESTS_DIR" --ldk-port "$LDK_MINT_PORT"

echo "✅ LDK channel setup complete!"
