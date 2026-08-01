#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
MINTD="${CDK_MINTD_BIN:-$REPO_ROOT/target/debug/cdk-mintd}"
FED_DIR="${FED_DIR:-$REPO_ROOT/target/cdk-federation-setup-smoke}"
MEMBERS="${MEMBERS:-3}"
THRESHOLD="${THRESHOLD:-2}"
COORDINATOR_PORT="${COORDINATOR_PORT:-18080}"
MEMBER_PORT_START="${MEMBER_PORT_START:-18081}"
STATUS_RETRIES="${STATUS_RETRIES:-1200}"
STATUS_SLEEP_SECONDS="${STATUS_SLEEP_SECONDS:-0.25}"
READINESS_MEMBER="${READINESS_MEMBER:-1}"
DKG_MAX_TICKS="${DKG_MAX_TICKS:-1200}"
DKG_RETRY_DELAY_MS="${DKG_RETRY_DELAY_MS:-250}"
DKG_CEREMONY_TIMEOUT_SECONDS="${DKG_CEREMONY_TIMEOUT_SECONDS:-600}"
AMOUNTS="${AMOUNTS:-1}"
JOIN_ORDER="${JOIN_ORDER:-forward}"
JOIN_DELAY_SECONDS="${JOIN_DELAY_SECONDS:-0}"
ACK_LOSS_MEMBER="${ACK_LOSS_MEMBER:-0}"
REPLACE_MISSING_MEMBER="${REPLACE_MISSING_MEMBER:-0}"
RESTART_COORDINATOR="${RESTART_COORDINATOR:-0}"
SATURATE_COORDINATOR="${SATURATE_COORDINATOR:-0}"
HELD_DKG_MEMBER="${HELD_DKG_MEMBER:-0}"
RESTART_DKG_MEMBER="${RESTART_DKG_MEMBER:-0}"
SUMMARY_FILE="${SUMMARY_FILE:-$FED_DIR/setup-summary.json}"
PIDS=()
DKG_PIDS=()
setup_started_at="$SECONDS"
status_requests=0
join_requests=0
approval_requests=0
review_requests=0
request_failures=0
setup_retries=0
max_total_rss_kb=0
max_process_rss_kb=0
max_status_latency_seconds=0
MEMBER_MAX_RSS_KB=()
MEMBER_CPU_TIME=()

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

member_dir() {
    printf '%s/member-%s' "$FED_DIR" "$1"
}

member_config() {
    printf '%s/setup.json' "$(member_dir "$1")"
}

member_database() {
    printf '%s/setup.sqlite' "$(member_dir "$1")"
}

member_state_dir() {
    printf '%s/state' "$(member_dir "$1")"
}

member_public_url() {
    printf 'http://localhost:%s' "$((MEMBER_PORT_START + $1 - 1))"
}

member_federation_url() {
    printf 'http://127.0.0.1:%s' "$((MEMBER_PORT_START + $1 - 1))"
}

cleanup() {
    local pid
    for pid in "${PIDS[@]:-}"; do
        if [[ -n "$pid" ]]; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
        fi
    done
}
trap cleanup EXIT
trap 'exit 130' INT TERM

wait_for_http_listener() {
    local url="$1"
    local pid="$2"
    local attempt code
    for ((attempt = 1; attempt <= STATUS_RETRIES; attempt++)); do
        kill -0 "$pid" 2>/dev/null || die "setup service exited before listening at $url"
        code="$(curl --silent --output /dev/null --write-out '%{http_code}' --max-time 1 "$url" || true)"
        if [[ "$code" != "000" ]]; then
            return 0
        fi
        sleep "$STATUS_SLEEP_SECONDS"
    done
    die "setup service did not listen at $url"
}

start_coordinator() {
    "$MINTD" federation setup serve \
        --config "$(member_config 1)" \
        --listen "127.0.0.1:$COORDINATOR_PORT" \
        --dkg-max-ticks "$DKG_MAX_TICKS" \
        --dkg-retry-delay-ms "$DKG_RETRY_DELAY_MS" \
        --dkg-ceremony-timeout-seconds "$DKG_CEREMONY_TIMEOUT_SECONDS" \
        >"$(member_dir 1)/coordinator.log" 2>&1 &
    coordinator_pid=$!
    PIDS+=("$coordinator_pid")
    wait_for_http_listener \
        "http://127.0.0.1:$COORDINATOR_PORT/federation/setup/v1/status" \
        "$coordinator_pid"
}

