#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
RUNNER="$SCRIPT_DIR/run_test_federation_dkg.sh"
OUTPUT_ROOT="${OUTPUT_ROOT:-$REPO_ROOT/target/cdk-federation-setup-matrix}"
MAX_MEMBERS="${MAX_MEMBERS:-64}"
MATRIX_MINTD="${MATRIX_CDK_MINTD_BIN:-$REPO_ROOT/target/release/cdk-mintd}"

run_profile() {
    local name="$1"
    local members="$2"
    local threshold="$3"
    local coordinator_port="$4"
    local member_port_start="$5"
    shift 5

    printf 'running federation setup profile %s (%s members, threshold %s)\n' \
        "$name" "$members" "$threshold"
    env \
        FED_DIR="$OUTPUT_ROOT/$name" \
        SUMMARY_FILE="$OUTPUT_ROOT/$name/setup-summary.json" \
        MEMBERS="$members" \
        THRESHOLD="$threshold" \
        COORDINATOR_PORT="$coordinator_port" \
        MEMBER_PORT_START="$member_port_start" \
        CDK_MINTD_BIN="$MATRIX_MINTD" \
        AMOUNTS=1 \
        STATUS_RETRIES="${STATUS_RETRIES:-2400}" \
        STATUS_SLEEP_SECONDS="${STATUS_SLEEP_SECONDS:-0.25}" \
        "$@" \
        "$RUNNER"
}

if [[ ! -x "$MATRIX_MINTD" ]]; then
    cd "$REPO_ROOT"
    cargo build --release -p cdk-mintd --no-default-features --features federation,sqlite
fi

rm -rf -- "$OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"

run_profile adversarial-3 3 2 18180 18181 \
    JOIN_ORDER=reverse \
    JOIN_DELAY_SECONDS=0.05 \
    ACK_LOSS_MEMBER=2 \
    REPLACE_MISSING_MEMBER=3 \
    RESTART_COORDINATOR=1 \
    SATURATE_COORDINATOR=3 \
    HELD_DKG_MEMBER=3 \
    RESTART_DKG_MEMBER=2

run_profile reordered-5 5 3 18280 18281 \
    JOIN_ORDER=reverse \
    JOIN_DELAY_SECONDS=0.02 \
    ACK_LOSS_MEMBER=4 \
    RESTART_COORDINATOR=1 \
    HELD_DKG_MEMBER=5

run_profile concurrent-11 11 8 18380 18381 \
    SATURATE_COORDINATOR=2 \
    RESTART_DKG_MEMBER=7

maximum_threshold="$(((MAX_MEMBERS * 2) / 3 + 1))"
run_profile maximum "$MAX_MEMBERS" "$maximum_threshold" 18480 18481 \
    DKG_MAX_TICKS=28800 \
    DKG_CEREMONY_TIMEOUT_SECONDS=7200 \
    STATUS_RETRIES=20000

jq -s \
    '{version: 1, result: "passed", profiles: .}' \
    "$OUTPUT_ROOT"/*/setup-summary.json \
    >"$OUTPUT_ROOT/matrix-summary.json"
printf 'federation setup process matrix passed: %s\n' "$OUTPUT_ROOT/matrix-summary.json"
