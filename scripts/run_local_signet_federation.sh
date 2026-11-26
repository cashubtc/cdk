#!/usr/bin/env bash
# Run a local five-member federation on Second's public signet: Lightning
# through the federated Bark (Ark) backend and on-chain through the joint
# FROST/BDK wallet. Every member runs on loopback; the setup is the real
# dealer-free ceremony (create, join, propose, approve, BLS and FROST DKG,
# activate).
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/local-signet-federation}"
BUILD="${BUILD:-1}"
MEMBERS="${MEMBERS:-5}"
THRESHOLD="${THRESHOLD:-3}"
PORT_START="${PORT_START:-23885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-23985}"
TIMEOUT_SECS="${TIMEOUT_SECS:-900}"
POLL_SECS="${POLL_SECS:-2}"
AMOUNTS="${AMOUNTS:-1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384,32768,65536}"
MINIMUM_CONFIRMATIONS="${MINIMUM_CONFIRMATIONS:-1}"
ESPLORA_URL="${ESPLORA_URL:-https://esplora.signet.2nd.dev/}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-2nddev-signet-esplora}"
BARK_SERVER_ADDRESS="${BARK_SERVER_ADDRESS:-https://ark.signet.2nd.dev/}"
BARK_ARK_SERVER_PUBLIC_KEY="${BARK_ARK_SERVER_PUBLIC_KEY:-03244a5a493e5f0c7b2ff513575715c036100ae219e5765ea688eb542a06f85aed}"
RUST_LOG="${RUST_LOG:-info}"

MINTD="$REPO_ROOT/target/release/cdk-mintd"
CLI="$REPO_ROOT/target/release/cdk-cli"
SETUP_PIDS=()

usage() {
    cat <<'EOF'
Run a local five-member federation on Second's signet (Bark Lightning and
joint FROST/BDK on-chain custody).

Usage:
  scripts/run_local_signet_federation.sh fresh    # wipe, run the setup ceremony, start
  scripts/run_local_signet_federation.sh start|stop|restart
  scripts/run_local_signet_federation.sh status|wallet-status|urls

State lives in target/local-signet-federation (RUN_DIR). BUILD=0 reuses the
release binaries. Logs are in member-N/work/mintd.log.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

member_dir() { printf '%s/member-%s' "$RUN_DIR" "$1"; }
work_dir() { printf '%s/work' "$(member_dir "$1")"; }
setup_config() { printf '%s/setup.json' "$(member_dir "$1")"; }
# The ceremony writes its DKG records into the member's own mint database,
# which is where the runtime looks for them.
setup_database() { printf '%s/cdk-mintd.sqlite' "$(work_dir "$1")"; }
setup_state_dir() { printf '%s/setup-state' "$(member_dir "$1")"; }
activated_public() { printf '%s/activated/federation-public.json' "$(setup_state_dir "$1")"; }
activated_private() { printf '%s/activated/federation-private.json' "$(setup_state_dir "$1")"; }
member_port() { printf '%s' "$((PORT_START + $1 - 1))"; }
# The mint and federation API share one listener; setup requires the two URLs
# to differ, so wallets use localhost and peers use 127.0.0.1.
member_url() { printf 'http://localhost:%s' "$(member_port "$1")"; }
federation_url() { printf 'http://127.0.0.1:%s' "$(member_port "$1")"; }
pid_file() { printf '%s/mintd.pid' "$(work_dir "$1")"; }

mintd_member() {
    local member="$1"
    shift
    "$MINTD" --work-dir "$(work_dir "$member")" "$@"
}

wait_until() {
    local description="$1"
    shift
    local deadline=$((SECONDS + TIMEOUT_SECS))
    until "$@"; do
        ((SECONDS < deadline)) || die "timed out waiting for $description"
        sleep "$POLL_SECS"
    done
}

http_listening() {
    local code
    code="$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 2 "$1" || true)"
    [[ "$code" != "000" ]]
}