stop_coordinator() {
    kill "$coordinator_pid" 2>/dev/null || true
    wait "$coordinator_pid" 2>/dev/null || true
    PIDS=()
}

join_member() {
    local member="$1"
    local invitation="$2"
    local output="$3"
    local dir
    dir="$(member_dir "$member")"
    mkdir -p "$dir"
    join_requests=$((join_requests + 1))
    "$MINTD" federation setup join \
        --invitation-file "$invitation" \
        --config "$(member_config "$member")" \
        --state-dir "$(member_state_dir "$member")" \
        --database "$(member_database "$member")" \
        --public-mint-url "$(member_public_url "$member")" \
        --federation-api-url "$(member_federation_url "$member")" \
        --allow-insecure-localhost \
        >"$output"
}

start_dkg_member() {
    local member="$1"
    local dir listen
    dir="$(member_dir "$member")"
    listen="127.0.0.1:$((MEMBER_PORT_START + member - 1))"
    "$MINTD" federation setup serve \
        --config "$(member_config "$member")" \
        --listen "$listen" \
        --dkg-max-ticks "$DKG_MAX_TICKS" \
        --dkg-retry-delay-ms "$DKG_RETRY_DELAY_MS" \
        --dkg-ceremony-timeout-seconds "$DKG_CEREMONY_TIMEOUT_SECONDS" \
        >"$dir/dkg.log" 2>&1 &
    DKG_PIDS[$member]=$!
    PIDS+=("$!")
}

sample_dkg_resources() {
    local member sample rss cpu_time total_rss=0 process_rss=0
    for member in $(seq 1 "$MEMBERS"); do
        [[ -n "${DKG_PIDS[$member]:-}" ]] || continue
        kill -0 "${DKG_PIDS[$member]}" 2>/dev/null || continue
        sample="$(ps -o rss= -o time= -p "${DKG_PIDS[$member]}" 2>/dev/null || true)"
        rss="$(awk 'NR == 1 { print $1 + 0 }' <<<"$sample")"
        cpu_time="$(awk 'NR == 1 { print $2 }' <<<"$sample")"
        rss="${rss:-0}"
        cpu_time="${cpu_time:-00:00:00}"
        total_rss=$((total_rss + rss))
        if ((rss > process_rss)); then
            process_rss="$rss"
        fi
        if ((rss > ${MEMBER_MAX_RSS_KB[$member]:-0})); then
            MEMBER_MAX_RSS_KB[$member]="$rss"
        fi
        MEMBER_CPU_TIME[$member]="$cpu_time"
    done
    if ((total_rss > max_total_rss_kb)); then
        max_total_rss_kb="$total_rss"
    fi
    if ((process_rss > max_process_rss_kb)); then
        max_process_rss_kb="$process_rss"
    fi
}

