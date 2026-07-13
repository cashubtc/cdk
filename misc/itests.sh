#!/usr/bin/env bash

source "$(dirname "$0")/itest_helpers.sh"

regtest_processes_by_work_dir() {
    if [ -z "${CDK_ITESTS_DIR:-}" ] || ! command -v pgrep >/dev/null 2>&1; then
        return
    fi

    # pgrep accepts an extended regular expression. Escape the path so a temp
    # directory containing regex punctuation cannot match an unrelated job.
    local work_dir_pattern
    work_dir_pattern=$(printf '%s' "$CDK_ITESTS_DIR" |
        sed 's/[][(){}.^$*+?|\\]/\\&/g')
    pgrep -f -- "$work_dir_pattern" 2>/dev/null || true
}

regtest_launcher_running() {
    if [ -z "${CDK_REGTEST_PID:-}" ] || [ "${CDK_REGTEST_REAPED:-0}" -eq 1 ]; then
        return 1
    fi

    local running_pid
    while read -r running_pid; do
        if [ "$running_pid" = "$CDK_REGTEST_PID" ]; then
            return 0
        fi
    done < <(jobs -pr)
    return 1
}

reap_regtest_launcher_if_exited() {
    if [ -n "${CDK_REGTEST_PID:-}" ] &&
        [ "${CDK_REGTEST_REAPED:-0}" -eq 0 ] &&
        ! regtest_launcher_running; then
        wait "$CDK_REGTEST_PID" 2>/dev/null || true
        CDK_REGTEST_REAPED=1
    fi
}

regtest_processes_running() {
    reap_regtest_launcher_if_exited

    if [ -n "${CDK_REGTEST_PGID:-}" ] &&
        kill -0 -- "-$CDK_REGTEST_PGID" 2>/dev/null; then
        return 0
    fi

    [ -n "$(regtest_processes_by_work_dir)" ]
}

signal_regtest_processes() {
    local signal="$1"
    local pids

    # The launcher and all of the services it starts have their own process
    # group. Signalling the group also catches children whose command line does
    # not happen to contain CDK_ITESTS_DIR.
    if [ -n "${CDK_REGTEST_PGID:-}" ]; then
        kill "-$signal" -- "-$CDK_REGTEST_PGID" 2>/dev/null || true
    fi

    # Keep the work-dir lookup as a fallback for a service that daemonized into
    # a different process group.
    pids=$(regtest_processes_by_work_dir)
    if [ -n "$pids" ]; then
        kill "-$signal" $pids 2>/dev/null || true
    fi
}

wait_for_regtest_processes() {
    local timeout_seconds="$1"
    local waited=0

    while regtest_processes_running; do
        if [ "$waited" -ge "$timeout_seconds" ]; then
            return 1
        fi
        sleep 1
        waited=$((waited + 1))
    done
}

stop_regtest_processes() {
    if [ -z "${CDK_REGTEST_PID:-}" ] && ! regtest_processes_running; then
        return
    fi

    echo "Stopping regtest process group"
    signal_regtest_processes TERM
    if ! wait_for_regtest_processes 10; then
        echo "Regtest processes did not stop within 10 seconds; force killing..."
        signal_regtest_processes KILL
        if ! wait_for_regtest_processes 5; then
            echo "WARNING: Some regtest processes remain after SIGKILL" >&2
        fi
    fi

    # Reap the launcher only after the bounded checks show that it exited.
    # Avoid an unconditional wait: a stuck child must not hang CI cleanup.
    reap_regtest_launcher_if_exited
}

