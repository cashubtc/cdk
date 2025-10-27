#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk regtest and mints"
    if [ ! -z "$CDK_REGTEST_PID" ]; then
        # First try graceful shutdown with SIGTERM
        kill -15 $CDK_REGTEST_PID 2>/dev/null
        sleep 2
        
        # Check if process is still running, if so force kill with SIGKILL
        if ps -p $CDK_REGTEST_PID > /dev/null 2>&1; then
            echo "Process still running, force killing..."
            kill -9 $CDK_REGTEST_PID 2>/dev/null
        fi
        
        # Wait for process to terminate
        wait $CDK_REGTEST_PID 2>/dev/null || true
    fi

    echo "Mint binary terminated"

    # # Remove the temporary directory
    # if [ ! -z "$CDK_ITESTS_DIR" ] && [ -d "$CDK_ITESTS_DIR" ]; then
    #     rm -rf "$CDK_ITESTS_DIR"
    #     echo "Temp directory removed: $CDK_ITESTS_DIR"
    # fi
    
    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT_0
    unset CDK_ITESTS_MINT_PORT_1
    unset CDK_MINTD_DATABASE
    unset CDK_TEST_MINT_URL
    unset CDK_TEST_MINT_URL_2
    unset CDK_REGTEST_PID
    unset RUST_BACKTRACE
    unset CDK_TEST_REGTEST
    unset CDK_TEST_LIGHTNING_CLIENT
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

export CDK_TEST_REGTEST=1

# Create a temporary directory
export CDK_ITESTS_DIR=$(mktemp -d)
export CDK_ITESTS_MINT_ADDR="127.0.0.1"
export CDK_ITESTS_MINT_PORT_0=8085
export CDK_ITESTS_MINT_PORT_1=8087

# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $CDK_ITESTS_DIR"
export CDK_MINTD_DATABASE="$1"

echo "========================================="
echo "Building all required binaries..."
echo "========================================="
BUILD_START=$(date +%s)

# Build in order of increasing feature sets to maximize cache reuse:
# 1. Build integration tests with all features first (superset)
# 2. Then build the binary (reuses shared dependencies)
# This way, the http_subscription build includes everything from base build

echo "[1/2] Building integration test binaries with all features..."
STEP_START=$(date +%s)
cargo build --tests -p cdk-integration-tests --features http_subscription
if [ $? -ne 0 ]; then
    echo "ERROR: Failed to build integration tests with http_subscription feature"
    exit 1
fi
STEP_END=$(date +%s)
STEP_ELAPSED=$((STEP_END - STEP_START))
echo "SUCCESS: Integration tests with http_subscription built in ${STEP_ELAPSED}s"
echo ""

echo "[2/2] Building start_regtest_mints binary..."
STEP_START=$(date +%s)
cargo build --bin start_regtest_mints
if [ $? -ne 0 ]; then
    echo "ERROR: Failed to build start_regtest_mints"
    exit 1
fi
STEP_END=$(date +%s)
STEP_ELAPSED=$((STEP_END - STEP_START))
echo "SUCCESS: start_regtest_mints built in ${STEP_ELAPSED}s"
echo ""

BUILD_END=$(date +%s)
BUILD_ELAPSED=$((BUILD_END - BUILD_START))
echo "========================================="
echo "All binaries built in ${BUILD_ELAPSED}s"
echo "========================================="
echo ""

echo "========================================="
echo "Starting regtest environment..."
echo "========================================="
REGTEST_START=$(date +%s)

echo "Launching start_regtest_mints binary..."
# Run the pre-built binary in background
target/debug/start_regtest_mints --enable-logging "$CDK_MINTD_DATABASE" "$CDK_ITESTS_DIR" "$CDK_ITESTS_MINT_ADDR" "$CDK_ITESTS_MINT_PORT_0" "$CDK_ITESTS_MINT_PORT_1" &
export CDK_REGTEST_PID=$!

# Give it a moment to start - reduced from 5 to 2 seconds since we have better waiting mechanisms now
sleep 2

# Look for the .env file in the current directory
ENV_FILE_PATH="$CDK_ITESTS_DIR/.env"

