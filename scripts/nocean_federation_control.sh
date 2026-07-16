#!/usr/bin/env bash
set -Eeuo pipefail

MEMBERS="${MEMBERS:-5}"
THRESHOLD="${THRESHOLD:-3}"
PORT_START="${PORT_START:-21885}"
PROMETHEUS_PORT_START="${PROMETHEUS_PORT_START:-21985}"
PUBLIC_BASE_DOMAIN="${PUBLIC_BASE_DOMAIN:-nocean.xyz}"
NETWORK="${NETWORK:-signet}"
MINIMUM_CONFIRMATIONS="${MINIMUM_CONFIRMATIONS:-1}"
FEE_POLICY_VERSION="${FEE_POLICY_VERSION:-1}"
APPLICATION_KEY_EPOCH="${APPLICATION_KEY_EPOCH:-1}"
ESPLORA_URL="${ESPLORA_URL:-https://mutinynet.com/api}"
CHAIN_SOURCE_ID="${CHAIN_SOURCE_ID:-mutinynet-public-esplora}"
CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS="${CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS:-20}"
CHECKPOINT_INTERVAL_FINALIZED_ITEMS="${CHECKPOINT_INTERVAL_FINALIZED_ITEMS:-20}"
STATUS_TIMEOUT_SECS="${STATUS_TIMEOUT_SECS:-900}"
STATUS_POLL_SECS="${STATUS_POLL_SECS:-2}"
SESSION="cdk-federation"

MINTD="$PWD/bin/cdk-mintd"
SETUP_PUBLIC="$PWD/config/dkg-setup-public.json"
CEREMONY_CONFIG="$PWD/config/dkg-ceremony.toml"
FINAL_PUBLIC="$PWD/member-1/dkg-final/federation-public.json"

die() {
    echo "error: $*" >&2
    exit 1
}

member_url() {
    printf 'https://cdk-%s.%s' "$1" "$PUBLIC_BASE_DOMAIN"
}

member_dir() {
    printf '%s/member-%s' "$PWD" "$1"
}

setup_config() {
    printf '%s/setup-config.toml' "$(member_dir "$1")"
}

runtime_config() {
    printf '%s/runtime-config.toml' "$(member_dir "$1")"
}

private_setup_config() {
    printf '%s/dkg-setup-private.json' "$(member_dir "$1")"
}

private_final_config() {
    printf '%s/dkg-final/federation-private.json' "$(member_dir "$1")"
}

work_dir() {
    printf '%s/work' "$(member_dir "$1")"
}

require_commands() {
    local command
    for command in tmux curl jq timeout awk find ps seq; do
        command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
    done
    [[ -x "$MINTD" ]] || die "missing executable $MINTD"
    for value in "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
        "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" "$PROMETHEUS_PORT_START"; do
        [[ "$value" =~ ^[1-9][0-9]*$ ]] \
            || die "numeric settings must be positive integers"
    done
    ((PROMETHEUS_PORT_START + MEMBERS - 1 <= 65535)) \
        || die "Prometheus member ports exceed 65535"
}

generate_member() {
    local member="$1"
    local dir url port prometheus_port
    dir="$(member_dir "$member")"
    url="$(member_url "$member")"
    port=$((PORT_START + member - 1))
    prometheus_port=$((PROMETHEUS_PORT_START + member - 1))
    mkdir -p "$dir/work"

    "$MINTD" federation dkg-member \
        --member-id "$member" \
        --public-mint-url "$url" \
        --federation-api-url "$url" \
        --public-output "$dir/enrollment.json" \
        --private-output "$dir/identity-private.json" \
        --force

    cat > "$(setup_config "$member")" <<EOF
[info]
url = "$url"
listen_host = "127.0.0.1"
listen_port = $port
seed = "nocean-federation-member-$member"
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "Nocean CDK federation member $member"
description = "Mutinynet federated Cashu mint member $member"

[prometheus]
enabled = true
address = "127.0.0.1"
port = $prometheus_port

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
unit = "sat"
min_mint = 1
max_mint = 50000000
min_melt = 1
max_melt = 50000000

[fake_wallet]
supported_units = ["sat"]
fee_percent = 0.0
reserve_fee_min = 1
min_delay_time = 0
max_delay_time = 0

[federation]
enabled = true
public_config_path = "$SETUP_PUBLIC"
private_config_path = "$(private_setup_config "$member")"
EOF

    cat > "$(runtime_config "$member")" <<EOF
[info]
url = "$url"
listen_host = "127.0.0.1"
listen_port = $port
seed = "nocean-federation-member-$member"
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "Nocean CDK federation member $member"
description = "Mutinynet federated Cashu mint member $member"

[prometheus]
enabled = true
address = "127.0.0.1"
port = $prometheus_port

[database]
engine = "sqlite"

[onchain]
onchain_backend = "federated-bdk"
min_mint = 1
max_mint = 50000000
min_melt = 1
max_melt = 50000000

[federation]
enabled = true
public_config_path = "$(member_dir "$member")/dkg-final/federation-public.json"
private_config_path = "$(private_final_config "$member")"

[federation.onchain_wallet]
enabled = true
network = "$NETWORK"
minimum_confirmations = $MINIMUM_CONFIRMATIONS
fee_policy_version = $FEE_POLICY_VERSION
chain_source_type = "esplora"
chain_source_scope = "shared"
chain_source_id = "$CHAIN_SOURCE_ID"
esplora_url = "$ESPLORA_URL"
esplora_parallel_requests = 1
request_timeout_secs = 30
esplora_max_retries = 3
sync_interval_secs = 15
EOF
}