start_regtest_processes() {
    local bin_name="start_regtest_mints"
    local -a command
    if command -v "$bin_name" &>/dev/null; then
        echo "Using pre-built binary: $bin_name"
        command=("$bin_name" "$@")
    else
        echo "Pre-built binary not found, falling back to: cargo run --bin $bin_name"
        command=(cargo run --bin "$bin_name" -- "$@")
    fi

    # In CI, util-linux's setsid isolates the launcher and every service it
    # starts in a new session/process group. Bash job control provides the same
    # dedicated-process-group property on platforms without setsid.
    set +m
    if command -v setsid >/dev/null 2>&1; then
        setsid "${command[@]}" &
    else
        set -m
        "${command[@]}" &
        set +m
    fi
    CDK_REGTEST_PID=$!
    export CDK_REGTEST_PID
    CDK_REGTEST_REAPED=0

    # Both launch paths make the launcher PID the new process-group ID. setsid
    # may not have executed before the parent is scheduled, so briefly wait
    # until group signalling succeeds.
    local attempts=0
    CDK_REGTEST_PGID=$CDK_REGTEST_PID
    while [ "$attempts" -lt 50 ]; do
        if kill -0 -- "-$CDK_REGTEST_PGID" 2>/dev/null; then
            break
        fi
        if ! regtest_launcher_running; then
            break
        fi
        attempts=$((attempts + 1))
        sleep 0.1
    done
    if ! kill -0 -- "-$CDK_REGTEST_PGID" 2>/dev/null; then
        echo "ERROR: Regtest launcher did not enter an isolated process group" >&2
        kill -TERM "$CDK_REGTEST_PID" 2>/dev/null || true
        return 1
    fi
    export CDK_REGTEST_PGID
}

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    echo "Killing the cdk regtest and mints"
    stop_regtest_processes

    echo "Mint binary terminated"

    # # Remove the temporary directory
    # if [ ! -z "$CDK_ITESTS_DIR" ] && [ -d "$CDK_ITESTS_DIR" ]; then
    #     rm -rf "$CDK_ITESTS_DIR"
    #     echo "Temp directory removed: $CDK_ITESTS_DIR"
    # fi

    # Stop PostgreSQL if it was started
    if [ -d "$PWD/.pg_data" ]; then
        stop-postgres 2>/dev/null || true
    fi

    # Unset all environment variables
    unset CDK_ITESTS_DIR
    unset CDK_ITESTS_MINT_ADDR
    unset CDK_ITESTS_MINT_PORT_0
    unset CDK_ITESTS_MINT_PORT_1
    unset CDK_MINTD_DATABASE
    unset CDK_TEST_MINT_URL
    unset CDK_TEST_MINT_URL_2
    unset CDK_REGTEST_PID
    unset CDK_REGTEST_PGID
    unset CDK_REGTEST_REAPED
    # unset RUST_BACKTRACE
    unset CDK_TEST_REGTEST
    unset CDK_TEST_LIGHTNING_CLIENT
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

ensure_regtest_running() {
    if regtest_launcher_running; then
        return 0
    fi

    echo "ERROR: Regtest launcher exited before the mints became ready"
    reap_regtest_launcher_if_exited
    exit 1
}

export CDK_TEST_REGTEST=1
# export RUST_BACKTRACE=full

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
SUITE=${2:-"all"}

# Start PostgreSQL if needed
if [ "${CDK_MINTD_DATABASE}" = "POSTGRES" ]; then
    echo "Starting PostgreSQL via nix..."
    start-postgres
    echo "PostgreSQL is ready"
fi

# Build harness binary only if not available as pre-built
if ! command -v start_regtest_mints &>/dev/null; then
    cargo build --bin start_regtest_mints
fi

EXTRA_ARGS=""
if [[ "$SUITE" == "onchain" ]]; then
    EXTRA_ARGS="--skip-ln"
fi

echo "Starting regtest and mints"
# Run the launcher and all its children in a dedicated process group.
if ! start_regtest_processes --enable-logging $EXTRA_ARGS "$CDK_MINTD_DATABASE" "$CDK_ITESTS_DIR" "$CDK_ITESTS_MINT_ADDR" "$CDK_ITESTS_MINT_PORT_0" "$CDK_ITESTS_MINT_PORT_1"; then
    exit 1
fi

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
    ensure_regtest_running
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
if [[ "$SUITE" == "onchain" ]]; then
    if [ -z "$CDK_TEST_MINT_URL" ] || [ -z "$CDK_ITESTS_DIR" ]; then
        echo "ERROR: Failed to source environment variables from the .env file"
        exit 1
    fi
else
    if [ -z "$CDK_TEST_MINT_URL" ] || [ -z "$CDK_TEST_MINT_URL_2" ] || [ -z "$CDK_ITESTS_DIR" ]; then
        echo "ERROR: Failed to source environment variables from the .env file"
        exit 1
    fi
fi

# Export all variables so they're available to the tests
export CDK_TEST_MINT_URL
export CDK_TEST_MINT_URL_2

URL="$CDK_TEST_MINT_URL/v1/info"


TIMEOUT=500
START_TIME=$(date +%s)
# Loop until the endpoint returns a 200 OK status or timeout is reached
while true; do
    ensure_regtest_running

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

if [[ "$SUITE" != "onchain" ]]; then
    URL="$CDK_TEST_MINT_URL_2/v1/info"


    TIMEOUT=100
    START_TIME=$(date +%s)
    # Loop until the endpoint returns a 200 OK status or timeout is reached
    while true; do
        ensure_regtest_running

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
fi

READY_FILE_PATH="$CDK_ITESTS_DIR/.ready"
max_wait=300
wait_count=0
while [ $wait_count -lt $max_wait ]; do
    if [ -f "$READY_FILE_PATH" ]; then
        echo "Regtest mints readiness file found at: $READY_FILE_PATH"
        break
    fi
    ensure_regtest_running
    wait_count=$((wait_count + 1))
    sleep 1
