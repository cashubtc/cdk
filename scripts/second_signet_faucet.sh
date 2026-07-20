#!/usr/bin/env bash
set -Eeuo pipefail

FAUCET_URL="${FAUCET_URL:-https://signet.2nd.dev/}"

usage() {
    cat <<'EOF'
Prepare a Signet BOLT11 invoice for the authenticated 2nd.dev faucet.

Usage:
  scripts/second_signet_faucet.sh INVOICE
  printf '%s' "$INVOICE" | scripts/second_signet_faucet.sh

The helper validates and copies the invoice, then opens the faucet. Select
Lightning, paste, and click "Send sats". If "Sign in with GitHub" is shown,
sign in and rerun the helper after the faucet page returns.

The faucet API requires browser authentication. This helper intentionally does
not read, copy, or persist browser cookies.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
esac

if (($# > 1)); then
    usage >&2
    exit 1
fi
if (($# == 1)); then
    invoice="$1"
else
    [[ ! -t 0 ]] || {
        usage >&2
        exit 1
    }
    IFS= read -r invoice
fi

[[ "$invoice" =~ ^lntbs[0-9a-z]+$ ]] \
    || die "invoice must be one lowercase Signet BOLT11 string beginning with lntbs"
[[ "$FAUCET_URL" =~ ^https://[A-Za-z0-9._:-]+/$ ]] \
    || die "FAUCET_URL must be a credential-free HTTPS origin ending in /"

if command -v pbcopy >/dev/null 2>&1; then
    printf '%s' "$invoice" | pbcopy
    echo "Copied the Signet invoice to the clipboard."
else
    echo "pbcopy is unavailable; copy the validated invoice from your input." >&2
fi

if command -v open >/dev/null 2>&1; then
    open "$FAUCET_URL"
else
    printf 'Open %s\n' "$FAUCET_URL"
fi

cat <<'EOF'
In the faucet: choose Lightning, paste the invoice, and click "Send sats".
If the page shows "Sign in with GitHub", sign in and rerun this helper.
EOF
