#!/usr/bin/env bash

# Helper script for interacting with CDK regtest environment
# Run this after starting interactive_regtest_mprocs.sh

# Check for environment state file first, then environment variable
ENV_FILE="/tmp/cdk_regtest_env"
if [ -f "$ENV_FILE" ]; then
    source "$ENV_FILE"
elif [ ! -z "$CDK_ITESTS_DIR" ]; then
    # Environment variable is set, create state file for other sessions
    echo "export CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\"" > "$ENV_FILE"
    echo "export CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\"" >> "$ENV_FILE"
    echo "export CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\"" >> "$ENV_FILE"
    echo "export CDK_TEST_MINT_URL_3=\"$CDK_TEST_MINT_URL_3\"" >> "$ENV_FILE"
    echo "export CDK_MINTD_PID=\"$CDK_MINTD_PID\"" >> "$ENV_FILE"
    echo "export CDK_MINTD_LND_PID=\"$CDK_MINTD_LND_PID\"" >> "$ENV_FILE"
    echo "export CDK_REGTEST_PID=\"$CDK_REGTEST_PID\"" >> "$ENV_FILE"
else
    echo "❌ CDK regtest environment not found!"
    echo "Please run './misc/interactive_regtest_mprocs.sh' or 'just regtest' first"
    exit 1
fi

# Validate that the environment is actually running
if [ -z "$CDK_ITESTS_DIR" ] || [ ! -d "$CDK_ITESTS_DIR" ]; then
    echo "❌ CDK regtest environment not found or directory missing!"
    echo "Please run './misc/interactive_regtest_mprocs.sh' or 'just regtest' first"
    [ -f "$ENV_FILE" ] && rm "$ENV_FILE"  # Clean up stale state file
    exit 1
fi

show_help() {
    echo "CDK Regtest Environment Helper"
    echo "============================="
    echo
    echo "Lightning Node Commands:"
    echo "  ln-cln1     <command>   - Execute command on CLN node 1"
    echo "  ln-cln2     <command>   - Execute command on CLN node 2"  
    echo "  ln-lnd1     <command>   - Execute command on LND node 1"
    echo "  ln-lnd2     <command>   - Execute command on LND node 2"
    echo
    echo "Bitcoin Commands:"
    echo "  btc         <command>   - Execute bitcoin-cli command"
    echo "  btc-mine    [blocks]    - Mine blocks (default: 10)"
    echo
    echo "CDK Mint Commands:"
    echo "  mint-info              - Show mint information"
    echo "  mint-test              - Run integration tests"
    echo "  restart-mints          - Stop, recompile, and restart both mints (log mode)"
    echo 
    echo "Environment Commands:"
    echo "  show-env               - Show environment variables"
    echo "  show-logs              - Show recent mint logs"
    echo "  show-status            - Show status of all components"
    echo "  logs                   - Start mprocs TUI (adapts to current mode)"
    echo
    echo "Environment Modes:"
    echo "  just regtest           - Log tailing mode (mints auto-start, logs to files)"
    echo "  just regtest-mprocs    - Direct management (mprocs controls mint processes)"
    echo
    echo "Examples:"
    echo "  $0 ln-cln1 getinfo"
    echo "  $0 ln-lnd1 getinfo"
    echo "  $0 btc getblockcount"
    echo "  $0 btc-mine 5"
    echo "  $0 mint-info"
    echo "  $0 restart-mints       # Only works in log tailing mode"
    echo "  $0 logs                # Start mprocs viewer"
}

# Bitcoin commands
btc_command() {
    bitcoin-cli -regtest -rpcuser=testuser -rpcpassword=testpass -rpcport=18443 "$@"
}

btc_mine() {
    local blocks=${1:-10}
    local address=$(btc_command getnewaddress)
    btc_command generatetoaddress "$blocks" "$address"
    echo "Mined $blocks blocks"
}

# CLN commands  
cln_command() {
    local node=$1
    shift
    lightning-cli --rpc-file="$CDK_ITESTS_DIR/cln/$node/regtest/lightning-rpc" "$@"
}

# LND commands
lnd_command() {
    local node=$1
    shift
    local port
    case $node in
        "one") port=10009 ;;
        "two") port=10010 ;;
        *) echo "Unknown LND node: $node"; return 1 ;;
    esac
    
    lncli --rpcserver=localhost:$port \
          --tlscertpath="$CDK_ITESTS_DIR/lnd/$node/tls.cert" \
          --macaroonpath="$CDK_ITESTS_DIR/lnd/$node/data/chain/bitcoin/regtest/admin.macaroon" \
          "$@"
}

