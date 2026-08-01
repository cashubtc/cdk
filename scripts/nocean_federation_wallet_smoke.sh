#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

CLIENT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$CLIENT_ROOT/operator.env"
[[ -f "$ENV_FILE" ]] || {
    printf 'error: operator metadata is missing\n' >&2
    exit 1
}
# shellcheck disable=SC1090
source "$ENV_FILE"
CLI="$CLIENT_ROOT/bin/cdk-cli"
MINTD="$CLIENT_ROOT/bin/cdk-mintd"
PUBLIC_CONFIG="$CLIENT_ROOT/ceremonies/active/setup-state/activated/federation-public.json"
WALLET_DIR="$CLIENT_ROOT/wallet-smoke"
SESSION="${WALLET_SESSION:?WALLET_SESSION is required}"
MINT_URL="${PUBLIC_MINT_URL:?PUBLIC_MINT_URL is required}"
ACTION_TIMEOUT_SECS="${ACTION_TIMEOUT_SECS:-300}"

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

validate() {
    local command
    for command in grep jq sed tail tmux; do
        command -v "$command" >/dev/null 2>&1 \
            || die "required command not found: $command"
    done
    [[ -x "$CLI" && -x "$MINTD" ]] || die "wallet smoke binaries are missing"
    [[ -f "$PUBLIC_CONFIG" ]] || die "activated public federation config is missing"
    [[ "$SESSION" =~ ^[A-Za-z0-9._-]+$ ]] || die "wallet session name is invalid"
    [[ "$MINT_URL" =~ ^https://[A-Za-z0-9._:/-]+$ ]] \
        || die "wallet smoke requires a credential-free HTTPS mint URL"
    [[ "$ACTION_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]] \
        || die "ACTION_TIMEOUT_SECS must be positive"
}

wallet_cli() {
    "$CLI" --work-dir "$WALLET_DIR" --non-interactive "$@"
}

initial_smoke() {
    [[ ! -e "$WALLET_DIR" ]] || die "wallet smoke directory already exists"
    mkdir -p "$WALLET_DIR"
    wallet_cli federation add \
        --public-config "$PUBLIC_CONFIG" \
        --member-request-timeout-secs 60 \
        --member-fallback-timeout-secs 60 \
        --quote-status-member-timeout-secs 60 \
        >"$WALLET_DIR/federation-add.log"
    wallet_cli mint-info "$MINT_URL" >"$WALLET_DIR/mint-info.log"
    wallet_cli --unit sat balance >"$WALLET_DIR/balance.log"
    grep -Eq '(^|[^0-9])0([^0-9]|$)' "$WALLET_DIR/balance.log" \
        || die "new federation wallet did not report an empty balance"
    jq -n \
        '{result: "passed", federation_import: true, threshold_mint_info: true, empty_balance: true, paid_operation: false}' \
        >"$WALLET_DIR/smoke-report.json"
}

offline_operation() {
    [[ -d "$WALLET_DIR" ]] || die "initial wallet smoke has not run"
    wallet_cli mint-info "$MINT_URL" >"$WALLET_DIR/offline-member-mint-info.log"
    wallet_cli --unit sat balance >"$WALLET_DIR/offline-member-balance.log"
    jq -n '{result: "passed", threshold_read_with_one_member_offline: true}' \
        >"$WALLET_DIR/offline-operation-report.json"
}

run_action() {
    local action="$1" status_file exit_code
    [[ "$action" =~ ^[a-z][a-z-]{0,31}$ ]] || die "wallet action is invalid"
    status_file="$CLIENT_ROOT/wallet-$action.status"
    rm -f -- "$status_file"
    trap 'exit_code=$?; printf "%s\n" "$exit_code" >"$status_file"' EXIT
    set +e
    case "$action" in
        smoke) initial_smoke ;;
        offline-operation) offline_operation ;;
        *) printf 'unsupported wallet action\n' >&2; false ;;
    esac
    exit_code=$?
    set -e
    printf '%s\n' "$exit_code" >"$status_file"
    trap - EXIT
    return "$exit_code"
}

session_run_wait() {
    local action="$1" status_file command deadline code
    [[ "$action" =~ ^[a-z][a-z-]{0,31}$ ]] || die "wallet action is invalid"
    status_file="$CLIENT_ROOT/wallet-$action.status"
    rm -f -- "$status_file"
    if tmux has-session -t "$SESSION" 2>/dev/null; then
        printf -v command 'cd %q && exec bash scripts/nocean_federation_wallet_smoke.sh run-action %q' \
            "$CLIENT_ROOT" "$action"
        tmux respawn-pane -k -t "$SESSION:wallet.0" "$command"
    else
        printf -v command 'cd %q && exec bash scripts/nocean_federation_wallet_smoke.sh run-action %q' \
            "$CLIENT_ROOT" "$action"
        tmux new-session -d -s "$SESSION" -n wallet -c "$CLIENT_ROOT" "$command"
        tmux set-option -t "$SESSION" remain-on-exit on
    fi
    tmux select-pane -t "$SESSION:wallet.0" -T "wallet-$action"
    deadline=$((SECONDS + ACTION_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        if [[ -f "$status_file" ]]; then
            code="$(sed -n '1p' "$status_file")"
            [[ "$code" =~ ^[0-9]+$ ]] || die "wallet action status is invalid"
            ((code == 0)) || die "wallet action failed: $action"
            return 0
        fi
        sleep 0.25
    done
    die "wallet action timed out: $action"
}

validate
case "${1:-}" in
    session-run-wait) session_run_wait "${2:?action required}" ;;
    run-action) run_action "${2:?action required}" ;;
    *) die "usage: $0 {session-run-wait|run-action}" ;;
esac
