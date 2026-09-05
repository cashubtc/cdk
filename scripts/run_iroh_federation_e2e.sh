#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
MINTD="${CDK_MINTD_BIN:-$REPO_ROOT/target/debug/cdk-mintd}"
FED_DIR="${FED_DIR:-}"
MEMBERS="${MEMBERS:-4}"
THRESHOLD="${THRESHOLD:-3}"
SETUP_HTTP_PORT_START="${SETUP_HTTP_PORT_START:-28980}"
IROH_PORT_START="${IROH_PORT_START:-29080}"
STATUS_RETRIES="${STATUS_RETRIES:-240}"
STATUS_SLEEP_SECONDS="${STATUS_SLEEP_SECONDS:-0.5}"
DKG_MAX_TICKS="${DKG_MAX_TICKS:-2400}"
DKG_RETRY_DELAY_MS="${DKG_RETRY_DELAY_MS:-100}"
DKG_CEREMONY_TIMEOUT_SECONDS="${DKG_CEREMONY_TIMEOUT_SECONDS:-600}"
PIDS=()

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

member_dir() {
    printf '%s/member-%s' "$FED_DIR" "$1"
}

member_work_dir() {
    printf '%s/work' "$(member_dir "$1")"
}

member_runtime_config() {
    printf '%s/runtime.toml' "$(member_dir "$1")"
}

member_operator_work_dir() {
    printf '%s/operator-work' "$(member_dir "$1")"
}

member_operator_config() {
    printf '%s/operator.toml' "$(member_dir "$1")"
}

member_setup_config() {
    printf '%s/setup.json' "$(member_dir "$1")"
}

member_setup_state() {
    printf '%s/setup-state' "$(member_dir "$1")"
}

member_database() {
    printf '%s/cdk-mintd.sqlite' "$(member_work_dir "$1")"
}

member_public_config() {
    printf '%s/activated/federation-public.json' "$(member_setup_state "$1")"
}

member_private_config() {
    printf '%s/activated/federation-private.json' "$(member_setup_state "$1")"
}

member_url() {
    sed -n '1p' "$(member_dir "$1")/endpoint.url"
}

member_ticket() {
    sed -n '1p' "$(member_dir "$1")/endpoint.ticket"
}

member_operator_url() {
    sed -n '1p' "$(member_dir "$1")/operator.url"
}

member_command() {
    local member="$1"
    shift
    "$MINTD" \
        --work-dir "$(member_operator_work_dir "$member")" \
        "$@"
}

stop_member() {
    local member="$1" pid="${PIDS[$1]:-}"
    if [[ -n "$pid" ]]; then
        kill -INT "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        unset 'PIDS[$member]'
    fi
}

stop_all() {
    local member
    for member in $(seq 1 "$MEMBERS"); do
        stop_member "$member"
    done
}

cleanup() {
    stop_all
}
trap cleanup EXIT
trap 'exit 130' INT TERM