# Mint commands
mint_info() {
    echo "CLN Mint (Port 8085):"
    curl -s "$CDK_TEST_MINT_URL/v1/info" | jq . 2>/dev/null || curl -s "$CDK_TEST_MINT_URL/v1/info"
    echo
    echo "LND Mint (Port 8087):"
    curl -s "$CDK_TEST_MINT_URL_2/v1/info" | jq . 2>/dev/null || curl -s "$CDK_TEST_MINT_URL_2/v1/info"
    echo
    if [ ! -z "$CDK_TEST_MINT_URL_3" ]; then
        echo "LDK Node Mint (Port 8089):"
        curl -s "$CDK_TEST_MINT_URL_3/v1/info" | jq . 2>/dev/null || curl -s "$CDK_TEST_MINT_URL_3/v1/info"
    fi
}

mint_test() {
    echo "Running integration tests..."
    cargo test -p cdk-integration-tests
}

# Environment info
show_env() {
    echo "CDK Regtest Environment Variables:"
    echo "================================="
    echo "CDK_ITESTS_DIR=$CDK_ITESTS_DIR"
    echo "CDK_TEST_MINT_URL=$CDK_TEST_MINT_URL"
    echo "CDK_TEST_MINT_URL_2=$CDK_TEST_MINT_URL_2"
    if [ ! -z "$CDK_TEST_MINT_URL_3" ]; then
        echo "CDK_TEST_MINT_URL_3=$CDK_TEST_MINT_URL_3"
    fi
    echo "CDK_MINTD_PID=$CDK_MINTD_PID"
    echo "CDK_MINTD_LND_PID=$CDK_MINTD_LND_PID"
    echo "CDK_REGTEST_PID=$CDK_REGTEST_PID"
}

show_logs() {
    echo "=== Recent CLN Mint Logs ==="
    if [ -f "$CDK_ITESTS_DIR/cln_mint/mintd.log" ]; then
        tail -10 "$CDK_ITESTS_DIR/cln_mint/mintd.log"
    else
        echo "Log file not found"
    fi
    echo
    echo "=== Recent LND Mint Logs ==="
    if [ -f "$CDK_ITESTS_DIR/lnd_mint/mintd.log" ]; then
        tail -10 "$CDK_ITESTS_DIR/lnd_mint/mintd.log"
    else
        echo "Log file not found"
    fi
    echo
    if [ ! -z "$CDK_TEST_MINT_URL_3" ]; then
        echo "=== Recent LDK Node Mint Logs ==="
        if [ -f "$CDK_ITESTS_DIR/ldk_node_mint/mintd.log" ]; then
            tail -10 "$CDK_ITESTS_DIR/ldk_node_mint/mintd.log"
        else
            echo "Log file not found"
        fi
    fi
}

start_mprocs() {
    echo "Starting mprocs log viewer..."
    
    if ! command -v mprocs >/dev/null 2>&1; then
        echo "❌ mprocs not found! Please install it with:"
        echo "   cargo install mprocs"
        echo "   or your package manager"
        return 1
    fi
    
    # Check if we have the direct process management config
    DIRECT_MPROCS_CONFIG="$CDK_ITESTS_DIR/mprocs.yaml"
    FALLBACK_MPROCS_CONFIG="$CDK_ITESTS_DIR/mprocs_fallback.yaml"
    
    if [ -f "$DIRECT_MPROCS_CONFIG" ]; then
        echo "Using direct process management mode..."
        echo "In mprocs: 's' to start, 'k' to kill, 'r' to restart processes"
        cd "$CDK_ITESTS_DIR"
        mprocs --config "$DIRECT_MPROCS_CONFIG"
        return
    fi
    
    # Create fallback mprocs configuration for log tailing
    cat > "$FALLBACK_MPROCS_CONFIG" << EOF
procs:
  cln-mint:
    shell: "touch $CDK_ITESTS_DIR/cln_mint/mintd.log && tail -f $CDK_ITESTS_DIR/cln_mint/mintd.log"
    autostart: true
  
  lnd-mint:
    shell: "touch $CDK_ITESTS_DIR/lnd_mint/mintd.log && tail -f $CDK_ITESTS_DIR/lnd_mint/mintd.log"
    autostart: true
  
  ldk-node-mint:
    shell: "touch $CDK_ITESTS_DIR/ldk_node_mint/mintd.log && tail -f $CDK_ITESTS_DIR/ldk_node_mint/mintd.log"
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
  
  ldk-node:
    shell: "while [ ! -f $CDK_ITESTS_DIR/ldk_node_mint/ldk_storage/ldk_node.log ]; do sleep 1; done && tail -f $CDK_ITESTS_DIR/ldk_node_mint/ldk_storage/ldk_node.log"
    autostart: true

settings:
  mouse_scroll_speed: 3
  proc_list_width: 20
  hide_keymap_window: false
EOF

    echo "Using log tailing mode..."
    echo "Use 'q' to quit the log viewer"
    cd "$CDK_ITESTS_DIR"
    mprocs --config "$FALLBACK_MPROCS_CONFIG"
}