stop_setup_services() {
    local pid
    for pid in "${SETUP_PIDS[@]:-}"; do
        [[ -n "$pid" ]] || continue
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    SETUP_PIDS=()
}

start_setup_service() {
    local member="$1" log="$2"
    "$MINTD" --work-dir "$(work_dir "$member")" federation setup serve \
        --config "$(setup_config "$member")" \
        --listen "127.0.0.1:$(member_port "$member")" \
        >"$(member_dir "$member")/$log" 2>&1 &
    SETUP_PIDS+=("$!")
}

write_bark_policy() {
    (umask 077 && cat >"$RUN_DIR/bark-policy.json") <<'EOF'
{
  "version": 1,
  "receive_base_fee_sat": 0,
  "receive_fee_ppm": 0,
  "maximum_outgoing_base_fee_sat": 20,
  "maximum_outgoing_fee_ppm": 5000,
  "minimum_incoming_cltv_delta": 18,
  "minimum_outgoing_cltv_delta": 18,
  "refresh_before_expiry_blocks": 72,
  "emergency_exit_before_expiry_blocks": 18,
  "receive_secret_retention_days": 30,
  "receive_anti_dos": "no_proof"
}
EOF
}

generate_member() {
    local member="$1" work secrets
    work="$(work_dir "$member")"
    secrets="$work/secrets"
    mkdir -p "$work/frost-protocol" "$secrets"
    chmod 700 "$secrets"
    (
        umask 077
        openssl rand -hex 32 | tr -d '\n' >"$secrets/mint-seed"
        openssl rand -hex 64 | tr -d '\n' >"$secrets/bark-seed"
        openssl rand -hex 32 | tr -d '\n' >"$secrets/bark-receive-key"
    )
    (umask 077 && cat >"$(member_dir "$member")/runtime.toml") <<EOF
[info]
url = "$(member_url "$member")"
listen_host = "127.0.0.1"
listen_port = $(member_port "$member")
seed = "file:$secrets/mint-seed"
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "Local signet federation member $member"
description = "Five-member CDK federation on Second signet (Bark Lightning, FROST on-chain)"

[prometheus]
enabled = true
address = "127.0.0.1"
port = $((PROMETHEUS_PORT_START + member - 1))

[database]
engine = "sqlite"

[[payment_backend]]
backend = "federated-bark"
unit = "sat"
min_mint = 1
max_mint = 1000000
min_melt = 1
max_melt = 1000000

[onchain]
onchain_backend = "federated-bdk"
min_mint = 1000
max_mint = 10000000
min_melt = 1000
max_melt = 10000000

[federation]
enabled = true
public_config_path = "$(activated_public "$member")"
private_config_path = "$(activated_private "$member")"

[federation.peer_sync]
operation_submission_timeout_secs = 180

[federation.onchain_wallet]
enabled = true
chain_source_type = "esplora"
chain_source_scope = "shared"
chain_source_id = "$CHAIN_SOURCE_ID"
esplora_url = "$ESPLORA_URL"
sync_interval_secs = 15

[federation.bark]
enabled = true
server_address = "$BARK_SERVER_ADDRESS"
esplora_url = "$ESPLORA_URL"
data_directory = "$work/federation-bark"
receive_secret_database_path = "$work/federation-bark/receive-secrets.redb"
frost_protocol_storage_root = "$work/frost-protocol"
operational_seed_hex = "file:$secrets/bark-seed"
receive_secret_key_hex = "file:$secrets/bark-receive-key"
create_if_missing = true
automatic_refresh = true
automatic_emergency_exit = false
EOF
}