# Wait for the .env file to be created in the current directory
max_wait=120
wait_count=0
while [ $wait_count -lt $max_wait ]; do
    if [ -f "$ENV_FILE_PATH" ]; then
        echo ".env file found at: $ENV_FILE_PATH"
        break
    fi
    wait_count=$((wait_count + 1))
    sleep 1
done

# Check if we found the .env file
if [ ! -f "$ENV_FILE_PATH" ]; then
    echo "ERROR: Could not find .env file at $ENV_FILE_PATH"
    exit 1
fi

# Source the environment variables from the .env file
echo "Sourcing environment variables from $ENV_FILE_PATH"
source "$ENV_FILE_PATH"

echo "Sourced environment variables:"
echo "CDK_TEST_MINT_URL=$CDK_TEST_MINT_URL"
echo "CDK_TEST_MINT_URL_2=$CDK_TEST_MINT_URL_2"
echo "CDK_ITESTS_DIR=$CDK_ITESTS_DIR"

# Validate that we sourced the variables
if [ -z "$CDK_TEST_MINT_URL" ] || [ -z "$CDK_TEST_MINT_URL_2" ] || [ -z "$CDK_ITESTS_DIR" ]; then
    echo "ERROR: Failed to source environment variables from the .env file"
    exit 1
fi

# Export all variables so they're available to the tests
export CDK_TEST_MINT_URL
export CDK_TEST_MINT_URL_2

URL="$CDK_TEST_MINT_URL/v1/info"


TIMEOUT=500
START_TIME=$(date +%s)
# Loop until the endpoint returns a 200 OK status or timeout is reached
while true; do
    # Get the current time
    CURRENT_TIME=$(date +%s)
    
    # Calculate the elapsed time
    ELAPSED_TIME=$((CURRENT_TIME - START_TIME))

    # Check if the elapsed time exceeds the timeout
    if [ $ELAPSED_TIME -ge $TIMEOUT ]; then
        echo "Timeout of $TIMEOUT seconds reached. Exiting..."
        exit 1
    fi

    # Make a request to the endpoint and capture the HTTP status code
    HTTP_STATUS=$(curl -o /dev/null -s -w "%{http_code}" $URL)

    # Check if the HTTP status is 200 OK
    if [ "$HTTP_STATUS" -eq 200 ]; then
        echo "Received 200 OK from $URL"
        break
    else
        echo "Waiting for 200 OK response, current status: $HTTP_STATUS"
        sleep 2  # Wait for 2 seconds before retrying
    fi
done

URL="$CDK_TEST_MINT_URL_2/v1/info"


TIMEOUT=100
START_TIME=$(date +%s)
# Loop until the endpoint returns a 200 OK status or timeout is reached
while true; do
    # Get the current time
    CURRENT_TIME=$(date +%s)
    
    # Calculate the elapsed time
    ELAPSED_TIME=$((CURRENT_TIME - START_TIME))

    # Check if the elapsed time exceeds the timeout
    if [ $ELAPSED_TIME -ge $TIMEOUT ]; then
        echo "Timeout of $TIMEOUT seconds reached. Exiting..."
        exit 1
    fi

    # Make a request to the endpoint and capture the HTTP status code
    HTTP_STATUS=$(curl -o /dev/null -s -w "%{http_code}" $URL)

    # Check if the HTTP status is 200 OK
    if [ "$HTTP_STATUS" -eq 200 ]; then
        echo "Received 200 OK from $URL"
        break
    else
        echo "Waiting for 200 OK response, current status: $HTTP_STATUS"
        sleep 2  # Wait for 2 seconds before retrying
    fi
done

REGTEST_END=$(date +%s)
REGTEST_ELAPSED=$((REGTEST_END - REGTEST_START))
echo "========================================="
echo "Regtest environment ready in ${REGTEST_ELAPSED}s"
echo "========================================="
echo ""

echo "========================================="
echo "Running tests..."
echo "========================================="
TESTS_START=$(date +%s)

# Run cargo test
echo "[Test 1/8] Running regtest test with CLN mint and LND client"
export CDK_TEST_LIGHTNING_CLIENT="lnd"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test regtest
if [ $? -ne 0 ]; then
    echo "regtest test with cln mint failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