show_status() {
    echo "CDK Regtest Environment Status:"
    echo "==============================="
    
    # Check processes
    echo "Processes:"
    if [ ! -z "$CDK_REGTEST_PID" ] && kill -0 $CDK_REGTEST_PID 2>/dev/null; then
        echo "  ✓ Regtest network (PID: $CDK_REGTEST_PID)"
    else
        echo "  ❌ Regtest network"
    fi
    
    if [ ! -z "$CDK_MINTD_PID" ] && kill -0 $CDK_MINTD_PID 2>/dev/null; then
        echo "  ✓ CLN Mint (PID: $CDK_MINTD_PID)"
    else
        echo "  ❌ CLN Mint"
    fi
    
    if [ ! -z "$CDK_MINTD_LND_PID" ] && kill -0 $CDK_MINTD_LND_PID 2>/dev/null; then
        echo "  ✓ LND Mint (PID: $CDK_MINTD_LND_PID)"
    else
        echo "  ❌ LND Mint"
    fi
    
    echo
    echo "Network connectivity:"
    if curl -s "$CDK_TEST_MINT_URL/v1/info" >/dev/null 2>&1; then
        echo "  ✓ CLN Mint responding"
    else
        echo "  ❌ CLN Mint not responding"
    fi
    
    if curl -s "$CDK_TEST_MINT_URL_2/v1/info" >/dev/null 2>&1; then
        echo "  ✓ LND Mint responding"
    else
        echo "  ❌ LND Mint not responding"
    fi
    
    if [ ! -z "$CDK_TEST_MINT_URL_3" ]; then
        if curl -s "$CDK_TEST_MINT_URL_3/v1/info" >/dev/null 2>&1; then
            echo "  ✓ LDK Node Mint responding"
        else
            echo "  ❌ LDK Node Mint not responding"
        fi
    fi
}

