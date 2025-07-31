#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    if [ -n "$FAKE_AUTH_MINT_PID" ]; then
        echo "Killing the fake auth mint process"
        kill -2 $FAKE_AUTH_MINT_PID 2>/dev/null || true
        wait $FAKE_AUTH_MINT_PID 2>/dev/null || true
    fi

    echo "Mint binary terminated"
    
    # Remove the temporary directory
    if [ -n "$CDK_ITESTS_DIR" ] && [ -d "$CDK_ITESTS_DIR" ]; then
        rm -rf "$CDK_ITESTS_DIR"
        echo "Temp directory removed: $CDK_ITESTS_DIR"
    fi

    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT
    unset FAKE_AUTH_MINT_PID
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT INT TERM

# Create a temporary directory
export CDK_ITESTS_DIR=$(mktemp -d)
export CDK_ITESTS_MINT_ADDR="127.0.0.1"
export CDK_ITESTS_MINT_PORT=8087

# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $CDK_ITESTS_DIR"

# Check if a database type was provided as first argument, default to sqlite
export MINT_DATABASE="${1:-sqlite}"

# Check if OPENID_DISCOVERY was provided as second argument, default to a test value
export OPENID_DISCOVERY="${2:-http://127.0.0.1:8080/realms/cdk-test-realm/.well-known/openid-configuration}"

# Build the project
cargo build -p cdk-integration-tests 

# Auth configuration
export CDK_TEST_OIDC_USER="cdk-test"
export CDK_TEST_OIDC_PASSWORD="cdkpassword"

# Start the fake auth mint in the background
echo "Starting fake auth mint with discovery URL: $OPENID_DISCOVERY"
echo "Using temp directory: $CDK_ITESTS_DIR"
cargo run -p cdk-integration-tests --bin start_fake_auth_mint -- --enable-logging "$MINT_DATABASE" "$CDK_ITESTS_DIR" "$OPENID_DISCOVERY" "$CDK_ITESTS_MINT_PORT" &

# Store the PID of the mint process
FAKE_AUTH_MINT_PID=$!

# Wait a moment for the mint to start
sleep 5

# Check if the mint is running
if ! kill -0 $FAKE_AUTH_MINT_PID 2>/dev/null; then
    echo "Failed to start fake auth mint"
    exit 1
fi

echo "Fake auth mint started with PID: $FAKE_AUTH_MINT_PID"

# Run cargo test
echo "Running fake auth integration tests..."
cargo test -p cdk-integration-tests --test fake_auth

# Capture the exit status of cargo test
test_status=$?

# Exit with the status of the test
exit $test_status