write_base_config() {
    local member="$1" mode="$2" output seed tickets=() ticket peer
    output="$(member_runtime_config "$member")"
    local secret_path="$(member_dir "$member")/mint-seed"
    if [[ ! -f "$secret_path" ]]; then
        (umask 077; openssl rand -hex 32 | tr -d '\n' >"$secret_path")
    fi
    seed="file:$secret_path"

    if [[ "$mode" != "bootstrap" ]]; then
        for peer in $(seq 1 "$MEMBERS"); do
            if [[ "$mode" == "runtime" && "$peer" == "$member" ]]; then
                continue
            fi
            ticket="$(member_ticket "$peer")"
            [[ -n "$ticket" ]] || die "member $peer endpoint ticket is missing"
            tickets+=("$ticket")
        done
    fi

    {
        printf '[info]\n'
        printf 'url = "%s"\n' "$(member_url "$member")"
        printf 'http_enabled = false\n'
        printf 'listen_host = "127.0.0.1"\n'
        printf 'listen_port = %s\n' "$((SETUP_HTTP_PORT_START + member - 1))"
        printf 'seed = "%s"\n' "$seed"
        printf 'enable_info_page = true\n\n'
        printf '[info.http_cache]\nbackend = "memory"\nttl = 60\ntti = 60\n\n'
        printf '[mint_info]\n'
        printf 'name = "Local all-Iroh federation member %s"\n' "$member"
        printf 'description = "Dealer-free all-Iroh transport completion drill"\n\n'
        printf '[iroh]\nenabled = true\ngenerate_secret_key = true\ndiscovery = "static"\n'
        printf 'bind_addr = "127.0.0.1:%s"\n' "$((IROH_PORT_START + member - 1))"
        printf 'static_tickets = ['
        for ticket in "${tickets[@]:-}"; do
            [[ -n "$ticket" ]] || continue
            printf '"%s",' "$ticket"
        done
        printf ']\n\n'
        printf '[database]\nengine = "sqlite"\n\n'
        printf '[[payment_backend]]\nbackend = "fakewallet"\nunit = "sat"\n'
        printf 'min_mint = 1\nmax_mint = 500000\nmin_melt = 1\nmax_melt = 500000\n\n'
        if [[ "$mode" != "bootstrap" ]]; then
        printf '[onchain]\nonchain_backend = "federated-bdk"\n'
        printf 'min_mint = 1\nmax_mint = 500000\nmin_melt = 1\nmax_melt = 500000\n\n'
        fi
        printf '[fake_wallet]\nsupported_units = ["sat"]\nfee_percent = 0.0\n'
        printf 'reserve_fee_min = 1\nmin_delay_time = 0\nmax_delay_time = 0\n'
        printf 'shared_incoming_payment_dir = "%s"\n' "$FED_DIR/fake-payment-network"
        if [[ "$mode" != "bootstrap" ]]; then
            printf '\n[federation]\nenabled = true\n'
            printf 'public_config_path = "%s"\n' "$(member_public_config "$member")"
            printf 'private_config_path = "%s"\n' "$(member_private_config "$member")"
            printf '\n[federation.peer_sync]\n'
            printf 'aleph_bft_active_unit_interval_ms = 100\n'
            printf 'aleph_bft_network_pump_max_messages_per_tick = 64\n'
            printf 'operation_submission_timeout_secs = 180\n'
            printf '\n[federation.onchain_wallet]\nenabled = true\nnetwork = "regtest"\n'
            printf 'minimum_confirmations = 1\nfee_policy_version = 1\n'
            printf 'chain_source_type = "esplora"\nchain_source_scope = "shared"\n'
            printf 'chain_source_id = "local-e2e-unavailable"\n'
            printf 'esplora_url = "http://127.0.0.1:1"\nesplora_parallel_requests = 1\n'
            printf 'request_timeout_secs = 1\nesplora_max_retries = 0\nsync_interval_secs = 3600\n'
        fi
    } >"$output"
    chmod 600 "$output"
    if [[ "$mode" == "bootstrap" ]]; then
        "$MINTD" --work-dir "$(member_work_dir "$member")" config init --new-mint --file "$output"
    else
        "$MINTD" --work-dir "$(member_work_dir "$member")" config apply --file "$output"
    fi
}

