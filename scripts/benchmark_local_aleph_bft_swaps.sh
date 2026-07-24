#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

MEMBERS="${MEMBERS:-5}"
SIGNATURE_THRESHOLD="${SIGNATURE_THRESHOLD:-$((MEMBERS / 2 + 1))}"
SWAPS="${SWAPS:-10}"
REPEATS="${REPEATS:-3}"
MATRIX="${MATRIX:-500:1 250:1 100:1 250:2 100:1 100:2 100:8 50:1 50:8}"
NETWORK_RTT_MS_VALUES="${NETWORK_RTT_MS_VALUES:-0}"
NETWORK_JITTER_PERCENT="${NETWORK_JITTER_PERCENT:-20}"
BUILD="${BUILD:-1}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/aleph-bft-swap-benchmarks/$RUN_ID}"
TEST_NAME="federation::tests::benchmark_aleph_bft_serial_swaps"

usage() {
    cat <<'EOF'
Run the ignored release-mode AlephBFT steady-state serial-swap benchmark.

Usage:
  scripts/benchmark_local_aleph_bft_swaps.sh

Environment overrides:
  MEMBERS               Federation member count (default: 5)
  SIGNATURE_THRESHOLD   Custody signature threshold (default: majority)
  SWAPS                 Strictly serial measured swaps per run (minimum: 10)
  REPEATS               Fresh runs per unique configuration (minimum: 3)
  MATRIX                Space-separated UNIT_INTERVAL_MS:PUMP_LIMIT entries
  NETWORK_RTT_MS_VALUES Space-separated nominal RTTs in ms (default: 0)
  NETWORK_JITTER_PERCENT Deterministic RTT jitter in percent (default: 20)
  BUILD                 Compile the release benchmark before running: 0 or 1
  RUN_DIR               Private output directory below target/

The default matrix intentionally contains the requested duplicate 100:1 row.
Duplicate settings are run once with REPEATS fresh repetitions, so the default
performs 8 configurations x 3 runs x 10 measured swaps.

Each run creates a fresh in-process federation using the production AlephBFT
adapter, private Axum routes, and SQLite application path. One warm-up swap
absorbs session startup and is excluded. Non-zero network RTT applies
deterministic +/- NETWORK_JITTER_PERCENT jitter and splits each sampled RTT
equally across request and response paths. Tick, recovery, request,
rebroadcast, channel-capacity, and delivery parameters remain unchanged.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

is_positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
}

is_nonnegative_integer() {
    [[ "$1" =~ ^[0-9]+$ ]]
}

command_name="${1:-}"
case "$command_name" in
    "")
        ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        usage >&2
        exit 1
        ;;
esac

for required in cargo jq sed; do
    command -v "$required" >/dev/null 2>&1 \
        || die "required command not found: $required"
done
is_positive_integer "$MEMBERS" || die "MEMBERS must be a positive integer"
((MEMBERS >= 4)) || die "MEMBERS must be at least 4"
is_positive_integer "$SIGNATURE_THRESHOLD" \
    || die "SIGNATURE_THRESHOLD must be a positive integer"
((SIGNATURE_THRESHOLD <= MEMBERS)) \
    || die "SIGNATURE_THRESHOLD must not exceed MEMBERS"
is_positive_integer "$SWAPS" || die "SWAPS must be a positive integer"
((SWAPS >= 10)) || die "SWAPS must be at least 10"
is_positive_integer "$REPEATS" || die "REPEATS must be a positive integer"
((REPEATS >= 3)) || die "REPEATS must be at least 3"
is_nonnegative_integer "$NETWORK_JITTER_PERCENT" \
    || die "NETWORK_JITTER_PERCENT must be a nonnegative integer"
((NETWORK_JITTER_PERCENT <= 100)) \
    || die "NETWORK_JITTER_PERCENT must not exceed 100"
