#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

MEMBERS="${MEMBERS:-5}"
THRESHOLD="${THRESHOLD:-$(((MEMBERS / 2) + 1))}"
PUBLIC_TRANSPORT="${PUBLIC_TRANSPORT:-iroh}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
PORT_START="${PORT_START:-21885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
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
CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS:-20}"
CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${CHECKPOINT_INTERVAL_FINALIZED_ITEMS:-20}"
ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS="${ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS:-100}"
ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK="${ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK:-8}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-1800}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-2}"
DKG_MAX_TICKS="${DKG_MAX_TICKS:-7200}"
DKG_RETRY_DELAY_MS="${DKG_RETRY_DELAY_MS:-250}"
DKG_CEREMONY_TIMEOUT_SECONDS="${DKG_CEREMONY_TIMEOUT_SECONDS:-1800}"
SESSION="${SESSION:-cdk-federation}"

MINTD="$PWD/bin/cdk-mintd"
COORDINATION_DIR="$PWD/setup-coordination"
FINAL_PUBLIC="$PWD/member-1/setup-state/activated/federation-public.json"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_positive() {
    [[ "$2" =~ ^[1-9][0-9]*$ ]] || die "$1 must be a positive integer"
}

member_dir() {
    printf '%s/member-%s' "$PWD" "$1"
}

work_dir() {
    printf '%s/work' "$(member_dir "$1")"
}

runtime_config() {
    printf '%s/runtime-config.toml' "$(member_dir "$1")"
}

setup_config() {
    printf '%s/setup.json' "$(member_dir "$1")"
}

setup_state_dir() {
    printf '%s/setup-state' "$(member_dir "$1")"
}

setup_database() {
    printf '%s/cdk-mintd.sqlite' "$(work_dir "$1")"
}

incoming_invitation() {
    printf '%s/incoming.invite' "$(member_dir "$1")"
}

activated_public_config() {
    printf '%s/activated/federation-public.json' "$(setup_state_dir "$1")"
}

activated_private_config() {
    printf '%s/activated/federation-private.json' "$(setup_state_dir "$1")"
}

iroh_url_file() {
    printf '%s/iroh-url' "$(member_dir "$1")"
}

member_url() {
    local member="$1"
    if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
        [[ -s "$(iroh_url_file "$member")" ]] || die "member $member Iroh URL is missing"
        sed -n '1p' "$(iroh_url_file "$member")"
        return
    fi
    printf 'https://cdk-%s.%s' "$member" "$PUBLIC_BASE_DOMAIN"
}

federation_url() {
    local member="$1"
    if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
        member_url "$member"
        return
    fi
    printf 'https://cdk-fed-%s.%s' "$member" "$PUBLIC_BASE_DOMAIN"
}

marker() {
    printf '%s/%s' "$COORDINATION_DIR" "$1"
}

touch_marker() {
    touch "$(marker "$1")"
}

wait_for_marker() {
    local name="$1" deadline
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        [[ -f "$(marker "$name")" ]] && return 0
        sleep 0.2
    done
    die "timed out waiting for setup phase marker: $name"
}

wait_for_all_member_markers() {
    local suffix="$1" deadline member ready
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        ready=0
        for member in $(seq 1 "$MEMBERS"); do
            [[ -f "$(marker "member-$member-$suffix")" ]] && ready=$((ready + 1))
        done
        printf '%s: %s/%s members ready\n' "$suffix" "$ready" "$MEMBERS"
        ((ready == MEMBERS)) && return 0
        sleep "$STATUS_POLL_SECS"
    done
    die "timed out waiting for all members to reach $suffix"
}

