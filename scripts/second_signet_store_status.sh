#!/usr/bin/env bash
set -Eeuo pipefail

STORE_ORIGIN="${STORE_ORIGIN:-https://signet.2nd.dev}"
POLL_SECONDS="${POLL_SECONDS:-1}"
TIMEOUT_SECONDS="${TIMEOUT_SECONDS:-600}"

usage() {
    cat <<'EOF'
Check a test-only 2nd.dev Signet store order without exposing its payment hash.

Usage:
  scripts/second_signet_store_status.sh ORDER_ID
  scripts/second_signet_store_status.sh --wait ORDER_ID

--wait follows the store's current one-second polling cadence for up to ten
minutes. Override POLL_SECONDS or TIMEOUT_SECONDS when needed.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

wait_for_terminal=0
case "${1:-}" in
    --wait)
        wait_for_terminal=1
        shift
        ;;
    -h|--help)
        usage
        exit 0
        ;;
esac

order_id="${1:-}"
[[ "$order_id" =~ ^[1-9][0-9]*$ && $# -eq 1 ]] || {
    usage >&2
    exit 1
}
[[ "$POLL_SECONDS" =~ ^[1-9][0-9]*$ ]] \
    || die "POLL_SECONDS must be a positive integer"
[[ "$TIMEOUT_SECONDS" =~ ^[1-9][0-9]*$ ]] \
    || die "TIMEOUT_SECONDS must be a positive integer"
[[ "$STORE_ORIGIN" =~ ^https://[A-Za-z0-9._:-]+$ ]] \
    || die "STORE_ORIGIN must be a credential-free HTTPS origin"

require_command curl
require_command jq

payload="$(jq -cn --argjson order_id "$order_id" '{order_id: $order_id}')"

check_status() {
    local response status
    response="$(
        curl --fail --silent --show-error \
            --connect-timeout 10 \
            --max-time 30 \
            --header 'Content-Type: application/json' \
            --data "$payload" \
            "$STORE_ORIGIN/api/v1/order/status"
    )"
    status="$(jq -er '.status | select(type == "string" and test("^[A-Z_]+$"))' <<<"$response")" \
        || die "store response did not contain a bounded status"
    printf '%s\n' "$status"
}

if ((wait_for_terminal == 0)); then
    check_status
    exit 0
fi

deadline=$((SECONDS + TIMEOUT_SECONDS))
while ((SECONDS < deadline)); do
    status="$(check_status)"
    case "$status" in
        PENDING)
            sleep "$POLL_SECONDS"
            ;;
        PAID)
            printf '%s\n' "$status"
            exit 0
            ;;
        *)
            printf '%s\n' "$status"
            exit 1
            ;;
    esac
done

echo "TIMED_OUT" >&2
exit 1
