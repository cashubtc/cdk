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
PIDS=()

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
trap cleanup EXIT INT TERM

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

if [[ ! -x "$MINTD" ]]; then
    cd "$REPO_ROOT"
    cargo build -p cdk-mintd --no-default-features --features federation,sqlite
fi
for command in curl jq sed cmp; do
    command -v "$command" >/dev/null 2>&1 || die "required command not found: $command"
done
[[ "$MEMBERS" =~ ^[1-9][0-9]*$ ]] || die "MEMBERS must be positive"
[[ "$THRESHOLD" =~ ^[1-9][0-9]*$ ]] || die "THRESHOLD must be positive"
((THRESHOLD <= MEMBERS)) || die "THRESHOLD cannot exceed MEMBERS"

rm -rf -- "$FED_DIR"
mkdir -p "$FED_DIR"

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
    --allow-insecure-localhost \
    >"$creator_dir/create.log"

"$MINTD" federation setup serve \
    --config "$(member_config 1)" \
    --listen "127.0.0.1:$COORDINATOR_PORT" \
    >"$creator_dir/coordinator.log" 2>&1 &
coordinator_pid=$!
PIDS+=("$coordinator_pid")
wait_for_http_listener \
    "http://127.0.0.1:$COORDINATOR_PORT/federation/setup/v1/status" \
    "$coordinator_pid"

for member in $(seq 2 "$MEMBERS"); do
    dir="$(member_dir "$member")"
    mkdir -p "$dir"
    invitation="$creator_dir/state/invitations/member-$member.invite"
    "$MINTD" federation setup join \
        --invitation-file "$invitation" \
        --config "$(member_config "$member")" \
        --state-dir "$(member_state_dir "$member")" \
        --database "$(member_database "$member")" \
        --public-mint-url "$(member_public_url "$member")" \
        --federation-api-url "$(member_federation_url "$member")" \
        --allow-insecure-localhost \
        >"$dir/join.log"
done

proposal_output="$creator_dir/propose.log"
"$MINTD" federation setup propose --config "$(member_config 1)" >"$proposal_output"
proposal_hash="$(sed -n 's/^proposal_hash: //p' "$proposal_output")"
[[ -n "$proposal_hash" ]] || die "setup proposal did not print its hash"

for member in $(seq 1 "$MEMBERS"); do
    dir="$(member_dir "$member")"
    "$MINTD" federation setup review \
        --config "$(member_config "$member")" \
        --json >"$dir/review.json"
    [[ "$(jq -r '.proposal_hash' "$dir/review.json")" == "$proposal_hash" ]] \
        || die "member $member reviewed a different proposal"
    "$MINTD" federation setup approve \
        --config "$(member_config "$member")" \
        --proposal-hash "$proposal_hash" \
        >"$dir/approve.log"
done

for member in $(seq 1 "$MEMBERS"); do
    "$MINTD" federation setup status \
        --config "$(member_config "$member")" \
        --json >"$(member_dir "$member")/certified-status.json"
    [[ "$(jq -r '.participant_phase' "$(member_dir "$member")/certified-status.json")" == "certified" ]] \
        || die "member $member did not persist the certified manifest"
done

kill "$coordinator_pid"
wait "$coordinator_pid" 2>/dev/null || true
PIDS=()

for member in $(seq 1 "$MEMBERS"); do
    dir="$(member_dir "$member")"
    listen="127.0.0.1:$((MEMBER_PORT_START + member - 1))"
    "$MINTD" federation setup serve \
        --config "$(member_config "$member")" \
        --listen "$listen" \
        >"$dir/dkg.log" 2>&1 &
    PIDS+=("$!")
done

for member in $(seq 1 "$MEMBERS"); do
    wait_for_http_listener \
        "$(member_federation_url "$member")/federation/v1/dkg/commitments" \
        "${PIDS[$((member - 1))]}"
done

for ((attempt = 1; attempt <= STATUS_RETRIES; attempt++)); do
    ready=0
    for member in $(seq 1 "$MEMBERS"); do
        status_file="$(member_dir "$member")/dkg-status.json"
        "$MINTD" federation setup status \
            --config "$(member_config "$member")" \
            --json >"$status_file"
        if [[ "$(jq -r '.dkg.finalized_ready // false' "$status_file")" == "true" ]]; then
            ready=$((ready + 1))
        fi
    done
    printf 'DKG members ready: %s/%s\n' "$ready" "$MEMBERS"
    if ((ready == MEMBERS)); then
        break
    fi
    sleep "$STATUS_SLEEP_SECONDS"
done
((ready == MEMBERS)) || die "DKG did not complete before the timeout"

cleanup
PIDS=()

for member in $(seq 1 "$MEMBERS"); do
    "$MINTD" federation setup activate \
        --config "$(member_config "$member")" \
        >"$(member_dir "$member")/activation.env" \
        2>"$(member_dir "$member")/activation-guidance.log"
done

reference_public="$(member_state_dir 1)/activated/federation-public.json"
for member in $(seq 1 "$MEMBERS"); do
    cmp -s "$reference_public" \
        "$(member_state_dir "$member")/activated/federation-public.json" \
        || die "member $member activated a different public config"
done

printf 'invitation-based federation DKG setup passed with %s independent operator stores\n' "$MEMBERS"
