#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BENCHMARK_SCRIPT="$SCRIPT_DIR/benchmark_nocean_federation.sh"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
OUTPUT_ROOT="${OUTPUT_ROOT:-$REPO_ROOT/target/federation-stress-reproductions/$RUN_ID}"
REPETITIONS="${REPETITIONS:-1}"
INCLUDE_SMOKE="${INCLUDE_SMOKE:-1}"
BUILD_CLI="${BUILD_CLI:-1}"

SMOKE_CASES="${SMOKE_CASES:-2:1 4:2}"
MAIN_CASES="${MAIN_CASES:-10:1 20:5 40:10}"
SATURATION_CASES="${SATURATION_CASES:-20:20}"
SMOKE_SETTLE_SECONDS="${SMOKE_SETTLE_SECONDS:-2}"
MEASURED_SETTLE_SECONDS="${MEASURED_SETTLE_SECONDS:-12}"

usage() {
    cat <<'EOF'
Reproduce the 2026-07-13 Nocean federated receive stress-test sequence.

Usage:
  scripts/reproduce_nocean_federation_stress_test.sh

The default sequence is:
  1. Smoke:      CASES="2:1 4:2", settle 2 seconds
  2. Main:       CASES="10:1 20:5 40:10", settle 12 seconds
  3. Saturation: CASES="20:20", settle 12 seconds

Each case is N:parallelism. The underlying benchmark prepares independent
tokens and wallets outside the timed interval, runs parallel receives, and
checks signed five-member convergence before and after each case.

Environment overrides:
  OUTPUT_ROOT                 Private output directory
  REPETITIONS                 Number of complete sequences (default: 1)
  INCLUDE_SMOKE               Run the smoke phase: 0 or 1 (default: 1)
  BUILD_CLI                   Build the federation CLI once: 0 or 1 (default: 1)
  SMOKE_CASES                 Smoke N:parallelism cases
  MAIN_CASES                  Sustained N:parallelism cases
  SATURATION_CASES            Burst N:parallelism cases
  SMOKE_SETTLE_SECONDS        Pre-case smoke settle delay (default: 2)
  MEASURED_SETTLE_SECONDS     Pre-case measured settle delay (default: 12)

All environment settings understood by benchmark_nocean_federation.sh, such as
MINT_URL, SSH_HOST, REMOTE_DIR, CLI, TOKEN_AMOUNT, and PREPARE_PARALLELISM, are
passed through automatically.

Run directories contain bearer tokens and must remain private. An interrupted
or failed run can leave spendable tokens in its token files.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

is_positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
    "") ;;
    *)
        usage >&2
        die "unexpected argument: $1"
        ;;
esac

[[ -x "$BENCHMARK_SCRIPT" ]] \
    || die "benchmark script is missing or not executable: $BENCHMARK_SCRIPT"
is_positive_integer "$REPETITIONS" || die "REPETITIONS must be a positive integer"
[[ "$INCLUDE_SMOKE" == "0" || "$INCLUDE_SMOKE" == "1" ]] \
    || die "INCLUDE_SMOKE must be 0 or 1"
[[ "$BUILD_CLI" == "0" || "$BUILD_CLI" == "1" ]] \
    || die "BUILD_CLI must be 0 or 1"
[[ "$SMOKE_SETTLE_SECONDS" =~ ^[0-9]+$ ]] \
    || die "SMOKE_SETTLE_SECONDS must be a non-negative integer"
[[ "$MEASURED_SETTLE_SECONDS" =~ ^[0-9]+$ ]] \
    || die "MEASURED_SETTLE_SECONDS must be a non-negative integer"

[[ ! -e "$OUTPUT_ROOT" ]] || die "output directory already exists: $OUTPUT_ROOT"
mkdir -p "$OUTPUT_ROOT"
chmod 700 "$OUTPUT_ROOT"

COMBINED_RESULTS="$OUTPUT_ROOT/results.csv"
printf '%s\n' \
    'repetition,phase,case,n,parallelism,successes,failures,elapsed_seconds,successful_tps,committed_tps,p50_seconds,p95_seconds,p99_seconds,max_seconds,operations_before,operations_after,operation_delta' \
    >"$COMBINED_RESULTS"

next_build_cli="$BUILD_CLI"

append_results() {
    local repetition="$1"
    local phase="$2"
    local phase_results="$3"

    awk -F, -v repetition="$repetition" -v phase="$phase" \
        'BEGIN { OFS="," } NR > 1 { print repetition, phase, $0 }' \
        "$phase_results" >>"$COMBINED_RESULTS"
}

run_phase() {
    local repetition="$1"
    local phase="$2"
    local cases="$3"
    local settle_seconds="$4"
    local phase_dir="$5"

    echo
    echo "Repetition $repetition/$REPETITIONS: $phase phase"
    echo "  cases: $cases"
    echo "  settle: ${settle_seconds}s"
    echo "  output: $phase_dir"

    CASES="$cases" \
        SETTLE_SECONDS="$settle_seconds" \
        BUILD_CLI="$next_build_cli" \
        RUN_DIR="$phase_dir" \
        "$BENCHMARK_SCRIPT"

    next_build_cli=0
    append_results "$repetition" "$phase" "$phase_dir/results.csv"
}

echo "Nocean federation stress-test reproduction"
echo "Output root: $OUTPUT_ROOT"
echo "Repetitions: $REPETITIONS"

for repetition in $(seq 1 "$REPETITIONS"); do
    repetition_name="repeat-$(printf '%02d' "$repetition")"
    repetition_dir="$OUTPUT_ROOT/$repetition_name"
    mkdir -p "$repetition_dir"
    chmod 700 "$repetition_dir"

    if [[ "$INCLUDE_SMOKE" == "1" ]]; then
        run_phase \
            "$repetition" \
            smoke \
            "$SMOKE_CASES" \
            "$SMOKE_SETTLE_SECONDS" \
            "$repetition_dir/smoke"
    fi

    run_phase \
        "$repetition" \
        main \
        "$MAIN_CASES" \
        "$MEASURED_SETTLE_SECONDS" \
        "$repetition_dir/main"

    run_phase \
        "$repetition" \
        saturation \
        "$SATURATION_CASES" \
        "$MEASURED_SETTLE_SECONDS" \
        "$repetition_dir/saturation"
done

echo
echo "Reproduction complete."
echo "Combined results: $COMBINED_RESULTS"
column -s, -t "$COMBINED_RESULTS" 2>/dev/null || cat "$COMBINED_RESULTS"
echo
echo "Keep $OUTPUT_ROOT private: it contains wallet databases and bearer tokens."
