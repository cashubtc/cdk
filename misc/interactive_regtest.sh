#!/usr/bin/env bash

# Interactive Regtest Environment for CDK
# This script sets up the regtest environment and keeps it running for interaction

set -e

# Function to wait for HTTP endpoint
wait_for_endpoint() {
    local url=$1
    local timeout=${2:-60}
    local start_time=$(date +%s)
    
    while true; do
        local current_time=$(date +%s)
        local elapsed_time=$((current_time - start_time))

        if [ $elapsed_time -ge $timeout ]; then
            echo "❌ Timeout waiting for $url"
            return 1
        fi

        local http_status=$(curl -o /dev/null -s -w "%{http_code}" "$url" 2>/dev/null || echo "000")

        if [ "$http_status" -eq 200 ]; then
            echo "✓ $url is ready"
            return 0
        fi
        
        sleep 2
    done
}

# Function to perform cleanup
cleanup() {
    echo "Cleaning up..."

    # Remove state file for other sessions
    rm -f "/tmp/cdk_regtest_env"

    if [ ! -z "$CDK_MINTD_PID" ] && kill -0 $CDK_MINTD_PID 2>/dev/null; then
        echo "Killing the cdk mintd (CLN)"
        kill -2 $CDK_MINTD_PID
        wait $CDK_MINTD_PID
    fi

    if [ ! -z "$CDK_MINTD_LND_PID" ] && kill -0 $CDK_MINTD_LND_PID 2>/dev/null; then
        echo "Killing the cdk mintd (LND)"
        kill -2 $CDK_MINTD_LND_PID
        wait $CDK_MINTD_LND_PID
    fi

    if [ ! -z "$CDK_REGTEST_PID" ] && kill -0 $CDK_REGTEST_PID 2>/dev/null; then
        echo "Killing the cdk regtest"
        kill -2 $CDK_REGTEST_PID
        wait $CDK_REGTEST_PID
    fi

    echo "Environment terminated"

    # Remove the temporary directory
    if [ ! -z "$CDK_ITESTS_DIR" ]; then
        rm -rf "$CDK_ITESTS_DIR"
        echo "Temp directory removed: $CDK_ITESTS_DIR"
    fi
    
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

# Check for mprocs and offer to install if missing
if ! command -v mprocs >/dev/null 2>&1; then
    echo "⚠️  mprocs not found - this tool provides a nice TUI for monitoring logs"
    echo "Install it with: cargo install mprocs"
    echo
    read -p "Would you like to install mprocs now? (y/n): " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Installing mprocs..."
        cargo install mprocs
        if [ $? -eq 0 ]; then
            echo "✓ mprocs installed successfully"
        else
            echo "❌ Failed to install mprocs. You can install it later with: cargo install mprocs"
        fi
    else
        echo "Skipping mprocs installation. The environment will work without it."
    fi
    echo
fi

# Parse command line arguments
CDK_MINTD_DATABASE=${1:-"sqlite"}  # Default to sqlite if not specified

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

echo "=============================================="
echo "Starting Interactive CDK Regtest Environment"
echo "=============================================="
echo "Temp directory: $CDK_ITESTS_DIR"
echo "Database type: $CDK_MINTD_DATABASE"
echo

export CDK_MINTD_DATABASE="$CDK_MINTD_DATABASE"

# Build the necessary binaries
echo "Building binaries..."
cargo build -p cdk-integration-tests --bin start_regtest

echo "Starting regtest network (Bitcoin + Lightning nodes)..."
cargo run --bin start_regtest &
export CDK_REGTEST_PID=$!

# Create named pipe for progress tracking
mkfifo "$CDK_ITESTS_DIR/progress_pipe"
rm -f "$CDK_ITESTS_DIR/signal_received"

# Start reading from pipe in background
(while read line; do
    case "$line" in
        "checkpoint1")
            echo "✓ Regtest network is ready"
            touch "$CDK_ITESTS_DIR/signal_received"
            exit 0
            ;;
    esac
done < "$CDK_ITESTS_DIR/progress_pipe") &

# Wait for regtest setup (up to 120 seconds)
echo "Waiting for regtest network to be ready..."
for ((i=0; i<120; i++)); do
    if [ -f "$CDK_ITESTS_DIR/signal_received" ]; then
        break
    fi
    sleep 1
done

if [ ! -f "$CDK_ITESTS_DIR/signal_received" ]; then
    echo "❌ Timeout waiting for regtest network"
    exit 1
fi

echo
echo "Starting CDK Mint #1 (CLN backend)..."
export CDK_MINTD_CLN_RPC_PATH="$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc"
export CDK_MINTD_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/cln_mint"
export CDK_MINTD_LISTEN_HOST=$CDK_ITESTS_MINT_ADDR
export CDK_MINTD_LISTEN_PORT=$CDK_ITESTS_MINT_PORT_0
export CDK_MINTD_LN_BACKEND="cln"
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
export RUST_BACKTRACE=1

mkdir -p "$CDK_MINTD_WORK_DIR"
cargo run --bin cdk-mintd > "$CDK_MINTD_WORK_DIR/mintd.log" 2>&1 &
export CDK_MINTD_PID=$!

# Wait for CLN mint to be ready
echo "Waiting for CLN mint to be ready..."
URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0/v1/info"
wait_for_endpoint "$URL" 60

echo
echo "Starting CDK Mint #2 (LND backend)..."
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

cargo run --bin cdk-mintd > "$CDK_MINTD_WORK_DIR/mintd.log" 2>&1 &
export CDK_MINTD_LND_PID=$!

