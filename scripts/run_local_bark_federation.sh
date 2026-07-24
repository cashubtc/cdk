#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/local-bark-federation}"
BUILD="${BUILD:-1}"
PORT_START="${PORT_START:-22885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-22985}"
SESSION="${SESSION:-cdk-bark-local}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-1200}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-3}"
NETWORK="${NETWORK:-signet}"
ESPLORA_URL="${ESPLORA_URL:-https://esplora.signet.2nd.dev/}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-2nddev-signet-esplora}"
BARK_SERVER_ADDRESS="${BARK_SERVER_ADDRESS:-https://ark.signet.2nd.dev/}"
BARK_ESPLORA_URL="${BARK_ESPLORA_URL:-$ESPLORA_URL}"
BARK_ARK_SERVER_PUBLIC_KEY="${BARK_ARK_SERVER_PUBLIC_KEY:-03244a5a493e5f0c7b2ff513575715c036100ae219e5765ea688eb542a06f85aed}"
BARK_APPLICATION_KEY_EPOCH="${BARK_APPLICATION_KEY_EPOCH:-1}"
ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="${ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS:-200}"
ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="${ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK:-1}"

MINTD="$REPO_ROOT/target/release/cdk-mintd"
CLI="$REPO_ROOT/target/release/cdk-cli"
CONTROL_TEMPLATE="$SCRIPT_DIR/nocean_federation_control.sh"
CONTROL_RENDERER="$SCRIPT_DIR/render_bark_federation_control.py"
CONTROL_SCRIPT="$RUN_DIR/scripts/nocean_federation_control.sh"

usage() {
    cat <<'EOF'
Build and run the five-member Bark Signet federation on local loopback.

Usage:
  scripts/run_local_bark_federation.sh fresh
  scripts/run_local_bark_federation.sh start|stop|restart|status|runtime-metrics

The default state directory is target/local-bark-federation. `fresh` replaces
only that directory's generated config, logs, and member-* state. Set BUILD=0
to reuse existing release binaries after the first run.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

command_name="${1:-}"
case "$command_name" in
    fresh|start|stop|restart|status|runtime-metrics) ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 1
        ;;
esac

[[ "$RUN_DIR" == "$REPO_ROOT"/target/* && "$RUN_DIR" != "$REPO_ROOT/target" ]] \
    || die "RUN_DIR must be a child of the workspace target directory"
[[ "/$RUN_DIR/" != *"/../"* ]] || die "RUN_DIR must not contain '..'"
[[ "$BUILD" == "0" || "$BUILD" == "1" ]] || die "BUILD must be 0 or 1"
[[ "$NETWORK" == "signet" ]] || die "the local Bark federation requires signet"
for value in "$PORT_START" "$PROMETHEUS_PORT_START" "$STATUS_TIMEOUT_SECS" \
    "$STATUS_POLL_SECS" "$BARK_APPLICATION_KEY_EPOCH" \
    "$ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS" \
    "$ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK"; do
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || die "numeric settings must be positive integers"
done
((PORT_START + 4 <= 65535)) || die "member ports exceed 65535"
((PROMETHEUS_PORT_START + 4 <= 65535)) || die "Prometheus ports exceed 65535"
[[ "$SESSION" =~ ^[A-Za-z0-9._-]+$ ]] || die "SESSION contains unsupported characters"
[[ -f "$CONTROL_TEMPLATE" ]] || die "missing federation control template"
[[ -f "$CONTROL_RENDERER" ]] || die "missing Bark federation control renderer"

for required in bash cargo jq openssl python3 tmux; do
    require_command "$required"
done

if [[ "$BUILD" == "1" ]]; then
    cargo build --locked --release -p cdk-mintd \
        --no-default-features \
        --features fakewallet,sqlite,info-page,federation-bark,prometheus
    cargo build --locked --release -p cdk-cli \
        --no-default-features \
        --features federation
fi
[[ -x "$MINTD" ]] || die "missing release cdk-mintd; rerun with BUILD=1"
[[ -x "$CLI" ]] || die "missing release cdk-cli; rerun with BUILD=1"

mkdir -p "$RUN_DIR/bin" "$RUN_DIR/scripts"
ln -sf "$MINTD" "$RUN_DIR/bin/cdk-mintd"
ln -sf "$CLI" "$RUN_DIR/bin/cdk-cli"
python3 "$CONTROL_RENDERER" \
    --source "$CONTROL_TEMPLATE" \
    --output "$CONTROL_SCRIPT" \
    --server-address "$BARK_SERVER_ADDRESS" \
    --esplora-url "$BARK_ESPLORA_URL" \
    --ark-server-public-key "$BARK_ARK_SERVER_PUBLIC_KEY" \
    --application-key-epoch "$BARK_APPLICATION_KEY_EPOCH" \
    --aleph-bft-active-unit-interval-ms "$ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS" \
    --aleph-bft-network-pump-max-messages-per-tick \
        "$ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK"
chmod 755 "$CONTROL_SCRIPT"

export PORT_START PROMETHEUS_PORT_START SESSION STATUS_TIMEOUT_SECS STATUS_POLL_SECS
export NETWORK ESPLORA_URL CHAIN_SOURCE_ID BARK_SERVER_ADDRESS BARK_ESPLORA_URL
export BARK_ARK_SERVER_PUBLIC_KEY BARK_APPLICATION_KEY_EPOCH
export ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS
export ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK
export PUBLIC_URL_MODE=loopback
export LOCAL_MEMBER_HOST=127.0.0.1

cd "$RUN_DIR"

wait_until_ready() {
    local deadline
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if bash "$CONTROL_SCRIPT" status >/dev/null 2>&1; then
            echo "Local five-member Bark federation is ready at http://127.0.0.1:$((PORT_START))"
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "local Bark federation did not become ready before the timeout"
}

case "$command_name" in
    fresh|start|restart)
        bash "$CONTROL_SCRIPT" "$command_name"
        wait_until_ready
        ;;
    stop)
        bash "$CONTROL_SCRIPT" stop
        ;;
    status)
        bash "$CONTROL_SCRIPT" status
        echo "Local five-member Bark federation is ready"
        ;;
    runtime-metrics)
        bash "$CONTROL_SCRIPT" runtime-metrics
        ;;
esac