restart_mints() {
    echo "==============================="
    echo "Restarting CDK Mints"
    echo "==============================="
    
    # Stop existing mints
    echo "Stopping existing mints..."
    if [ ! -z "$CDK_MINTD_PID" ] && kill -0 $CDK_MINTD_PID 2>/dev/null; then
        echo "  Stopping CLN Mint (PID: $CDK_MINTD_PID)"
        kill -2 $CDK_MINTD_PID
        wait $CDK_MINTD_PID 2>/dev/null || true
    fi
    
    if [ ! -z "$CDK_MINTD_LND_PID" ] && kill -0 $CDK_MINTD_LND_PID 2>/dev/null; then
        echo "  Stopping LND Mint (PID: $CDK_MINTD_LND_PID)"
        kill -2 $CDK_MINTD_LND_PID
        wait $CDK_MINTD_LND_PID 2>/dev/null || true
    fi
    
    # Recompile
    echo "Recompiling cdk-mintd..."
    if ! cargo build --bin cdk-mintd; then
        echo "❌ Compilation failed"
        return 1
    fi
    echo "✓ Compilation successful"
    
    # Restart CLN mint
    echo "Starting CLN Mint..."
    export CDK_MINTD_CLN_RPC_PATH="$CDK_ITESTS_DIR/cln/one/regtest/lightning-rpc"
    export CDK_MINTD_URL="http://127.0.0.1:8085"
    export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/cln_mint"
    export CDK_MINTD_LISTEN_HOST="127.0.0.1"
    export CDK_MINTD_LISTEN_PORT=8085
    export CDK_MINTD_LN_BACKEND="cln"
    export CDK_MINTD_MNEMONIC="eye survey guilt napkin crystal cup whisper salt luggage manage unveil loyal"
    export RUST_BACKTRACE=1
    
    cargo run --bin cdk-mintd > "$CDK_MINTD_WORK_DIR/mintd.log" 2>&1 &
    NEW_CLN_PID=$!
    
    # Wait for CLN mint to be ready
    echo "Waiting for CLN mint to start..."
    local start_time=$(date +%s)
    while true; do
        local current_time=$(date +%s)
        local elapsed_time=$((current_time - start_time))
        
        if [ $elapsed_time -ge 30 ]; then
            echo "❌ Timeout waiting for CLN mint"
            return 1
        fi
        
        if curl -s "http://127.0.0.1:8085/v1/info" >/dev/null 2>&1; then
            echo "✓ CLN Mint ready"
            break
        fi
        sleep 1
    done
    
    # Restart LND mint
    echo "Starting LND Mint..."
    export CDK_MINTD_LND_ADDRESS="https://localhost:10010"
    export CDK_MINTD_LND_CERT_FILE="$CDK_ITESTS_DIR/lnd/two/tls.cert"
    export CDK_MINTD_LND_MACAROON_FILE="$CDK_ITESTS_DIR/lnd/two/data/chain/bitcoin/regtest/admin.macaroon"
    export CDK_MINTD_URL="http://127.0.0.1:8087"
    export CDK_MINTD_WORK_DIR="$CDK_ITESTS_DIR/lnd_mint"
    export CDK_MINTD_LISTEN_HOST="127.0.0.1"
    export CDK_MINTD_LISTEN_PORT=8087
    export CDK_MINTD_LN_BACKEND="lnd"
    export CDK_MINTD_MNEMONIC="cattle gold bind busy sound reduce tone addict baby spend february strategy"
    
    cargo run --bin cdk-mintd > "$CDK_MINTD_WORK_DIR/mintd.log" 2>&1 &
    NEW_LND_PID=$!
    
    # Wait for LND mint to be ready
    echo "Waiting for LND mint to start..."
    start_time=$(date +%s)
    while true; do
        current_time=$(date +%s)
        elapsed_time=$((current_time - start_time))
        
        if [ $elapsed_time -ge 30 ]; then
            echo "❌ Timeout waiting for LND mint"
            return 1
        fi
        
        if curl -s "http://127.0.0.1:8087/v1/info" >/dev/null 2>&1; then
            echo "✓ LND Mint ready"
            break
        fi
        sleep 1
    done
    
    # Update PIDs in state file
    CDK_MINTD_PID=$NEW_CLN_PID
    CDK_MINTD_LND_PID=$NEW_LND_PID
    
    # Update state file
    echo "export CDK_ITESTS_DIR=\"$CDK_ITESTS_DIR\"" > "$ENV_FILE"
    echo "export CDK_TEST_MINT_URL=\"$CDK_TEST_MINT_URL\"" >> "$ENV_FILE"
    echo "export CDK_TEST_MINT_URL_2=\"$CDK_TEST_MINT_URL_2\"" >> "$ENV_FILE"
    echo "export CDK_MINTD_PID=\"$CDK_MINTD_PID\"" >> "$ENV_FILE"
    echo "export CDK_MINTD_LND_PID=\"$CDK_MINTD_LND_PID\"" >> "$ENV_FILE"
    echo "export CDK_REGTEST_PID=\"$CDK_REGTEST_PID\"" >> "$ENV_FILE"
    
    echo
    echo "✅ Mints restarted successfully!"
    echo "  CLN Mint: http://127.0.0.1:8085 (PID: $CDK_MINTD_PID)"
    echo "  LND Mint: http://127.0.0.1:8087 (PID: $CDK_MINTD_LND_PID)"
    echo "==============================="
}

# Main command dispatcher
case "$1" in
    "ln-cln1")
        shift
        cln_command "one" "$@"
        ;;
    "ln-cln2") 
        shift
        cln_command "two" "$@"
        ;;
    "ln-lnd1")
        shift
        lnd_command "one" "$@"
        ;;
    "ln-lnd2")
        shift  
        lnd_command "two" "$@"
        ;;
    "btc")
        shift
        btc_command "$@"
        ;;
    "btc-mine")
        shift
        btc_mine "$@"
        ;;
    "mint-info")
        mint_info
        ;;
    "mint-test")
        mint_test
        ;;
    "restart-mints")
        restart_mints
        ;;
    "show-env")
        show_env
        ;;
    "show-logs")
        show_logs
        ;;
    "show-status")
        show_status
        ;;
    "logs")
        start_mprocs
        ;;
    "help"|"-h"|"--help"|"")
        show_help
        ;;
    *)
        echo "Unknown command: $1"
        echo "Run '$0 help' for available commands"
        exit 1
        ;;
esac
