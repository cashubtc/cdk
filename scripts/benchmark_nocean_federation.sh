#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

MINT_URL="${MINT_URL:-}"
SSH_HOST="${SSH_HOST:-nocean}"
REMOTE_DIR="${REMOTE_DIR:-cdk-federation}"
LOCAL_FEDERATION_DIR="${LOCAL_FEDERATION_DIR:-}"
EXPECTED_MEMBERS="${EXPECTED_MEMBERS:-5}"
CASES="${CASES:-10:1 20:5 20:10 20:20}"
TOKEN_AMOUNT="${TOKEN_AMOUNT:-1}"
FUNDING_BUFFER="${FUNDING_BUFFER:-50}"
PREPARE_PARALLELISM="${PREPARE_PARALLELISM:-12}"
SETTLE_SECONDS="${SETTLE_SECONDS:-12}"
IDLE_CPU_SECONDS="${IDLE_CPU_SECONDS:-30}"
PROFILE_HOOK="${PROFILE_HOOK:-}"
CONVERGENCE_ATTEMPTS="${CONVERGENCE_ATTEMPTS:-90}"
CONVERGENCE_DELAY_SECONDS="${CONVERGENCE_DELAY_SECONDS:-2}"
BUILD_CLI="${BUILD_CLI:-1}"
CLI="${CLI:-$REPO_ROOT/target/release/cdk-cli}"
MINTD="${MINTD:-$REPO_ROOT/target/debug/cdk-mintd}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/federation-load-tests/$RUN_ID}"
RUNTIME_SNAPSHOT_HEADER='member,pid,cpu_seconds,rss_kb,log_lines,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified'
DATABASE_SNAPSHOT_HEADER='member,checkpoint_rows,checkpoint_signature_rows,checkpoint_quorum_rows,finalized_unit_rows,finalized_item_rows,journal_next_index,journal_order_digest,finalized_next_item_index,finalized_item_digest,application_next_index,application_digest,latest_audited_materialized_state_digest'
PERFORMANCE_WINDOW_HEADER='metric,labels,start_value,end_value,delta'
RESULTS_HEADER='case,n,parallelism,token_creation_seconds,wallet_preparation_seconds,token_preparation_seconds,profile_hook,successes,failures,elapsed_seconds,successful_tps,immediate_committed_tps,p50_seconds,p95_seconds,p99_seconds,max_seconds,operations_before,operations_at_window_end,immediate_operation_delta,eventual_operations_after,eventual_operation_delta,late_operation_delta,observation_seconds,cpu_seconds_total,cpu_millis_per_member_second,max_end_rss_kb,log_lines,log_lines_per_member_second,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified,idle_observation_seconds,idle_cpu_seconds_total,idle_cpu_millis_per_member_second,idle_max_end_rss_kb,database_status,new_retained_checkpoints,new_retained_checkpoint_signer_pairs,new_retained_checkpoint_quorums,new_retained_finalized_units,new_retained_finalized_items,persisted_state_match,cached_audited_materialized_digest_match,current_materialized_digest_match,performance_metrics_status,signing_share_seconds_sum,signing_share_count'
RESULTS_EXPECTED_FIELDS=51
PROFILE_HOOK_ACTIVE=0
ACTIVE_PROFILE_CASE=""
ACTIVE_PROFILE_DIR=""