initialize() {
    tmux kill-session -t "$SESSION" 2>/dev/null || true
    rm -rf "$PWD/config" "$PWD/logs"
    find "$PWD" -maxdepth 1 -type d -name 'member-*' -exec rm -rf {} +
    mkdir -p "$PWD/config" "$PWD/logs"

    local member
    for member in $(seq 1 "$MEMBERS"); do
        generate_member "$member"
    done

    local -a roster_args=()
    for member in $(seq 1 "$MEMBERS"); do
        roster_args+=(--member "$(member_dir "$member")/enrollment.json")
    done
    "$MINTD" federation dkg-roster \
        "${roster_args[@]}" \
        --threshold "$THRESHOLD" \
        --unit sat \
        --amounts 1,2,4,8,16,32,64,128,256,512,1024,2048,4096,8192,16384,32768,65536 \
        --checkpoint-interval-journal-operations \
            "$CHECKPOINT_INTERVAL_JOURNAL_OPERATIONS" \
        --checkpoint-interval-finalized-items \
            "$CHECKPOINT_INTERVAL_FINALIZED_ITEMS" \
        --public-output "$SETUP_PUBLIC" \
        --ceremony-config-output "$CEREMONY_CONFIG" \
        --force

    for member in $(seq 1 "$MEMBERS"); do
        "$MINTD" federation dkg-bind \
            --public-config "$SETUP_PUBLIC" \
            --private-identity "$(member_dir "$member")/identity-private.json" \
            --output "$(private_setup_config "$member")" \
            --force
        cat "$CEREMONY_CONFIG" >> "$(setup_config "$member")"
        chmod 600 \
            "$(setup_config "$member")" \
            "$(runtime_config "$member")" \
            "$(member_dir "$member")/identity-private.json" \
            "$(private_setup_config "$member")"
    done
}

run_setup_member() {
    local member="$1"
    exec env \
        CDK_FEDERATION_DISABLE_PEER_SYNC=1 \
        RUST_LOG="${RUST_LOG:-info}" \
        "$MINTD" \
            --work-dir "$(work_dir "$member")" \
            --config "$(setup_config "$member")"
}

run_runtime_member() {
    local member="$1"
    exec env \
        CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=180 \
        RUST_LOG="${RUST_LOG:-info}" \
        "$MINTD" \
            --work-dir "$(work_dir "$member")" \
            --config "$(runtime_config "$member")"
}

dkg_member_ready() {
    local member="$1"
    "$MINTD" \
        --work-dir "$(work_dir "$member")" \
        --config "$(setup_config "$member")" \
        federation dkg-status \
        --public-config "$SETUP_PUBLIC" \
        --private-config "$(private_setup_config "$member")" \
        --from-database \
        --progress-only 2>/dev/null \
        | grep -q "keyset_shares=1 .*transcript_signatures=$MEMBERS .*activations=$MEMBERS frost_root_public_configs=1"
}