echo "[Test 2/8] Running happy_path_mint_wallet test with CLN mint and CLN client"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test happy_path_mint_wallet
if [ $? -ne 0 ]; then
    echo "happy_path_mint_wallet with cln mint test failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

# Run cargo test with the http_subscription feature
echo "[Test 3/8] Running regtest test with http_subscription feature (CLN client)"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test regtest --features http_subscription
if [ $? -ne 0 ]; then
    echo "regtest test with http_subscription failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

echo "[Test 4/8] Running bolt12 test with CLN mint (CLN client)"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test bolt12
if [ $? -ne 0 ]; then
    echo "regtest test failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

# Switch Mints: Run tests with LND mint
echo "Switching to LND mint for tests"

echo "[Test 5/8] Running regtest test with LND mint and LND client"
CDK_TEST_MINT_URL_SWITCHED=$CDK_TEST_MINT_URL_2
CDK_TEST_MINT_URL_2_SWITCHED=$CDK_TEST_MINT_URL
export CDK_TEST_MINT_URL=$CDK_TEST_MINT_URL_SWITCHED
export CDK_TEST_MINT_URL_2=$CDK_TEST_MINT_URL_2_SWITCHED

TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test regtest
if [ $? -ne 0 ]; then
    echo "regtest test with LND mint failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

echo "[Test 6/8] Running happy_path_mint_wallet test with LND mint and LND client"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test happy_path_mint_wallet
if [ $? -ne 0 ]; then
    echo "happy_path_mint_wallet test with LND mint failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""


export CDK_TEST_MINT_URL="http://127.0.0.1:8089"
 
TIMEOUT=100
START_TIME=$(date +%s)
# Loop until the endpoint returns a 200 OK status or timeout is reached
while true; do
    # Get the current time
    CURRENT_TIME=$(date +%s)
    
    # Calculate the elapsed time
    ELAPSED_TIME=$((CURRENT_TIME - START_TIME))

    # Check if the elapsed time exceeds the timeout
    if [ $ELAPSED_TIME -ge $TIMEOUT ]; then
        echo "Timeout of $TIMEOUT seconds reached. Exiting..."
        exit 1
    fi

    # Make a request to the endpoint and capture the HTTP status code
    HTTP_STATUS=$(curl -o /dev/null -s -w "%{http_code}" $CDK_TEST_MINT_URL/v1/info)

    # Check if the HTTP status is 200 OK
    if [ "$HTTP_STATUS" -eq 200 ]; then
        echo "Received 200 OK from $CDK_TEST_MINT_URL"
        break
    else
        echo "Waiting for 200 OK response, current status: $HTTP_STATUS"
        sleep 2  # Wait for 2 seconds before retrying
    fi
done


echo "[Test 7/8] Running happy_path_mint_wallet test with LDK mint and CLN client"
export CDK_TEST_LIGHTNING_CLIENT="cln"  # Use CLN client for LDK tests
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test happy_path_mint_wallet
if [ $? -ne 0 ]; then
    echo "happy_path_mint_wallet test with LDK mint failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

echo "[Test 8/8] Running regtest test with LDK mint and CLN client"
TEST_START=$(date +%s)
cargo test -p cdk-integration-tests --test regtest
if [ $? -ne 0 ]; then
    echo "regtest test LDK mint failed, exiting"
    exit 1
fi
TEST_END=$(date +%s)
echo "  Completed in $((TEST_END - TEST_START))s"
echo ""

TESTS_END=$(date +%s)
TESTS_ELAPSED=$((TESTS_END - TESTS_START))

echo "========================================="
echo "TIMING SUMMARY"
echo "========================================="
echo "Build phase:            ${BUILD_ELAPSED}s"
echo "Regtest startup:        ${REGTEST_ELAPSED}s"
echo "Test execution:         ${TESTS_ELAPSED}s"
echo "========================================="
TOTAL_ELAPSED=$((BUILD_ELAPSED + REGTEST_ELAPSED + TESTS_ELAPSED))
echo "TOTAL TIME:             ${TOTAL_ELAPSED}s"
echo "========================================="
echo ""
echo "All tests passed successfully"
exit 0