[[ "$BUILD" == "0" || "$BUILD" == "1" ]] || die "BUILD must be 0 or 1"
[[ "$RUN_DIR" == "$REPO_ROOT"/target/* && "$RUN_DIR" != "$REPO_ROOT/target" ]] \
    || die "RUN_DIR must be a child of the workspace target directory"
[[ "/$RUN_DIR/" != *"/../"* ]] || die "RUN_DIR must not contain '..'"

declare -a configurations=()
configuration_seen=" "
for configuration in $MATRIX; do
    [[ "$configuration" =~ ^([1-9][0-9]*):([1-9][0-9]*)$ ]] \
        || die "invalid MATRIX entry: $configuration"
    if [[ "$configuration_seen" != *" $configuration "* ]]; then
        configuration_seen+="$configuration "
        configurations+=("$configuration")
    fi
done
((${#configurations[@]} > 0)) || die "MATRIX contains no configurations"

declare -a network_rtts=()
network_rtt_seen=" "
for network_rtt_ms in $NETWORK_RTT_MS_VALUES; do
    is_nonnegative_integer "$network_rtt_ms" \
        || die "invalid NETWORK_RTT_MS_VALUES entry: $network_rtt_ms"
    if [[ "$network_rtt_seen" != *" $network_rtt_ms "* ]]; then
        network_rtt_seen+="$network_rtt_ms "
        network_rtts+=("$network_rtt_ms")
    fi
done
((${#network_rtts[@]} > 0)) || die "NETWORK_RTT_MS_VALUES contains no values"

mkdir -p "$RUN_DIR/logs"
results_jsonl="$RUN_DIR/results.jsonl"
results_csv="$RUN_DIR/results.csv"
summary_csv="$RUN_DIR/summary.csv"
: >"$results_jsonl"

cd "$REPO_ROOT"
if [[ "$BUILD" == "1" ]]; then
    cargo test --locked --release -p cdk-axum --features federation-bark \
        "$TEST_NAME" --no-run
fi

total_runs=$((${#configurations[@]} * ${#network_rtts[@]} * REPEATS))
completed_runs=0
for network_rtt_ms in "${network_rtts[@]}"; do
    for configuration in "${configurations[@]}"; do
        unit_interval_ms="${configuration%%:*}"
        pump_messages_per_tick="${configuration##*:}"
        for run in $(seq 1 "$REPEATS"); do
            completed_runs=$((completed_runs + 1))
            log="$RUN_DIR/logs/rtt${network_rtt_ms}ms-${unit_interval_ms}ms-pump${pump_messages_per_tick}-run${run}.log"
            echo "[$completed_runs/$total_runs] rtt=${network_rtt_ms}ms interval=${unit_interval_ms}ms pump=${pump_messages_per_tick} run=$run"
            set +e
            CDK_ALEPH_BFT_BENCH_MEMBERS="$MEMBERS" \
            CDK_ALEPH_BFT_BENCH_SIGNATURE_THRESHOLD="$SIGNATURE_THRESHOLD" \
            CDK_ALEPH_BFT_BENCH_UNIT_INTERVAL_MS="$unit_interval_ms" \
            CDK_ALEPH_BFT_BENCH_PUMP_MESSAGES_PER_TICK="$pump_messages_per_tick" \
            CDK_ALEPH_BFT_BENCH_NETWORK_RTT_MS="$network_rtt_ms" \
            CDK_ALEPH_BFT_BENCH_NETWORK_JITTER_PERCENT="$NETWORK_JITTER_PERCENT" \
            CDK_ALEPH_BFT_BENCH_NETWORK_JITTER_SEED="$run" \
            CDK_ALEPH_BFT_BENCH_SWAPS="$SWAPS" \
                cargo test --locked --release -p cdk-axum --features federation-bark \
                    "$TEST_NAME" -- \
                    --ignored --exact --nocapture --test-threads=1 \
                    >"$log" 2>&1
            status=$?
            set -e

            result="$(sed -n 's/^.*ALEPH_BFT_SWAP_BENCH //p' "$log" | tail -n 1)"
            if [[ "$status" -eq 0 && -n "$result" ]] && jq -e . >/dev/null 2>&1 <<<"$result"; then
                jq -c \
                    --argjson run "$run" \
                    '. + {run: $run, status: "ok"}' \
                    <<<"$result" >>"$results_jsonl"
            else
                jq -nc \
                    --argjson members "$MEMBERS" \
                    --argjson signature_threshold "$SIGNATURE_THRESHOLD" \
                    --argjson network_rtt_ms "$network_rtt_ms" \
                    --argjson network_jitter_percent "$NETWORK_JITTER_PERCENT" \
                    --argjson unit_interval_ms "$unit_interval_ms" \
                    --argjson pump_messages_per_tick "$pump_messages_per_tick" \
                    --argjson swaps "$SWAPS" \
                    --argjson run "$run" \
                    --argjson exit_status "$status" \
                    '{
                        members: $members,
                        signature_threshold: $signature_threshold,
                        network_rtt_ms: $network_rtt_ms,
                        network_jitter_percent: $network_jitter_percent,
                        unit_interval_ms: $unit_interval_ms,
                        pump_messages_per_tick: $pump_messages_per_tick,
                        swaps: $swaps,
                        run: $run,
                        status: "failed",
                        exit_status: $exit_status
                    }' >>"$results_jsonl"
            fi
        done
    done
done

jq -sr '
    [
        "network_rtt_ms",
        "network_jitter_percent",
        "unit_interval_ms",
        "pump_messages_per_tick",
        "run",
        "status",
        "startup_ms",
        "steady_state_ms",
        "median_ms",
        "p95_ms",
        "min_ms",
        "max_ms",
        "total_deliveries",
        "application_deliveries",
        "helper_deliveries",
        "delivery_bytes",
        "request_body_bytes",
        "request_body_kbit_per_second",
        "messages_drained",
        "max_outbound_queue_depth",
        "backlogged_pump_ticks",
        "outbound_messages_dropped",
        "retry_messages_drained",
        "max_retry_queue_depth",
        "retry_deliveries_dropped",
        "latencies_ms"
    ],
    (
        .[]
        | [
            .network_rtt_ms,
            .network_jitter_percent,
            .unit_interval_ms,
            .pump_messages_per_tick,
            .run,
            .status,
            .startup_ms,
            .steady_state_ms,
            .median_ms,
            .p95_ms,
            .min_ms,
            .max_ms,
            .total_deliveries,
            .application_deliveries,
            .helper_deliveries,
            .delivery_bytes,
            .request_body_bytes,
            .request_body_kbit_per_second,
            .messages_drained,
            .max_outbound_queue_depth,
            .backlogged_pump_ticks,
            .outbound_messages_dropped,
            .retry_messages_drained,
            .max_retry_queue_depth,
            .retry_deliveries_dropped,
            (.latencies_ms // [] | tojson)
        ]
    ) | @csv
' "$results_jsonl" >"$results_csv"

jq -sr '
    def median:
        sort as $values
        | ($values | length) as $length
        | if $length == 0 then null
          elif $length % 2 == 1 then $values[($length / 2 | floor)]
          else (($values[$length / 2 - 1] + $values[$length / 2]) / 2)
          end;
    [
        "network_rtt_ms",
        "network_jitter_percent",
        "unit_interval_ms",
        "pump_messages_per_tick",
        "successful_runs",
        "failed_runs",
        "median_of_run_medians_ms",
        "max_observed_p95_ms",
        "median_total_deliveries",
        "median_application_deliveries",
        "median_helper_deliveries",
        "median_delivery_bytes",
        "median_request_body_bytes",
        "median_request_body_kbit_per_second",
        "median_steady_state_ms",
        "median_messages_drained",
        "max_outbound_queue_depth",
        "total_backlogged_pump_ticks",
        "total_outbound_drops",
        "total_retry_messages"
    ],
    (
        group_by([
            .network_rtt_ms,
            .network_jitter_percent,
            .unit_interval_ms,
            .pump_messages_per_tick
        ])[]
        | (map(select(.status == "ok"))) as $ok
        | [
            .[0].network_rtt_ms,
            .[0].network_jitter_percent,
            .[0].unit_interval_ms,
            .[0].pump_messages_per_tick,
            ($ok | length),
            (map(select(.status != "ok")) | length),
            ($ok | map(.median_ms) | median),
            ($ok | map(.p95_ms) | max),
            ($ok | map(.total_deliveries) | median),
            ($ok | map(.application_deliveries) | median),
            ($ok | map(.helper_deliveries) | median),
            ($ok | map(.delivery_bytes) | median),
            ($ok | map(.request_body_bytes) | median),
            ($ok | map(.request_body_kbit_per_second) | median),
            ($ok | map(.steady_state_ms) | median),
            ($ok | map(.messages_drained) | median),
            ($ok | map(.max_outbound_queue_depth) | max),
            ($ok | map(.backlogged_pump_ticks) | add),
            ($ok | map(.outbound_messages_dropped) | add),
            ($ok | map(.retry_messages_drained) | add)
        ]
    )
    | @csv
' "$results_jsonl" >"$summary_csv"

echo
echo "Per-run results: $results_csv"
echo "Configuration summary: $summary_csv"
column -s, -t "$summary_csv" 2>/dev/null || cat "$summary_csv"
