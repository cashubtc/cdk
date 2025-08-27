#!/usr/bin/env bash

# Interactive Regtest Environment for CDK with Direct Process Management
# This script sets up mprocs to manage the mint processes directly

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
    unset CDK_REGTEST_PID
    unset RUST_BACKTRACE
    unset CDK_TEST_REGTEST
}

# Set up trap to call cleanup on script exit
trap cleanup EXIT

export CDK_TEST_REGTEST=1

# Check for mprocs and offer to install if missing
if ! command -v mprocs >/dev/null 2>&1; then
    echo "⚠️  mprocs not found - this tool is required for direct process management"
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
            echo "❌ Failed to install mprocs."
            exit 1
        fi
    else
        echo "❌ mprocs is required for this mode. Exiting."
        exit 1
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
export CDK_ITESTS_MINT_PORT_2=8089

# Check if the temporary directory was created successfully
if [[ ! -d "$CDK_ITESTS_DIR" ]]; then
    echo "Failed to create temp directory"
    exit 1
fi

echo "=============================================="
echo "Starting CDK Regtest with Direct Process Management"
echo "=============================================="
echo "Temp directory: $CDK_ITESTS_DIR"
echo "Database type: $CDK_MINTD_DATABASE"
echo

export CDK_MINTD_DATABASE="$CDK_MINTD_DATABASE"

# Build the necessary binaries
echo "Building binaries..."
cargo build -p cdk-integration-tests --bin start_regtest
cargo build --bin cdk-mintd

echo "Starting regtest network (Bitcoin + Lightning nodes)..."
cargo run --bin start_regtest -- --enable-logging "$CDK_ITESTS_DIR" &
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
for ((i=0; i<220; i++)); do
    if [ -f "$CDK_ITESTS_DIR/signal_received" ]; then
        break
    fi
    sleep 1
done

if [ ! -f "$CDK_ITESTS_DIR/signal_received" ]; then
    echo "❌ Timeout waiting for regtest network"
    exit 1
fi

# Create work directories for mints
mkdir -p "$CDK_ITESTS_DIR/cln_mint"
mkdir -p "$CDK_ITESTS_DIR/lnd_mint"
mkdir -p "$CDK_ITESTS_DIR/ldk_node_mint"

# Set environment variables for easy access
export CDK_TEST_MINT_URL="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_0"
export CDK_TEST_MINT_URL_2="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_1"
export CDK_TEST_MINT_URL_3="http://$CDK_ITESTS_MINT_ADDR:$CDK_ITESTS_MINT_PORT_2"

# Create state file for other terminal sessions
ENV_FILE="/tmp/cdk_regtest_env"
echo "export CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\"" > "$ENV_FILE"
echo "export CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\"" >> "$ENV_FILE"
echo "export CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\"" >> "$ENV_FILE"
echo "export CDK_TEST_MINT_URL_3=\"$CDK_TEST_MINT_URL_3\"" >> "$ENV_FILE"
echo "export CDK_REGTEST_PID=\"$CDK_REGTEST_PID\"" >> "$ENV_FILE"

# Get the project root directory (where justfile is located)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Create environment setup scripts for mprocs to use
cat > "$CDK_ITESTS_DIR/start_cln_mint.sh" << EOF
#!/usr/bin/env bash
cd "$PROJECT_ROOT"
export CDK_MINTD_CLN_RPC_PATH="$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc"
export CDK_MINTD_URL="http://127.0.0.1:8085"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/cln_mint"
export CDK_MINTD_LISTEN_HOST="127.0.0.1"
export CDK_MINTD_LISTEN_PORT=8085
export CDK_MINTD_LN_BACKEND="cln"
export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
export CDK_MINTD_LOGGING_OUTPUT="both"
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
export CDK_MINTD_LOGGING_FILE_LEVEL="debug"
export RUST_BACKTRACE=1
export CDK_MINTD_DATABASE="$CDK_MINTD_DATABASE"

echo "Starting CLN Mint on port 8085..."
echo "Project root: $PROJECT_ROOT"
echo "Working directory: \$CDK_MINTD_WORK_DIR"
echo "CLN RPC path: \$CDK_MINTD_CLN_RPC_PATH"
echo "Database type: \$CDK_MINTD_DATABASE"
echo "Logging: \$CDK_MINTD_LOGGING_OUTPUT (console: \$CDK_MINTD_LOGGING_CONSOLE_LEVEL, file: \$CDK_MINTD_LOGGING_FILE_LEVEL)"
echo "---"

