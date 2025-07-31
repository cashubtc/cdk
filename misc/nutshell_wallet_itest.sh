#!/usr/bin/env bash

set -e

# Configuration
MINT_PORT=8085
WALLET_PORT=4448
MINT_CONTAINER_NAME="nutshell-mint"
WALLET_CONTAINER_NAME="nutshell-wallet"
# Use host.docker.internal for the mint URL so containers can access it
MINT_URL="http://0.0.0.0:${MINT_PORT}"
WALLET_URL="http://localhost:${WALLET_PORT}"
CDK_MINTD_PID=""

# Function to clean up resources
cleanup() {
  echo "Cleaning up resources..."
  
  if docker ps -a | grep -q ${WALLET_CONTAINER_NAME}; then
    echo "Stopping and removing Docker container '${WALLET_CONTAINER_NAME}'..."
    docker stop ${WALLET_CONTAINER_NAME} >/dev/null 2>&1
    docker rm ${WALLET_CONTAINER_NAME} >/dev/null 2>&1
  fi
  
  if [ -n "$CDK_MINTD_PID" ]; then
    echo "Stopping mintd process (PID: $CDK_MINTD_PID)..."
    kill -TERM $CDK_MINTD_PID >/dev/null 2>&1 || true
  fi
  
  # Unset variables
  unset MINT_URL WALLET_URL MINT_PORT WALLET_PORT MINT_CONTAINER_NAME WALLET_CONTAINER_NAME
  unset CDK_MINTD_PID CDK_MINTD_URL CDK_MINTD_WORK_DIR CDK_MINTD_LISTEN_HOST CDK_MINTD_LISTEN_PORT
  unset CDK_MINTD_LN_BACKEND CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS CDK_MINTD_MNEMONIC
  unset CDK_MINTD_FAKE_WALLET_FEE_PERCENT CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN CDK_MINTD_DATABASE
  unset TEST_STATUS
  unset CDK_MINTD_INPUT_FEE_PPK
  echo "Cleanup complete."
}

# Set up trap to call cleanup function on script exit
trap cleanup EXIT INT TERM



# Create a temporary directory for mintd
CDK_ITESTS=$(mktemp -d)
echo "Created temporary directory for mintd: $CDK_ITESTS"

export CDK_MINTD_URL="$MINT_URL"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS"
export CDK_MINTD_LISTEN_HOST="127.0.0.1"
export CDK_MINTD_LISTEN_PORT="8085"
export CDK_MINTD_LN_BACKEND="fakewallet"
export CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS="sat,usd"
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
export CDK_MINTD_FAKE_WALLET_FEE_PERCENT="0"
export CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN="1"
export CDK_MINTD_INPUT_FEE_PPK="100"


export CDK_ITESTS_DIR="$CDK_ITESTS"


echo "Starting fake mintd"
cargo run --bin cdk-mintd &
CDK_MINTD_PID=$!

# Wait for the mint to be ready
echo "Waiting for mintd to start..."
TIMEOUT=300
START_TIME=$(date +%s)

# Try different URLs since the endpoint might vary
URLS=("http://localhost:${MINT_PORT}/v1/info" "http://127.0.0.1:${MINT_PORT}/v1/info" "http://0.0.0.0:${MINT_PORT}/v1/info")

# Loop until one of the endpoints returns a 200 OK status or timeout is reached
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

    # Try each URL
    for URL in "${URLS[@]}"; do
        # Make a request to the endpoint and capture the HTTP status code
        HTTP_STATUS=$(curl -o /dev/null -s -w "%{http_code}" "$URL" || echo "000")
        
        # Check if the HTTP status is 200 OK
        if [ "$HTTP_STATUS" -eq 200 ]; then
            echo "Received 200 OK from $URL"
            MINT_URL=$(echo "$URL" | sed 's|/v1/info||')
            echo "Setting MINT_URL to $MINT_URL"
            export MINT_URL
            break 2  # Break out of both loops
        else
            echo "Waiting for 200 OK response from $URL, current status: $HTTP_STATUS"
        fi
    done
    
    # Wait before retrying
    sleep 5
done




# Check if Docker is available and accessible
if docker info > /dev/null 2>&1; then
  echo "Docker is available, starting Nutshell wallet container"
  # Use the MINT_URL which is already set to host.docker.internal
  docker run -d --name ${WALLET_CONTAINER_NAME} \
    --network=host \
    -p ${WALLET_PORT}:${WALLET_PORT} \
    -e MINT_URL=${MINT_URL} \
    cashubtc/nutshell:0.16.5 \
    poetry run cashu -d
else
  echo "Docker is not accessible, skipping Nutshell wallet container setup"
  # Set a flag to indicate we're not using Docker
  export NO_DOCKER=true
fi

# Wait for the mint to be ready
echo "Waiting for Nutshell Mint to start..."
sleep 5

# Check if the Mint API is responding (use localhost for local curl check)
echo "Checking if Nutshell Mint API is available..."
if curl -s "http://localhost:${MINT_PORT}/v1/info" > /dev/null; then
  echo "Nutshell Mint is running and accessible at ${MINT_URL}"
else
  echo "Warning: Nutshell Mint API is not responding. It might not be ready yet."
fi

# Only check wallet if Docker is available
if [ -z "$NO_DOCKER" ]; then
  # Check if the Wallet API is responding
  echo "Checking if Nutshell Wallet API is available..."
  if curl -s "${WALLET_URL}/info" > /dev/null; then
    echo "Nutshell Wallet is running in container '${WALLET_CONTAINER_NAME}'"
    echo "You can access it at ${WALLET_URL}"
  else
    echo "Warning: Nutshell Wallet API is not responding. The container might not be ready yet."
  fi
fi

# Export URLs as environment variables
export MINT_URL=${MINT_URL}
export WALLET_URL=${WALLET_URL}
export CDK_TEST_MINT_URL=${MINT_URL}

# Run the integration test
echo "Running integration test..."
cargo test -p cdk-integration-tests --test nutshell_wallet -- --test-threads 1
cargo test -p cdk-integration-tests --test test_fees -- --test-threads 1
TEST_STATUS=$?

# Exit with the test status
echo "Integration test completed with status: $TEST_STATUS"
exit $TEST_STATUS
