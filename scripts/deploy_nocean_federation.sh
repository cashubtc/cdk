#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

SSH_HOST="${SSH_HOST:-nocean}"
REMOTE_DIR="${REMOTE_DIR:-cdk-federation}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
DEPLOY_MODE="${DEPLOY_MODE:-update}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-1200}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-3}"
DOCKER_PLATFORM="${DOCKER_PLATFORM:-linux/amd64}"
BUILD_TARGET_DIR="$REPO_ROOT/target/linux-x86_64"
CARGO_REGISTRY_VOLUME="${CARGO_REGISTRY_VOLUME:-cdk-nocean-cargo-registry}"
NETWORK="${NETWORK:-signet}"
MINIMUM_CONFIRMATIONS="${MINIMUM_CONFIRMATIONS:-1}"
FEE_POLICY_VERSION="${FEE_POLICY_VERSION:-1}"
APPLICATION_KEY_EPOCH="${APPLICATION_KEY_EPOCH:-1}"
ESPLORA_URL="${ESPLORA_URL:-https://esplora.signet.2nd.dev/}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-2nddev-signet-esplora}"
PAYMENT_MODE="${PAYMENT_MODE:-federated-bdk-bark}"
BARK_SERVER_ADDRESS="${BARK_SERVER_ADDRESS:-https://ark.signet.2nd.dev/}"
BARK_ESPLORA_URL="${BARK_ESPLORA_URL:-$ESPLORA_URL}"
BARK_ARK_SERVER_PUBLIC_KEY="${BARK_ARK_SERVER_PUBLIC_KEY:-03244a5a493e5f0c7b2ff513575715c036100ae219e5765ea688eb542a06f85aed}"
BARK_APPLICATION_KEY_EPOCH="${BARK_APPLICATION_KEY_EPOCH:-1}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS:-20}"
CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${CHECKPOINT_INTERVAL_FINALIZED_ITEMS:-20}"
CONTROL_SCRIPT="$SCRIPT_DIR/nocean_federation_control.sh"
CONTROL_RENDERER="$SCRIPT_DIR/render_bark_federation_control.py"
BARK_REPO_ROOT="${BARK_REPO_ROOT:-$REPO_ROOT/../bark-cdk}"

usage() {
    cat <<'EOF'
Build CDK locally for x86_64 Linux, copy the binaries and deployment control
script to nocean with SCP, start the federation, and verify public and private
health.

Usage:
  DEPLOY_MODE=fresh scripts/deploy_nocean_federation.sh
  scripts/deploy_nocean_federation.sh

Modes:
  update   Preserve an existing finalized FROST federation, replace the
           binaries, and restart it. This is the default.
  fresh    Erase all member databases and configs, run a new BLS+FROST DKG,
           and start federated BDK on-chain plus Bark BOLT11.
  restart  Backward-compatible alias for update.

Relevant environment overrides:
  SSH_HOST, REMOTE_DIR, PUBLIC_BASE_DOMAIN
  DEPLOY_MODE=update|fresh|restart
  PAYMENT_MODE=federated-bdk-bark|fakewallet
  STATUS_TIMEOUT_SECS, STATUS_POLL_SECS
  DOCKER_PLATFORM, CARGO_REGISTRY_VOLUME, RUST_IMAGE
  NETWORK, MINIMUM_CONFIRMATIONS, FEE_POLICY_VERSION
  APPLICATION_KEY_EPOCH, ESPLORA_URL, CHAIN_SOURCE_ID
  BARK_REPO_ROOT, BARK_SERVER_ADDRESS, BARK_ESPLORA_URL
  BARK_ARK_SERVER_PUBLIC_KEY, BARK_APPLICATION_KEY_EPOCH
  PROMETHEUS_PORT_START
  CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS, CHECKPOINT_INTERVAL_FINALIZED_ITEMS
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

case "${1:-}" in
    -h|--help) usage; exit 0 ;;
    "") ;;
    *) usage >&2; die "unexpected argument: $1" ;;
esac

for command in docker scp ssh awk python3; do
    require_command "$command"
done
[[ "$DEPLOY_MODE" == "update" || "$DEPLOY_MODE" == "fresh" || "$DEPLOY_MODE" == "restart" ]] \
    || die "DEPLOY_MODE must be update, fresh, or restart"
if [[ "$DEPLOY_MODE" == "update" ]]; then
    REMOTE_DEPLOY_MODE="restart"
else
    REMOTE_DEPLOY_MODE="$DEPLOY_MODE"
fi
[[ "$PAYMENT_MODE" == "federated-bdk-bark" || "$PAYMENT_MODE" == "fakewallet" ]] \
    || die "PAYMENT_MODE must be federated-bdk-bark or fakewallet"