run_ceremony() {
    local member proposal_hash status_file
    printf '== creating the setup (member 1 coordinates)\n'
    mintd_member 1 federation setup create \
        --config "$(setup_config 1)" \
        --state-dir "$(setup_state_dir 1)" \
        --database "$(setup_database 1)" \
        --members "$MEMBERS" \
        --threshold "$THRESHOLD" \
        --network signet \
        --public-mint-url "$(member_url 1)" \
        --federation-api-url "$(federation_url 1)" \
        --coordinator-url "$(federation_url 1)" \
        --amounts "$AMOUNTS" \
        --frost-minimum-confirmations "$MINIMUM_CONFIRMATIONS" \
        --frost-bark-ark-server-public-key "$BARK_ARK_SERVER_PUBLIC_KEY" \
        --frost-bark-application-key-epoch 1 \
        --frost-bark-policy-config "$RUN_DIR/bark-policy.json" \
        --allow-insecure-localhost \
        >"$(member_dir 1)/create.log"

    start_setup_service 1 coordinator.log
    wait_until "the setup coordinator" \
        http_listening "$(federation_url 1)/federation/setup/v1/status"

    for member in $(seq 2 "$MEMBERS"); do
        printf '== member %s joins\n' "$member"
        mintd_member "$member" federation setup join \
            --invitation-file "$(setup_state_dir 1)/invitations/member-$member.invite" \
            --config "$(setup_config "$member")" \
            --state-dir "$(setup_state_dir "$member")" \
            --database "$(setup_database "$member")" \
            --public-mint-url "$(member_url "$member")" \
            --federation-api-url "$(federation_url "$member")" \
            --allow-insecure-localhost \
            >"$(member_dir "$member")/join.log"
    done

    mintd_member 1 federation setup propose --config "$(setup_config 1)" \
        >"$(member_dir 1)/propose.log"
    proposal_hash="$(sed -n 's/^proposal_hash: //p' "$(member_dir 1)/propose.log")"
    [[ "$proposal_hash" =~ ^[0-9a-f]{64}$ ]] || die "setup proposal did not print its hash"
    printf '== proposal %s\n' "$proposal_hash"

    for member in $(seq 1 "$MEMBERS"); do
        mintd_member "$member" federation setup review \
            --config "$(setup_config "$member")" --json >"$(member_dir "$member")/review.json"
        [[ "$(jq -r '.proposal_hash' "$(member_dir "$member")/review.json")" == "$proposal_hash" ]] \
            || die "member $member reviewed a different proposal"
        mintd_member "$member" federation setup approve \
            --config "$(setup_config "$member")" \
            --proposal-hash "$proposal_hash" >"$(member_dir "$member")/approve.log"
    done

    for member in $(seq 1 "$MEMBERS"); do
        status_file="$(member_dir "$member")/certified-status.json"
        mintd_member "$member" federation setup status \
            --config "$(setup_config "$member")" --json >"$status_file"
        [[ "$(jq -r '.participant_phase' "$status_file")" == "certified" ]] \
            || die "member $member did not persist the certified manifest"
    done
    stop_setup_services
    printf '== every member certified the manifest; running BLS and FROST DKG\n'

    for member in $(seq 1 "$MEMBERS"); do
        start_setup_service "$member" dkg.log
    done
    for member in $(seq 1 "$MEMBERS"); do
        wait_until "member $member DKG" dkg_finalized "$member"
        printf '== member %s DKG finalized\n' "$member"
    done
    stop_setup_services

    for member in $(seq 1 "$MEMBERS"); do
        mintd_member "$member" federation setup activate --config "$(setup_config "$member")" \
            >"$(member_dir "$member")/activation.log" 2>&1
    done
    for member in $(seq 2 "$MEMBERS"); do
        cmp -s "$(activated_public 1)" "$(activated_public "$member")" \
            || die "member $member activated a different public config"
    done
    printf '== every member activated the same public config\n'
}

dkg_finalized() {
    local member="$1" output
    output="$(member_dir "$member")/dkg-status.json"
    mintd_member "$member" federation setup status \
        --config "$(setup_config "$member")" --json >"$output" 2>/dev/null \
        && [[ "$(jq -r '.dkg.finalized_ready // false' "$output")" == "true" ]]
}

