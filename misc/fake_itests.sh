#!/usr/bin/env bash

# Script to run fake mint tests with proper handling of race conditions
# This script ensures the .env file is properly created and available
# before running tests

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    if [ -n "$FAKE_MINT_PID" ]; then
        echo "Killing the fake mint process"
        kill -2 $FAKE_MINT_PID 2>/dev/null || true
        wait $FAKE_MINT_PID 2>/dev/null || true
    fi

    if [ -n "$CDK_SIGNATORY_PID" ]; then
        echo "Killing the signatory process"
        kill -9 $CDK_SIGNATORY_PID 2>/dev/null || true
        wait $CDK_SIGNATORY_PID 2>/dev/null || true
    fi

    echo "Mint binary terminated"

    # Remove the temporary directory
    if [ -n "$CDK_ITESTS_DIR" ] && [ -d "$CDK_ITESTS_DIR" ]; then
        rm -rf "$CDK_ITESTS_DIR"
        echo "Temp directory removed: $CDK_ITESTS_DIR"
    fi

    if [ -n "$CONTAINER_NAME" ]; then
        docker rm "${CONTAINER_NAME}" -f
    fi

    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_TEST_MINT_URL
    unset FAKE_MINT_PID
    unset CDK_SIGNATORY_PID
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT INT TERM

# Create a temporary directory
export CDK_ITESTS_DIR=$(mktemp -d)

# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "Temp directory created: $CDK_ITESTS_DIR"

# Check if a database type was provided as first argument, default to sqlite
export CDK_MINTD_DATABASE="${1:-sqlite}"

cargo build -p cdk-integration-tests

# Start the fake mint binary with the new Rust-based approach
echo "Starting fake mint using Rust binary..."

if [ "${CDK_MINTD_DATABASE}" = "POSTGRES" ]; then
    export CONTAINER_NAME="rust-fake-test-pg"
    DB_USER="test"
    DB_PASS="test"
    DB_NAME="testdb"
    DB_PORT="15433"

    docker run -d --rm \
      --name "${CONTAINER_NAME}" \
      -e POSTGRES_USER="${DB_USER}" \
      -e POSTGRES_PASSWORD="${DB_PASS}" \
      -e POSTGRES_DB="${DB_NAME}" \
      -p ${DB_PORT}:5432 \
      postgres:16
    export CDK_MINTD_DATABASE_URL="postgresql://${DB_USER}:${DB_PASS}@localhost:${DB_PORT}/${DB_NAME}"

    echo "Waiting for PostgreSQL to be ready and database '${DB_NAME}' to exist..."
    until docker exec -e PGPASSWORD="${DB_PASS}" "${CONTAINER_NAME}" \
        psql -U "${DB_USER}" -d "${DB_NAME}" -c "SELECT 1;" >/dev/null 2>&1; do
      sleep 0.5
    done
    echo "PostgreSQL container is ready"
fi

if [ "$2" = "external_signatory" ]; then
    echo "Starting with external signatory support"

    bash -x `dirname $0`/../crates/cdk-signatory/generate_certs.sh $CDK_ITESTS_DIR
    cargo build --bin signatory
    cargo run --bin signatory -- -w $CDK_ITESTS_DIR -u "sat" -u "usd"  &
    export CDK_SIGNATORY_PID=$!
    sleep 5

    cargo run --bin start_fake_mint -- --enable-logging --external-signatory "$CDK_MINTD_DATABASE" "$CDK_ITESTS_DIR" &
else
    cargo run --bin start_fake_mint -- --enable-logging "$CDK_MINTD_DATABASE" "$CDK_ITESTS_DIR" &
fi
export FAKE_MINT_PID=$!

# Give the mint a moment to start
sleep 3

# Look for the .env file in the temp directory
ENV_FILE_PATH="$CDK_ITESTS_DIR/.env"

# Wait for the .env file to be created (with longer timeout)
max_wait=200
wait_count=0
while [ $wait_count -lt $max_wait ]; do
    if [ -f "$ENV_FILE_PATH" ]; then
        echo ".env file found at: $ENV_FILE_PATH"
        break
    fi
    echo "Waiting for .env file to be created... ($wait_count/$max_wait)"
    wait_count=$((wait_count + 1))
    sleep 1
done

# Check if we found the .env file
if [ ! -f "$ENV_FILE_PATH" ]; then
    echo "ERROR: Could not find .env file at $ENV_FILE_PATH after $max_wait seconds"
    exit 1
fi

# Source the environment variables from the .env file
echo "Sourcing environment variables from $ENV_FILE_PATH"
source "$ENV_FILE_PATH"

echo "Sourced environment variables:"
echo "CDK_TEST_MINT_URL=$CDK_TEST_MINT_URL"
echo "CDK_ITESTS_DIR=$CDK_ITESTS_DIR"

# Validate that we sourced the variables
if [ -z "$CDK_TEST_MINT_URL" ] || [ -z "$CDK_ITESTS_DIR" ]; then
    echo "ERROR: Failed to source environment variables from the .env file"
    exit 1
fi

# Export all variables so they're available to the tests
export CDK_TEST_MINT_URL

URL="$CDK_TEST_MINT_URL/v1/info"

TIMEOUT=120
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

# Run first test
echo "Running fake_wallet test"
cargo test -p cdk-integration-tests --test fake_wallet
status1=$?

# Exit immediately if the first test failed
if [ $status1 -ne 0 ]; then
    echo "First test failed with status $status1, exiting"
    exit $status1
fi

# Run second test only if the first one succeeded
echo "Running happy_path_mint_wallet test"
cargo test -p cdk-integration-tests --test happy_path_mint_wallet
status2=$?

# Exit with the status of the second test
if [ $status2 -ne 0 ]; then
    echo "Second test failed with status $status2, exiting"
    exit $status2
fi

# Both tests passed
echo "All tests passed successfully"
exit 0