validate_environment() {
    local command value
    for command in awk cmp curl find install jq openssl ps sed seq tmux; do
        command -v "$command" >/dev/null 2>&1 \
            || die "required command not found: $command"
    done
    [[ -x "$MINTD" ]] || die "cdk-mintd is not installed"
    for value in "$MEMBERS" "$THRESHOLD" "$PORT_START" "$PROMETHEUS_PORT_START" \
        "$MINIMUM_CONFIRMATIONS" "$FEE_POLICY_VERSION" "$APPLICATION_KEY_EPOCH" \
        "$BARK_APPLICATION_KEY_EPOCH" "$STATUS_TIMEOUT_SECS" "$STATUS_POLL_SECS" \
        "$DKG_MAX_TICKS" "$DKG_RETRY_DELAY_MS" "$DKG_CEREMONY_TIMEOUT_SECONDS" \
        "$ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS" \
        "$ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK"; do
        require_positive numeric_setting "$value"
    done
    ((THRESHOLD <= MEMBERS)) || die "THRESHOLD exceeds member count"
    ((PORT_START + MEMBERS - 1 <= 65535)) || die "member ports exceed 65535"
    ((PROMETHEUS_PORT_START + MEMBERS - 1 <= 65535)) \
        || die "Prometheus member ports exceed 65535"
    [[ "$PUBLIC_TRANSPORT" == "iroh" || "$PUBLIC_TRANSPORT" == "https" ]] \
        || die "PUBLIC_TRANSPORT must be iroh or https"
    [[ "$NETWORK" == "signet" ]] || die "the Nocean federation requires NETWORK=signet"
    [[ "$PUBLIC_BASE_DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] \
        || die "PUBLIC_BASE_DOMAIN contains unsupported characters"
    [[ "$FEDERATION_AMOUNTS" =~ ^[1-9][0-9]*(,[1-9][0-9]*)*$ ]] \
        || die "FEDERATION_AMOUNTS must be a comma-separated list of positive integers"
    [[ "$ESPLORA_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
        || die "ESPLORA_URL must be a credential-free HTTPS URL"
    [[ "$BARK_SERVER_ADDRESS" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
        || die "BARK_SERVER_ADDRESS must be a credential-free HTTPS URL"
    [[ "$BARK_ESPLORA_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
        || die "BARK_ESPLORA_URL must be a credential-free HTTPS URL"
    [[ "$BARK_ARK_SERVER_PUBLIC_KEY" =~ ^0[23][0-9a-fA-F]{64}$ ]] \
        || die "BARK_ARK_SERVER_PUBLIC_KEY must be a compressed secp256k1 public key"
}

write_bark_policy() {
    local output="$PWD/bark-policy.json"
    cat >"$output" <<'EOF'
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
    chmod 600 "$output"
}

initialize_iroh_identity() {
    local member="$1" url
    mkdir -p "$(work_dir "$member")"
    url="$("$MINTD" --work-dir "$(work_dir "$member")" iroh init)"
    [[ "$url" =~ ^iroh://[a-z0-9]+$ ]] || die "member $member returned an invalid Iroh URL"
    printf '%s\n' "$url" >"$(iroh_url_file "$member")"
    chmod 600 "$(iroh_url_file "$member")"
}

generate_member() {
    local member="$1" dir listen_port prometheus_port public_url iroh_enabled http_enabled
    local seed bark_operational_seed bark_receive_secret
    member="$1"
    dir="$(member_dir "$member")"
    listen_port=$((PORT_START + member - 1))
    prometheus_port=$((PROMETHEUS_PORT_START + member - 1))
    mkdir -p "$dir" "$(work_dir "$member")/frost-protocol"
    if [[ "$PUBLIC_TRANSPORT" == "iroh" ]]; then
        initialize_iroh_identity "$member"
        iroh_enabled=true
        http_enabled=false
    else
        iroh_enabled=false
        http_enabled=true
    fi
    public_url="$(member_url "$member")"
    seed="$(openssl rand -hex 32)"
    bark_operational_seed="$(openssl rand -hex 64)"
    bark_receive_secret="$(openssl rand -hex 32)"
    local secrets_dir="$(work_dir "$member")/secrets"
    mkdir -p "$secrets_dir"
    chmod 700 "$secrets_dir"
    (umask 077; printf '%s' "$seed" >"$secrets_dir/mint-seed"; printf '%s' "$bark_operational_seed" >"$secrets_dir/bark-seed"; printf '%s' "$bark_receive_secret" >"$secrets_dir/bark-receive-key")
    cat >"$(runtime_config "$member")" <<EOF
[info]
url = "$public_url"
http_enabled = $http_enabled
listen_host = "127.0.0.1"
listen_port = $listen_port
seed = "file:$secrets_dir/mint-seed"
enable_info_page = true

[iroh]
enabled = $iroh_enabled
discovery = "n0"

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "Nocean CDK federation member $member"
description = "Five independent CDK operators configured through setup invitations"

[prometheus]
enabled = true
address = "127.0.0.1"
port = $prometheus_port

[database]
engine = "sqlite"

[[payment_backend]]
backend = "federated-bark"
unit = "sat"
min_mint = 1
max_mint = 50000000
min_melt = 1
max_melt = 50000000

[onchain]
onchain_backend = "federated-bdk"
min_mint = 1
max_mint = 50000000
min_melt = 1
max_melt = 50000000

[federation]
enabled = true
public_config_path = "$(activated_public_config "$member")"
private_config_path = "$(activated_private_config "$member")"

[federation.peer_sync]
aleph_bft_active_unit_interval_ms = $ALEPH_BFT_ACTIVE_UNIT_INTERVAL_MS
aleph_bft_network_pump_max_messages_per_tick = $ALEPH_BFT_NETWORK_PUMP_MAX_MESSAGES_PER_TICK

[federation.onchain_wallet]
enabled = true
network = "$NETWORK"
minimum_confirmations = $MINIMUM_CONFIRMATIONS
fee_policy_version = $FEE_POLICY_VERSION
chain_source_type = "esplora"
chain_source_scope = "shared"
chain_source_id = "$CHAIN_SOURCE_ID"
esplora_url = "$ESPLORA_URL"
sync_interval_secs = 15

[federation.bark]
enabled = true
network = "$NETWORK"
ark_server_public_key = "$BARK_ARK_SERVER_PUBLIC_KEY"
server_address = "$BARK_SERVER_ADDRESS"
esplora_url = "$BARK_ESPLORA_URL"
data_directory = "$(work_dir "$member")/federation-bark"
receive_secret_database_path = "$(work_dir "$member")/federation-bark/receive-secrets.redb"
frost_protocol_storage_root = "$(work_dir "$member")/frost-protocol"
operational_seed_hex = "file:$secrets_dir/bark-seed"
receive_secret_key_hex = "file:$secrets_dir/bark-receive-key"
create_if_missing = true
automatic_refresh = true
automatic_emergency_exit = false
EOF
    chmod 600 "$(runtime_config "$member")"
    "$MINTD" --work-dir "$(work_dir "$member")" config init --new-mint --file "$(runtime_config "$member")"
}

mintd_member() {
    local member="$1"
    shift
    "$MINTD" --work-dir "$(work_dir "$member")" "$@"
}

create_setup() {
    mintd_member 1 federation setup create \
        --config "$(setup_config 1)" \
        --state-dir "$(setup_state_dir 1)" \
        --database "$(setup_database 1)" \
        --members "$MEMBERS" \
        --threshold "$THRESHOLD" \
        --network "$NETWORK" \
        --public-mint-url "$(member_url 1)" \
        --federation-api-url "$(federation_url 1)" \
        --coordinator-url "$(federation_url 1)" \
        --checkpoint-interval-journal-operations "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
        --checkpoint-interval-finalized-items "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" \
        --amounts "$FEDERATION_AMOUNTS" \
        --frost-minimum-confirmations "$MINIMUM_CONFIRMATIONS" \
        --frost-fee-policy-version "$FEE_POLICY_VERSION" \
        --frost-application-key-epoch "$APPLICATION_KEY_EPOCH" \
        --frost-bark-ark-server-public-key "$BARK_ARK_SERVER_PUBLIC_KEY" \
        --frost-bark-application-key-epoch "$BARK_APPLICATION_KEY_EPOCH" \
        --frost-bark-policy-config "$PWD/bark-policy.json"
}

deliver_invitations() {
    local member source destination
    for member in $(seq 2 "$MEMBERS"); do
        source="$(setup_state_dir 1)/invitations/member-$member.invite"
        destination="$(incoming_invitation "$member")"
        [[ -s "$source" ]] || die "creator did not generate member $member invitation"
        grep -q '^cashufedsetupA' "$source" \
            || die "member $member invitation does not use the setup invitation encoding"
        install -m 600 "$source" "$destination"
        printf 'Delivered opaque %s setup invitation to member %s\n' \
            "$PUBLIC_TRANSPORT" "$member"
    done
}

start_setup_service() {
    local member="$1"
    "$MINTD" --work-dir "$(work_dir "$member")" \
        federation setup serve \
        --config "$(setup_config "$member")" \
        --listen "127.0.0.1:$((PORT_START + member - 1))" \
        --dkg-max-ticks "$DKG_MAX_TICKS" \
        --dkg-retry-delay-ms "$DKG_RETRY_DELAY_MS" \
        --dkg-ceremony-timeout-seconds "$DKG_CEREMONY_TIMEOUT_SECONDS" &
    SETUP_SERVICE_PID=$!
}

stop_setup_service() {
    local pid="$1"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

join_setup() {
    local member="$1" deadline
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if mintd_member "$member" federation setup join \
            --invitation-file "$(incoming_invitation "$member")" \
            --config "$(setup_config "$member")" \
            --state-dir "$(setup_state_dir "$member")" \
            --database "$(setup_database "$member")" \
            --public-mint-url "$(member_url "$member")" \
            --federation-api-url "$(federation_url "$member")"; then
            return 0
        fi
        printf 'Member %s join is waiting for the %s coordinator; retrying safely\n' \
            "$member" "$PUBLIC_TRANSPORT"
        sleep "$STATUS_POLL_SECS"
    done
    die "member $member could not join before the timeout"
}

wait_for_complete_enrollment() {
    local output deadline
    output="$(member_dir 1)/enrollment-status.json"
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if mintd_member 1 federation setup status --config "$(setup_config 1)" --json \
            >"$output" \
            && [[ "$(jq -r '.missing_enrollments | length' "$output")" == "0" ]]; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "the coordinator did not observe every enrollment before the timeout"
}

review_and_approve() {
    local member="$1" output proposal_hash deadline
    output="$(member_dir "$member")/review.json"
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if mintd_member "$member" federation setup review \
            --config "$(setup_config "$member")" --json | tee "$output"; then
            proposal_hash="$(jq -r '.proposal_hash' "$output")"
            [[ "$proposal_hash" =~ ^[0-9a-f]{64}$ ]] \
                || die "member $member reviewed an invalid proposal hash"
            mintd_member "$member" federation setup approve \
                --config "$(setup_config "$member")" \
                --proposal-hash "$proposal_hash"
            return 0
        fi
        printf 'Member %s is waiting for the setup proposal\n' "$member"
        sleep "$STATUS_POLL_SECS"
    done
    die "member $member could not review the proposal before the timeout"
}

wait_for_certified_manifest() {
    local member="$1" output deadline
    output="$(member_dir "$member")/certified-status.json"
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if mintd_member "$member" federation setup status \
            --config "$(setup_config "$member")" --json >"$output" \
            && [[ "$(jq -r '.participant_phase' "$output")" == "certified" ]]; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "member $member did not persist the certified manifest before the timeout"
}

wait_for_dkg() {
    local member="$1" output deadline
    output="$(member_dir "$member")/dkg-status.json"
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if mintd_member "$member" federation setup status \
            --config "$(setup_config "$member")" --json >"$output" \
            && [[ "$(jq -r '.dkg.finalized_ready // false' "$output")" == "true" ]]; then
            return 0
        fi
        sleep "$STATUS_POLL_SECS"
    done
    die "member $member DKG did not finalize before the timeout"
}

run_creator_setup() {
    local coordinator_pid dkg_pid
    printf 'Member 1 is creating the %s setup and protected invitation URLs\n' \
        "$PUBLIC_TRANSPORT"
    create_setup | tee "$(member_dir 1)/create.log"
    deliver_invitations
    start_setup_service 1
    coordinator_pid="$SETUP_SERVICE_PID"
    trap 'stop_setup_service "$coordinator_pid"' EXIT
    touch_marker coordinator-started
    wait_for_complete_enrollment
    mintd_member 1 federation setup propose --config "$(setup_config 1)" \
        | tee "$(member_dir 1)/propose.log"
    review_and_approve 1
    wait_for_certified_manifest 1
    touch_marker member-1-certified
    wait_for_marker stop-coordinator
    stop_setup_service "$coordinator_pid"
    trap - EXIT
    touch_marker coordinator-stopped
    wait_for_marker dkg-start
    printf 'Member 1 is starting DKG concurrently with every other member\n'
    start_setup_service 1
    dkg_pid="$SETUP_SERVICE_PID"
    trap 'stop_setup_service "$dkg_pid"' EXIT
    wait_for_dkg 1
    touch_marker member-1-dkg-ready
    wait_for_marker activate
    stop_setup_service "$dkg_pid"
    trap - EXIT
    activate_and_run 1
}

run_joiner_setup() {
    local member="$1" dkg_pid
    wait_for_marker coordinator-started
    while [[ ! -s "$(incoming_invitation "$member")" ]]; do
        sleep 0.2
    done
    printf 'Member %s is joining through its protected %s invitation URL\n' \
        "$member" "$PUBLIC_TRANSPORT"
    join_setup "$member"
    review_and_approve "$member"
    wait_for_certified_manifest "$member"
    touch_marker "member-$member-certified"
    wait_for_marker dkg-start
    printf 'Member %s is starting DKG concurrently with every other member\n' "$member"
    start_setup_service "$member"
    dkg_pid="$SETUP_SERVICE_PID"
    trap 'stop_setup_service "$dkg_pid"' EXIT
    wait_for_dkg "$member"
    touch_marker "member-$member-dkg-ready"
    wait_for_marker activate
    stop_setup_service "$dkg_pid"
    trap - EXIT
    activate_and_run "$member"
}

activate_and_run() {
    local member="$1"
    mintd_member "$member" federation setup activate --config "$(setup_config "$member")" \
        | tee "$(member_dir "$member")/activation.env"
    touch_marker "member-$member-activated"
    wait_for_marker runtime-start
    printf 'Member %s is starting its finalized mint runtime\n' "$member"
    exec env \
        CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=180 \
        RUST_LOG="${RUST_LOG:-info}" \
        "$MINTD" --work-dir "$(work_dir "$member")"
}

orchestrate() {
    printf 'Waiting for independent operators to certify the setup proposal\n'
    wait_for_all_member_markers certified
    touch_marker stop-coordinator
    wait_for_marker coordinator-stopped
    printf 'Starting all %s DKG participants without batches or serialization\n' "$MEMBERS"
    touch_marker dkg-start
    wait_for_all_member_markers dkg-ready
    printf 'Every member finalized DKG; activating local outputs in parallel\n'
    touch_marker activate
    wait_for_all_member_markers activated
    local member
    for member in $(seq 2 "$MEMBERS"); do
        cmp -s "$FINAL_PUBLIC" "$(activated_public_config "$member")" \
            || die "member $member activated a different public federation config"
    done
    touch_marker runtime-start
    printf 'All finalized member runtimes are starting in parallel\n'
    until status; do
        sleep 15
    done
    printf 'All federation members are ready; this pane will retain the signed status snapshot\n'
}

start_tmux_fresh() {
    local member
    tmux new-session -d -s "$SESSION" -n federation -x 240 -y 80 -c "$PWD" \
        -e "SESSION=$SESSION" \
        "exec bash scripts/nocean_federation_control.sh member-lifecycle 1"
    tmux set-option -t "$SESSION" remain-on-exit on
    tmux select-pane -t "$SESSION:0.0" -T member-1
    for member in $(seq 2 "$MEMBERS"); do
        tmux split-window -t "$SESSION:0" -c "$PWD" -e "SESSION=$SESSION" \
            "exec bash scripts/nocean_federation_control.sh member-lifecycle '$member'"
        tmux select-pane -T "member-$member"
        tmux select-layout -t "$SESSION:0" tiled >/dev/null
    done
    tmux split-window -t "$SESSION:0" -c "$PWD" -e "SESSION=$SESSION" \
        "exec bash scripts/nocean_federation_control.sh orchestrate"
    tmux select-pane -T status
    tmux select-layout -t "$SESSION:0" tiled >/dev/null
    printf 'Started fresh %s federation in tmux session %s\n' "$PUBLIC_TRANSPORT" "$SESSION"
}

fresh() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    find "$PWD" -maxdepth 1 -type d -name 'member-*' -exec rm -rf -- {} +
    rm -rf -- "$COORDINATION_DIR"
    rm -f -- "$PWD/bark-policy.json"
    mkdir -p "$COORDINATION_DIR"
    write_bark_policy
    local member
    for member in $(seq 1 "$MEMBERS"); do
        generate_member "$member"
    done
    start_tmux_fresh
}

run_runtime_member() {
    local member="$1"
    [[ -f "$(activated_private_config "$member")" ]] \
        || die "member $member activated private config is missing"
    exec env \
        CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=180 \
        RUST_LOG="${RUST_LOG:-info}" \
        "$MINTD" --work-dir "$(work_dir "$member")"
}

start_runtime() {
    [[ -f "$FINAL_PUBLIC" ]] || die "finalized public config is missing; run fresh first"
    tmux has-session -t "$SESSION" 2>/dev/null \
        && die "tmux session $SESSION already exists"
    local member
    tmux new-session -d -s "$SESSION" -n federation -x 240 -y 80 -c "$PWD" \
        "exec bash scripts/nocean_federation_control.sh runtime-member 1"
    tmux set-option -t "$SESSION" remain-on-exit on
    tmux select-pane -t "$SESSION:0.0" -T member-1
    for member in $(seq 2 "$MEMBERS"); do
        tmux split-window -t "$SESSION:0" -c "$PWD" \
            "exec bash scripts/nocean_federation_control.sh runtime-member '$member'"
        tmux select-pane -T "member-$member"
        tmux select-layout -t "$SESSION:0" tiled >/dev/null
    done
    tmux split-window -t "$SESSION:0" -c "$PWD" \
        "exec bash scripts/nocean_federation_control.sh status"
    tmux select-pane -T status
    tmux select-layout -t "$SESSION:0" tiled >/dev/null
}

stop() {
    if tmux has-session -t "$SESSION" 2>/dev/null; then
        tmux kill-session -t "$SESSION"
        printf 'Stopped federation tmux session %s\n' "$SESSION"
    fi
}

restart() {
    stop
    start_runtime
}

status() {
    mintd_member 1 federation status \
        --public-config "$FINAL_PUBLIC" \
        --private-config "$(activated_private_config 1)" \
        --require-ready \
        --require-converged \
        --max-operation-lag 0 \
        --require-no-digest-alerts \
        --require-no-payment-observation-alerts \
        --require-no-invalid-operation-alerts
}

wallet_status() {
    mintd_member 1 federation wallet-status \
        --public-config "$FINAL_PUBLIC" \
        --private-config "$(activated_private_config 1)" \
        --require-ready
}

status_loop() {
    while true; do
        clear
        date -u
        status || true
        sleep 60
    done
}

validate_environment
case "${1:-}" in
    fresh) fresh ;;
    start) start_runtime ;;
    stop) stop ;;
    restart) restart ;;
    member-lifecycle)
        member="${2:?member ID required}"
        require_positive member "$member"
        ((member <= MEMBERS)) || die "member ID exceeds member count"
        if ((member == 1)); then
            run_creator_setup
        else
            run_joiner_setup "$member"
        fi
        ;;
    runtime-member) run_runtime_member "${2:?member ID required}" ;;
    orchestrate) orchestrate ;;
    status) status ;;
    wallet-status) wallet_status ;;
    status-loop) status_loop ;;
    *) die "usage: $0 {fresh|start|stop|restart|status|wallet-status}" ;;
esac