export_member() {
    local member="$1"
    local final_dir
    final_dir="$(member_dir "$member")/dkg-final"
    mkdir -p "$final_dir"
    "$MINTD" \
        --work-dir "$(work_dir "$member")" \
        --config "$(setup_config "$member")" \
        federation dkg-status \
        --public-config "$SETUP_PUBLIC" \
        --private-config "$(private_setup_config "$member")" \
        --from-database \
        --require-ready \
        --final-config-dir "$final_dir" \
        --frost-cdk-network "$NETWORK" \
        --frost-cdk-minimum-confirmations "$MINIMUM_CONFIRMATIONS" \
        --frost-cdk-fee-policy-version "$FEE_POLICY_VERSION" \
        --frost-cdk-application-key-epoch "$APPLICATION_KEY_EPOCH" \
        --force
}

member_pane() {
    tmux list-panes -t "$SESSION" -F '#{pane_id} #{pane_title}' \
        | awk -v title="member-$1" '$2 == title { print $1; exit }'
}

runtime_process_sample() {
    local pid="$1"
    ps -p "$pid" -o time= -o rss= 2>/dev/null | awk '
        function cpu_seconds(raw, parts, count, day_parts, days) {
            days = 0
            if (index(raw, "-") > 0) {
                split(raw, day_parts, "-")
                days = day_parts[1] + 0
                raw = day_parts[2]
            }
            count = split(raw, parts, ":")
            if (count == 2) return (days * 86400) + (parts[1] * 60) + parts[2]
            if (count == 3) {
                return (days * 86400) + (parts[1] * 3600) + (parts[2] * 60) + parts[3]
            }
            return (days * 86400) + raw
        }
        NF { printf "%.2f,%d\n", cpu_seconds($1), $2; exit }
    '
}

