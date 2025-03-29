#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk mintd"
    kill -2 $CDK_MINTD_PID
    wait $CDK_MINTD_PID

    
    echo "Killing the cdk lnd mintd"
    kill -2 $CDK_MINTD_LND_PID
    wait $CDK_MINTD_LND_PID

    echo "Killing the cdk regtest"
    kill -2 $CDK_REGTEST_PID
    wait $CDK_REGTEST_PID


    echo "Mint binary terminated"

    # Remove the temporary directory
    rm -rf "$CDK_ITESTS_DIR"
    echo "Temp directory removed: $CDK_ITESTS_DIR"
    
    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT_0
    unset CDK_ITESTS_MINT_PORT_1
    unset CDK_MINTD_DATABASE
    unset CDK_TEST_MINT_URL
    unset CDK_TEST_MINT_URL_2
    unset CDK_MINTD_URL
    unset CDK_MINTD_WORK_DIR
    unset CDK_MINTD_LISTEN_HOST
    unset CDK_MINTD_LISTEN_PORT
    unset CDK_MINTD_LN_BACKEND
    unset CDK_MINTD_MNEMONIC
    unset CDK_MINTD_CLN_RPC_PATH
    unset CDK_MINTD_LND_ADDRESS
    unset CDK_MINTD_LND_CERT_FILE
    unset CDK_MINTD_LND_MACAROON_FILE
    unset CDK_MINTD_PID
    unset CDK_MINTD_LND_PID
    unset CDK_REGTEST_PID
    unset RUST_BACKTRACE
    unset CDK_TEST_REGTEST
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

cargo build -p cdk-integration-tests 

cargo run --bin start_regtest &

export CDK_REGTEST_PID=$!
mkfifo "$CDK_ITESTS_DIR/progress_pipe"
rm -f "$CDK_ITESTS_DIR/signal_received"  # Ensure clean state
# Start reading from pipe in background
(while read line; do
    case "$line" in
        "checkpoint1")
            echo "Reached first checkpoint"
            touch "$CDK_ITESTS_DIR/signal_received"
            exit 0
            ;;
    esac
done < "$CDK_ITESTS_DIR/progress_pipe") &
# Wait for up to 120 seconds
for ((i=0; i<120; i++)); do
    if [ -f "$CDK_ITESTS_DIR/signal_received" ]; then
        echo "break signal received"
        break
    fi
    sleep 1
done
echo "Regtest set up continuing"

echo "Starting regtest mint"
# cargo run --bin regtest_mint &

export CDK_MINTD_CLN_RPC_PATH="$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc"
export CDK_MINTD_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR"
export CDK_MINTD_LISTEN_HOST=$CDK_ITESTS_MINT_ADDR
export CDK_MINTD_LISTEN_PORT=$CDK_ITESTS_MINT_PORT_0
export CDK_MINTD_LN_BACKEND="cln"
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
export RUST_BACKTRACE=1

echo "Starting cln mintd"
cargo run --bin cdk-mintd --features "redb" &
export CDK_MINTD_PID=$!


echo $CDK_ITESTS_DIR

URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0/v1/info"

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


export CDK_MINTD_LND_ADDRESS="https://localhost:10010"
export CDK_MINTD_LND_CERT_FILE="$CDK_ITESTS_DIR/lnd/two/tls.cert"
export CDK_MINTD_LND_MACAROON_FILE="$CDK_ITESTS_DIR/lnd/two/data/chain/bitcoin/regtest/admin.macaroon"

export CDK_MINTD_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1"
mkdir -p "$CDK_ITESTS_DIR/lnd_mint"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/lnd_mint"
export CDK_MINTD_LISTEN_HOST=$CDK_ITESTS_MINT_ADDR
export CDK_MINTD_LISTEN_PORT=$CDK_ITESTS_MINT_PORT_1
export CDK_MINTD_LN_BACKEND="lnd"
export CDK_MINTD_MNEMONIC="cattle gold bind busy sound reduce tone addict baby spend february strategy"

echo "Starting lnd mintd"
cargo run --bin cdk-mintd --features "redb" &
export CDK_MINTD_LND_PID=$!

URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1/v1/info"

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



export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_TEST_MINT_URL_2="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1"

# Run tests and exit immediately on failure

# Run cargo test
echo "Running regtest test with CLN mint"
cargo test -p cdk-integration-tests --test regtest
if [ $? -ne 0 ]; then
    echo "regtest test failed, exiting"
    exit 1
fi

echo "Running happy_path_mint_wallet test with CLN mint"
cargo test -p cdk-integration-tests --test happy_path_mint_wallet test_happy_mint_melt_round_trip
if [ $? -ne 0 ]; then
    echo "happy_path_mint_wallet test failed, exiting"
    exit 1
fi

# # Run cargo test with the http_subscription feature
echo "Running regtest test with http_subscription feature"
cargo test -p cdk-integration-tests --test regtest --features http_subscription
if [ $? -ne 0 ]; then
    echo "regtest test with http_subscription failed, exiting"
    exit 1
fi

# Switch Mints: Run tests with LND mint
echo "Switching to LND mint for tests"
export CDK_ITESTS_MINT_PORT_0=8087
export CDK_ITESTS_MINT_PORT_1=8085
export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_TEST_MINT_URL_2="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1"

echo "Running regtest test with LND mint"
cargo test -p cdk-integration-tests --test regtest
if [ $? -ne 0 ]; then
    echo "regtest test with LND mint failed, exiting"
    exit 1
fi

echo "Running happy_path_mint_wallet test with LND mint"
cargo test -p cdk-integration-tests --test happy_path_mint_wallet
if [ $? -ne 0 ]; then
    echo "happy_path_mint_wallet test with LND mint failed, exiting"
    exit 1
fi

echo "All tests passed successfully"
exit 0