[[ -f "$CONTROL_SCRIPT" ]] || die "missing deployment control script: $CONTROL_SCRIPT"
[[ -f "$CONTROL_RENDERER" ]] || die "missing Bark control renderer: $CONTROL_RENDERER"
[[ -f "$BARK_REPO_ROOT/bark/Cargo.toml" ]] \
    || die "BARK_REPO_ROOT must point to the Bark frost-wallet checkout"
BARK_REPO_ROOT="$(cd -- "$BARK_REPO_ROOT" && pwd)"
[[ "$REMOTE_DIR" =~ ^[A-Za-z0-9._/-]+$ && "$REMOTE_DIR" != /* ]] \
    || die "REMOTE_DIR must be a safe path relative to the remote home"
[[ "/$REMOTE_DIR/" != *"/../"* ]] || die "REMOTE_DIR must not contain '..'"
[[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
    || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
for value in "$STATUS_TIMEOUT_SECS" "$STATUS_POLL_SECS" "$MINIMUM_CONFIRMATIONS" \
    "$FEE_POLICY_VERSION" "$APPLICATION_KEY_EPOCH" "$BARK_APPLICATION_KEY_EPOCH" \
    "$PROMETHEUS_PORT_START" \
    "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
    "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS"; do
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || die "numeric settings must be positive integers"
done
((PROMETHEUS_PORT_START + 5 - 1 <= 65535)) \
    || die "Prometheus member ports exceed 65535"
[[ "$NETWORK" == "signet" ]] || die "this Bark deployment requires NETWORK=signet"
[[ "$BARK_ARK_SERVER_PUBLIC_KEY" =~ ^0[23][0-9a-fA-F]{64}$ ]] \
    || die "BARK_ARK_SERVER_PUBLIC_KEY must be a compressed secp256k1 public key"
[[ "$BARK_SERVER_ADDRESS" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
    || die "BARK_SERVER_ADDRESS must be a credential-free HTTPS URL"
[[ "$BARK_ESPLORA_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
    || die "BARK_ESPLORA_URL must be a credential-free HTTPS URL"

GENERATED_CONTROL_SCRIPT="$(mktemp)"
cleanup() {
    rm -f "$GENERATED_CONTROL_SCRIPT"
}
trap cleanup EXIT

python3 "$CONTROL_RENDERER" \
    --source "$CONTROL_SCRIPT" \
    --output "$GENERATED_CONTROL_SCRIPT" \
    --server-address "$BARK_SERVER_ADDRESS" \
    --esplora-url "$BARK_ESPLORA_URL" \
    --ark-server-public-key "$BARK_ARK_SERVER_PUBLIC_KEY" \
    --application-key-epoch "$BARK_APPLICATION_KEY_EPOCH"
chmod 755 "$GENERATED_CONTROL_SCRIPT"

RUST_VERSION="$(awk -F '"' '/^[[:space:]]*channel[[:space:]]*=/ { print $2; exit }' \
    "$REPO_ROOT/rust-toolchain.toml")"
[[ -n "$RUST_VERSION" ]] || die "could not read rust-toolchain.toml"
RUST_IMAGE="${RUST_IMAGE:-rust:${RUST_VERSION}-bookworm}"
MINTD="$BUILD_TARGET_DIR/release/cdk-mintd"
CLI="$BUILD_TARGET_DIR/release/cdk-cli"

echo "Building cdk-mintd and cdk-cli locally for $DOCKER_PLATFORM..."
docker run --rm \
    --platform "$DOCKER_PLATFORM" \
    --env "RUSTUP_TOOLCHAIN=$RUST_VERSION" \
    --mount "type=bind,src=$REPO_ROOT,dst=/work" \
    --mount "type=bind,src=$BARK_REPO_ROOT,dst=/bark-cdk,readonly" \
    --mount "type=volume,src=$CARGO_REGISTRY_VOLUME,dst=/usr/local/cargo/registry" \
    --workdir /work \
    "$RUST_IMAGE" \
    bash -c '
        set -euo pipefail
        apt-get update >/dev/null
        apt-get install -y --no-install-recommends protobuf-compiler cmake clang >/dev/null
        export CARGO_TARGET_DIR=/work/target/linux-x86_64
        cargo build --locked --release -p cdk-mintd \
            --no-default-features \
            --features fakewallet,sqlite,info-page,federation-bark,prometheus
        cargo build --locked --release -p cdk-cli \
            --no-default-features \
            --features federation
        "$CARGO_TARGET_DIR/release/cdk-mintd" --version
        "$CARGO_TARGET_DIR/release/cdk-mintd" federation wallet-status --help >/dev/null
        "$CARGO_TARGET_DIR/release/cdk-cli" --version
        "$CARGO_TARGET_DIR/release/cdk-cli" federation add --help >/dev/null
    '

[[ -x "$MINTD" && -x "$CLI" ]] || die "local Linux build did not produce both binaries"
MINTD_HASH="$(sha256_file "$MINTD")"
CLI_HASH="$(sha256_file "$CLI")"
MINTD_UPLOAD="cdk-mintd.upload-$MINTD_HASH"
CLI_UPLOAD="cdk-cli.upload-$CLI_HASH"

echo "Uploading locally built binaries and FROST deployment control with SCP..."
scp "$MINTD" "$SSH_HOST:$REMOTE_DIR/bin/$MINTD_UPLOAD"
scp "$CLI" "$SSH_HOST:$REMOTE_DIR/bin/$CLI_UPLOAD"
scp "$GENERATED_CONTROL_SCRIPT" "$SSH_HOST:$REMOTE_DIR/scripts/nocean_federation_control.sh.upload"

echo "Installing and starting the $DEPLOY_MODE deployment..."
ssh "$SSH_HOST" bash -s -- \
    "$REMOTE_DIR" "$MINTD_HASH" "$CLI_HASH" "$MINTD_UPLOAD" "$CLI_UPLOAD" \
    "$REMOTE_DEPLOY_MODE" "$PUBLIC_BASE_DOMAIN" "$NETWORK" "$MINIMUM_CONFIRMATIONS" \
    "$FEE_POLICY_VERSION" "$APPLICATION_KEY_EPOCH" "$ESPLORA_URL" "$CHAIN_SOURCE_ID" \
    "$STATUS_TIMEOUT_SECS" "$STATUS_POLL_SECS" \
    "$PROMETHEUS_PORT_START" \
    "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
    "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" "$PAYMENT_MODE" \
    "$BARK_SERVER_ADDRESS" "$BARK_ESPLORA_URL" "$BARK_ARK_SERVER_PUBLIC_KEY" \
    "$BARK_APPLICATION_KEY_EPOCH" <<'REMOTE_SCRIPT'
set -Eeuo pipefail

REMOTE_DIR="$1"
MINTD_HASH="$2"
CLI_HASH="$3"
MINTD_UPLOAD="$4"
CLI_UPLOAD="$5"
DEPLOY_MODE="$6"
export PUBLIC_BASE_DOMAIN="$7"
export NETWORK="$8"
export MINIMUM_CONFIRMATIONS="$9"
export FEE_POLICY_VERSION="${10}"
export APPLICATION_KEY_EPOCH="${11}"
export ESPLORA_URL="${12}"
export CHAIN_SOURCE_ID="${13}"
export STATUS_TIMEOUT_SECS="${14}"
export STATUS_POLL_SECS="${15}"
export PROMETHEUS_PORT_START="${16}"
export CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${17}"
export CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${18}"
export PAYMENT_MODE="${19}"
export BARK_SERVER_ADDRESS="${20}"
export BARK_ESPLORA_URL="${21}"
export BARK_ARK_SERVER_PUBLIC_KEY="${22}"
export BARK_APPLICATION_KEY_EPOCH="${23}"

cd "$HOME/$REMOTE_DIR"
for command in sha256sum tmux curl timeout awk find jq; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "error: required remote command not found: $command" >&2
        exit 1
    }
done

[[ "$(sha256sum "bin/$MINTD_UPLOAD" | awk '{print $1}')" == "$MINTD_HASH" ]]
[[ "$(sha256sum "bin/$CLI_UPLOAD" | awk '{print $1}')" == "$CLI_HASH" ]]
chmod 755 "bin/$MINTD_UPLOAD" "bin/$CLI_UPLOAD" scripts/nocean_federation_control.sh.upload
mv "bin/$MINTD_UPLOAD" bin/cdk-mintd
mv "bin/$CLI_UPLOAD" bin/cdk-cli
mv scripts/nocean_federation_control.sh.upload scripts/nocean_federation_control.sh
bin/cdk-mintd --version
bin/cdk-cli --version

bash scripts/nocean_federation_control.sh "$DEPLOY_MODE"

deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
status_output="$(mktemp)"
wallet_output="$(mktemp)"
trap 'rm -f "$status_output" "$wallet_output"' EXIT

tmux_is_healthy() {
    tmux has-session -t cdk-federation 2>/dev/null || return 1
    local panes
    panes="$(tmux list-panes -t cdk-federation -F '#{pane_title} #{pane_current_command} #{pane_dead}')"
    [[ "$(printf '%s\n' "$panes" | wc -l | tr -d '[:space:]')" == "6" ]] || return 1
    [[ "$(awk '$1 ~ /^member-/ && $2 == "cdk-mintd" && $3 == "0" { count++ } END { print count + 0 }' <<<"$panes")" == "5" ]]
}

public_routes_are_healthy() {
    local member base
    for member in 1 2 3 4 5; do
        base="https://cdk-$member.$PUBLIC_BASE_DOMAIN"
        [[ "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/v1/info")" == "200" ]] || return 1
        [[ "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/federation/v1/config")" == "200" ]] || return 1
        [[ "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/v1/config")" == "404" ]] || return 1
    done
}

private_state_is_healthy() {
    local public_config="member-1/dkg-final/federation-public.json"
    local private_config="member-1/dkg-final/federation-private.json"
    [[ -f "$public_config" && -f "$private_config" ]] || return 1
    if [[ "$PAYMENT_MODE" == "fakewallet" ]]; then
        jq -e '.frost_wallet == null' "$public_config" >/dev/null || return 1
    else
        jq -e \
            --arg network "$NETWORK" \
            --argjson confirmations "$MINIMUM_CONFIRMATIONS" \
            '.frost_wallet != null
             and .frost_wallet.cdk_onchain_application.network == $network
             and .frost_wallet.cdk_onchain_application.confirmation_policy.minimum_confirmations == $confirmations
             and .frost_wallet.bark_application != null
             and .frost_wallet.bark_application.network == $network' \
            "$public_config" >/dev/null || return 1
    fi
    timeout 45 bin/cdk-mintd federation status \
        --public-config "$public_config" \
        --private-config "$private_config" \
        --require-ready \
        --require-converged \
        --max-operation-lag 0 \
        --require-no-digest-alerts \
        --require-no-payment-observation-alerts \
        --require-no-invalid-operation-alerts >"$status_output" 2>&1 || return 1
    [[ "$(grep -c '^member ' "$status_output")" == "5" ]] || return 1
    if [[ "$PAYMENT_MODE" == "fakewallet" ]]; then
        local methods
        methods="$(curl -fsS --max-time 10 "https://cdk-1.$PUBLIC_BASE_DOMAIN/v1/info" \
            | jq -r '
                def benchmark_methods($nut):
                    [.nuts[$nut].methods[].method
                     | select(. == "bolt11" or . == "bolt12" or . == "onchain")]
                    | unique | sort | join(",");
                benchmark_methods("4") + "|" + benchmark_methods("5")
            ')" \
            || return 1
        [[ "$methods" == "bolt11,bolt12,onchain|bolt11,bolt12,onchain" ]] || return 1
        printf 'payment_methods=%s\n' "$methods" >"$wallet_output"
        return 0
    fi
    local methods
    methods="$(curl -fsS --max-time 10 "https://cdk-1.$PUBLIC_BASE_DOMAIN/v1/info" \
        | jq -r '
            def payment_methods($nut):
                [.nuts[$nut].methods[].method
                 | select(. == "bolt11" or . == "onchain")]
                | unique | sort | join(",");
            payment_methods("4") + "|" + payment_methods("5")
        ')" \
        || return 1
    [[ "$methods" == "bolt11,onchain|bolt11,onchain" ]] || return 1
    timeout 45 bin/cdk-mintd federation wallet-status \
        --public-config "$public_config" \
        --private-config "$private_config" \
        --require-ready >"$wallet_output" 2>&1
}

attempt=0
echo "Waiting up to $STATUS_TIMEOUT_SECS seconds for dual DKG and $PAYMENT_MODE readiness..."
while ((SECONDS < deadline)); do
    attempt=$((attempt + 1))
    if tmux_is_healthy && public_routes_are_healthy && private_state_is_healthy; then
        echo "Federation deployment is healthy."
        tmux list-panes -t cdk-federation \
            -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}'
        cat "$status_output"
        cat "$wallet_output"
        sha256sum bin/cdk-mintd bin/cdk-cli
        exit 0
    fi
    if ((attempt % 10 == 0)); then
        echo "Still waiting for finalized config and five ready $PAYMENT_MODE members..."
    fi
    sleep "$STATUS_POLL_SECS"
done

echo "error: federation did not become healthy before the timeout" >&2
tmux list-panes -t cdk-federation \
    -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}' >&2 || true
tmux capture-pane -p -J -t cdk-federation:0 -S -120 >&2 || true
cat "$status_output" "$wallet_output" >&2 || true
exit 1
REMOTE_SCRIPT

echo "Deployment completed successfully."
