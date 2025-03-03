#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk mintd"
    kill -2 $cdk_mintd_pid
    wait $cdk_mintd_pid

    
    echo "Killing the cdk lnd mintd"
    kill -2 $cdk_mintd_lnd_pid
    wait $cdk_mintd_lnd_pid

    echo "Killing the cdk regtest"
    kill -2 $cdk_regtest_pid
    wait $cdk_regtest_pid


    echo "Mint binary terminated"

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
export cdk_itests_mint_port_0=8085;
export cdk_itests_mint_port_1=8087;

# Check if the temporary directory was created successfully
if [[ ! -d "$cdk_itests" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $cdk_itests"
export MINT_DATABASE="$1";

cargo build -p cdk-integration-tests 

cargo run --bin start_regtest &

cdk_regtest_pid=$!
mkfifo "$cdk_itests/progress_pipe"
rm -f "$cdk_itests/signal_received"  # Ensure clean state
# Start reading from pipe in background
(while read line; do
    case "$line" in
        "checkpoint1")
            echo "Reached first checkpoint"
            touch "$cdk_itests/signal_received"
            exit 0
            ;;
    esac
done < "$cdk_itests/progress_pipe") &
# Wait for up to 120 seconds
for ((i=0; i<120; i++)); do
    if [ -f "$cdk_itests/signal_received" ]; then
        echo "break signal received"
        break
    fi
    sleep 1
done
echo "Regtest set up continuing"

echo "Starting regtest mint"
# cargo run --bin regtest_mint &

export CDK_MINTD_CLN_RPC_PATH="$cdk_itests/cln/one/regtest/lightning-rpc";


export CDK_MINTD_URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port_0";
export CDK_MINTD_WORK_DIR="$cdk_itests";
export CDK_MINTD_LISTEN_HOST=$cdk_itests_mint_addr;
export CDK_MINTD_LISTEN_PORT=$cdk_itests_mint_port_0;
export CDK_MINTD_LN_BACKEND="cln";
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal";
export CDK_MINTD_DATABASE=$MINT_DATABASE;

echo "Starting cln mintd";
cargo run --bin cdk-mintd &
cdk_mintd_pid=$!


echo $cdk_itests

URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port_0/v1/info"

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


export CDK_MINTD_LND_ADDRESS="https://localhost:10010";
export CDK_MINTD_LND_CERT_FILE="$cdk_itests/lnd/two/tls.cert";
export CDK_MINTD_LND_MACAROON_FILE="$cdk_itests/lnd/two/data/chain/bitcoin/regtest/admin.macaroon";

export CDK_MINTD_URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port_1";
mkdir -p "$cdk_itests/lnd_mint"
export CDK_MINTD_WORK_DIR="$cdk_itests/lnd_mint";
export CDK_MINTD_LISTEN_HOST=$cdk_itests_mint_addr;
export CDK_MINTD_LISTEN_PORT=$cdk_itests_mint_port_1;
export CDK_MINTD_LN_BACKEND="lnd";
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal";

echo "Starting lnd mintd";
cargo run --bin cdk-mintd &
cdk_mintd_lnd_pid=$!

URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port_1/v1/info"

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

# Switch Mints: Run tests with LND mint
export cdk_itests_mint_port_0=8087;
export cdk_itests_mint_port_1=8085;
cargo test -p cdk-integration-tests --test regtest

# Capture the exit status of cargo test
test_status=$?

# Exit with the status of the tests
exit $test_status
