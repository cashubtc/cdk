#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk mintd"
    kill -2 $CDK_MINTD_PID
    wait $CDK_MINTD_PID

    echo "Mint binary terminated"
    
    # Remove the temporary directory
    rm -rf "$CDK_ITESTS_DIR"
    echo "Temp directory removed: $CDK_ITESTS_DIR"
    
    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT
    unset CDK_MINTD_DATABASE
    unset CDK_TEST_MINT_URL
    unset CDK_MINTD_URL
    unset CDK_MINTD_WORK_DIR
    unset CDK_MINTD_LISTEN_HOST
    unset CDK_MINTD_LISTEN_PORT
    unset CDK_MINTD_LN_BACKEND
    unset CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS
    unset CDK_MINTD_MNEMONIC
    unset CDK_MINTD_FAKE_WALLET_FEE_PERCENT
    unset CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN
    unset CDK_MINTD_PID
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

# Create a temporary directory
export CDK_ITESTS_DIR=$(mktemp -d)
export CDK_ITESTS_MINT_ADDR="127.0.0.1"
export CDK_ITESTS_MINT_PORT=8086

# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $CDK_ITESTS_DIR"
export CDK_MINTD_DATABASE="$1"

cargo build -p cdk-integration-tests 


export CDK_MINTD_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR"
export CDK_MINTD_LISTEN_HOST=$CDK_ITESTS_MINT_ADDR
export CDK_MINTD_LISTEN_PORT=$CDK_ITESTS_MINT_PORT
export CDK_MINTD_LN_BACKEND="fakewallet"
export CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS="sat,usd"
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
export CDK_MINTD_FAKE_WALLET_FEE_PERCENT="0"
export CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN="1"


echo "Starting fake mintd"
cargo run --bin cdk-mintd --features "redb" &
export CDK_MINTD_PID=$!

URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT/v1/info"
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
cargo test -p cdk-integration-tests --test fake_wallet
status1=$?

export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT"

cargo test -p cdk-integration-tests --test happy_path_mint_wallet
status2=$?

# Exit with failure if either test failed
if [ $status1 -ne 0 ] || [ $status2 -ne 0 ]; then
    exit 1
else
    exit 0
fi
