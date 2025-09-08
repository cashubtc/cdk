#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."


    echo "Killing the cdk payment processor"
    kill -2 $CDK_PAYMENT_PROCESSOR_PID
    wait $CDK_PAYMENT_PROCESSOR_PID

    echo "Killing the cdk mintd"
    kill -2 $CDK_MINTD_PID
    wait $CDK_MINTD_PID

    echo "Killing the cdk regtest"
    kill -2 $CDK_REGTEST_PID
    wait $CDK_REGTEST_PID

    echo "Mint binary terminated"

    # Remove the temporary directory
    rm -rf "$CDK_ITESTS_DIR"
    echo "Temp directory removed: $CDK_ITESTS_DIR"
    
    # Unset all environment variables that were set
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT_0
    unset CDK_REGTEST_PID
    unset LN_BACKEND
    unset MINT_DATABASE
    unset CDK_TEST_REGTEST
    unset CDK_TEST_MINT_URL
    unset CDK_PAYMENT_PROCESSOR_CLN_RPC_PATH
    unset CDK_PAYMENT_PROCESSOR_LND_ADDRESS
    unset CDK_PAYMENT_PROCESSOR_LND_CERT_FILE
    unset CDK_PAYMENT_PROCESSOR_LND_MACAROON_FILE
    unset CDK_PAYMENT_PROCESSOR_LN_BACKEND
    unset CDK_PAYMENT_PROCESSOR_LISTEN_HOST
    unset CDK_PAYMENT_PROCESSOR_LISTEN_PORT
    unset CDK_PAYMENT_PROCESSOR_PID
    unset CDK_MINTD_URL
    unset CDK_MINTD_WORK_DIR
    unset CDK_MINTD_LISTEN_HOST
    unset CDK_MINTD_LISTEN_PORT
    unset CDK_MINTD_LN_BACKEND
    unset CDK_MINTD_GRPC_PAYMENT_PROCESSOR_ADDRESS
    unset CDK_MINTD_GRPC_PAYMENT_PROCESSOR_PORT
    unset CDK_MINTD_GRPC_PAYMENT_PROCESSOR_SUPPORTED_UNITS
    unset CDK_MINTD_MNEMONIC
    unset CDK_MINTD_PID
    unset CDK_PAYMENT_PROCESSOR_CLN_BOLT12
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

# Create a temporary directory
export CDK_ITESTS_DIR=$(mktemp -d)
export CDK_ITESTS_MINT_ADDR="127.0.0.1";
export CDK_ITESTS_MINT_PORT_0=8086;


export LN_BACKEND="$1";

URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0/v1/info"
# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $CDK_ITESTS_DIR"
export MINT_DATABASE="$1";

cargo build -p cdk-integration-tests 


export CDK_TEST_REGTEST=0
if [ "$LN_BACKEND" != "FAKEWALLET" ]; then
    export CDK_TEST_REGTEST=1
    cargo run --bin start_regtest "$CDK_ITESTS_DIR" &
    CDK_REGTEST_PID=$!
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
    export CDK_PAYMENT_PROCESSOR_CLN_BOLT12=true
fi

# Start payment processor


export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"

export CDK_PAYMENT_PROCESSOR_CLN_RPC_PATH="$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc";

export CDK_PAYMENT_PROCESSOR_LND_ADDRESS="https://localhost:10010";
export CDK_PAYMENT_PROCESSOR_LND_CERT_FILE="$CDK_ITESTS_DIR/lnd/two/tls.cert";
export CDK_PAYMENT_PROCESSOR_LND_MACAROON_FILE="$CDK_ITESTS_DIR/lnd/two/data/chain/bitcoin/regtest/admin.macaroon";

export CDK_PAYMENT_PROCESSOR_LN_BACKEND=$LN_BACKEND;
export CDK_PAYMENT_PROCESSOR_LISTEN_HOST="127.0.0.1";
export CDK_PAYMENT_PROCESSOR_LISTEN_PORT="8090";

echo "$CDK_PAYMENT_PROCESSOR_CLN_RPC_PATH"

cargo b --bin cdk-payment-processor

cargo run --bin cdk-payment-processor &

CDK_PAYMENT_PROCESSOR_PID=$!


export CDK_MINTD_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0";
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR";
export CDK_MINTD_LISTEN_HOST=$CDK_ITESTS_MINT_ADDR;
export CDK_MINTD_LISTEN_PORT=$CDK_ITESTS_MINT_PORT_0;
export CDK_MINTD_LN_BACKEND="grpcprocessor";
export CDK_MINTD_GRPC_PAYMENT_PROCESSOR_ADDRESS="http://127.0.0.1";
export CDK_MINTD_GRPC_PAYMENT_PROCESSOR_PORT="8090";
export CDK_MINTD_GRPC_PAYMENT_PROCESSOR_SUPPORTED_UNITS="sat";
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal";
 
cargo build --bin cdk-mintd --no-default-features --features grpc-processor

cargo run --bin cdk-mintd --no-default-features --features grpc-processor &
CDK_MINTD_PID=$!

echo $CDK_ITESTS_DIR

TIMEOUT=300
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


cargo test -p cdk-integration-tests --test happy_path_mint_wallet

# Capture the exit status of cargo test
test_status=$?

if [ "$LN_BACKEND" = "CLN" ]; then
    echo "Running bolt12 tests for CLN backend"
    cargo test -p cdk-integration-tests --test bolt12
    bolt12_test_status=$?
    
    # Exit with non-zero status if either test failed
    if [ $test_status -ne 0 ] || [ $bolt12_test_status -ne 0 ]; then
        echo "Tests failed - happy_path_mint_wallet: $test_status, bolt12: $bolt12_test_status"
        exit 1
    fi
fi

# Exit with the status of the tests
exit $test_status
