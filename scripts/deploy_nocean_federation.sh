#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

SSH_HOST="${SSH_HOST:-nocean}"
REMOTE_DIR="${REMOTE_DIR:-cdk-federation}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
PUBLIC_TRANSPORT="${PUBLIC_TRANSPORT:-iroh}"
DEPLOY_MODE="${DEPLOY_MODE:-update}"
MEMBERS="${MEMBERS:-5}"
THRESHOLD="${THRESHOLD:-$(((MEMBERS / 2) + 1))}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-1800}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-3}"
DOCKER_PLATFORM="${DOCKER_PLATFORM:-linux/amd64}"
BUILD_TARGET_DIR="$REPO_ROOT/target/linux-x86_64"
CARGO_REGISTRY_VOLUME="${CARGO_REGISTRY_VOLUME:-cdk-nocean-cargo-registry}"
NETWORK="${NETWORK:-signet}"
FEDERATION_AMOUNTS="${FEDERATION_AMOUNTS:-1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384,32768,65536,131072,262144,524288,1048576,2097152,4194304,8388608,16777216,33554432,67108864,134217728,268435456,536870912,1073741824,2147483648,4294967296,8589934592,17179869184,34359738368,68719476736,137438953472,274877906944,549755813888,1099511627776,2199023255552,4398046511104,8796093022208,17592186044416,35184372088832,70368744177664,140737488355328,281474976710656,562949953421312,1125899906842624,2251799813685248,4503599627370496,9007199254740992,18014398509481984,36028797018963968,72057594037927936,144115188075855872,288230376151711744,576460752303423488,1152921504606846976,2305843009213693952,4611686018427387904,9223372036854775808}"
MINIMUM_CONFIRMATIONS="${MINIMUM_CONFIRMATIONS:-1}"
FEE_POLICY_VERSION="${FEE_POLICY_VERSION:-1}"
APPLICATION_KEY_EPOCH="${APPLICATION_KEY_EPOCH:-1}"
ESPLORA_URL="${ESPLORA_URL:-https://esplora.signet.2nd.dev/}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-2nddev-signet-esplora}"
BARK_SERVER_ADDRESS="${BARK_SERVER_ADDRESS:-https://ark.signet.2nd.dev/}"
BARK_ESPLORA_URL="${BARK_ESPLORA_URL:-$ESPLORA_URL}"
BARK_ARK_SERVER_PUBLIC_KEY="${BARK_ARK_SERVER_PUBLIC_KEY:-03244a5a493e5f0c7b2ff513575715c036100ae219e5765ea688eb542a06f85aed}"
BARK_APPLICATION_KEY_EPOCH="${BARK_APPLICATION_KEY_EPOCH:-1}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS:-20}"
CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${CHECKPOINT_INTERVAL_FINALIZED_ITEMS:-20}"
ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="${ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS:-100}"
ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="${ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK:-8}"
DKG_MAX_TICKS="${DKG_MAX_TICKS:-7200}"
DKG_RETRY_DELAY_MS="${DKG_RETRY_DELAY_MS:-250}"
DKG_CEREMONY_TIMEOUT_SECONDS="${DKG_CEREMONY_TIMEOUT_SECONDS:-1800}"
CONTROL_SCRIPT="$SCRIPT_DIR/nocean_federation_control.sh"
BARK_REPO_ROOT="${BARK_REPO_ROOT:-$REPO_ROOT/../bark-cdk}"