member_running() {
    [[ -f "$(pid_file "$1")" ]] && kill -0 "$(cat "$(pid_file "$1")")" 2>/dev/null
}

start_members() {
    local member
    [[ -f "$(activated_public 1)" ]] || die "no activated federation; run fresh first"
    for member in $(seq 1 "$MEMBERS"); do
        if member_running "$member"; then
            printf 'member %s already running\n' "$member"
            continue
        fi
        RUST_LOG="$RUST_LOG" nohup "$MINTD" --work-dir "$(work_dir "$member")" \
            >>"$(work_dir "$member")/mintd.log" 2>&1 </dev/null &
        printf '%s' "$!" >"$(pid_file "$member")"
        printf 'started member %s (pid %s)\n' "$member" "$!"
    done
    for member in $(seq 1 "$MEMBERS"); do
        wait_until "member $member to serve /v1/info" member_serving "$member"
    done
    printf '== all members serve /v1/info; waiting for a ready, converged federation\n'
    wait_until "a ready federation" status >/dev/null 2>&1
    status
}

member_serving() {
    member_running "$1" || die "member $1 exited; see $(work_dir "$1")/mintd.log"
    curl -fsS --max-time 2 "$(member_url "$1")/v1/info" >/dev/null 2>&1
}

stop_members() {
    local member pid
    for member in $(seq 1 "$MEMBERS"); do
        [[ -f "$(pid_file "$member")" ]] || continue
        pid="$(cat "$(pid_file "$member")")"
        kill "$pid" 2>/dev/null || true
        rm -f "$(pid_file "$member")"
        printf 'stopped member %s\n' "$member"
    done
}

status() {
    mintd_member 1 federation status \
        --public-config "$(activated_public 1)" \
        --private-config "$(activated_private 1)" \
        --require-ready \
        --require-converged \
        --require-no-digest-alerts \
        --require-no-invalid-operation-alerts
}

wallet_status() {
    mintd_member 1 federation wallet-status \
        --public-config "$(activated_public 1)" \
        --private-config "$(activated_private 1)"
}

urls() {
    local member
    for member in $(seq 1 "$MEMBERS"); do
        printf 'member %s: %s\n' "$member" "$(member_url "$member")"
    done
}

fresh() {
    local member
    [[ "$RUN_DIR" == "$REPO_ROOT"/target/* ]] || die "RUN_DIR must be under target/"
    stop_members
    rm -rf -- "$RUN_DIR"
    mkdir -p "$RUN_DIR"
    write_bark_policy
    for member in $(seq 1 "$MEMBERS"); do
        generate_member "$member"
    done
    for member in $(seq 1 "$MEMBERS"); do
        mintd_member "$member" config init --new-mint \
            --file "$(member_dir "$member")/runtime.toml" >/dev/null
    done
    run_ceremony
    start_members
    urls
}

command_name="${1:-}"
case "$command_name" in
    fresh | start | stop | restart | status | wallet-status | urls) ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 1
        ;;
esac

for required in curl jq openssl cmp; do
    command -v "$required" >/dev/null 2>&1 || die "required command not found: $required"
done
if [[ "$BUILD" == "1" && "$command_name" =~ ^(fresh|start|restart)$ ]]; then
    (cd "$REPO_ROOT" && cargo build --locked --release -p cdk-mintd --no-default-features \
        --features sqlite,info-page,federation-bdk,federation-bark,prometheus)
    (cd "$REPO_ROOT" && cargo build --locked --release -p cdk-cli --no-default-features \
        --features federation)
fi
[[ -x "$MINTD" ]] || die "missing $MINTD; run with BUILD=1"
trap stop_setup_services EXIT

case "$command_name" in
    fresh) fresh ;;
    start) start_members && urls ;;
    stop) stop_members ;;
    restart)
        stop_members
        sleep 2
        start_members
        ;;
    status) status ;;
    wallet-status) wallet_status ;;
    urls) urls ;;
esac