done

if [ ! -f "$READY_FILE_PATH" ]; then
    echo "ERROR: Regtest mints did not become ready within $max_wait seconds"
    exit 1
fi

# Run cargo test
if [[ "$SUITE" == "all" || "$SUITE" == "ln" ]]; then
    echo "Running regtest test with CLN mint and CLN client"
    export CDK_TEST_LIGHTNING_CLIENT="lnd"
    run_test regtest
    if [ $? -ne 0 ]; then
        echo "regtest test with cln mint failed, exiting"
        exit 1
    fi

    echo "Running happy_path_mint_wallet test with CLN mint and CLN client"
    run_test happy_path_mint_wallet -- --test-threads 1
    if [ $? -ne 0 ]; then
        echo "happy_path_mint_wallet with cln mint test failed, exiting"
        exit 1
    fi

    echo "Running regtest test with cln mint for bolt12 (CLN client)"
    # The tests share one CLN node and mint. Concurrent BOLT12 RPC bursts can
    # leave CLN requests timing out before payment dispatch.
    run_test bolt12 -- --test-threads 1
    if [ $? -ne 0 ]; then
        echo "regtest test failed, exiting"
        exit 1
    fi
fi

if [[ "$SUITE" == "onchain" ]]; then
    echo "Running onchain_regtest test with dedicated onchain mint"
    run_test onchain_regtest -- --nocapture --test-threads 1
    if [ $? -ne 0 ]; then
        echo "onchain_regtest failed, exiting"
        exit 1
    fi
    echo "Onchain tests passed successfully"
    exit 0
fi

if [[ "$SUITE" == "all" ]]; then
    echo "Running onchain_regtest test with CLN mint"
    run_test onchain_regtest
    if [ $? -ne 0 ]; then
        echo "onchain_regtest with cln mint test failed, exiting"
        exit 1
    fi
fi

# Switch Mints: Run tests with LND mint
echo "Switching to LND mint for tests"

CDK_TEST_MINT_URL_SWITCHED=$CDK_TEST_MINT_URL_2
CDK_TEST_MINT_URL_2_SWITCHED=$CDK_TEST_MINT_URL
export CDK_TEST_MINT_URL=$CDK_TEST_MINT_URL_SWITCHED
export CDK_TEST_MINT_URL_2=$CDK_TEST_MINT_URL_2_SWITCHED

if [[ "$SUITE" == "all" || "$SUITE" == "ln" ]]; then
    echo "Running regtest test with LND mint and LND client"
    run_test regtest
    if [ $? -ne 0 ]; then
        echo "regtest test with LND mint failed, exiting"
        exit 1
    fi

    echo "Running happy_path_mint_wallet test with LND mint and LND client"
    run_test happy_path_mint_wallet -- --test-threads 1
    if [ $? -ne 0 ]; then
        echo "happy_path_mint_wallet test with LND mint failed, exiting"
        exit 1
    fi
fi

if [[ "$SUITE" == "all" || "$SUITE" == "onchain" ]]; then
    echo "Running onchain_regtest test with LND mint"
    run_test onchain_regtest
    if [ $? -ne 0 ]; then
        echo "onchain_regtest test with LND mint failed, exiting"
        exit 1
    fi
fi



if [[ "$SUITE" != "onchain" ]]; then
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
fi


if [[ "$SUITE" == "all" || "$SUITE" == "ln" ]]; then
    echo "Running happy_path_mint_wallet test with LDK mint and CLN client"
    export CDK_TEST_LIGHTNING_CLIENT="cln"  # Use CLN client for LDK tests
    run_test happy_path_mint_wallet -- --test-threads 1
    if [ $? -ne 0 ]; then
        echo "happy_path_mint_wallet test with LDK mint failed, exiting"
        exit 1
    fi

    echo "Running regtest test with LDK mint and CLN client"
    run_test regtest
    if [ $? -ne 0 ]; then
        echo "regtest test LDK mint failed, exiting"
        exit 1
    fi

    echo "Running bolt12 test with LDK mint (CLN client)"
    # Serialized: concurrent fetchinvoice onion messages to ldk-node get dropped
    # under burst, timing out CLN's invoice fetch (see onchain_regtest precedent)
    run_test bolt12 -- --test-threads 1
    if [ $? -ne 0 ]; then
        echo "bolt12 test with LDK mint failed, exiting"
        exit 1
    fi
fi

if [[ "$SUITE" == "all" || "$SUITE" == "onchain" ]]; then
    echo "Running onchain_regtest test with LDK mint"
    run_test onchain_regtest
    if [ $? -ne 0 ]; then
        echo "onchain_regtest test with LDK mint failed, exiting"
        exit 1
    fi
fi


echo "All tests passed successfully"
exit 0