exec cargo run --bin cdk-mintd
EOF

cat > "$CDK_ITESTS_DIR/start_lnd_mint.sh" << EOF
#!/usr/bin/env bash
cd "$PROJECT_ROOT"
export CDK_MINTD_LND_ADDRESS="https://localhost:10010"
export CDK_MINTD_LND_CERT_FILE="$CDK_ITESTS_DIR/lnd/two/tls.cert"
export CDK_MINTD_LND_MACAROON_FILE="$CDK_ITESTS_DIR/lnd/two/data/chain/bitcoin/regtest/admin.macaroon"
export CDK_MINTD_URL="http://127.0.0.1:8087"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/lnd_mint"
export CDK_MINTD_LISTEN_HOST="127.0.0.1"
export CDK_MINTD_LISTEN_PORT=8087
export CDK_MINTD_LN_BACKEND="lnd"
export CDK_MINTD_MNEMONIC="cattle gold bind busy sound reduce tone addict baby spend february strategy"
export CDK_MINTD_LOGGING_OUTPUT="both"
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
export CDK_MINTD_LOGGING_FILE_LEVEL="debug"
export RUST_BACKTRACE=1
export CDK_MINTD_DATABASE="$CDK_MINTD_DATABASE"

echo "Starting LND Mint on port 8087..."
echo "Project root: $PROJECT_ROOT"
echo "Working directory: \$CDK_MINTD_WORK_DIR"
echo "LND address: \$CDK_MINTD_LND_ADDRESS"
echo "Database type: \$CDK_MINTD_DATABASE"
echo "Logging: \$CDK_MINTD_LOGGING_OUTPUT (console: \$CDK_MINTD_LOGGING_CONSOLE_LEVEL, file: \$CDK_MINTD_LOGGING_FILE_LEVEL)"
echo "---"

exec cargo run --bin cdk-mintd
EOF

cat > "$CDK_ITESTS_DIR/start_ldk_node_mint.sh" << EOF
#!/usr/bin/env bash
cd "$PROJECT_ROOT"
export CDK_MINTD_URL="http://127.0.0.1:8089"
export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/ldk_node_mint"
export CDK_MINTD_LISTEN_HOST="127.0.0.1"
export CDK_MINTD_LISTEN_PORT=8089
export CDK_MINTD_LN_BACKEND="ldk-node"
export CDK_MINTD_LOGGING_CONSOLE_LEVEL="debug"
export CDK_MINTD_LOGGING_FILE_LEVEL="debug"
export CDK_MINTD_MNEMONIC="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
export RUST_BACKTRACE=1
export CDK_MINTD_DATABASE="$CDK_MINTD_DATABASE"

# LDK Node specific environment variables
export CDK_MINTD_LDK_NODE_BITCOIN_NETWORK="regtest"
export CDK_MINTD_LDK_NODE_CHAIN_SOURCE_TYPE="bitcoinrpc"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_HOST="127.0.0.1"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_PORT=18443
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_USER="testuser"
export CDK_MINTD_LDK_NODE_BITCOIND_RPC_PASSWORD="testpass"
export CDK_MINTD_LDK_NODE_STORAGE_DIR_PATH="$CDK_ITESTS_DIR/ldk_mint"
export CDK_MINTD_LDK_NODE_LDK_NODE_HOST="127.0.0.1"
export CDK_MINTD_LDK_NODE_LDK_NODE_PORT=8090
export CDK_MINTD_LDK_NODE_GOSSIP_SOURCE_TYPE="p2p"
export CDK_MINTD_LDK_NODE_FEE_PERCENT=0.02
export CDK_MINTD_LDK_NODE_RESERVE_FEE_MIN=2

echo "Starting LDK Node Mint on port 8089..."
echo "Project root: $PROJECT_ROOT"
echo "Working directory: \$CDK_MINTD_WORK_DIR"
echo "Bitcoin RPC: 127.0.0.1:18443 (testuser/testpass)"
echo "LDK Node listen: 127.0.0.1:8090"
echo "Storage directory: \$CDK_MINTD_LDK_NODE_STORAGE_DIR_PATH"
echo "Database type: \$CDK_MINTD_DATABASE"
echo "---"

exec cargo run --bin cdk-mintd --features ldk-node
EOF

