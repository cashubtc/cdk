#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
RUN_DIR="${RUN_DIR:-$REPO_ROOT/target/local-bark-federation}"

command -v sqlite3 >/dev/null 2>&1 || {
    echo "error: sqlite3 is required" >&2
    exit 1
}

member_db() {
    printf '%s/member-%s/work/cdk-mintd.sqlite' "$RUN_DIR" "$1"
}

bark_db() {
    printf '%s/member-%s/work/federation-bark/db.sqlite' "$RUN_DIR" "$1"
}

for member in 1 2 3 4 5; do
    [[ -f "$(member_db "$member")" ]] || {
        echo "error: local federation member $member database is missing" >&2
        exit 1
    }
    [[ -f "$(bark_db "$member")" ]] || {
        echo "error: local Bark member $member database is missing" >&2
        exit 1
    }
done

primary_db="$(member_db 1)"

echo "accepted operations"
sqlite3 -header -column "$primary_db" <<'SQL'
SELECT
    json_extract(envelope_json, '$.operation.type') AS operation,
    COALESCE(json_extract(envelope_json, '$.operation.data.type'), '-') AS subtype,
    COUNT(*) AS count
FROM federation_journal_entries
GROUP BY operation, subtype
ORDER BY operation, subtype;
SQL

echo
echo "finalized items"
sqlite3 -header -column "$primary_db" <<'SQL'
SELECT
    COUNT(*) AS total,
    SUM(status = 'accepted') AS accepted,
    SUM(status = 'duplicate_operation') AS duplicates
FROM federation_aleph_bft_finalized_items;
SQL

echo
echo "redacted quote timing"
sqlite3 -header -column "$primary_db" <<'SQL'
SELECT
    'incoming' AS flow,
    ROW_NUMBER() OVER (ORDER BY created_time, id) AS sample,
    amount,
    created_time,
    updated_at AS terminal_time,
    updated_at - created_time AS duration_seconds,
    amount_paid,
    amount_issued,
    CASE
        WHEN amount_issued >= amount THEN 'issued'
        WHEN amount_paid >= amount THEN 'paid'
        ELSE 'pending'
    END AS state
FROM mint_quote
UNION ALL
SELECT
    'outgoing',
    ROW_NUMBER() OVER (ORDER BY created_time, id),
    amount,
    created_time,
    COALESCE(paid_time, created_time),
    COALESCE(paid_time, created_time) - created_time,
    CASE WHEN state = 'PAID' THEN amount ELSE 0 END,
    0,
    lower(state)
FROM melt_quote
ORDER BY flow, sample;
SQL

echo
echo "redacted Bark movements"
for member in 1 2 3 4 5; do
    sqlite3 -header -column "$(bark_db "$member")" <<SQL
SELECT
    $member AS member,
    ROW_NUMBER() OVER (ORDER BY created_at, id) AS sample,
    status,
    movement_kind,
    intended_balance,
    effective_balance,
    offchain_fee,
    created_at,
    completed_at
FROM bark_movements
ORDER BY created_at, id;
SQL
done

echo
echo "member convergence"
for member in 1 2 3 4 5; do
    sqlite3 -noheader "$(member_db "$member")" <<SQL
SELECT
    'member=$member operations=' || COUNT(*) ||
    ' order_digest=' || COALESCE((
        SELECT order_digest
        FROM federation_journal_entries
        ORDER BY operation_index DESC
        LIMIT 1
    ), '-') ||
    ' application_digest=' || COALESCE((
        SELECT application_digest
        FROM federation_journal_entries
        ORDER BY operation_index DESC
        LIMIT 1
    ), '-')
FROM federation_journal_entries;
SQL
done
