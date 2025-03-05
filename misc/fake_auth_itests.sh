
#!/usr/bin/env bash

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk mintd"
    kill -2 $cdk_mintd_pid
    wait $cdk_mintd_pid

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
export cdk_itests_mint_port=8087;

# Check if the temporary directory was created successfully
if [[ ! -d "$cdk_itests" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $cdk_itests"
export MINT_DATABASE="$1";
export OPENID_DISCOVERY="$2";

cargo build -p cdk-integration-tests 

export CDK_MINTD_URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port";
export CDK_MINTD_WORK_DIR="$cdk_itests";
export CDK_MINTD_LISTEN_HOST=$cdk_itests_mint_addr;
export CDK_MINTD_LISTEN_PORT=$cdk_itests_mint_port;
export CDK_MINTD_LN_BACKEND="fakewallet";
export CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS="sat";
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal";
export CDK_MINTD_FAKE_WALLET_FEE_PERCENT="0";
export CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN="1";
export CDK_MINTD_DATABASE=$MINT_DATABASE;

# Auth configuration
export CDK_TEST_OIDC_USER="cdk-test";
export CDK_TEST_OIDC_PASSWORD="cdkpassword";

export CDK_MINTD_AUTH_OPENID_DISCOVERY=$OPENID_DISCOVERY;
export CDK_MINTD_AUTH_OPENID_CLIENT_ID="cashu-client";
export CDK_MINTD_AUTH_MINT_MAX_BAT="50";
export CDK_MINTD_AUTH_ENABLED_MINT="true";
export CDK_MINTD_AUTH_ENABLED_MELT="true";
export CDK_MINTD_AUTH_ENABLED_SWAP="true";
export CDK_MINTD_AUTH_ENABLED_CHECK_MINT_QUOTE="true";
export CDK_MINTD_AUTH_ENABLED_CHECK_MELT_QUOTE="true";
export CDK_MINTD_AUTH_ENABLED_RESTORE="true";
export CDK_MINTD_AUTH_ENABLED_CHECK_PROOF_STATE="true";

echo "Starting auth mintd";
cargo run --bin cdk-mintd --features redb &
cdk_mintd_pid=$!

URL="http://$cdk_itests_mint_addr:$cdk_itests_mint_port/v1/info"
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
cargo test -p cdk-integration-tests --test fake_auth

# Capture the exit status of cargo test
test_status=$?

# Exit with the status of the test
exit $test_status
