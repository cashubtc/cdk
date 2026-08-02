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
TUI="$CLIENT_ROOT/bin/fed-cdk-tui"
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
    for command in jq sed tmux; do
        command -v "$command" >/dev/null 2>&1 \
            || die "required command not found: $command"
    done
    [[ -x "$TUI" ]] || die "Iroh TUI wallet binary is missing"
    [[ "$SESSION" =~ ^[A-Za-z0-9._-]+$ ]] || die "wallet session name is invalid"
    [[ "$MINT_URL" =~ ^iroh://[a-z0-9]+$ ]] \
        || die "wallet smoke requires a canonical Iroh mint URL"
    [[ "$ACTION_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]] \
        || die "ACTION_TIMEOUT_SECS must be positive"
}

tui_check() {
    local mint_url="$1"
    "$TUI" check --mint-url "$mint_url"
}

initial_smoke() {
    mkdir -p "$WALLET_DIR"
    tui_check "$MINT_URL" >"$WALLET_DIR/iroh-check.json"
    jq -e '.result == "passed" and .wallet_transport == "iroh" and
        .all_public_mint_urls_iroh == true and .available_balance_sat == 0 and
        .payment_capabilities.mint_bolt11 == true and
        .payment_capabilities.melt_bolt11 == true and
        .payment_capabilities.mint_onchain == true and
        .payment_capabilities.melt_onchain == true' \
        "$WALLET_DIR/iroh-check.json" >/dev/null \
        || die "TUI Iroh wallet check did not return the expected federation result"
    jq -n \
        '{result: "passed", federation_import_over_iroh: true, threshold_mint_info_over_iroh: true, empty_balance: true, payment_methods: {bolt11: true, onchain: true}, paid_operation: false}' \
        >"$WALLET_DIR/smoke-report.json"
}

offline_operation() {
    [[ -d "$WALLET_DIR" ]] || die "initial wallet smoke has not run"
    tui_check "$MINT_URL" >"$WALLET_DIR/offline-member-iroh-check.json"
    jq -e '.result == "passed" and .wallet_transport == "iroh"' \
        "$WALLET_DIR/offline-member-iroh-check.json" >/dev/null \
        || die "TUI Iroh wallet check failed with one member offline"
    jq -n '{result: "passed", threshold_read_with_one_member_offline_over_iroh: true}' \
        >"$WALLET_DIR/offline-operation-report.json"
}

probe_url() {
    local mint_url="$1" label="$2"
    [[ "$mint_url" =~ ^iroh://[a-z0-9]+$ ]] || die "probe URL is not canonical Iroh"
    [[ "$label" =~ ^[A-Za-z0-9._-]+$ ]] || die "probe label is invalid"
    mkdir -p "$WALLET_DIR/probes"
    tui_check "$mint_url" >"$WALLET_DIR/probes/$label.json"
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
    probe-url) probe_url "${2:?mint URL required}" "${3:?label required}" ;;
    *) die "usage: $0 {session-run-wait|run-action|probe-url}" ;;
esac