usage() {
    cat <<'EOF'
Prepare independent Cashu tokens, receive them through isolated wallets with
controlled parallelism, and report end-to-end federation throughput.

Usage:
  scripts/benchmark_nocean_federation.sh
  scripts/benchmark_nocean_federation.sh --self-test

Set LOCAL_FEDERATION_DIR to a running generated federation directory to use
local member files, PID files, and loopback Prometheus instead of SSH/SCP.

Default benchmark matrix:
  CASES="10:1 20:5 20:10 20:20"

Each case is N:parallelism. Token creation and wallet setup happen outside the
receive interval and are timed separately. The receive interval includes
cdk-cli process startup, wallet DB access, network latency, and the federated
receive/swap operation.

The report distinguishes wallet-confirmed TPS, operations committed by the end
of the receive window, and operations that commit later during quiescence. The
committed TPS numerator is always the immediate window-end delta.

The same observation window reports per-member and aggregate process CPU, RSS,
log rate, sync-state transitions, checkpoint work, bounded performance counters,
and labelled histogram sum/count deltas. Local mode additionally uses live,
read-only SQLite transactions to report newly retained logical checkpoint,
signature, quorum, finalized-unit, and finalized-item keys. Journal, finalized,
and application state must converge; the cached audited materialized-state
digest is diagnostic, while an exact current-state digest comparison is a
separate post-run gate. The idle CPU window is measured after run quiescence.
Remote mode never copies or queries private databases and records those fields
as N/A.

Environment overrides:
  CASES                       Space-separated N:parallelism cases
  TOKEN_AMOUNT                Amount in sat per prepared token (default: 1)
  FUNDING_BUFFER              Extra sender balance in sat (default: 50)
  PREPARE_PARALLELISM         Parallel receiver-wallet setup jobs (default: 12)
  SETTLE_SECONDS              Idle delay before each timed case (default: 12)
  IDLE_CPU_SECONDS            Explicit post-run idle CPU window (default: 30)
  PROFILE_HOOK                Optional executable invoked as start/stop CASE CASE_DIR
  BUILD_CLI                   Build the native release CLI first: 0 or 1 (default: 1)
  CLI                         Native federation-enabled cdk-cli path
  MINT_URL                    Wallet-facing URL for the federation
  SSH_HOST                    SSH alias used for config and signed status checks
  REMOTE_DIR                  Remote deployment directory below $HOME
  LOCAL_FEDERATION_DIR        Generated local federation directory; disables SSH/SCP
  EXPECTED_MEMBERS            Members required in each metrics snapshot (default: 5)
  RUN_DIR                     Output directory

The output directory is private because failed cases can leave unspent bearer
tokens in their token files. Local database key sets are deleted after aggregate
set differences are computed; checkpoint IDs never become CSV labels or leave
the private run directory.
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

operation_count_from_status() {
    sed -n 's/^member [0-9][0-9]*:.* operations=\([0-9][0-9]*\).*/\1/p' \
        | head -n 1
}

minimum_operation_count_from_status() {
    sed -n 's/^member [0-9][0-9]*:.* operations=\([0-9][0-9]*\).*/\1/p' \
        | sort -n \
        | head -n 1
}

aggregate_runtime_window() {
    local start_snapshot="$1"
    local end_snapshot="$2"
    local duration_seconds="$3"
    local expected_members="$4"
    local member_output="$5"

    awk -F, \
        -v duration="$duration_seconds" \
        -v expected_members="$expected_members" \
        -v member_output="$member_output" \
        -v snapshot_header="$RUNTIME_SNAPSHOT_HEADER" '
        BEGIN {
            OFS = ","
            print "member,cpu_seconds,end_rss_kb,log_lines,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified" > member_output
        }
        FNR == NR {
            if (FNR == 1) {
                if ($0 != snapshot_header) invalid = 1
                next
            }
            if ($1 in start_seen) invalid = 1
            start_seen[$1] = 1
            start_pid[$1] = $2
            for (field = 3; field <= 12; field++) {
                start[$1, field] = $field
            }
            start_members += 1
            next
        }
        FNR == 1 {
            if ($0 != snapshot_header) invalid = 1
            next
        }
        {
            member = $1
            if (member in end_seen) invalid = 1
            end_seen[member] = 1
            if (!(member in start_seen) || $2 != start_pid[member]) invalid = 1
            for (field = 3; field <= 12; field++) {
                if (field != 4 && $field < start[member, field]) invalid = 1
            }
            cpu = $3 - start[member, 3]
            logs = $5 - start[member, 5]
            sync = $6 - start[member, 6]
            scheduled = $7 - start[member, 7]
            recovery = $8 - start[member, 8]
            quorums = $9 - start[member, 9]
            promoted = $10 - start[member, 10]
            catch_up = $11 - start[member, 11]
            signatures = $12 - start[member, 12]
            printf "%s,%.2f,%d,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f\n",
                member, cpu, $4, logs, sync, scheduled, recovery, quorums,
                promoted, catch_up, signatures >> member_output
            total_cpu += cpu
            total_logs += logs
            total_sync += sync
            total_scheduled += scheduled
            total_recovery += recovery
            total_quorums += quorums
            total_promoted += promoted
            total_catch_up += catch_up
            total_signatures += signatures
            if ($4 > max_rss) max_rss = $4
            end_members += 1
        }
        END {
            if (duration <= 0 || invalid || start_members != expected_members || end_members != expected_members) exit 1
            printf "%d,%.2f,%.3f,%d,%.0f,%.3f,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f,%.0f\n",
                duration, total_cpu, (total_cpu * 1000) / (duration * end_members),
                max_rss, total_logs, total_logs / (duration * end_members),
                total_sync, total_scheduled, total_recovery, total_quorums,
                total_promoted, total_catch_up, total_signatures
        }
    ' "$start_snapshot" "$end_snapshot"
}

parse_federation_performance_metrics() {
    awk '
        function normalized_labels(labels, count, parts, idx, label, result) {
            count = split(labels, parts, ",")
            for (idx = 1; idx <= count; idx++) {
                label = parts[idx]
                gsub(/^[[:space:]]+|[[:space:]]+$/, "", label)
                if (label ~ /^member="[^"]*"$/ || label == "") continue
                if (result != "") result = result ";"
                result = result label
            }
            if (result == "") return "none"
            return result
        }
        /^[#]/ || NF < 2 { next }
        {
            sample = $1
            value = $2
            metric = sample
            labels = ""
            brace = index(sample, "{")
            if (brace > 0) {
                metric = substr(sample, 1, brace - 1)
                labels = substr(sample, brace + 1, length(sample) - brace - 1)
            }
            performance = metric ~ /^cdk_federation_performance_/ \
                && (metric ~ /_total$/ || metric ~ /_sum$/ || metric ~ /_count$/)
            signing = metric == "cdk_federation_signing_share_latency_seconds_sum" \
                || metric == "cdk_federation_signing_share_latency_seconds_count"
            if (performance || signing) {
                printf "%s\t%s\t%s\n", metric, normalized_labels(labels), value
            }
        }
    '
}

capture_local_performance_snapshot() {
    local output="$1"
    local member port metrics

    : >"$output.rows"
    for member in $(seq 1 "$EXPECTED_MEMBERS"); do
        port="$(local_runtime_prometheus_port "$member")"
        [[ "$port" =~ ^[1-9][0-9]*$ ]] \
            || die "local member $member config has no Prometheus port"
        metrics="$(curl --max-time 10 -fsS "http://127.0.0.1:$port/metrics")" \
            || die "local member $member Prometheus performance metrics are unavailable"
        printf '%s\n' "$metrics" \
            | parse_federation_performance_metrics >>"$output.rows"
    done
    printf 'metric\tlabels\tvalue\n' >"$output"
    awk -F '\t' '
        {
            key = $1 SUBSEP $2
            value[key] += $3
            metric[key] = $1
            labels[key] = $2
        }
        END {
            for (key in value) printf "%s\t%s\t%.9f\n", metric[key], labels[key], value[key]
        }
    ' "$output.rows" | sort >>"$output"
    rm -f "$output.rows"
}

aggregate_performance_window() {
    local start_snapshot="$1"
    local end_snapshot="$2"
    local output="$3"

    printf '%s\n' "$PERFORMANCE_WINDOW_HEADER" >"$output"
    if ! awk -F '\t' '
        function csv_labels(labels, escaped) {
            escaped = labels
            gsub(/"/, "\"\"", escaped)
            return "\"" escaped "\""
        }
        FNR == NR {
            if (FNR == 1) next
            key = $1 SUBSEP $2
            if (key in start) invalid = 1
            start[key] = $3 + 0
            metric[key] = $1
            labels[key] = $2
            keys[key] = 1
            next
        }
        FNR == 1 { next }
        {
            key = $1 SUBSEP $2
            if (key in finish) invalid = 1
            finish[key] = $3 + 0
            metric[key] = $1
            labels[key] = $2
            keys[key] = 1
        }
        END {
            for (key in keys) {
                start_value = (key in start) ? start[key] : 0
                end_value = (key in finish) ? finish[key] : 0
                delta = end_value - start_value
                if (delta < -0.000000001) invalid = 1
                printf "%s,%s,%.9f,%.9f,%.9f\n",
                    metric[key], csv_labels(labels[key]),
                    start_value, end_value, delta
            }
            if (invalid) exit 1
        }
    ' "$start_snapshot" "$end_snapshot" >"$output.rows"
    then
        rm -f "$output.rows"
        return 1
    fi
    sort "$output.rows" >>"$output"
    rm -f "$output.rows"
}

performance_window_delta() {
    local window="$1"
    local metric_pattern="$2"

    awk -F, -v metric_pattern="$metric_pattern" '
        $1 ~ metric_pattern {
            value += $5
            found = 1
        }
        END {
            if (found) printf "%.9f\n", value
            else print "N/A"
        }
    ' "$window"
}

performance_snapshot_has_required_families() {
    local snapshot="$1"

    awk -F '\t' '
        $1 == "cdk_federation_performance_work_total" { work = 1 }
        $1 == "cdk_federation_performance_operation_duration_seconds_sum" { operation_sum = 1 }
        $1 == "cdk_federation_performance_operation_duration_seconds_count" { operation_count = 1 }
        END { exit !(work && operation_sum && operation_count) }
    ' "$snapshot"
}

capture_local_database_snapshot() {
    local output="$1"
    local key_output="$2"
    local member database raw_rows row tab

    tab=$'\t'
    printf '%s\n' "$DATABASE_SNAPSHOT_HEADER" >"$output"
    : >"$key_output.unsorted"
    chmod 600 "$output" "$key_output.unsorted"

    for member in $(seq 1 "$EXPECTED_MEMBERS"); do
        database="$LOCAL_FEDERATION_DIR/member-$member/work/cdk-mintd.sqlite"
        if [[ ! -f "$database" ]]; then
            echo "local member $member SQLite database is unavailable" >&2
            rm -f "$key_output.unsorted"
            return 1
        fi
        raw_rows="$output.member-$member.rows"
        if ! sqlite3 -batch -readonly -noheader -separator "$tab" \
            -cmd '.timeout 5000' \
            "$database" \
            "PRAGMA query_only = ON;
             BEGIN;
             SELECT 'S',
                    $member,
                    (SELECT COUNT(*) FROM federation_checkpoints),
                    (SELECT COUNT(*) FROM federation_checkpoint_signatures),
                    (SELECT COUNT(*) FROM federation_checkpoint_quorums),
                    (SELECT COUNT(*) FROM (
                        SELECT session, unit_finalization_index
                        FROM federation_aleph_bft_finalized_items
                        GROUP BY session, unit_finalization_index
                    )),
                    (SELECT COUNT(*) FROM federation_aleph_bft_finalized_items),
                    journal_next_index,
                    journal_order_digest,
                    finalized_next_item_index,
                    finalized_item_digest,
                    application_next_index,
                    application_digest,
                    COALESCE(latest_materialized_state_digest, 'N/A')
             FROM federation_checkpoint_state
             WHERE state_key = 'active';
             SELECT 'K', durable_key
             FROM (
                 SELECT 'C|' || checkpoint_id AS durable_key
                 FROM federation_checkpoints
                 UNION ALL
                 SELECT 'S|' || checkpoint_id || '|' || signer_member_id
                 FROM federation_checkpoint_signatures
                 UNION ALL
                 SELECT 'Q|' || checkpoint_id
                 FROM federation_checkpoint_quorums
                 UNION ALL
                 SELECT 'U|' || session || '|' || unit_finalization_index
                 FROM federation_aleph_bft_finalized_items
                 GROUP BY session, unit_finalization_index
                 UNION ALL
                 SELECT 'I|' || session || '|' || unit_finalization_index || '|' || item_index_in_unit
                 FROM federation_aleph_bft_finalized_items
             );
             COMMIT;" >"$raw_rows"
        then
            echo "could not inspect the live read-only SQLite state for member $member" >&2
            rm -f "$raw_rows" "$key_output.unsorted"
            return 1
        fi
        chmod 600 "$raw_rows"
        if ! awk -F '\t' '$1 != "S" && $1 != "K" { invalid = 1 } END { exit invalid }' \
            "$raw_rows"
        then
            echo "member $member SQLite snapshot returned an unexpected row" >&2
            rm -f "$raw_rows" "$key_output.unsorted"
            return 1
        fi
        row="$(awk -F '\t' '
            $1 == "S" {
                seen += 1
                for (field = 2; field <= NF; field++) {
                    if (field > 2) printf ","
                    printf "%s", $field
                }
                printf "\n"
            }
            END { if (seen != 1) exit 1 }
        ' "$raw_rows")" || {
            echo "member $member SQLite snapshot has no unique active checkpoint state" >&2
            rm -f "$raw_rows" "$key_output.unsorted"
            return 1
        }
        printf '%s\n' "$row" >>"$output"
        awk -F '\t' '$1 == "K" { print $2 }' "$raw_rows" >>"$key_output.unsorted"
        rm -f "$raw_rows"
    done

    sort -u "$key_output.unsorted" >"$key_output"
    chmod 600 "$output" "$key_output"
    rm -f "$key_output.unsorted"
}

database_summary_has_matching_persisted_state() {
    local snapshot="$1"
    local expected_members="$2"

    awk -F, -v expected_members="$expected_members" \
        -v expected_header="$DATABASE_SNAPSHOT_HEADER" '
        NR == 1 {
            if ($0 != expected_header) invalid = 1
            next
        }
        NR == 2 {
            for (field = 5; field <= 12; field++) base[field] = $field
        }
        NR > 2 {
            # Checkpoint/signature/quorum row counts are member-local evidence
            # stores and need not match. Finalized history and current persisted
            # journal/finalized/application digests must match. The cached audit
            # in field 13 can legitimately trail or name a different candidate.
            for (field = 5; field <= 12; field++) {
                if ($field != base[field]) invalid = 1
            }
        }
        END {
            if (invalid || NR - 1 != expected_members) exit 1
        }
    ' "$snapshot"
}

cached_audited_materialized_digest_status() {
    local snapshot="$1"

    awk -F, '
        NR == 1 { next }
        $13 == "" || $13 == "N/A" { missing = 1; next }
        !seen { base = $13; seen = 1; next }
        $13 != base { mismatch = 1 }
        END {
            if (missing || !seen) print "N/A"
            else if (mismatch) print "0"
            else print "1"
        }
    ' "$snapshot"
}

capture_converged_local_database_snapshot() {
    local output="$1"
    local key_output="$2"
    local attempt

    for attempt in $(seq 1 "$CONVERGENCE_ATTEMPTS"); do
        if capture_local_database_snapshot "$output" "$key_output" \
            && database_summary_has_matching_persisted_state \
                "$output" "$EXPECTED_MEMBERS"
        then
            return 0
        fi
        rm -f "$output" "$key_output" "$key_output.unsorted" "$output".member-*.rows
        if (( attempt < CONVERGENCE_ATTEMPTS )); then
            sleep "$CONVERGENCE_DELAY_SECONDS"
        fi
    done
    return 1
}

database_window_counts() {
    local start_keys="$1"
    local end_keys="$2"

    awk -F '|' '
        FILENAME == ARGV[1] {
            before[$0] = 1
            next
        }
        !($0 in before) { count[$1] += 1 }
        END {
            printf "%d,%d,%d,%d,%d\n", count["C"], count["S"],
                count["Q"], count["U"], count["I"]
        }
    ' "$start_keys" "$end_keys"
}

run_self_test() {
    local temp_dir start_snapshot end_snapshot member_output summary
    local parsed_metrics performance_start performance_end performance_window
    local database_summary database_keys_before database_keys_after database_counts
    temp_dir="$(mktemp -d)"
    start_snapshot="$temp_dir/start.csv"
    end_snapshot="$temp_dir/end.csv"
    member_output="$temp_dir/members.csv"
    printf '%s\n' \
        "$RUNTIME_SNAPSHOT_HEADER" \
        '1,100,10.00,100,50,1,2,0,1,0,0,4' \
        '2,200,20.00,200,70,0,1,0,0,0,0,2' >"$start_snapshot"
    printf '%s\n' \
        "$RUNTIME_SNAPSHOT_HEADER" \
        '1,100,11.50,110,55,2,3,0,2,1,1,6' \
        '2,200,22.50,220,77,2,3,1,1,0,1,5' >"$end_snapshot"
    summary="$(aggregate_runtime_window \
        "$start_snapshot" "$end_snapshot" 10 2 "$member_output")" \
        || die "runtime metrics aggregation self-test failed"
    [[ "$summary" == '10,4.00,200.000,220,12,0.600,3,3,1,2,1,2,5' ]] \
        || die "runtime metrics aggregation self-test produced an unexpected summary"
    grep -qx '1,1.50,110,5,1,1,0,1,1,1,2' "$member_output" \
        || die "runtime metrics aggregation self-test produced an unexpected member row"
    [[ "$(printf '%s\n' \
        'member 1: sync=healthy sync_local_operations=none operations=7 checkpoint_lag_operations=0' \
        | operation_count_from_status)" == "7" ]] \
        || die "signed status operation parser self-test failed"

    parsed_metrics="$temp_dir/parsed-metrics.tsv"
    printf '%s\n' \
        'cdk_federation_performance_work_total{member="1",work="config_validation"} 2' \
        'cdk_federation_performance_operation_duration_seconds_sum{operation="checkpoint_capture",outcome="success"} 0.5' \
        'cdk_federation_performance_operation_duration_seconds_count{operation="checkpoint_capture",outcome="success"} 2' \
        'cdk_federation_performance_operation_duration_seconds_bucket{operation="checkpoint_capture",outcome="success",le="1"} 2' \
        | parse_federation_performance_metrics >"$parsed_metrics"
    grep -Fxq $'cdk_federation_performance_operation_duration_seconds_sum\toperation="checkpoint_capture";outcome="success"\t0.5' \
        "$parsed_metrics" \
        || die "performance metric parser did not normalize production operation labels"
    performance_snapshot_has_required_families "$parsed_metrics" \
        || die "production performance metric families were not recognized"

    performance_start="$temp_dir/performance-start.tsv"
    performance_end="$temp_dir/performance-end.tsv"
    performance_window="$temp_dir/performance-window.csv"
    printf '%s\n' \
        $'metric\tlabels\tvalue' \
        $'cdk_federation_performance_work_total\twork="config_validation"\t5' \
        $'cdk_federation_performance_operation_duration_seconds_sum\toperation="checkpoint_capture";outcome="success"\t0.5' \
        $'cdk_federation_performance_operation_duration_seconds_count\toperation="checkpoint_capture";outcome="success"\t2' \
        >"$performance_start"
    printf '%s\n' \
        $'metric\tlabels\tvalue' \
        $'cdk_federation_performance_work_total\twork="config_validation"\t12' \
        $'cdk_federation_performance_operation_duration_seconds_sum\toperation="checkpoint_capture";outcome="success"\t1.25' \
        $'cdk_federation_performance_operation_duration_seconds_count\toperation="checkpoint_capture";outcome="success"\t5' \
        >"$performance_end"
    aggregate_performance_window \
        "$performance_start" "$performance_end" "$performance_window" \
        || die "performance metrics aggregation self-test failed"
    [[ "$(performance_window_delta \
        "$performance_window" '^cdk_federation_performance_operation_duration_seconds_sum$')" \
        == "0.750000000" ]] \
        || die "performance operation histogram self-test produced an unexpected delta"
    awk -F, 'NR > 1 && NF != 5 { invalid = 1 } END { exit invalid }' \
        "$performance_window" \
        || die "performance window did not preserve its five-column CSV schema"

    database_summary="$temp_dir/database-summary.csv"
    printf '%s\n' \
        "$DATABASE_SNAPSHOT_HEADER" \
        '1,2,3,1,4,8,10,journal,8,finalized,10,application,materialized' \
        '2,2,4,1,4,8,10,journal,8,finalized,10,application,N/A' \
        >"$database_summary"
    database_summary_has_matching_persisted_state "$database_summary" 2 \
        || die "database convergence self-test rejected equal persisted state with local audit differences"
    [[ "$(cached_audited_materialized_digest_status "$database_summary")" == "N/A" ]] \
        || die "absent cached materialized audits were incorrectly reported as matching"
    printf '%s\n' \
        "$DATABASE_SNAPSHOT_HEADER" \
        '1,2,3,1,4,8,10,journal,8,finalized,10,application,materialized' \
        '2,2,3,1,4,8,10,different,8,finalized,10,application,materialized' \
        >"$database_summary"
    if database_summary_has_matching_persisted_state "$database_summary" 2; then
        die "database convergence self-test accepted a digest mismatch"
    fi

    database_keys_before="$temp_dir/database-before.keys"
    database_keys_after="$temp_dir/database-after.keys"
    : >"$database_keys_before"
    printf '%s\n' 'C|new' 'S|new|1' 'Q|new' 'U|1|2' 'I|1|2|0' \
        >"$database_keys_after"
    database_counts="$(database_window_counts \
        "$database_keys_before" "$database_keys_after")"
    [[ "$database_counts" == '1,1,1,1,1' ]] \
        || die "database empty-before key self-test produced $database_counts"

    [[ "$(awk -F, '{ print NF }' <<<"$RESULTS_HEADER")" == "$RESULTS_EXPECTED_FIELDS" ]] \
        || die "results CSV schema self-test expected $RESULTS_EXPECTED_FIELDS columns"
    rm -rf "$temp_dir"
    echo "Runtime, performance, and durable-state self-tests passed."
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
    --self-test)
        require_command awk
        require_command grep
        require_command mktemp
        require_command sort
        run_self_test
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
require_command grep
require_command seq
require_command sort
require_command xargs

if [[ -z "$LOCAL_FEDERATION_DIR" ]]; then
    require_command scp
    require_command ssh
else
    require_command curl
    require_command ps
    require_command sqlite3
    [[ -d "$LOCAL_FEDERATION_DIR" ]] \
        || die "LOCAL_FEDERATION_DIR does not exist: $LOCAL_FEDERATION_DIR"
    [[ -x "$MINTD" ]] || die "local federation mint daemon is not executable: $MINTD"
fi

if [[ "$BUILD_CLI" == "1" ]]; then
    require_command cargo
fi

is_positive_integer "$TOKEN_AMOUNT" || die "TOKEN_AMOUNT must be a positive integer"
[[ "$FUNDING_BUFFER" =~ ^[0-9]+$ ]] || die "FUNDING_BUFFER must be a non-negative integer"
is_positive_integer "$PREPARE_PARALLELISM" || die "PREPARE_PARALLELISM must be positive"
is_positive_integer "$EXPECTED_MEMBERS" || die "EXPECTED_MEMBERS must be positive"
[[ "$SETTLE_SECONDS" =~ ^[0-9]+$ ]] || die "SETTLE_SECONDS must be non-negative"
is_positive_integer "$IDLE_CPU_SECONDS" || die "IDLE_CPU_SECONDS must be positive"
is_positive_integer "$CONVERGENCE_ATTEMPTS" || die "CONVERGENCE_ATTEMPTS must be positive"
is_positive_integer "$CONVERGENCE_DELAY_SECONDS" \
    || die "CONVERGENCE_DELAY_SECONDS must be positive"
[[ "$BUILD_CLI" == "0" || "$BUILD_CLI" == "1" ]] || die "BUILD_CLI must be 0 or 1"
if [[ -n "$PROFILE_HOOK" ]]; then
    [[ -x "$PROFILE_HOOK" ]] || die "PROFILE_HOOK is not executable: $PROFILE_HOOK"
fi
if [[ -z "$LOCAL_FEDERATION_DIR" ]]; then
    [[ "$REMOTE_DIR" =~ ^[A-Za-z0-9._/-]+$ ]] \
        || die "REMOTE_DIR contains unsupported characters"
    [[ "$REMOTE_DIR" != /* ]] || die "REMOTE_DIR must be relative to the remote home"
    [[ "/$REMOTE_DIR/" != *"/../"* ]] \
        || die "REMOTE_DIR must not contain '..' path segments"
    MINT_URL="${MINT_URL:-https://cdk-1.nocean.xyz}"
else
    MINT_URL="${MINT_URL:-$(awk -F' = ' '/^url = / { gsub("\"", "", $2); print $2; exit }' "$LOCAL_FEDERATION_DIR/member-1/config.toml")}"
    [[ -n "$MINT_URL" ]] || die "could not read the local member-1 mint URL"
fi

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
if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
    cp "$LOCAL_FEDERATION_DIR/federation-public.json" "$PUBLIC_CONFIG"
else
    scp \
        "$SSH_HOST:$REMOTE_DIR/member-1/setup-state/activated/federation-public.json" \
        "$PUBLIC_CONFIG"
fi
chmod 600 "$PUBLIC_CONFIG"

printf '%s\n' \
    "$RESULTS_HEADER" \
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
        if output="$(query_converged_status 2>&1)" \
            && [[ "$(grep -c '^member ' <<<"$output")" == "$EXPECTED_MEMBERS" ]]
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

wait_for_quiescence() {
    local attempt output="" current_operations previous_operations=""

    for attempt in $(seq 1 "$CONVERGENCE_ATTEMPTS"); do
        if output="$(query_converged_status 2>&1)" \
            && [[ "$(grep -c '^member ' <<<"$output")" == "$EXPECTED_MEMBERS" ]]
        then
            current_operations="$(printf '%s\n' "$output" | operation_count_from_status)"
            if [[ -n "$current_operations" \
                && "$current_operations" == "$previous_operations" ]]
            then
                printf '%s\n' "$output"
                return 0
            fi
            previous_operations="$current_operations"
        else
            previous_operations=""
        fi

        if (( attempt % 10 == 0 )); then
            echo "Still waiting for a stable federation operation height ($attempt/$CONVERGENCE_ATTEMPTS)..." >&2
        fi
        sleep "$CONVERGENCE_DELAY_SECONDS"
    done

    printf '%s\n' "$output" >&2
    return 1
}

query_observed_status() {
    if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
        "$MINTD" federation status \
            --public-config "$LOCAL_FEDERATION_DIR/federation-public.json" \
            --private-config "$LOCAL_FEDERATION_DIR/member-1/federation-private.json"
        return
    fi
    ssh "$SSH_HOST" \
        "cd ~/$REMOTE_DIR && timeout 30 bin/cdk-mintd federation status --public-config member-1/setup-state/activated/federation-public.json --private-config member-1/setup-state/activated/federation-private.json"
}

query_converged_status() {
    local status_args=(
        federation status
        --require-ready
        --require-converged
        --max-operation-lag 0
        --require-no-digest-alerts
        --require-no-payment-observation-alerts
        --require-no-invalid-operation-alerts
    )
    if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
        "$MINTD" "${status_args[@]}" \
            --public-config "$LOCAL_FEDERATION_DIR/federation-public.json" \
            --private-config "$LOCAL_FEDERATION_DIR/member-1/federation-private.json"
        return
    fi
    ssh "$SSH_HOST" \
        "cd ~/$REMOTE_DIR && timeout 30 bin/cdk-mintd federation status --public-config member-1/setup-state/activated/federation-public.json --private-config member-1/setup-state/activated/federation-private.json --require-ready --require-converged --max-operation-lag 0 --require-no-digest-alerts --require-no-payment-observation-alerts --require-no-invalid-operation-alerts"
}

wait_for_local_materialized_digest_convergence() {
    local output="$1"
    local attempt temporary_output="$output.pending"

    for attempt in $(seq 1 "$CONVERGENCE_ATTEMPTS"); do
        if "$MINTD" federation compare-digests \
            --public-config "$LOCAL_FEDERATION_DIR/federation-public.json" \
            --private-config "$LOCAL_FEDERATION_DIR/member-1/federation-private.json" \
            >"$temporary_output" 2>&1
        then
            chmod 600 "$temporary_output"
            mv "$temporary_output" "$output"
            return 0
        fi
        chmod 600 "$temporary_output" 2>/dev/null || true
        if (( attempt < CONVERGENCE_ATTEMPTS )); then
            sleep "$CONVERGENCE_DELAY_SECONDS"
        fi
    done
    mv "$temporary_output" "$output" 2>/dev/null || true
    return 1
}

capture_runtime_snapshot() {
    local output="$1"

    if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
        capture_local_runtime_snapshot >"$output"
    else
        ssh "$SSH_HOST" \
            "cd ~/$REMOTE_DIR && bash scripts/nocean_federation_control.sh runtime-metrics" \
            >"$output"
    fi
    [[ "$(sed -n '1p' "$output")" == "$RUNTIME_SNAPSHOT_HEADER" ]] \
        || die "runtime metrics returned an unexpected schema"
    date +%s >"$output.epoch"
}

local_runtime_process_sample() {
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

local_runtime_log_lines() {
    local member="$1"
    local -a log_files=()
    shopt -s nullglob
    log_files=("$LOCAL_FEDERATION_DIR/member-$member/work/logs"/cdk-mintd.log*)
    shopt -u nullglob
    if ((${#log_files[@]} == 0)); then
        printf '0\n'
        return
    fi
    awk 'END { print NR }' "${log_files[@]}"
}

local_runtime_prometheus_port() {
    local member="$1"
    awk '
        /^\[prometheus\]$/ { in_prometheus = 1; next }
        /^\[/ { in_prometheus = 0 }
        in_prometheus && $1 == "port" && $2 == "=" { print $3; exit }
    ' "$LOCAL_FEDERATION_DIR/member-$member/config.toml"
}

local_runtime_federation_metrics() {
    local member="$1"
    local port metrics
    port="$(local_runtime_prometheus_port "$member")"
    [[ "$port" =~ ^[1-9][0-9]*$ ]] \
        || die "local member $member config has no Prometheus port"
    metrics="$(curl --max-time 10 -fsS "http://127.0.0.1:$port/metrics")" \
        || die "local member $member Prometheus metrics are unavailable"
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

capture_local_runtime_snapshot() {
    printf '%s\n' "$RUNTIME_SNAPSHOT_HEADER"
    local member pid process_sample log_lines federation_metrics
    for member in $(seq 1 "$EXPECTED_MEMBERS"); do
        read -r pid <"$LOCAL_FEDERATION_DIR/member-$member/work/mintd.pid"
        kill -0 "$pid" 2>/dev/null || die "local member $member is not running"
        process_sample="$(local_runtime_process_sample "$pid")"
        [[ -n "$process_sample" ]] \
            || die "local member $member process metrics are unavailable"
        log_lines="$(local_runtime_log_lines "$member")"
        federation_metrics="$(local_runtime_federation_metrics "$member")"
        printf '%s,%s,%s,%s,%s\n' \
            "$member" "$pid" "$process_sample" "$log_lines" "$federation_metrics"
    done
}

echo "Importing the federation into the funding wallet..."
add_federation "$FUNDING_DIR" >/dev/null

funding_amount=$((total_tokens * TOKEN_AMOUNT + FUNDING_BUFFER))
echo "Minting $funding_amount sat with $TOKEN_AMOUNT-sat target proofs for $total_tokens benchmark tokens..."
"$CLI" \
    --work-dir "$FUNDING_DIR" \
    --non-interactive \
    --unit sat \
    mint "$MINT_URL" "$funding_amount" \
    --split-target "$TOKEN_AMOUNT" \
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

prepare_case_tokens() {
    local token_count="$1"
    local i id token_file token token_output_file

    : >"$CASE_DIR/token-files.txt"
    : >"$CASE_DIR/token-preparation-errors.log"
    token_output_file="$CASE_DIR/prepared-tokens.txt"
    if ! "$CLI" \
        --work-dir "$FUNDING_DIR" \
        --non-interactive \
        --unit sat \
        send \
        --mint-url "$MINT_URL" \
        --amount "$TOKEN_AMOUNT" \
        --count "$token_count" \
        --offline \
        >"$token_output_file" \
        2>>"$CASE_DIR/token-preparation-errors.log"
    then
        return 1
    fi

    i=0
    while IFS= read -r token; do
        [[ -n "$token" ]] || continue
        case "$token" in
            cashu*) ;;
            *)
                printf '%s\n' "$token" >>"$CASE_DIR/token-preparation-errors.log"
                return 1
                ;;
        esac
        i=$((i + 1))
        global_token_index=$((global_token_index + 1))
        id="token-$(printf '%06d' "$global_token_index")"
        token_file="$CASE_DIR/tokens/$id.cashu"
        printf '%s\n' "$token" >"$token_file"
        chmod 600 "$token_file"
        printf '%s\n' "$token_file" >>"$CASE_DIR/token-files.txt"

        if (( i % 10 == 0 || i == token_count )); then
            echo "  prepared $i/$token_count tokens"
        fi
    done <"$token_output_file"

    [[ "$i" == "$token_count" ]] || {
        printf 'expected %s tokens, got %s\n' "$token_count" "$i" \
            >>"$CASE_DIR/token-preparation-errors.log"
        return 1
    }
    rm -f -- "$token_output_file"
}

start_profile_hook() {
    local case_name="$1"
    local case_dir="$2"

    [[ -n "$PROFILE_HOOK" ]] || return 0
    "$PROFILE_HOOK" start "$case_name" "$case_dir" || return 1
    ACTIVE_PROFILE_CASE="$case_name"
    ACTIVE_PROFILE_DIR="$case_dir"
    PROFILE_HOOK_ACTIVE=1
}

stop_profile_hook() {
    [[ "$PROFILE_HOOK_ACTIVE" == "1" ]] || return 0
    "$PROFILE_HOOK" stop "$ACTIVE_PROFILE_CASE" "$ACTIVE_PROFILE_DIR" || return 1
    PROFILE_HOOK_ACTIVE=0
    ACTIVE_PROFILE_CASE=""
    ACTIVE_PROFILE_DIR=""
}

cleanup_active_profile_hook() {
    [[ "$PROFILE_HOOK_ACTIVE" == "1" ]] || return 0
    PROFILE_HOOK_ACTIVE=0
    "$PROFILE_HOOK" stop "$ACTIVE_PROFILE_CASE" "$ACTIVE_PROFILE_DIR" \
        >/dev/null 2>&1 || true
    ACTIVE_PROFILE_CASE=""
    ACTIVE_PROFILE_DIR=""
}

trap cleanup_active_profile_hook EXIT
trap 'cleanup_active_profile_hook; trap - EXIT; exit 130' INT
trap 'cleanup_active_profile_hook; trap - EXIT; exit 143' TERM

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
    TIMEFORMAT='%R'
    if ! { time prepare_case_tokens "$n"; } \
        2>"$CASE_DIR/token-creation.seconds"
    then
        die "token creation failed for $case_name; see token-preparation-errors.log"
    fi
    token_creation_seconds="$(tail -n 1 "$CASE_DIR/token-creation.seconds")"

    echo "Preparing $n isolated receiver wallets with $PREPARE_PARALLELISM setup jobs..."
    if ! { time {
        sed -n 's/.*\/\(token-[0-9][0-9]*\)\.cashu$/\1/p' "$CASE_DIR/token-files.txt" \
            | xargs -P "$PREPARE_PARALLELISM" -I '{}' \
                bash -c 'prepare_receiver_wallet "$1"' _ '{}' \
                2>"$CASE_DIR/wallet-preparation-errors.log"
    }; } 2>"$CASE_DIR/wallet-preparation.seconds"
    then
        die "receiver wallet preparation failed for $case_name"
    fi
    wallet_preparation_seconds="$(tail -n 1 "$CASE_DIR/wallet-preparation.seconds")"
    token_preparation_seconds="$(awk \
        -v token="$token_creation_seconds" \
        -v wallet="$wallet_preparation_seconds" \
        'BEGIN { printf "%.3f", token + wallet }')"

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

    if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
        capture_converged_local_database_snapshot \
            "$CASE_DIR/database-before.csv" "$CASE_DIR/database-before.keys" \
            || die "local durable state was not converged before $case_name"
        capture_local_performance_snapshot "$CASE_DIR/performance-before.tsv"
    fi
    capture_runtime_snapshot "$CASE_DIR/runtime-before.csv"

    echo "Receiving $n tokens with parallelism $parallelism..."
    profile_hook_status='disabled'
    if [[ -n "$PROFILE_HOOK" ]]; then
        start_profile_hook "$case_name" "$CASE_DIR" \
            || die "PROFILE_HOOK failed to start for $case_name"
        profile_hook_status='captured'
    fi
    TIMEFORMAT='%R'
    set +e
    { time xargs -P "$parallelism" -I '{}' \
        bash -c 'receive_one "$1"' _ '{}' \
        <"$CASE_DIR/token-files.txt" \
        2>"$CASE_DIR/xargs-errors.log"; } \
        2>"$CASE_DIR/batch.seconds"
    batch_status=$?
    set -e
    if [[ -n "$PROFILE_HOOK" ]]; then
        stop_profile_hook \
            || die "PROFILE_HOOK failed to stop for $case_name"
    fi

    window_end_status="$(query_observed_status)" \
        || die "could not capture the receive-window-end operation height for $case_name"
    [[ "$(grep -c '^member ' <<<"$window_end_status")" == "$EXPECTED_MEMBERS" ]] \
        || die "receive-window-end status did not contain every federation member"
    operations_at_window_end="$(printf '%s\n' "$window_end_status" \
        | minimum_operation_count_from_status)"
    [[ -n "$operations_at_window_end" ]] \
        || die "could not parse the receive-window-end operation count"
    printf '%s\n' "$window_end_status" >"$CASE_DIR/status-window-end.txt"

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

    echo "Waiting for a stable post-case operation height..."
    after_status="$(wait_for_quiescence)" \
        || die "federation did not become quiescent after $case_name"
    eventual_operations_after="$(printf '%s\n' "$after_status" | operation_count_from_status)"
    [[ -n "$eventual_operations_after" ]] \
        || die "could not parse the post-case operation count"
    immediate_operation_delta=$((operations_at_window_end - operations_before))
    eventual_operation_delta=$((eventual_operations_after - operations_before))
    late_operation_delta=$((eventual_operations_after - operations_at_window_end))
    (( immediate_operation_delta >= 0 )) \
        || die "receive-window-end operation count moved backwards for $case_name"
    (( late_operation_delta >= 0 )) \
        || die "eventual operation count precedes the receive-window-end count for $case_name"
    committed_tps="$(awk -v operations="$immediate_operation_delta" -v elapsed="$elapsed_seconds" \
        'BEGIN { if (elapsed > 0) printf "%.3f", operations / elapsed; else print "0.000" }')"
    printf '%s\n' "$after_status" >"$CASE_DIR/status-after.txt"

    capture_runtime_snapshot "$CASE_DIR/runtime-after.csv"
    if [[ -n "$LOCAL_FEDERATION_DIR" ]]; then
        capture_local_performance_snapshot "$CASE_DIR/performance-after.tsv"
        performance_snapshot_has_required_families \
            "$CASE_DIR/performance-after.tsv" \
            || die "required federation performance metric families are not exposed"
        capture_converged_local_database_snapshot \
            "$CASE_DIR/database-after.csv" "$CASE_DIR/database-after.keys" \
            || die "local durable state was not converged after $case_name"
        database_status='local_live_read_only_transaction'
        persisted_state_match='1'
        cached_audited_materialized_digest_match="$(
            cached_audited_materialized_digest_status "$CASE_DIR/database-after.csv"
        )"
        if wait_for_local_materialized_digest_convergence \
            "$CASE_DIR/materialized-digests-after.txt"
        then
            current_materialized_digest_match='1'
        else
            current_materialized_digest_match='0'
            die "exact current materialized state did not converge after $case_name"
        fi
        database_counts="$(database_window_counts \
            "$CASE_DIR/database-before.keys" "$CASE_DIR/database-after.keys")"
        IFS=',' read -r \
            new_retained_checkpoints \
            new_retained_checkpoint_signer_pairs \
            new_retained_checkpoint_quorums \
            new_retained_finalized_units \
            new_retained_finalized_items <<<"$database_counts"
        rm -f "$CASE_DIR/database-before.keys" "$CASE_DIR/database-after.keys"

        aggregate_performance_window \
            "$CASE_DIR/performance-before.tsv" \
            "$CASE_DIR/performance-after.tsv" \
            "$CASE_DIR/performance-window.csv" \
            || die "performance metrics for $case_name reset or have duplicate series"
        performance_metrics_status='work_and_operation_families_captured'
        signing_share_seconds_sum="$(performance_window_delta \
            "$CASE_DIR/performance-window.csv" \
            '^cdk_federation_signing_share_latency_seconds_sum$')"
        signing_share_count="$(performance_window_delta \
            "$CASE_DIR/performance-window.csv" \
            '^cdk_federation_signing_share_latency_seconds_count$')"
    else
        database_status='N/A_remote_database_not_inspected'
        persisted_state_match='N/A'
        cached_audited_materialized_digest_match='N/A'
        current_materialized_digest_match='N/A'
        new_retained_checkpoints='N/A'
        new_retained_checkpoint_signer_pairs='N/A'
        new_retained_checkpoint_quorums='N/A'
        new_retained_finalized_units='N/A'
        new_retained_finalized_items='N/A'
        performance_metrics_status='N/A_remote_metrics_not_exported'
        signing_share_seconds_sum='N/A'
        signing_share_count='N/A'
    fi

    read -r runtime_before_epoch <"$CASE_DIR/runtime-before.csv.epoch"
    read -r runtime_after_epoch <"$CASE_DIR/runtime-after.csv.epoch"
    observation_seconds=$((runtime_after_epoch - runtime_before_epoch))
    runtime_summary="$(aggregate_runtime_window \
        "$CASE_DIR/runtime-before.csv" \
        "$CASE_DIR/runtime-after.csv" \
        "$observation_seconds" \
        "$EXPECTED_MEMBERS" \
        "$CASE_DIR/runtime-members.csv")" \
        || die "runtime metrics for $case_name are incomplete or non-monotonic"
    IFS=',' read -r \
        observation_seconds \
        cpu_seconds_total \
        cpu_millis_per_member_second \
        max_end_rss_kb \
        log_lines \
        log_lines_per_member_second \
        sync_transitions \
        scheduled_checkpoints \
        recovery_checkpoints \
        quorums_transferred \
        checkpoints_promoted \
        catch_up_batches_applied \
        signatures_verified <<<"$runtime_summary"
    printf '%s\n' \
        'observation_seconds,cpu_seconds_total,cpu_millis_per_member_second,max_end_rss_kb,log_lines,log_lines_per_member_second,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified' \
        "$runtime_summary" >"$CASE_DIR/runtime-summary.csv"

    echo "Measuring a post-run $IDLE_CPU_SECONDS-second idle CPU window..."
    capture_runtime_snapshot "$CASE_DIR/idle-runtime-before.csv"
    sleep "$IDLE_CPU_SECONDS"
    capture_runtime_snapshot "$CASE_DIR/idle-runtime-after.csv"
    read -r idle_runtime_before_epoch <"$CASE_DIR/idle-runtime-before.csv.epoch"
    read -r idle_runtime_after_epoch <"$CASE_DIR/idle-runtime-after.csv.epoch"
    idle_observation_seconds=$((idle_runtime_after_epoch - idle_runtime_before_epoch))
    idle_runtime_summary="$(aggregate_runtime_window \
        "$CASE_DIR/idle-runtime-before.csv" \
        "$CASE_DIR/idle-runtime-after.csv" \
        "$idle_observation_seconds" \
        "$EXPECTED_MEMBERS" \
        "$CASE_DIR/idle-runtime-members.csv")" \
        || die "idle runtime metrics for $case_name are incomplete or non-monotonic"
    IFS=',' read -r \
        idle_observation_seconds \
        idle_cpu_seconds_total \
        idle_cpu_millis_per_member_second \
        idle_max_end_rss_kb \
        _ <<<"$idle_runtime_summary"
    printf '%s\n' \
        'observation_seconds,cpu_seconds_total,cpu_millis_per_member_second,max_end_rss_kb,log_lines,log_lines_per_member_second,sync_transitions,scheduled_checkpoints,recovery_checkpoints,quorums_transferred,checkpoints_promoted,catch_up_batches_applied,signatures_verified' \
        "$idle_runtime_summary" >"$CASE_DIR/idle-runtime-summary.csv"

    result_fields=( \
        "$case_name" \
        "$n" \
        "$parallelism" \
        "$token_creation_seconds" \
        "$wallet_preparation_seconds" \
        "$token_preparation_seconds" \
        "$profile_hook_status" \
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
        "$operations_at_window_end" \
        "$immediate_operation_delta" \
        "$eventual_operations_after" \
        "$eventual_operation_delta" \
        "$late_operation_delta" \
        "$observation_seconds" \
        "$cpu_seconds_total" \
        "$cpu_millis_per_member_second" \
        "$max_end_rss_kb" \
        "$log_lines" \
        "$log_lines_per_member_second" \
        "$sync_transitions" \
        "$scheduled_checkpoints" \
        "$recovery_checkpoints" \
        "$quorums_transferred" \
        "$checkpoints_promoted" \
        "$catch_up_batches_applied" \
        "$signatures_verified" \
        "$idle_observation_seconds" \
        "$idle_cpu_seconds_total" \
        "$idle_cpu_millis_per_member_second" \
        "$idle_max_end_rss_kb" \
        "$database_status" \
        "$new_retained_checkpoints" \
        "$new_retained_checkpoint_signer_pairs" \
        "$new_retained_checkpoint_quorums" \
        "$new_retained_finalized_units" \
        "$new_retained_finalized_items" \
        "$persisted_state_match" \
        "$cached_audited_materialized_digest_match" \
        "$current_materialized_digest_match" \
        "$performance_metrics_status" \
        "$signing_share_seconds_sum" \
        "$signing_share_count" \
    )
    ((${#result_fields[@]} == RESULTS_EXPECTED_FIELDS)) \
        || die "internal results schema mismatch: expected $RESULTS_EXPECTED_FIELDS fields, found ${#result_fields[@]}"
    (IFS=,; printf '%s\n' "${result_fields[*]}") >>"$RESULTS_CSV"

    printf 'Result: N=%s parallelism=%s successes=%s failures=%s elapsed=%ss confirmed-TPS=%s immediate-committed-TPS=%s p50=%ss p95=%ss p99=%ss operations=+%s immediate/+%s late CPU=%ss post-idle-CPU=%ss new-retained-checkpoints=+%s finalized-units=+%s finalized-items=+%s logs=+%s sync-transitions=+%s scheduled-checkpoints(member-sum)=+%s\n' \
        "$n" "$parallelism" "$successes" "$failures" "$elapsed_seconds" \
        "$successful_tps" "$committed_tps" "$p50" "$p95" "$p99" \
        "$immediate_operation_delta" "$late_operation_delta" \
        "$cpu_seconds_total" "$idle_cpu_seconds_total" "$new_retained_checkpoints" \
        "$new_retained_finalized_units" "$new_retained_finalized_items" \
        "$log_lines" "$sync_transitions" "$scheduled_checkpoints"

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