# Wait for LND mint to be ready
echo "Waiting for LND mint to be ready..."
URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1/v1/info"
wait_for_endpoint "$URL" 60

# Set environment variables for easy access
export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_TEST_MINT_URL_2="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1"

# Create state file for other terminal sessions
ENV_FILE="/tmp/cdk_regtest_env"
echo "export CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\"" > "$ENV_FILE"
echo "export CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\"" >> "$ENV_FILE"
echo "export CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\"" >> "$ENV_FILE"
echo "export CDK_MINTD_PID=\"$CDK_MINTD_PID\"" >> "$ENV_FILE"
echo "export CDK_MINTD_LND_PID=\"$CDK_MINTD_LND_PID\"" >> "$ENV_FILE"
echo "export CDK_REGTEST_PID=\"$CDK_REGTEST_PID\"" >> "$ENV_FILE"

echo
echo "=============================================="
echo "🎉 CDK Regtest Environment is Ready!"
echo "=============================================="
echo
echo "Network Information:"
echo "  • Bitcoin RPC: 127.0.0.1:18443 (user: testuser, pass: testpass)"
echo "  • CLN Node 1: $CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc"
echo "  • CLN Node 2: $CDK_ITESTS_DIR/cln/two/regtest/lightning-rpc"  
echo "  • LND Node 1: https://localhost:10009"
echo "  • LND Node 2: https://localhost:10010"
echo
echo "CDK Mints:"
echo "  • CLN Mint:   $CDK_TEST_MINT_URL"
echo "  • LND Mint:   $CDK_TEST_MINT_URL_2"
echo
echo "Files and Directories:"
echo "  • Working Directory:  $CDK_ITESTS_DIR"
echo "  • CLN Mint Logs:      $CDK_ITESTS_DIR/cln_mint/mintd.log"
echo "  • LND Mint Logs:      $CDK_ITESTS_DIR/lnd_mint/mintd.log"
echo "  • LND 1 TLS Cert:     $CDK_ITESTS_DIR/lnd/one/tls.cert"
echo "  • LND 1 Macaroon:     $CDK_ITESTS_DIR/lnd/one/data/chain/bitcoin/regtest/admin.macaroon"
echo "  • LND 2 TLS Cert:     $CDK_ITESTS_DIR/lnd/two/tls.cert"
echo "  • LND 2 Macaroon:     $CDK_ITESTS_DIR/lnd/two/data/chain/bitcoin/regtest/admin.macaroon"
echo
echo "Environment Variables (available in other terminals):"
echo "  • CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\""
echo "  • CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\""
echo "  • CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\""
echo
echo "You can now:"
echo "  • Use 'just' commands in other terminals: 'just ln-cln1 getinfo'"
echo "  • Run integration tests: 'just mint-test' or 'cargo test -p cdk-integration-tests'"
echo "  • Use CDK CLI tools with the mint URLs above"
echo "  • Interact with Lightning nodes directly"
echo "  • Access Bitcoin regtest node"
echo
echo "State File: /tmp/cdk_regtest_env (allows other terminals to find this environment)"
echo
echo "Starting mprocs to monitor logs..."
echo "Press 'q' to quit mprocs and stop the environment"
echo "=============================================="

# Create mprocs configuration
MPROCS_CONFIG="$CDK_ITESTS_DIR/mprocs.yaml"
cat > "$MPROCS_CONFIG" << EOF
procs:
  cln-mint:
    shell: "touch $CDK_ITESTS_DIR/cln_mint/mintd.log && tail -f $CDK_ITESTS_DIR/cln_mint/mintd.log"
    autostart: true
  
  lnd-mint:
    shell: "touch $CDK_ITESTS_DIR/lnd_mint/mintd.log && tail -f $CDK_ITESTS_DIR/lnd_mint/mintd.log"
    autostart: true
  
  bitcoind:
    shell: "touch $CDK_ITESTS_DIR/bitcoin/regtest/debug.log && tail -f $CDK_ITESTS_DIR/bitcoin/regtest/debug.log"
    autostart: true
  
  cln-one:
    shell: "while [ ! -f $CDK_ITESTS_DIR/cln/one/regtest/log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/cln/one/regtest/log"
    autostart: true
  
  cln-two:
    shell: "while [ ! -f $CDK_ITESTS_DIR/cln/two/regtest/log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/cln/two/regtest/log"
    autostart: true
  
  lnd-one:
    shell: "while [ ! -f $CDK_ITESTS_DIR/lnd/one/logs/bitcoin/regtest/lnd.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/lnd/one/logs/bitcoin/regtest/lnd.log"
    autostart: true
  
  lnd-two:
    shell: "while [ ! -f $CDK_ITESTS_DIR/lnd/two/logs/bitcoin/regtest/lnd.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/lnd/two/logs/bitcoin/regtest/lnd.log"
    autostart: true

settings:
  mouse_scroll_speed: 3
  proc_list_width: 20
  hide_keymap_window: false
EOF

# Wait a bit for log files to be created
sleep 2

# Start mprocs to show all logs
if command -v mprocs >/dev/null 2>&1; then
    cd "$CDK_ITESTS_DIR"
    mprocs --config "$MPROCS_CONFIG"
else
    echo "⚠️  mprocs not found. Install it with: cargo install mprocs"
    echo "Falling back to simple wait loop..."
    echo "Press Ctrl+C to stop the environment"
    # Keep the script running
    while true; do
        sleep 1
    done
fi
