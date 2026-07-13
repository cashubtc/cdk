#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

MINT_URL="${MINT_URL:-https://cdk-1.nocean.xyz}"
SSH_HOST="${SSH_HOST:-nocean}"
REMOTE_DIR="${REMOTE_DIR:-cdk-federation}"
CASES="${CASES:-10:1 20:5 20:10 20:20}"
TOKEN_AMOUNT="${TOKEN_AMOUNT:-1}"
FUNDING_BUFFER="${FUNDING_BUFFER:-50}"
PREPARE_PARALLELISM="${PREPARE_PARALLELISM:-12}"
SETTLE_SECONDS="${SETTLE_SECONDS:-12}"
CONVERGENCE_ATTEMPTS="${CONVERGENCE_ATTEMPTS:-90}"
CONVERGENCE_DELAY_SECONDS="${CONVERGENCE_DELAY_SECONDS:-2}"
BUILD_CLI="${BUILD_CLI:-1}"
CLI="${CLI:-$REPO_ROOT/target/release/cdk-cli}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/federation-load-tests/$RUN_ID}"

usage() {
    cat <<'EOF'
Prepare independent Cashu tokens, receive them through isolated wallets with
controlled parallelism, and report end-to-end federation throughput.

Usage:
  scripts/benchmark_nocean_federation.sh

Default benchmark matrix:
  CASES="10:1 20:5 20:10 20:20"

Each case is N:parallelism. Token creation and wallet setup happen outside the
timed interval. The timed interval includes cdk-cli process startup, wallet DB
access, network latency, and the federated receive/swap operation.

The report distinguishes wallet-confirmed TPS from committed journal TPS. Under
overload an operation can commit while the client fails to collect enough
signature shares for a successful response.

Environment overrides:
  CASES                       Space-separated N:parallelism cases
  TOKEN_AMOUNT                Amount in sat per prepared token (default: 1)
  FUNDING_BUFFER              Extra sender balance in sat (default: 50)
  PREPARE_PARALLELISM         Parallel receiver-wallet setup jobs (default: 12)
  SETTLE_SECONDS              Idle delay before each timed case (default: 12)
  BUILD_CLI                   Build the native release CLI first: 0 or 1 (default: 1)
  CLI                         Native federation-enabled cdk-cli path
  MINT_URL                    Wallet-facing URL for the federation
  SSH_HOST                    SSH alias used for config and signed status checks
  REMOTE_DIR                  Remote deployment directory below $HOME
  RUN_DIR                     Output directory

The output directory is private because failed cases can leave unspent bearer
tokens in their token files.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
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

require_command awk
require_command find
require_command scp
require_command seq
require_command sort
require_command ssh
require_command xargs

if [[ "$BUILD_CLI" == "1" ]]; then
    require_command cargo
fi

is_positive_integer "$TOKEN_AMOUNT" || die "TOKEN_AMOUNT must be a positive integer"
[[ "$FUNDING_BUFFER" =~ ^[0-9]+$ ]] || die "FUNDING_BUFFER must be a non-negative integer"
is_positive_integer "$PREPARE_PARALLELISM" || die "PREPARE_PARALLELISM must be positive"
[[ "$SETTLE_SECONDS" =~ ^[0-9]+$ ]] || die "SETTLE_SECONDS must be non-negative"
is_positive_integer "$CONVERGENCE_ATTEMPTS" || die "CONVERGENCE_ATTEMPTS must be positive"
is_positive_integer "$CONVERGENCE_DELAY_SECONDS" \
    || die "CONVERGENCE_DELAY_SECONDS must be positive"
[[ "$BUILD_CLI" == "0" || "$BUILD_CLI" == "1" ]] || die "BUILD_CLI must be 0 or 1"
[[ "$REMOTE_DIR" =~ ^[A-Za-z0-9._/-]+$ ]] || die "REMOTE_DIR contains unsupported characters"
[[ "$REMOTE_DIR" != /* ]] || die "REMOTE_DIR must be relative to the remote home"
[[ "/$REMOTE_DIR/" != *"/../"* ]] || die "REMOTE_DIR must not contain '..' path segments"

total_tokens=0
case_count=0
for benchmark_case in $CASES; do
    [[ "$benchmark_case" =~ ^([1-9][0-9]*):([1-9][0-9]*)$ ]] \
        || die "invalid benchmark case '$benchmark_case'; expected N:parallelism"
    n="${BASH_REMATCH[1]}"
    parallelism="${BASH_REMATCH[2]}"
    (( parallelism <= n )) || die "parallelism must not exceed N in case $benchmark_case"
    total_tokens=$((total_tokens + n))
    case_count=$((case_count + 1))
done
(( case_count > 0 )) || die "CASES must contain at least one benchmark case"

if [[ "$BUILD_CLI" == "1" ]]; then
    echo "Building the current native federation CLI..."
    cargo build --locked --release -p cdk-cli \
        --no-default-features \
        --features federation
fi

[[ -x "$CLI" ]] || die "federation CLI not found or not executable: $CLI"
"$CLI" federation add --help >/dev/null \
    || die "CLI is not built with federation support: $CLI"

[[ ! -e "$RUN_DIR" ]] || die "run directory already exists: $RUN_DIR"
mkdir -p "$RUN_DIR"
chmod 700 "$RUN_DIR"

FUNDING_DIR="$RUN_DIR/funding-wallet"
PUBLIC_CONFIG="$RUN_DIR/federation-public.json"
RESULTS_CSV="$RUN_DIR/results.csv"
mkdir -p "$FUNDING_DIR"
chmod 700 "$FUNDING_DIR"

echo "Copying an immutable public federation config snapshot..."
scp \
    "$SSH_HOST:$REMOTE_DIR/member-1/dkg-final/federation-public.json" \
    "$PUBLIC_CONFIG"
chmod 600 "$PUBLIC_CONFIG"

printf '%s\n' \
    'case,n,parallelism,successes,failures,elapsed_seconds,successful_tps,committed_tps,p50_seconds,p95_seconds,p99_seconds,max_seconds,operations_before,operations_after,operation_delta' \
    >"$RESULTS_CSV"

add_federation() {
    local wallet_dir="$1"

    "$CLI" \
        --work-dir "$wallet_dir" \
        --non-interactive \
        federation add \
        --public-config "$PUBLIC_CONFIG" \
        --local-mint-url "$MINT_URL" \
        --quote-coordinator-member-id 1 \
        --member-request-timeout-secs 90 \
        --member-fallback-timeout-secs 90 \
        --quote-status-member-timeout-secs 90 \
        --fanout-concurrency 5
}

wait_for_convergence() {
    local attempt output=""

    for attempt in $(seq 1 "$CONVERGENCE_ATTEMPTS"); do
        if output="$(
            ssh "$SSH_HOST" \
                "cd ~/$REMOTE_DIR && timeout 30 bin/cdk-mintd federation compare-digests --public-config member-1/dkg-final/federation-public.json --private-config member-1/dkg-final/federation-private.json" \
                2>&1
        )" \
            && grep -q 'converged=true' <<<"$output" \
            && grep -q 'members=1,2,3,4,5 ready=5/5 sync_states=healthy' <<<"$output"
        then
            printf '%s\n' "$output"
            return 0
        fi

        if (( attempt % 10 == 0 )); then
            echo "Still waiting for signed federation convergence ($attempt/$CONVERGENCE_ATTEMPTS)..." >&2
        fi
        sleep "$CONVERGENCE_DELAY_SECONDS"
    done

    printf '%s\n' "$output" >&2
    return 1
}

operation_count_from_status() {
    sed -n 's/.*digest_group operations=\([0-9][0-9]*\).*/\1/p' \
        | head -n 1
}

echo "Importing the federation into the funding wallet..."
add_federation "$FUNDING_DIR" >/dev/null

funding_amount=$((total_tokens * TOKEN_AMOUNT + FUNDING_BUFFER))
echo "Minting $funding_amount sat for $total_tokens benchmark tokens..."
"$CLI" \
    --work-dir "$FUNDING_DIR" \
    --non-interactive \
    --unit sat \
    mint "$MINT_URL" "$funding_amount" \
    --wait-duration 180

prepare_receiver_wallet() {
    local id="$1"
    local wallet_dir="$CASE_DIR/wallets/$id"
    local log="$CASE_DIR/setup-logs/$id.log"

    mkdir -p "$wallet_dir"
    chmod 700 "$wallet_dir"
    if ! add_federation "$wallet_dir" >"$log" 2>&1; then
        echo "receiver wallet setup failed for $id; see $log" >&2
        return 1
    fi
}

receive_one() {
    local token_file="$1"
    local filename id wallet_dir log duration_file token

    filename="$(basename -- "$token_file")"
    id="${filename%.cashu}"
    wallet_dir="$CASE_DIR/wallets/$id"
    log="$CASE_DIR/receive-logs/$id.log"
    token="$(<"$token_file")"
    TIMEFORMAT='%R'

    if { time "$CLI" \
        --work-dir "$wallet_dir" \
        --non-interactive \
        --unit sat \
        receive "$token" >"$log" 2>&1; } \
        2>"$CASE_DIR/durations/$id.seconds"
    then
        touch "$CASE_DIR/success/$id"
    else
        touch "$CASE_DIR/failure/$id"
        return 1
    fi
}

export -f add_federation prepare_receiver_wallet receive_one
export CLI MINT_URL PUBLIC_CONFIG CASE_DIR

case_index=0
global_token_index=0
for benchmark_case in $CASES; do
    case_index=$((case_index + 1))
    n="${benchmark_case%%:*}"
    parallelism="${benchmark_case##*:}"
    case_name="case-$(printf '%02d' "$case_index")-n${n}-p${parallelism}"
    CASE_DIR="$RUN_DIR/$case_name"
    export CASE_DIR

    mkdir -p \
        "$CASE_DIR/tokens" \
        "$CASE_DIR/wallets" \
        "$CASE_DIR/setup-logs" \
        "$CASE_DIR/receive-logs" \
        "$CASE_DIR/durations" \
        "$CASE_DIR/success" \
        "$CASE_DIR/failure"
    chmod 700 "$CASE_DIR" "$CASE_DIR"/*

    echo
    echo "Preparing $n independent $TOKEN_AMOUNT-sat tokens for $case_name..."
    : >"$CASE_DIR/token-files.txt"
    for i in $(seq 1 "$n"); do
        global_token_index=$((global_token_index + 1))
        id="token-$(printf '%06d' "$global_token_index")"
        token_file="$CASE_DIR/tokens/$id.cashu"
        send_output="$(
            "$CLI" \
                --work-dir "$FUNDING_DIR" \
                --non-interactive \
                --unit sat \
                send \
                --mint-url "$MINT_URL" \
                --amount "$TOKEN_AMOUNT"
        )"
        token="$(printf '%s\n' "$send_output" | tail -n 1)"
        case "$token" in
            cashu*) ;;
            *)
                printf '%s\n' "$send_output" >&2
                die "send did not return a Cashu token while preparing $id"
                ;;
        esac
        printf '%s\n' "$token" >"$token_file"
        chmod 600 "$token_file"
        printf '%s\n' "$token_file" >>"$CASE_DIR/token-files.txt"

        if (( i % 10 == 0 || i == n )); then
            echo "  prepared $i/$n tokens"
        fi
    done
    unset token send_output

    echo "Preparing $n isolated receiver wallets with $PREPARE_PARALLELISM setup jobs..."
    if ! sed -n 's/.*\/\(token-[0-9][0-9]*\)\.cashu$/\1/p' "$CASE_DIR/token-files.txt" \
        | xargs -P "$PREPARE_PARALLELISM" -I '{}' \
            bash -c 'prepare_receiver_wallet "$1"' _ '{}'
    then
        die "receiver wallet preparation failed for $case_name"
    fi

    echo "Waiting for token-preparation operations to converge..."
    before_status="$(wait_for_convergence)" \
        || die "federation did not converge before $case_name"
    operations_before="$(printf '%s\n' "$before_status" | operation_count_from_status)"
    [[ -n "$operations_before" ]] || die "could not parse the pre-case operation count"
    printf '%s\n' "$before_status" >"$CASE_DIR/status-before.txt"

    if (( SETTLE_SECONDS > 0 )); then
        echo "Allowing $SETTLE_SECONDS seconds for the federation to return to idle cadence..."
        sleep "$SETTLE_SECONDS"
    fi

    echo "Receiving $n tokens with parallelism $parallelism..."
    TIMEFORMAT='%R'
    set +e
    { time xargs -P "$parallelism" -I '{}' \
        bash -c 'receive_one "$1"' _ '{}' \
        <"$CASE_DIR/token-files.txt" \
        2>"$CASE_DIR/xargs-errors.log"; } \
        2>"$CASE_DIR/batch.seconds"
    batch_status=$?
    set -e

    elapsed_seconds="$(tail -n 1 "$CASE_DIR/batch.seconds")"
    successes="$(find "$CASE_DIR/success" -type f | wc -l | tr -d '[:space:]')"
    failures="$(find "$CASE_DIR/failure" -type f | wc -l | tr -d '[:space:]')"
    [[ "$((successes + failures))" == "$n" ]] \
        || die "case $case_name lost worker results: successes=$successes failures=$failures n=$n"

    if (( successes > 0 )); then
        for success_marker in "$CASE_DIR"/success/*; do
            id="$(basename -- "$success_marker")"
            cat "$CASE_DIR/durations/$id.seconds"
        done | sort -n >"$CASE_DIR/success-durations.sorted"

        latency_stats="$(awk '
            {
                values[NR] = $1
                sum += $1
            }
            END {
                p50 = values[int((NR - 1) * 0.50) + 1]
                p95 = values[int((NR - 1) * 0.95) + 1]
                p99 = values[int((NR - 1) * 0.99) + 1]
                printf "%.3f,%.3f,%.3f,%.3f", p50, p95, p99, values[NR]
            }
        ' "$CASE_DIR/success-durations.sorted")"
    else
        latency_stats="0.000,0.000,0.000,0.000"
    fi
    IFS=',' read -r p50 p95 p99 max_latency <<<"$latency_stats"
    successful_tps="$(awk -v successes="$successes" -v elapsed="$elapsed_seconds" \
        'BEGIN { if (elapsed > 0) printf "%.3f", successes / elapsed; else print "0.000" }')"

    echo "Waiting for post-case convergence..."
    after_status="$(wait_for_convergence)" \
        || die "federation did not converge after $case_name"
    operations_after="$(printf '%s\n' "$after_status" | operation_count_from_status)"
    [[ -n "$operations_after" ]] || die "could not parse the post-case operation count"
    operation_delta=$((operations_after - operations_before))
    committed_tps="$(awk -v operations="$operation_delta" -v elapsed="$elapsed_seconds" \
        'BEGIN { if (elapsed > 0) printf "%.3f", operations / elapsed; else print "0.000" }')"
    printf '%s\n' "$after_status" >"$CASE_DIR/status-after.txt"

    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n' \
        "$case_name" \
        "$n" \
        "$parallelism" \
        "$successes" \
        "$failures" \
        "$elapsed_seconds" \
        "$successful_tps" \
        "$committed_tps" \
        "$p50" \
        "$p95" \
        "$p99" \
        "$max_latency" \
        "$operations_before" \
        "$operations_after" \
        "$operation_delta" \
        >>"$RESULTS_CSV"

    printf 'Result: N=%s parallelism=%s successes=%s failures=%s elapsed=%ss confirmed-TPS=%s committed-TPS=%s p50=%ss p95=%ss p99=%ss operations=+%s\n' \
        "$n" "$parallelism" "$successes" "$failures" "$elapsed_seconds" \
        "$successful_tps" "$committed_tps" "$p50" "$p95" "$p99" "$operation_delta"

    if (( batch_status != 0 )); then
        echo "The parallel runner reported failures; logs remain in $CASE_DIR." >&2
    fi
done

echo
echo "Benchmark complete."
echo "Results: $RESULTS_CSV"
column -s, -t "$RESULTS_CSV" 2>/dev/null || cat "$RESULTS_CSV"
echo
echo "Keep $RUN_DIR private: failed receives may leave spendable bearer tokens."