runtime_log_lines() {
    local member="$1"
    local -a log_files=()
    shopt -s nullglob
    log_files=("$(work_dir "$member")"/logs/cdk-mintd.log*)
    shopt -u nullglob
    if ((${#log_files[@]} == 0)); then
        printf '0\n'
        return
    fi
    awk 'END { print NR }' "${log_files[@]}"
}

runtime_prometheus_port() {
    local member="$1"
    awk '
        /^\[prometheus\]$/ { in_prometheus = 1; next }
        /^\[/ { in_prometheus = 0 }
        in_prometheus && $1 == "port" && $2 == "=" { print $3; exit }
    ' "$(runtime_config "$member")"
}

runtime_federation_metrics() {
    local member="$1"
    local port metrics
    port="$(runtime_prometheus_port "$member")"
    [[ "$port" =~ ^[1-9][0-9]*$ ]] \
        || die "member $member runtime config has no Prometheus port"
    metrics="$(curl --max-time 10 -fsS "http://127.0.0.1:$port/metrics")" \
        || die "member $member Prometheus metrics are unavailable"
    printf '%s\n' "$metrics" | awk '
        /^cdk_federation_sync_transitions_total\{/ {
            sync_transitions += $NF
        }
        /^cdk_federation_checkpoint_work_total\{/ {
            if ($0 ~ /work="scheduled_checkpoints_created"/) scheduled += $NF
            if ($0 ~ /work="recovery_checkpoints_created"/) recovery += $NF
            if ($0 ~ /work="quorums_transferred"/) quorums += $NF
            if ($0 ~ /work="checkpoints_promoted"/) promoted += $NF
            if ($0 ~ /work="catch_up_batches_applied"/) catch_up += $NF
            if ($0 ~ /work="signatures_verified"/) signatures += $NF
        }
        END {
            printf "%.0f,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f\n",
                sync_transitions, scheduled, recovery, quorums, promoted,
                catch_up, signatures
        }
    '
}

runtime_metrics() {
    tmux has-session -t "$SESSION" 2>/dev/null \
        || die "tmux federation session is not running"
    printf '%s\n' \
        'member,pid,cpu_seconds,rss_kb,log_lines,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified'
    local member pane pid process_sample log_lines federation_metrics
    for member in $(seq 1 "$MEMBERS"); do
        pane="$(member_pane "$member")"
        [[ -n "$pane" ]] || die "could not find tmux pane for member $member"
        pid="$(tmux display-message -p -t "$pane" '#{pane_pid}')"
        process_sample="$(runtime_process_sample "$pid")"
        [[ -n "$process_sample" ]] \
            || die "member $member process metrics are unavailable"
        log_lines="$(runtime_log_lines "$member")"
        federation_metrics="$(runtime_federation_metrics "$member")"
        printf '%s,%s,%s,%s,%s\n' \
            "$member" "$pid" "$process_sample" "$log_lines" "$federation_metrics"
    done
}

orchestrate() {
    local deadline member pane
    deadline=$((SECONDS + STATUS_TIMEOUT_SECS))
    echo "Waiting for the BLS and FROST DKG ceremony..."
    while ((SECONDS < deadline)); do
        local ready=0
        for member in $(seq 1 "$MEMBERS"); do
            if dkg_member_ready "$member"; then
                ready=$((ready + 1))
            fi
        done
        echo "DKG members ready: $ready/$MEMBERS"
        if ((ready == MEMBERS)); then
            break
        fi
        sleep "$STATUS_POLL_SECS"
    done

    for member in $(seq 1 "$MEMBERS"); do
        dkg_member_ready "$member" || die "DKG did not complete before the timeout"
    done

    echo "Exporting finalized BLS and FROST member configs..."
    for member in $(seq 1 "$MEMBERS"); do
        mkdir -p "$(member_dir "$member")/dkg-final"
        export_member "$member" > "$(member_dir "$member")/dkg-final/export.log" 2>&1
        cmp -s "$FINAL_PUBLIC" "$(member_dir "$member")/dkg-final/federation-public.json" \
            || die "member $member exported a different public config"
    done
    jq -e '.frost_wallet != null' "$FINAL_PUBLIC" >/dev/null \
        || die "final public config does not contain an active FROST wallet"

    echo "Starting finalized federated-BDK runtimes..."
    for member in $(seq 1 "$MEMBERS"); do
        pane="$(member_pane "$member")"
        [[ -n "$pane" ]] || die "could not find tmux pane for member $member"
        tmux respawn-pane -k -t "$pane" \
            "cd '$PWD' && exec bash scripts/nocean_federation_control.sh runtime-member '$member'"
    done

    echo "The finalized federation is running. This pane remains available for status."
    while true; do
        sleep 30
        "$0" status || true
    done
}

start_fresh() {
    initialize
    local member
    tmux new-session -d -s "$SESSION" -n federation -x 240 -y 80 -c "$PWD" \
        "exec bash scripts/nocean_federation_control.sh setup-member 1"
    tmux set-option -t "$SESSION" remain-on-exit on
    tmux select-pane -t "$SESSION:0.0" -T member-1
    for member in $(seq 2 "$MEMBERS"); do
        tmux split-window -t "$SESSION:0" -c "$PWD" \
            "exec bash scripts/nocean_federation_control.sh setup-member '$member'"
        tmux select-pane -T "member-$member"
        tmux select-layout -t "$SESSION:0" tiled >/dev/null
    done
    tmux split-window -t "$SESSION:0" -c "$PWD" \
        "exec bash scripts/nocean_federation_control.sh orchestrate"
    tmux select-pane -T status
    tmux select-layout -t "$SESSION:0" tiled >/dev/null
    echo "Started fresh federation ceremony in tmux session $SESSION"
}

restart_runtime() {
    [[ -f "$FINAL_PUBLIC" ]] || die "finalized public config is missing; run fresh first"
    jq -e '.frost_wallet != null' "$FINAL_PUBLIC" >/dev/null \
        || die "finalized public config does not contain a FROST wallet"
    tmux kill-session -t "$SESSION" 2>/dev/null || true
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
    tmux split-window -t "$SESSION:0" -c "$PWD" "exec bash scripts/nocean_federation_control.sh status-loop"
    tmux select-pane -T status
    tmux select-layout -t "$SESSION:0" tiled >/dev/null
}

status() {
    [[ -f "$FINAL_PUBLIC" ]] || return 1
    jq -e '.frost_wallet != null' "$FINAL_PUBLIC" >/dev/null || return 1
    "$MINTD" federation wallet-status \
        --public-config "$FINAL_PUBLIC" \
        --private-config "$(private_final_config 1)" \
        --require-ready >/dev/null
}

status_loop() {
    while true; do
        clear
        date -u
        "$MINTD" federation wallet-status \
            --public-config "$FINAL_PUBLIC" \
            --private-config "$(private_final_config 1)" \
            --require-ready || true
        sleep 10
    done
}

require_commands
case "${1:-}" in
    fresh) start_fresh ;;
    restart) restart_runtime ;;
    setup-member) run_setup_member "${2:?member ID required}" ;;
    runtime-member) run_runtime_member "${2:?member ID required}" ;;
    orchestrate) orchestrate ;;
    status) status ;;
    runtime-metrics) runtime_metrics ;;
    status-loop) status_loop ;;
    *) die "usage: $0 {fresh|restart|status}" ;;
esac