# Make scripts executable
chmod +x "$CDK_ITESTS_DIR/start_cln_mint.sh"
chmod +x "$CDK_ITESTS_DIR/start_lnd_mint.sh"
chmod +x "$CDK_ITESTS_DIR/start_ldk_node_mint.sh"

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
echo "CDK Mints (will be managed by mprocs):"
echo "  • CLN Mint:       $CDK_TEST_MINT_URL"
echo "  • LND Mint:       $CDK_TEST_MINT_URL_2"
echo "  • LDK Node Mint:  $CDK_TEST_MINT_URL_3"
echo
echo "Files and Directories:"
echo "  • Working Directory:  $CDK_ITESTS_DIR"
echo "  • Start Scripts:      $CDK_ITESTS_DIR/start_{cln,lnd,ldk_node}_mint.sh"
echo
echo "Environment Variables (available in other terminals):"
echo "  • CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\""
echo "  • CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\""
echo "  • CDK_TEST_MINT_URL_3=\"$CDK_TEST_MINT_URL_3\""
echo "  • CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\""
echo
echo "Starting mprocs with direct process management..."
echo
echo "In mprocs you can:"
echo "  • 's' to start a process"
echo "  • 'k' to kill a process"
echo "  • 'r' to restart a process"
echo "  • 'Enter' to focus on a process"
echo "  • 'q' to quit and stop the environment"
echo "=============================================="

# Wait a moment for everything to settle
sleep 2

# Create mprocs configuration with direct process management
MPROCS_CONFIG="$CDK_ITESTS_DIR/mprocs.yaml"
cat > "$MPROCS_CONFIG" << EOF
procs:
  cln-mint:
    shell: "$CDK_ITESTS_DIR/start_cln_mint.sh"
    autostart: true
    env:
      CDK_ITESTS_DIR: "$CDK_ITESTS_DIR"
      CDK_MINTD_DATABASE: "$CDK_MINTD_DATABASE"
  
  lnd-mint:
    shell: "$CDK_ITESTS_DIR/start_lnd_mint.sh"
    autostart: true
    env:
      CDK_ITESTS_DIR: "$CDK_ITESTS_DIR"
      CDK_MINTD_DATABASE: "$CDK_MINTD_DATABASE"
  
  ldk-node-mint:
    shell: "$CDK_ITESTS_DIR/start_ldk_node_mint.sh"
    autostart: true
    env:
      CDK_ITESTS_DIR: "$CDK_ITESTS_DIR"
      CDK_MINTD_DATABASE: "$CDK_MINTD_DATABASE"
  
  bitcoind:
    shell: "while [ ! -f $CDK_ITESTS_DIR/bitcoin/regtest/debug.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/bitcoin/regtest/debug.log"
    autostart: true
  
  cln-one:
    shell: "while [ ! -f $CDK_ITESTS_DIR/cln/one/debug.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/cln/one/debug.log"
    autostart: true
  
  cln-two:
    shell: "while [ ! -f $CDK_ITESTS_DIR/cln/two/debug.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/cln/two/debug.log"
    autostart: true
  
  lnd-one:
    shell: "while [ ! -f $CDK_ITESTS_DIR/lnd/one/logs/bitcoin/regtest/lnd.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/lnd/one/logs/bitcoin/regtest/lnd.log"
    autostart: true
  
  lnd-two:
    shell: "while [ ! -f $CDK_ITESTS_DIR/lnd/two/logs/bitcoin/regtest/lnd.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/lnd/two/logs/bitcoin/regtest/lnd.log"
    autostart: true
  
  ldk-node:
    shell: "while [ ! -f $CDK_ITESTS_DIR/ldk_mint/ldk_node.log ]; do sleep 1; done && $PROJECT_ROOT/misc/scripts/filtered_ldk_node_log.sh $CDK_ITESTS_DIR/ldk_mint/ldk_node.log"
    autostart: true

settings:
  mouse_scroll_speed: 3
  proc_list_width: 20
  hide_keymap_window: false
  keymap_procs:
    toggle_process: 's'
    kill_process: 'k'
    restart_process: 'r'
    focus_process: 'Enter'
    show_keymap: '?'
EOF

# Start mprocs with direct process management
echo "Starting mprocs..."
cd "$CDK_ITESTS_DIR"
mprocs --config "$MPROCS_CONFIG"