usage() {
    cat <<'EOF'
Build CDK for x86_64 Linux, install it in /root/cdk-federation on Nocean,
start one observable N-member federation, and verify its private and public
health. Setup enrollment, proposal approval, DKG, and activation use the
cdk-mintd federation setup CLI.

Usage:
  DEPLOY_MODE=fresh scripts/deploy_nocean_federation.sh
  scripts/deploy_nocean_federation.sh

Modes:
  update   Preserve finalized state, replace cdk-mintd, and restart it.
           This is the default.
  fresh    Delete member state, run invitation-based setup and concurrent DKG,
           then start federated BDK on-chain plus Bark BOLT11.
  restart  Backward-compatible alias for update.

Transport:
  PUBLIC_TRANSPORT=iroh   Iroh-only setup, DKG, federation, and wallet traffic.
                          HTTP is disabled. This is the default.
  PUBLIC_TRANSPORT=https  HTTPS setup and wallet traffic through the existing
                          cdk-N and cdk-fed-N DNS names.

Important overrides:
  SSH_HOST, REMOTE_DIR, PUBLIC_BASE_DOMAIN
  MEMBERS, THRESHOLD, PUBLIC_TRANSPORT, DEPLOY_MODE
  STATUS_TIMEOUT_SECS, STATUS_POLL_SECS
  DOCKER_PLATFORM, CARGO_REGISTRY_VOLUME, RUST_IMAGE
  NETWORK, FEDERATION_AMOUNTS, MINIMUM_CONFIRMATIONS
  ESPLORA_URL, CHAIN_SOURCE_ID
  BARK_REPO_ROOT, BARK_SERVER_ADDRESS, BARK_ESPLORA_URL
  BARK_ARK_SERVER_PUBLIC_KEY, BARK_APPLICATION_KEY_EPOCH
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

require_positive() {
    [[ "$2" =~ ^[1-9][0-9]*$ ]] || die "$1 must be a positive integer"
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

for command in awk docker scp ssh; do
    require_command "$command"
done
for setting in "$MEMBERS" "$THRESHOLD" "$STATUS_TIMEOUT_SECS" "$STATUS_POLL_SECS" \
    "$MINIMUM_CONFIRMATIONS" "$FEE_POLICY_VERSION" "$APPLICATION_KEY_EPOCH" \
    "$BARK_APPLICATION_KEY_EPOCH" "$PROMETHEUS_PORT_START" \
    "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
    "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" "$ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS" \
    "$ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK" "$DKG_MAX_TICKS" \
    "$DKG_RETRY_DELAY_MS" "$DKG_CEREMONY_TIMEOUT_SECONDS"; do
    require_positive numeric_setting "$setting"
done
((THRESHOLD <= MEMBERS)) || die "THRESHOLD exceeds member count"
((PROMETHEUS_PORT_START + MEMBERS - 1 <= 65535)) \
    || die "Prometheus member ports exceed 65535"
[[ "$DEPLOY_MODE" == "update" || "$DEPLOY_MODE" == "fresh" || "$DEPLOY_MODE" == "restart" ]] \
    || die "DEPLOY_MODE must be update, fresh, or restart"
[[ "$PUBLIC_TRANSPORT" == "iroh" || "$PUBLIC_TRANSPORT" == "https" ]] \
    || die "PUBLIC_TRANSPORT must be iroh or https"
[[ "$NETWORK" == "signet" ]] || die "the Nocean federation requires NETWORK=signet"
[[ -f "$CONTROL_SCRIPT" ]] || die "missing federation control script"
[[ -f "$BARK_REPO_ROOT/bark/Cargo.toml" ]] \
    || die "BARK_REPO_ROOT must point to the Bark frost-wallet checkout"
BARK_REPO_ROOT="$(cd -- "$BARK_REPO_ROOT" && pwd)"
[[ "$REMOTE_DIR" =~ ^[A-Za-z0-9._/-]+$ && "$REMOTE_DIR" != /* ]] \
    || die "REMOTE_DIR must be a safe path relative to the remote home"
[[ "/$REMOTE_DIR/" != *"/../"* ]] || die "REMOTE_DIR must not contain '..'"
[[ "$REMOTE_DIR" == "cdk-federation" ]] \
    || die "the canonical Nocean deployment must use REMOTE_DIR=cdk-federation"
[[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
    || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
[[ "$FEDERATION_AMOUNTS" =~ ^[1-9][0-9]*(,[1-9][0-9]*)*$ ]] \
    || die "FEDERATION_AMOUNTS must be a comma-separated list of positive integers"
[[ "$BARK_ARK_SERVER_PUBLIC_KEY" =~ ^0[23][0-9a-fA-F]{64}$ ]] \
    || die "BARK_ARK_SERVER_PUBLIC_KEY must be a compressed secp256k1 public key"
[[ "$ESPLORA_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
    || die "ESPLORA_URL must be a credential-free HTTPS URL"
[[ "$BARK_SERVER_ADDRESS" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
    || die "BARK_SERVER_ADDRESS must be a credential-free HTTPS URL"
[[ "$BARK_ESPLORA_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
    || die "BARK_ESPLORA_URL must be a credential-free HTTPS URL"

if [[ "$DEPLOY_MODE" == "update" ]]; then
    REMOTE_DEPLOY_MODE=restart
else
    REMOTE_DEPLOY_MODE="$DEPLOY_MODE"
fi

RUST_VERSION="$(awk -F '"' '/^[[:space:]]*channel[[:space:]]*=/ { print $2; exit }' \
    "$REPO_ROOT/rust-toolchain.toml")"
[[ -n "$RUST_VERSION" ]] || die "could not read rust-toolchain.toml"
RUST_IMAGE="${RUST_IMAGE:-rust:${RUST_VERSION}-bookworm}"
MINTD="$BUILD_TARGET_DIR/release/cdk-mintd"

printf 'Building invitation-based cdk-mintd for %s...\n' "$DOCKER_PLATFORM"
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
            --features sqlite,info-page,federation-bdk,federation-bark,iroh,prometheus
        "$CARGO_TARGET_DIR/release/cdk-mintd" --version
        "$CARGO_TARGET_DIR/release/cdk-mintd" federation setup create --help >/dev/null
        "$CARGO_TARGET_DIR/release/cdk-mintd" federation setup join --help >/dev/null
        "$CARGO_TARGET_DIR/release/cdk-mintd" iroh init --help >/dev/null
    '

[[ -x "$MINTD" ]] || die "local Linux build did not produce cdk-mintd"
MINTD_HASH="$(sha256_file "$MINTD")"
MINTD_UPLOAD="cdk-mintd.upload-$MINTD_HASH"

printf 'Preparing the canonical remote deployment directory...\n'
ssh "$SSH_HOST" "mkdir -p '$REMOTE_DIR/bin' '$REMOTE_DIR/scripts'"

printf 'Uploading cdk-mintd and the single-host federation control script...\n'
scp "$MINTD" "$SSH_HOST:$REMOTE_DIR/bin/$MINTD_UPLOAD"
scp "$CONTROL_SCRIPT" "$SSH_HOST:$REMOTE_DIR/scripts/nocean_federation_control.sh.upload"

printf 'Installing and starting the %s %s deployment...\n' \
    "$DEPLOY_MODE" "$PUBLIC_TRANSPORT"
ssh "$SSH_HOST" bash -s -- \
    "$REMOTE_DIR" "$MINTD_HASH" "$MINTD_UPLOAD" "$REMOTE_DEPLOY_MODE" \
    "$PUBLIC_TRANSPORT" "$PUBLIC_BASE_DOMAIN" "$MEMBERS" "$THRESHOLD" \
    "$NETWORK" "$FEDERATION_AMOUNTS" "$MINIMUM_CONFIRMATIONS" \
    "$FEE_POLICY_VERSION" "$APPLICATION_KEY_EPOCH" "$ESPLORA_URL" \
    "$CHAIN_SOURCE_ID" "$BARK_SERVER_ADDRESS" "$BARK_ESPLORA_URL" \
    "$BARK_ARK_SERVER_PUBLIC_KEY" "$BARK_APPLICATION_KEY_EPOCH" \
    "$PROMETHEUS_PORT_START" "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
    "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" "$ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS" \
    "$ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK" "$STATUS_TIMEOUT_SECS" \
    "$STATUS_POLL_SECS" "$DKG_MAX_TICKS" "$DKG_RETRY_DELAY_MS" \
    "$DKG_CEREMONY_TIMEOUT_SECONDS" <<'REMOTE_SCRIPT'
set -Eeuo pipefail

REMOTE_DIR="$1"
MINTD_HASH="$2"
MINTD_UPLOAD="$3"
DEPLOY_MODE="$4"
export PUBLIC_TRANSPORT="$5"
export PUBLIC_BASE_DOMAIN="$6"
export MEMBERS="$7"
export THRESHOLD="$8"
export NETWORK="$9"
export FEDERATION_AMOUNTS="${10}"
export MINIMUM_CONFIRMATIONS="${11}"
export FEE_POLICY_VERSION="${12}"
export APPLICATION_KEY_EPOCH="${13}"
export ESPLORA_URL="${14}"
export CHAIN_SOURCE_ID="${15}"
export BARK_SERVER_ADDRESS="${16}"
export BARK_ESPLORA_URL="${17}"
export BARK_ARK_SERVER_PUBLIC_KEY="${18}"
export BARK_APPLICATION_KEY_EPOCH="${19}"
export PROMETHEUS_PORT_START="${20}"
export CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${21}"
export CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${22}"
export ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="${23}"
export ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="${24}"
export STATUS_TIMEOUT_SECS="${25}"
export STATUS_POLL_SECS="${26}"
export DKG_MAX_TICKS="${27}"
export DKG_RETRY_DELAY_MS="${28}"
export DKG_CEREMONY_TIMEOUT_SECONDS="${29}"

cd "$HOME/$REMOTE_DIR"
for command in awk curl jq pgrep sha256sum timeout tmux; do
    command -v "$command" >/dev/null 2>&1 || {
        printf 'error: required remote command not found: %s\n' "$command" >&2
        exit 1
    }
done
[[ "$(sha256sum "bin/$MINTD_UPLOAD" | awk '{print $1}')" == "$MINTD_HASH" ]]
chmod 755 "bin/$MINTD_UPLOAD" scripts/nocean_federation_control.sh.upload
mv "bin/$MINTD_UPLOAD" bin/cdk-mintd
mv scripts/nocean_federation_control.sh.upload scripts/nocean_federation_control.sh
bin/cdk-mintd --version

bash scripts/nocean_federation_control.sh "$DEPLOY_MODE"

deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
status_output="$(mktemp)"
wallet_output="$(mktemp)"
trap 'rm -f "$status_output" "$wallet_output"' EXIT

tmux_is_healthy() {
    tmux has-session -t cdk-federation 2>/dev/null || return 1
    local panes expected_panes
    panes="$(tmux list-panes -t cdk-federation -F '#{pane_title} #{pane_current_command} #{pane_dead}')"
    expected_panes=$((MEMBERS + 1))
    [[ "$(printf '%s\n' "$panes" | wc -l | tr -d '[:space:]')" == "$expected_panes" ]] \
        || return 1
    [[ "$(awk '$1 ~ /^member-/ && $2 == "cdk-mintd" && $3 == "0" { count++ } END { print count + 0 }' <<<"$panes")" == "$MEMBERS" ]]
}

processes_are_scoped() {
    local count
    count="$(pgrep -af '^/root/cdk-federation/bin/cdk-mintd .*runtime-config\.toml$' \
        | wc -l | tr -d '[:space:]')"
    [[ "$count" == "$MEMBERS" ]] || return 1
    ! pgrep -af 'fed-cdk-tui' >/dev/null
}

https_routes_are_healthy() {
    [[ "$PUBLIC_TRANSPORT" == "https" ]] || return 0
    local member base
    for member in $(seq 1 "$MEMBERS"); do
        base="https://cdk-$member.$PUBLIC_BASE_DOMAIN"
        [[ "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/v1/info")" == "200" ]] \
            || return 1
        [[ "$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 "$base/federation/v1/config")" == "200" ]] \
            || return 1
    done
}

private_state_is_healthy() {
    local public_config="member-1/setup-state/activated/federation-public.json"
    [[ -f "$public_config" ]] || return 1
    jq -e \
        --arg network "$NETWORK" \
        --argjson members "$MEMBERS" \
        '.threshold_params.member_count == $members
         and .frost_wallet != null
         and .frost_wallet.version == 3
         and .frost_wallet.cdk_onchain_application.network == $network
         and ((.frost_wallet.extension_applications // {}) | length) == 2
         and ([.frost_wallet.extension_applications[].configuration] | any(
             .network == $network and has("ark_server_public_key")
         ))
         and ([.frost_wallet.extension_applications[].configuration] | any(
             .network == $network and has("minimum_confirmations")
         ))' \
        "$public_config" >/dev/null || return 1
    timeout 60 bash scripts/nocean_federation_control.sh status \
        >"$status_output" 2>&1 || return 1
    [[ "$(grep -c '^member ' "$status_output")" == "$MEMBERS" ]] || return 1
    timeout 60 bash scripts/nocean_federation_control.sh wallet-status \
        >"$wallet_output" 2>&1
}

attempt=0
printf 'Waiting up to %s seconds for %s setup, concurrent DKG, and runtime readiness...\n' \
    "$STATUS_TIMEOUT_SECS" "$PUBLIC_TRANSPORT"
while ((SECONDS < deadline)); do
    attempt=$((attempt + 1))
    if tmux_is_healthy \
        && processes_are_scoped \
        && https_routes_are_healthy \
        && private_state_is_healthy; then
        printf 'Federation deployment is healthy.\n'
        tmux list-panes -t cdk-federation \
            -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}'
        cat "$status_output"
        cat "$wallet_output"
        if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
            for member in $(seq 1 "$MEMBERS"); do
                printf 'member_%s_url=%s\n' "$member" "$(sed -n '1p' "member-$member/iroh-url")"
            done
        fi
        sha256sum bin/cdk-mintd
        exit 0
    fi
    if ((attempt % 10 == 0)); then
        printf 'Still waiting for %s/%s finalized members...\n' "$MEMBERS" "$MEMBERS"
    fi
    sleep "$STATUS_POLL_SECS"
done

printf 'error: federation did not become healthy before the timeout\n' >&2
tmux list-panes -t cdk-federation \
    -F '#{pane_index} #{pane_title} #{pane_current_command} dead=#{pane_dead}' >&2 || true
for pane in $(tmux list-panes -t cdk-federation -F '#{pane_id}' 2>/dev/null || true); do
    printf '\n--- pane %s ---\n' "$pane" >&2
    tmux capture-pane -p -J -t "$pane" -S -100 >&2 || true
done
cat "$status_output" "$wallet_output" >&2 || true
exit 1
REMOTE_SCRIPT

printf 'Deployment completed successfully.\n'
