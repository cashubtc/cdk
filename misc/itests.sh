#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    # Kill the Rust binary process
    echo "Killing the Rust binary with PID $RUST_BIN_PID"
    kill $CDK_ITEST_MINT_BIN_PID

    # Wait for the Rust binary to terminate
    wait $CDK_ITEST_MINT_BIN_PID

    echo "Mint binary terminated"
    
    # Kill processes
    lncli --lnddir="$cdk_itests/lnd" --network=regtest stop
    lightning-cli --regtest --lightning-dir="$cdk_itests/cln/" stop
    bitcoin-cli --datadir="$cdk_itests/bitcoin"  -rpcuser=testuser -rpcpassword=testpass -rpcport=18443 stop

    # Remove the temporary directory
    rm -rf "$cdk_itests"
    echo "Temp directory removed: $cdk_itests"
    unset cdk_itests
    unset cdk_itests_mint_addr
    unset cdk_itests_mint_port
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

# Create a temporary directory
export cdk_itests=$(mktemp -d)
export cdk_itests_mint_addr="127.0.0.1";
export cdk_itests_mint_port=8085;

URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port/v1/info"
# Check if the temporary directory was created successfully
if [[ ! -d "$cdk_itests" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $cdk_itests"
export MINT_DATABASE="$1";

cargo build -p cdk-integration-tests 
cargo build --bin cdk-integration-tests 
cargo run --bin cdk-integration-tests &
# Capture its PID
CDK_ITEST_MINT_BIN_PID=$!

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


# Run cargo test
cargo test -p cdk-integration-tests --test regtest

# Run cargo test with the http_subscription feature
cargo test -p cdk-integration-tests --test regtest --features http_subscription

# Capture the exit status of cargo test
test_status=$?

# Exit with the status of the tests
exit $test_status
