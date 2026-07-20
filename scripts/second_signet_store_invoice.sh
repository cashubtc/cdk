#!/usr/bin/env bash
set -Eeuo pipefail

STORE_ORIGIN="${STORE_ORIGIN:-https://signet.2nd.dev}"

usage() {
    cat <<'EOF'
Create a test-only BOLT11 invoice for the 2nd.dev Signet store.

Usage:
  scripts/second_signet_store_invoice.sh [--json|--invoice|--copy] AMOUNT_SAT

The store currently offers 1,000, 5,000, and 10,000-sat purchases. Output
defaults to compact JSON containing order_id and invoice. --invoice prints only
the invoice; --copy copies it to the macOS clipboard and prints only order_id.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

output_mode="json"
case "${1:-}" in
    --json|--invoice|--copy)
        output_mode="${1#--}"
        shift
        ;;
    -h|--help)
        usage
        exit 0
        ;;
esac

amount="${1:-}"
[[ -n "$amount" && $# -eq 1 ]] || {
    usage >&2
    exit 1
}
case "$amount" in
    1000|5000|10000) ;;
    *) die "AMOUNT_SAT must be one of: 1000, 5000, 10000" ;;
esac
[[ "$STORE_ORIGIN" =~ ^https://[A-Za-z0-9._:-]+$ ]] \
    || die "STORE_ORIGIN must be a credential-free HTTPS origin"

require_command curl
require_command jq

payload="$(jq -cn --argjson amount "$amount" '{amount: $amount}')"
response="$(
    curl --fail --silent --show-error \
        --connect-timeout 10 \
        --max-time 30 \
        --header 'Content-Type: application/json' \
        --data "$payload" \
        "$STORE_ORIGIN/api/v1/order/create"
)"

order_id="$(jq -er '.order_id | select(type == "number")' <<<"$response")" \
    || die "store response did not contain a numeric order_id"
invoice="$(jq -er '.invoice | select(type == "string" and startswith("lntbs"))' <<<"$response")" \
    || die "store response did not contain a Signet BOLT11 invoice"

case "$output_mode" in
    json)
        jq -cn \
            --argjson order_id "$order_id" \
            --arg invoice "$invoice" \
            '{order_id: $order_id, invoice: $invoice}'
        ;;
    invoice)
        printf '%s\n' "$invoice"
        ;;
    copy)
        require_command pbcopy
        printf '%s' "$invoice" | pbcopy
        printf '%s\n' "$order_id"
        echo "Copied the Signet store invoice to the clipboard." >&2
        ;;
esac