write_operator_config() {
    local member="$1" output seed ticket tickets=()
    output="$(member_operator_config "$member")"
    local secret_path="$(member_dir "$member")/operator-seed"
    (umask 077; openssl rand -hex 32 | tr -d '\n' >"$secret_path")
    seed="file:$secret_path"
    for peer in $(seq 1 "$MEMBERS"); do
        ticket="$(member_ticket "$peer")"
        [[ -n "$ticket" ]] || die "member $peer endpoint ticket is missing"
        tickets+=("$ticket")
    done

    {
        printf '[info]\nurl = "%s"\nhttp_enabled = false\nseed = "%s"\n\n' \
            "$(member_operator_url "$member")" "$seed"
        printf '[iroh]\nenabled = true\ngenerate_secret_key = true\ndiscovery = "static"\n'
        printf 'bind_addr = "127.0.0.1:0"\nstatic_tickets = ['
        for ticket in "${tickets[@]}"; do
            printf '"%s",' "$ticket"
        done
        printf ']\n\n[database]\nengine = "sqlite"\n\n'
        printf '[[payment_backend]]\nbackend = "fakewallet"\nunit = "sat"\n'
        printf 'min_mint = 1\nmax_mint = 500000\nmin_melt = 1\nmax_melt = 500000\n\n'
        printf '[fake_wallet]\nsupported_units = ["sat"]\nfee_percent = 0.0\n'
        printf 'reserve_fee_min = 1\nmin_delay_time = 0\nmax_delay_time = 0\n'
    } >"$output"
    chmod 600 "$output"
    "$MINTD" --work-dir "$(member_operator_work_dir "$member")" config init --new-mint --file "$output"
}

start_setup_service() {
    local member="$1" log
    log="$(member_dir "$member")/setup-service.log"
    (
        exec "$MINTD" \
            --work-dir "$(member_work_dir "$member")" \
            federation setup serve \
            --config "$(member_setup_config "$member")" \
            --listen "127.0.0.1:$((SETUP_HTTP_PORT_START + member - 1))" \
            --dkg-max-ticks "$DKG_MAX_TICKS" \
            --dkg-retry-delay-ms "$DKG_RETRY_DELAY_MS" \
            --dkg-ceremony-timeout-seconds "$DKG_CEREMONY_TIMEOUT_SECONDS"
    ) >"$log" 2>&1 &
    PIDS[$member]=$!
}

start_runtime_member() {
    local member="$1" log
    log="$(member_dir "$member")/runtime.log"
    (
        exec "$MINTD" \
            --work-dir "$(member_work_dir "$member")"
    ) >"$log" 2>&1 &
    PIDS[$member]=$!
}

ensure_service_started() {
    local member="$1" mode="$2" attempt pid
    for attempt in $(seq 1 5); do
        if [[ "$mode" == "setup" ]]; then
            start_setup_service "$member"
        else
            start_runtime_member "$member"
        fi
        pid="${PIDS[$member]}"
        sleep 2
        if kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        wait "$pid" 2>/dev/null || true
        unset 'PIDS[$member]'
        sleep 1
    done
    die "member $member $mode service could not claim its persistent Iroh endpoint"
}

start_services_all() {
    local mode="$1" attempt member pid all_started
    for member in $(seq 1 "$MEMBERS"); do
        if [[ "$mode" == "setup" ]]; then
            start_setup_service "$member"
        else
            start_runtime_member "$member"
        fi
    done
    for attempt in $(seq 1 5); do
        sleep 2
        all_started=true
        for member in $(seq 1 "$MEMBERS"); do
            pid="${PIDS[$member]:-}"
            if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
                continue
            fi
            all_started=false
            if [[ -n "$pid" ]]; then
                wait "$pid" 2>/dev/null || true
            fi
            unset 'PIDS[$member]'
            if [[ "$mode" == "setup" ]]; then
                start_setup_service "$member"
            else
                start_runtime_member "$member"
            fi
        done
        [[ "$all_started" == "true" ]] && return 0
        sleep 1
    done
    die "one or more $mode services could not claim their persistent Iroh endpoints"
}