if [[ ! -x "$MINTD" ]]; then
    cd "$REPO_ROOT"
    case "$MINTD" in
        */release/*)
            cargo build --release -p cdk-mintd --no-default-features --features federation,sqlite
            ;;
        *)
            cargo build -p cdk-mintd --no-default-features --features federation,sqlite
            ;;
    esac
fi
for command in awk cmp curl jq ps sed; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
[[ "$MEMBERS" =~ ^[1-9][0-9]*$ ]] || die "MEMBERS must be positive"
[[ "$THRESHOLD" =~ ^[1-9][0-9]*$ ]] || die "THRESHOLD must be positive"
[[ "$READINESS_MEMBER" =~ ^[1-9][0-9]*$ ]] || die "READINESS_MEMBER must be positive"
[[ "$DKG_MAX_TICKS" =~ ^[1-9][0-9]*$ ]] || die "DKG_MAX_TICKS must be positive"
[[ "$DKG_RETRY_DELAY_MS" =~ ^[1-9][0-9]*$ ]] || die "DKG_RETRY_DELAY_MS must be positive"
[[ "$DKG_CEREMONY_TIMEOUT_SECONDS" =~ ^[1-9][0-9]*$ ]] \
    || die "DKG_CEREMONY_TIMEOUT_SECONDS must be positive"
((THRESHOLD <= MEMBERS)) || die "THRESHOLD cannot exceed MEMBERS"
((READINESS_MEMBER <= MEMBERS)) || die "READINESS_MEMBER exceeds member count"

rm -rf -- "$FED_DIR"
mkdir -p "$FED_DIR"
: >"$FED_DIR/dkg-phase-samples.jsonl"

creator_dir="$(member_dir 1)"
mkdir -p "$creator_dir"
"$MINTD" federation setup create \
    --config "$(member_config 1)" \
    --state-dir "$(member_state_dir 1)" \
    --database "$(member_database 1)" \
    --members "$MEMBERS" \
    --threshold "$THRESHOLD" \
    --network regtest \
    --public-mint-url "$(member_public_url 1)" \
    --federation-api-url "$(member_federation_url 1)" \
    --coordinator-url "http://127.0.0.1:$COORDINATOR_PORT" \
    --amounts "$AMOUNTS" \
    --allow-insecure-localhost \
    >"$creator_dir/create.log"

start_coordinator

case "$JOIN_ORDER" in
    forward)
        join_members="$(seq 2 "$MEMBERS")"
        ;;
    reverse)
        join_members="$(seq "$MEMBERS" -1 2)"
        ;;
    *)
        die "JOIN_ORDER must be forward or reverse"
        ;;
esac

for member in $join_members; do
    if ((member == REPLACE_MISSING_MEMBER)); then
        continue
    fi
    invitation="$creator_dir/state/invitations/member-$member.invite"
    join_member "$member" "$invitation" "$(member_dir "$member")/join.log"
    if ((member == ACK_LOSS_MEMBER)); then
        setup_retries=$((setup_retries + 1))
        join_member "$member" "$invitation" "$(member_dir "$member")/join-replay.log"
    fi
    sleep "$JOIN_DELAY_SECONDS"
done

if ((REPLACE_MISSING_MEMBER > 0)); then
    ((REPLACE_MISSING_MEMBER >= 2 && REPLACE_MISSING_MEMBER <= MEMBERS)) \
        || die "REPLACE_MISSING_MEMBER must identify a non-creator member"
    if "$MINTD" federation setup propose \
        --config "$(member_config 1)" \
        >"$creator_dir/incomplete-propose.log" 2>&1; then
        die "incomplete enrollment unexpectedly produced a proposal"
    fi
    request_failures=$((request_failures + 1))
    replacement_invitation="$creator_dir/state/invitations/member-$REPLACE_MISSING_MEMBER-replacement.invite"
    "$MINTD" federation setup invite \
        --config "$(member_config 1)" \
        --member "$REPLACE_MISSING_MEMBER" \
        --output "$replacement_invitation" \
        --replace \
        >"$creator_dir/replacement.log"
    join_member \
        "$REPLACE_MISSING_MEMBER" \
        "$replacement_invitation" \
        "$(member_dir "$REPLACE_MISSING_MEMBER")/join-replacement.log"
fi

if ((RESTART_COORDINATOR > 0)); then
    stop_coordinator
    setup_retries=$((setup_retries + 1))
    start_coordinator
fi

proposal_output="$creator_dir/propose.log"
"$MINTD" federation setup propose --config "$(member_config 1)" >"$proposal_output"
proposal_hash="$(sed -n 's/^proposal_hash: //p' "$proposal_output")"
[[ -n "$proposal_hash" ]] || die "setup proposal did not print its hash"
"$MINTD" federation setup propose \
    --config "$(member_config 1)" \
    >"$creator_dir/propose-replay.log"
replayed_proposal_hash="$(sed -n 's/^proposal_hash: //p' "$creator_dir/propose-replay.log")"
[[ "$replayed_proposal_hash" == "$proposal_hash" ]] \
    || die "exact proposal replay produced a different hash"
setup_retries=$((setup_retries + 1))

if ((SATURATE_COORDINATOR > 0)); then
    saturation_pids=()
    for wave in $(seq 1 "$SATURATE_COORDINATOR"); do
        for member in $(seq 2 "$MEMBERS"); do
            "$MINTD" federation setup review \
                --config "$(member_config "$member")" \
                --json >"$(member_dir "$member")/saturation-review-$wave.json" 2>&1 &
            saturation_pids+=("$!")
            PIDS+=("$!")
            review_requests=$((review_requests + 1))
        done
    done
    for pid in "${saturation_pids[@]}"; do
        if ! wait "$pid"; then
            request_failures=$((request_failures + 1))
        fi
    done
fi

for member in $(seq 1 "$MEMBERS"); do
    dir="$(member_dir "$member")"
    "$MINTD" federation setup review \
        --config "$(member_config "$member")" \
        --json >"$dir/review.json"
    review_requests=$((review_requests + 1))
    [[ "$(jq -r '.proposal_hash' "$dir/review.json")" == "$proposal_hash" ]] \
        || die "member $member reviewed a different proposal"
    "$MINTD" federation setup approve \
        --config "$(member_config "$member")" \
        --proposal-hash "$proposal_hash" \
        >"$dir/approve.log"
    approval_requests=$((approval_requests + 1))
    if ((member == ACK_LOSS_MEMBER)); then
        "$MINTD" federation setup approve \
            --config "$(member_config "$member")" \
            --proposal-hash "$proposal_hash" \
            >"$dir/approve-replay.log"
        approval_requests=$((approval_requests + 1))
        setup_retries=$((setup_retries + 1))
    fi
done

if ((MEMBERS >= 2)); then
    wrong_proposal_hash="$(printf '%064d' 0)"
    if "$MINTD" federation setup approve \
        --config "$(member_config 2)" \
        --proposal-hash "$wrong_proposal_hash" \
        >"$(member_dir 2)/approve-conflict.log" 2>&1; then
        die "a member unexpectedly approved a second proposal hash"
    fi
    approval_requests=$((approval_requests + 1))
    request_failures=$((request_failures + 1))
fi

for member in $(seq 1 "$MEMBERS"); do
    status_requests=$((status_requests + 1))
    "$MINTD" federation setup status \
        --config "$(member_config "$member")" \
        --json >"$(member_dir "$member")/certified-status.json"
    [[ "$(jq -r '.participant_phase' "$(member_dir "$member")/certified-status.json")" == "certified" ]] \
        || die "member $member did not persist the certified manifest"
done

bootstrap_seconds=$((SECONDS - setup_started_at))
stop_coordinator
dkg_started_at="$SECONDS"

for member in $(seq 1 "$MEMBERS"); do
    if ((member != HELD_DKG_MEMBER)); then
        start_dkg_member "$member"
    fi
done

for member in $(seq 1 "$MEMBERS"); do
    if ((member == HELD_DKG_MEMBER)); then
        continue
    fi
    wait_for_http_listener \
        "$(member_federation_url "$member")/federation/v1/dkg/commitments" \
        "${DKG_PIDS[$member]}"
done

if ((HELD_DKG_MEMBER > 0)); then
    ((HELD_DKG_MEMBER <= MEMBERS)) || die "HELD_DKG_MEMBER exceeds member count"
    sleep 1
    status_requests=$((status_requests + 1))
    "$MINTD" federation setup status \
        --config "$(member_config 1)" \
        --json >"$creator_dir/held-member-status.json"
    [[ "$(jq -r '.dkg.finalized_ready // false' "$creator_dir/held-member-status.json")" == "false" ]] \
        || die "DKG finalized while a certified member was absent"
    start_dkg_member "$HELD_DKG_MEMBER"
    wait_for_http_listener \
        "$(member_federation_url "$HELD_DKG_MEMBER")/federation/v1/dkg/commitments" \
        "${DKG_PIDS[$HELD_DKG_MEMBER]}"
fi

if ((RESTART_DKG_MEMBER > 0)); then
    ((RESTART_DKG_MEMBER <= MEMBERS)) || die "RESTART_DKG_MEMBER exceeds member count"
    sleep 1
    kill "${DKG_PIDS[$RESTART_DKG_MEMBER]}"
    wait "${DKG_PIDS[$RESTART_DKG_MEMBER]}" 2>/dev/null || true
    setup_retries=$((setup_retries + 1))
    start_dkg_member "$RESTART_DKG_MEMBER"
    wait_for_http_listener \
        "$(member_federation_url "$RESTART_DKG_MEMBER")/federation/v1/dkg/commitments" \
        "${DKG_PIDS[$RESTART_DKG_MEMBER]}"
fi

for ((attempt = 1; attempt <= STATUS_RETRIES; attempt++)); do
    sample_dkg_resources
    status_started_at="$SECONDS"
    readiness_status="$(member_dir "$READINESS_MEMBER")/readiness-status.json"
    status_requests=$((status_requests + 1))
    if "$MINTD" federation setup status \
        --config "$(member_config "$READINESS_MEMBER")" \
        --json >"$readiness_status" \
        2>"$(member_dir "$READINESS_MEMBER")/readiness-status-error.log"; then
        sentinel_ready="$(jq -r '.dkg.finalized_ready // false' "$readiness_status")"
    else
        request_failures=$((request_failures + 1))
        sentinel_ready=false
    fi
    status_latency_seconds=$((SECONDS - status_started_at))
    if ((status_latency_seconds > max_status_latency_seconds)); then
        max_status_latency_seconds="$status_latency_seconds"
    fi
    jq -cn \
        --argjson elapsed_seconds "$((SECONDS - dkg_started_at))" \
        --argjson attempt "$attempt" \
        --argjson sentinel_ready "$sentinel_ready" \
        '{elapsed_seconds: $elapsed_seconds, attempt: $attempt, sentinel_ready: $sentinel_ready}' \
        >>"$FED_DIR/dkg-phase-samples.jsonl"
    printf 'DKG readiness sentinel: %s\n' "$sentinel_ready"
    if [[ "$sentinel_ready" == "true" ]]; then
        break
    fi
    sleep "$STATUS_SLEEP_SECONDS"
done
[[ "$sentinel_ready" == "true" ]] || die "DKG did not complete before the timeout"

sample_dkg_resources
ready=0
status_started_at="$SECONDS"
status_pids=()
for member in $(seq 1 "$MEMBERS"); do
    status_file="$(member_dir "$member")/dkg-status.json"
    "$MINTD" federation setup status \
        --config "$(member_config "$member")" \
        --json >"$status_file" &
    status_pids+=("$!")
    PIDS+=("$!")
    status_requests=$((status_requests + 1))
done
for member in $(seq 1 "$MEMBERS"); do
    status_file="$(member_dir "$member")/dkg-status.json"
    if ! wait "${status_pids[$((member - 1))]}"; then
        request_failures=$((request_failures + 1))
        continue
    fi
    if [[ "$(jq -r '.dkg.finalized_ready // false' "$status_file")" == "true" ]]; then
        ready=$((ready + 1))
    fi
done
status_latency_seconds=$((SECONDS - status_started_at))
if ((status_latency_seconds > max_status_latency_seconds)); then
    max_status_latency_seconds="$status_latency_seconds"
fi
printf 'DKG members ready: %s/%s\n' "$ready" "$MEMBERS"
((ready == MEMBERS)) || die "not every member validated the finalized DKG"
dkg_seconds=$((SECONDS - dkg_started_at))

cleanup
PIDS=()

activation_started_at="$SECONDS"
activation_pids=()
for member in $(seq 1 "$MEMBERS"); do
    "$MINTD" federation setup activate \
        --config "$(member_config "$member")" \
        >"$(member_dir "$member")/activation.env" \
        2>"$(member_dir "$member")/activation-guidance.log" &
    activation_pids+=("$!")
    PIDS+=("$!")
done
for member in $(seq 1 "$MEMBERS"); do
    wait "${activation_pids[$((member - 1))]}" \
        || die "member $member activation failed"
done
activation_seconds=$((SECONDS - activation_started_at))
total_seconds=$((SECONDS - setup_started_at))

reference_public="$(member_state_dir 1)/activated/federation-public.json"
reference_status="$(member_dir 1)/dkg-status.json"
reference_proposal_hash="$(jq -r '.proposal_hash' "$reference_status")"
reference_manifest_hash="$(jq -r '.manifest_hash' "$reference_status")"
reference_transcript_hash="$(jq -r '.dkg.transcript_hash' "$reference_status")"
reference_final_config_digest="$(jq -r '.dkg.final_config_digest' "$reference_status")"
reference_setup_authorization="$(jq -r '.setup_authorization' "$reference_public")"
for value in \
    "$reference_proposal_hash" \
    "$reference_manifest_hash" \
    "$reference_transcript_hash" \
    "$reference_final_config_digest" \
    "$reference_setup_authorization"; do
    [[ -n "$value" && "$value" != "null" ]] || die "final setup evidence is incomplete"
done
for member in $(seq 1 "$MEMBERS"); do
    cmp -s "$reference_public" \
        "$(member_state_dir "$member")/activated/federation-public.json" \
        || die "member $member activated a different public config"
    status_file="$(member_dir "$member")/dkg-status.json"
    [[ "$(jq -r '.proposal_hash' "$status_file")" == "$reference_proposal_hash" ]] \
        || die "member $member finalized a different proposal"
    [[ "$(jq -r '.manifest_hash' "$status_file")" == "$reference_manifest_hash" ]] \
        || die "member $member finalized a different manifest"
    [[ "$(jq -r '.dkg.transcript_hash' "$status_file")" == "$reference_transcript_hash" ]] \
        || die "member $member finalized a different DKG transcript"
    [[ "$(jq -r '.dkg.final_config_digest' "$status_file")" == "$reference_final_config_digest" ]] \
        || die "member $member finalized a different config digest"
done

resources_json='[]'
for member in $(seq 1 "$MEMBERS"); do
    resources_json="$(jq -c \
        --argjson member "$member" \
        --argjson max_rss_kb "${MEMBER_MAX_RSS_KB[$member]:-0}" \
        --arg cpu_time "${MEMBER_CPU_TIME[$member]:-00:00:00}" \
        '. + [{member: $member, max_rss_kb: $max_rss_kb, cpu_time: $cpu_time}]' \
        <<<"$resources_json")"
done
mkdir -p "$(dirname "$SUMMARY_FILE")"
jq -n \
    --argjson members "$MEMBERS" \
    --argjson threshold "$THRESHOLD" \
    --arg join_order "$JOIN_ORDER" \
    --argjson dkg_max_ticks "$DKG_MAX_TICKS" \
    --argjson dkg_retry_delay_ms "$DKG_RETRY_DELAY_MS" \
    --argjson dkg_ceremony_timeout_seconds "$DKG_CEREMONY_TIMEOUT_SECONDS" \
    --argjson acknowledgement_replay_member "$ACK_LOSS_MEMBER" \
    --argjson replacement_member "$REPLACE_MISSING_MEMBER" \
    --argjson coordinator_restarts "$RESTART_COORDINATOR" \
    --argjson saturation_waves "$SATURATE_COORDINATOR" \
    --argjson held_dkg_member "$HELD_DKG_MEMBER" \
    --argjson restarted_dkg_member "$RESTART_DKG_MEMBER" \
    --arg proposal_hash "$reference_proposal_hash" \
    --arg manifest_hash "$reference_manifest_hash" \
    --arg setup_authorization "$reference_setup_authorization" \
    --arg transcript_hash "$reference_transcript_hash" \
    --arg final_config_digest "$reference_final_config_digest" \
    --argjson bootstrap_seconds "$bootstrap_seconds" \
    --argjson dkg_seconds "$dkg_seconds" \
    --argjson activation_seconds "$activation_seconds" \
    --argjson total_seconds "$total_seconds" \
    --argjson dkg_status_attempts "$attempt" \
    --argjson status_latency_max_seconds "$max_status_latency_seconds" \
    --argjson join_requests "$join_requests" \
    --argjson review_requests "$review_requests" \
    --argjson approval_requests "$approval_requests" \
    --argjson status_requests "$status_requests" \
    --argjson request_failures "$request_failures" \
    --argjson retries "$setup_retries" \
    --argjson max_total_rss_kb "$max_total_rss_kb" \
    --argjson max_process_rss_kb "$max_process_rss_kb" \
    --argjson process_resources "$resources_json" \
    '{
        version: 1,
        result: "passed",
        members: $members,
        threshold: $threshold,
        join_order: $join_order,
        driver: {
            max_ticks: $dkg_max_ticks,
            retry_delay_ms: $dkg_retry_delay_ms,
            ceremony_timeout_seconds: $dkg_ceremony_timeout_seconds
        },
        scenarios: {
            acknowledgement_replay_member: $acknowledgement_replay_member,
            replacement_member: $replacement_member,
            coordinator_restarts: $coordinator_restarts,
            saturation_waves: $saturation_waves,
            held_dkg_member: $held_dkg_member,
            restarted_dkg_member: $restarted_dkg_member
        },
        evidence: {
            proposal_hash: $proposal_hash,
            manifest_hash: $manifest_hash,
            setup_authorization: $setup_authorization,
            transcript_hash: $transcript_hash,
            final_config_digest: $final_config_digest,
            public_config_identical: true
        },
        timing_seconds: {
            bootstrap: $bootstrap_seconds,
            dkg: $dkg_seconds,
            activation: $activation_seconds,
            total: $total_seconds,
            status_latency_max: $status_latency_max_seconds
        },
        requests: {
            joins: $join_requests,
            reviews: $review_requests,
            approvals: $approval_requests,
            statuses: $status_requests,
            failures: $request_failures,
            retries: $retries,
            dkg_status_attempts: $dkg_status_attempts
        },
        resources: {
            max_total_rss_kb: $max_total_rss_kb,
            max_process_rss_kb: $max_process_rss_kb,
            processes: $process_resources
        }
    }' >"$SUMMARY_FILE"

printf 'invitation-based federation DKG setup passed with %s independent operator stores\n' "$MEMBERS"
printf 'redacted setup summary: %s\n' "$SUMMARY_FILE"