wait_for_setup_phase() {
    local expected="$1" attempt member phase all_ready
    for attempt in $(seq 1 "$STATUS_RETRIES"); do
        all_ready=true
        for member in $(seq 1 "$MEMBERS"); do
            if ! member_command "$member" federation setup status \
                --config "$(member_setup_config "$member")" --json \
                >"$(member_dir "$member")/setup-status.json" 2>/dev/null; then
                all_ready=false
                continue
            fi
            phase="$(jq -r '.participant_phase' "$(member_dir "$member")/setup-status.json")"
            if [[ "$expected" == "ready_to_activate" ]]; then
                [[ "$phase" == "$expected" ]] \
                    && [[ "$(jq -r '.dkg.finalized_ready // false' "$(member_dir "$member")/setup-status.json")" == "true" ]] \
                    || all_ready=false
            else
                [[ "$phase" == "$expected" ]] || all_ready=false
            fi
        done
        [[ "$all_ready" == "true" ]] && return 0
        sleep "$STATUS_SLEEP_SECONDS"
    done
    die "federation setup did not reach phase $expected"
}

wait_for_coordinator_iroh() {
    local attempt pid
    pid="${PIDS[1]}"
    for attempt in $(seq 1 "$STATUS_RETRIES"); do
        kill -0 "$pid" 2>/dev/null || die "setup coordinator exited before Iroh became ready"
        if member_command 1 federation setup status \
            --config "$(member_setup_config 1)" --json \
            >"$(member_dir 1)/coordinator-status.json" 2>/dev/null; then
            return 0
        fi
        sleep "$STATUS_SLEEP_SECONDS"
    done
    die "setup coordinator did not become reachable over Iroh"
}

wait_for_runtime() {
    local require_checkpoint="$1" attempt checkpoint_count member_args=() member
    shift
    for member in "$@"; do
        member_args+=(--member "$member")
    done
    if ((${#member_args[@]} == 0)); then
        for member in $(seq 1 "$MEMBERS"); do
            member_args+=(--member "$member")
        done
    fi
    for attempt in $(seq 1 "$STATUS_RETRIES"); do
        if member_command 1 federation status \
            --public-config "$(member_public_config 1)" \
            --private-config "$(member_private_config 1)" \
            "${member_args[@]}" \
            --require-ready \
            --require-converged \
            --max-operation-lag 0 \
            --require-no-digest-alerts \
            --require-no-payment-observation-alerts \
            --require-no-invalid-operation-alerts \
            >"$FED_DIR/runtime-status.txt" 2>/dev/null; then
            if [[ "$require_checkpoint" == "false" ]]; then
                return 0
            fi
            checkpoint_count="$(grep -c 'trusted_checkpoint=' "$FED_DIR/runtime-status.txt" || true)"
            if ((checkpoint_count == ${#member_args[@]} / 2)) \
                && ! grep -q 'trusted_checkpoint=none' "$FED_DIR/runtime-status.txt"; then
                return 0
            fi
        fi
        sleep "$STATUS_SLEEP_SECONDS"
    done
    die "federation runtime did not become ready and converged"
}

run_smoke() {
    local scenario="$1" seed_offset="$2"
    member_command 1 federation smoke-operations \
        --public-config "$(member_public_config 1)" \
        --private-config "$(member_private_config 1)" \
        --scenario "$scenario" \
        --seed-offset "$seed_offset" \
        --unit sat \
        --quote-retries 300 \
        --quote-sleep-ms 100 \
        --operation-timeout-seconds 900
}

join_member() {
    local member="$1" attempt
    for attempt in $(seq 1 10); do
        if member_command "$member" federation setup join \
            --invitation-file "$(member_dir "$member")/incoming.invite" \
            --config "$(member_setup_config "$member")" \
            --state-dir "$(member_setup_state "$member")" \
            --database "$(member_database "$member")" \
            --public-mint-url "$(member_url "$member")" \
            --federation-api-url "$(member_url "$member")" \
            >"$(member_dir "$member")/join.log" 2>"$(member_dir "$member")/join-error.log"; then
            return 0
        fi
        sleep "$STATUS_SLEEP_SECONDS"
    done
    return 1
}

for command in cargo cmp grep jq mktemp openssl sed; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
[[ "$MEMBERS" =~ ^[1-9][0-9]*$ ]] || die "MEMBERS must be positive"
[[ "$THRESHOLD" =~ ^[1-9][0-9]*$ ]] || die "THRESHOLD must be positive"
((THRESHOLD <= MEMBERS)) || die "THRESHOLD cannot exceed MEMBERS"
((MEMBERS >= 4)) || die "the outage/catch-up drill requires at least four members"

cd "$REPO_ROOT"
cargo build -p cdk-mintd --no-default-features \
    --features federation,federation-test-utils,sqlite,fakewallet,iroh

if [[ -z "$FED_DIR" ]]; then
    mkdir -p "$REPO_ROOT/target"
    FED_DIR="$(mktemp -d "$REPO_ROOT/target/cdk-iroh-federation-e2e.XXXXXX")"
else
    mkdir -- "$FED_DIR" \
        || die "FED_DIR must name a new directory so existing data is never overwritten"
fi

printf 'Generating independent persistent Iroh identities...\n'
for member in $(seq 1 "$MEMBERS"); do
    mkdir -p "$(member_work_dir "$member")" "$(member_operator_work_dir "$member")" \
        "$(member_setup_state "$member")"
    ticket_output="$("$MINTD" --work-dir "$(member_work_dir "$member")" iroh ticket \
        --bind-addr "127.0.0.1:$((IROH_PORT_START + member - 1))")"
    member_url_value="$(sed -n '1p' <<<"$ticket_output")"
    member_ticket_value="$(sed -n '2p' <<<"$ticket_output")"
    [[ "$member_url_value" =~ ^iroh://[a-z0-9]+$ ]] || die "member $member Iroh URL is invalid"
    [[ "$member_ticket_value" =~ ^endpoint[a-z0-9]+$ ]] \
        || die "member $member Iroh endpoint ticket is invalid"
    printf '%s\n' "$member_url_value" >"$(member_dir "$member")/endpoint.url"
    printf '%s\n' "$member_ticket_value" >"$(member_dir "$member")/endpoint.ticket"
    operator_ticket_output="$("$MINTD" --work-dir "$(member_operator_work_dir "$member")" \
        iroh ticket --bind-addr "127.0.0.1:0")"
    operator_url_value="$(sed -n '1p' <<<"$operator_ticket_output")"
    [[ "$operator_url_value" =~ ^iroh://[a-z0-9]+$ ]] \
        || die "member $member operator Iroh URL is invalid"
    printf '%s\n' "$operator_url_value" >"$(member_dir "$member")/operator.url"
done
sleep 1

for member in $(seq 1 "$MEMBERS"); do
    write_base_config "$member" setup
    write_operator_config "$member"
done

printf 'Running invitation enrollment and unanimous proposal approval over Iroh...\n'
member_command 1 federation setup create \
    --config "$(member_setup_config 1)" \
    --state-dir "$(member_setup_state 1)" \
    --database "$(member_database 1)" \
    --members "$MEMBERS" \
    --threshold "$THRESHOLD" \
    --network regtest \
    --public-mint-url "$(member_url 1)" \
    --federation-api-url "$(member_url 1)" \
    --coordinator-url "$(member_url 1)" \
    --checkpoint-interval-journal-operations 5 \
    --checkpoint-interval-finalized-items 1000 \
    >"$(member_dir 1)/create.log"
ensure_service_started 1 setup
wait_for_coordinator_iroh

join_pids=()
for member in $(seq 2 "$MEMBERS"); do
    cp "$(member_setup_state 1)/invitations/member-$member.invite" \
        "$(member_dir "$member")/incoming.invite"
    chmod 600 "$(member_dir "$member")/incoming.invite"
    join_member "$member" &
    join_pids[$member]=$!
done
for member in $(seq 2 "$MEMBERS"); do
    wait "${join_pids[$member]}" || die "member $member could not join over Iroh"
done

member_command 1 federation setup propose \
    --config "$(member_setup_config 1)" >"$(member_dir 1)/propose.log"
proposal_hash="$(sed -n 's/^proposal_hash: //p' "$(member_dir 1)/propose.log")"
[[ "$proposal_hash" =~ ^[0-9a-f]{64}$ ]] || die "proposal hash is invalid"
for member in $(seq 1 "$MEMBERS"); do
    member_command "$member" federation setup review \
        --config "$(member_setup_config "$member")" --json \
        >"$(member_dir "$member")/review.json"
    [[ "$(jq -r '.proposal_hash' "$(member_dir "$member")/review.json")" == "$proposal_hash" ]] \
        || die "member $member reviewed another proposal"
    member_command "$member" federation setup approve \
        --config "$(member_setup_config "$member")" \
        --proposal-hash "$proposal_hash" \
        >"$(member_dir "$member")/approve.log"
done
wait_for_setup_phase certified
stop_member 1

printf 'Starting every dealer-free BLS/FROST DKG participant concurrently over Iroh...\n'
dkg_started_at="$SECONDS"
start_services_all setup
wait_for_setup_phase ready_to_activate
dkg_seconds=$((SECONDS - dkg_started_at))
stop_all

activation_pids=()
for member in $(seq 1 "$MEMBERS"); do
    member_command "$member" federation setup activate \
        --config "$(member_setup_config "$member")" \
        >"$(member_dir "$member")/activation.env" \
        2>"$(member_dir "$member")/activation-guidance.log" &
    activation_pids[$member]=$!
done
for member in $(seq 1 "$MEMBERS"); do
    wait "${activation_pids[$member]}" || die "member $member activation failed"
    cmp -s "$(member_public_config 1)" "$(member_public_config "$member")" \
        || die "member $member activated another public config"
    write_base_config "$member" runtime
done

printf 'Starting Iroh-only runtimes and running paid mint/swap/melt/restore operations...\n'
start_services_all runtime
wait_for_runtime false
run_smoke standard 0 >"$FED_DIR/standard-smoke.log"
wait_for_runtime true

offline_member="$MEMBERS"
stop_member "$offline_member"
remaining_members=()
for member in $(seq 1 "$((MEMBERS - 1))"); do
    remaining_members+=("$member")
done
wait_for_runtime true "${remaining_members[@]}"
run_smoke offline-member-operations 1 >"$FED_DIR/offline-smoke.log"
wait_for_runtime true "${remaining_members[@]}"
ensure_service_started "$offline_member" runtime
wait_for_runtime true

printf 'Restarting every member and requiring exact catch-up and checkpoint convergence...\n'
stop_all
start_services_all runtime
wait_for_runtime true

for member in $(seq 1 "$MEMBERS"); do
    jq -e '.members | to_entries | all(.value.public_mint_url | startswith("iroh://"))' \
        "$(member_public_config "$member")" >/dev/null \
        || die "member $member public config contains a non-Iroh public URL"
    jq -e '.members | to_entries | all(.value.federation_api_url | startswith("iroh://"))' \
        "$(member_public_config "$member")" >/dev/null \
        || die "member $member public config contains a non-Iroh federation URL"
done

jq -n \
    --argjson members "$MEMBERS" \
    --argjson threshold "$THRESHOLD" \
    --argjson dkg_seconds "$dkg_seconds" \
    '{
        result: "passed",
        topology: "all-iroh-static-loopback",
        http_listener_enabled: false,
        dns_required: false,
        members: $members,
        threshold: $threshold,
        dealer_free_bls_and_frost_dkg: true,
        dkg_concurrent: true,
        dkg_seconds: $dkg_seconds,
        paid_wallet_operations: ["mint", "swap", "melt", "restore"],
        checkpoint_quorum: true,
        one_member_outage_and_catch_up: true,
        coordinated_restart: true
    }' >"$FED_DIR/result.json"

printf 'all-Iroh federation completion drill passed: %s\n' "$FED_DIR/result.json"
