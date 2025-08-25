#!/usr/bin/env bash
set -Eeuo pipefail

# ---------- Defaults ----------
MINT_URL="${MINT_URL:-http://localhost:8085}"
CAIRO_EXECUTABLE_PATH="${CAIRO_EXECUTABLE_PATH:-crates/cashu/src/nuts/nutxx/test/is_prime_executable.json}"
AMOUNT_MINT="${AMOUNT_MINT:-1000}"
AMOUNT_TOKEN="${AMOUNT_TOKEN:-200}"
CLI="${CLI:-./target/release/cdk-cli}"
POLL_TIMEOUT_SEC="${POLL_TIMEOUT_SEC:-60}"

# Colors (disabled if NO_COLOR is set or not a TTY)
if [[ -z "${NO_COLOR:-}" ]] && [[ -t 1 ]]; then
  readonly RED='\033[0;31m'
  readonly GREEN='\033[0;32m'
  readonly YELLOW='\033[0;33m'
  readonly BLUE='\033[0;34m'
  readonly WHITE='\033[1;37m'
  readonly GRAY='\033[0;90m'
  readonly BOLD='\033[1m'
  readonly DIM='\033[2m'
  readonly RESET='\033[0m'
else
  readonly RED='' GREEN='' YELLOW='' BLUE='' WHITE='' GRAY='' BOLD='' DIM='' RESET=''
fi

usage() {
  cat <<EOF
Cairo CLI Integration Test

Usage: $0 [URL] [options]

Positional:
  URL                         Mint URL (e.g. http://localhost:8085)

Options:
  -m, --mint URL              Mint URL (overrides positional)
      --amount-mint N         Amount to mint (sats)      [default: $AMOUNT_MINT]
      --amount-token N        Amount to send/receive     [default: $AMOUNT_TOKEN]
      --cli PATH              Path to cdk-cli binary     [default: $CLI]
      --cairo PATH            Path to Cairo JSON         [default: $CAIRO_EXECUTABLE_PATH]
  -h, --help                  Show this help

Examples:
  $0
  $0 http://localhost:8085
  $0 --amount-mint 2000 --amount-token 500
EOF
}

# ---------- Args ----------
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage; exit 0 ;;
    -m|--mint) MINT_URL="$2"; shift 2 ;;
    --amount-mint) AMOUNT_MINT="$2"; shift 2 ;;
    --amount-token) AMOUNT_TOKEN="$2"; shift 2 ;;
    --cli) CLI="$2"; shift 2 ;;
    --cairo|--cairo-executable) CAIRO_EXECUTABLE_PATH="$2"; shift 2 ;;
    http*://*) MINT_URL="$1"; shift ;;
    *) echo "Unknown argument: $1" >&2; usage; exit 1 ;;
  esac
done

#----------- Exit early if Integration test ------------
if [[ "${CI:-}" == "true" || "${INTEGRATION_TEST:-}" == "true" ]]; then
  set -e  # Exit on any error in test mode
fi

# ---------- Nix wrap ----------
if [[ -z "${IN_NIX_SHELL:-}" ]] && command -v nix >/dev/null 2>&1; then
  echo "Entering nix shell..."
  exec nix develop -c "$0" "$@"
fi

# ---------- Utils ----------
log() {
  printf "${GRAY}[%s]${RESET} %s\n" "$(date '+%H:%M:%S')" "$*"
}

success() {
  printf "${GREEN}[OK]${RESET} %s\n" "$*"
}

error() {
  printf "${RED}[ERROR]${RESET} %s\n" "$*"
}

info() {
  printf "${BLUE}[INFO]${RESET} %s\n" "$*"
}

warn() {
  printf "${YELLOW}[WARN]${RESET} %s\n" "$*"
}

progress() {
  printf "${GRAY}[WAIT]${RESET} %s\n" "$*"
}

die() {
  error "$*"
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "Missing required command: $1"
}

into() {
  while IFS= read -r line; do
    printf "    ${DIM}%s${RESET}\n" "$line"
  done
}

extract_token() {
  awk '/^cashu/{print; exit}'
}

received_amount() {
  sed -n 's/^Received:[[:space:]]*//p' | tr -d '\r' | sed 's/[[:space:]]*$//'
}

pay_invoice_non_interactive() {
  local inv="$1"
  if command -v script >/dev/null 2>&1; then
    case "$(uname -s)" in
      Darwin) script -q /dev/null bash -lc "yes yes | just ln-lnd1 payinvoice \"$inv\"" ;;
      Linux)  script -qfc "yes yes | just ln-lnd1 payinvoice \"$inv\"" /dev/null ;;
      *)      script -q /dev/null sh  -c  "yes yes | just ln-lnd1 payinvoice \"$inv\"" ;;
    esac
  else
    yes yes | just ln-lnd1 payinvoice "$inv"
  fi
}

section() {
  echo
  printf "${BOLD}%s${RESET}\n" "$*"
  printf "${GRAY}%s${RESET}\n" "------------------------------------------------------------"
  echo
}

test_header() {
  local test_num="$1"
  local test_name="$2"
  echo
  printf "${BOLD}Test %s: %s${RESET}\n" "$test_num" "$test_name"
  printf "${GRAY}%s${RESET}\n" "------------------------------------------------------------"
}

# Cleanup background jobs if we exit early
mint_pid="" pay_pid=""
cleanup() {
  [[ -n "$mint_pid" ]] && kill "$mint_pid" 2>/dev/null || true
  [[ -n "$pay_pid"  ]] && kill "$pay_pid"  2>/dev/null || true
}
trap cleanup EXIT

# ---------- Build cdk-cli (release) ----------
need cargo
info "Building cdk-cli (release)..."
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
if ! ( cd "$REPO_ROOT" && cargo build --release -p cdk-cli ); then
  die "Failed to build cdk-cli"
fi
# Point CLI to the freshly built binary if default or missing
if [[ "$CLI" == "./target/release/cdk-cli" || ! -x "$CLI" ]]; then
  CLI="$REPO_ROOT/target/release/cdk-cli"
fi

# ---------- Welcome & Setup ----------
echo "Cairo CLI Integration Test"
echo "Testing Cairo proof functionality in CDK CLI"
echo

# ---------- Preconditions ----------
info "Checking prerequisites..."
need awk; need sed; need just; need tee; need grep; need mktemp
[[ -x "$CLI" ]] || die "Binary not found at $CLI"

echo
printf "${BOLD}Configuration:${RESET}\n"
printf "  Mint URL:      %s\n" "$MINT_URL"
printf "  CLI Binary:    %s\n" "$CLI"
printf "  Cairo JSON:    %s\n" "$CAIRO_EXECUTABLE_PATH"
printf "  Mint Amount:   %s sats\n" "$AMOUNT_MINT"
printf "  Token Amount:  %s sats\n" "$AMOUNT_TOKEN"

# ---------- 1) Mint & pay (concurrent) ----------
section "Minting $AMOUNT_MINT sats and paying invoice"

progress "Creating mint quote..."
mint_log="$(mktemp)"
( "$CLI" mint "$MINT_URL" "$AMOUNT_MINT" 2>&1 | tee "$mint_log" ) &
mint_pid=$!

progress "Waiting for invoice..."
invoice=""
spinner_chars='|/-\'
for i in {1..300}; do
  if line="$(grep -m1 '^Please pay: ' "$mint_log" 2>/dev/null || true)"; then
    invoice="${line#Please pay: }"
    [[ -n "$invoice" ]] && break
  fi
  if [[ $((i % 5)) -eq 0 ]]; then
    char_idx=$(( (i / 5) % 4 ))
    printf "\r[WAIT] Waiting for invoice... (%s/300) %s" "$i" "${spinner_chars:$char_idx:1}"
  fi
  sleep 0.2
done
printf "\r%*s\r" 80 ""  # clear line

if [[ -n "$invoice" ]]; then
  success "Invoice received"
else
  kill "$mint_pid" 2>/dev/null || true
  die "Mint didn't generate an invoice"
fi

progress "Paying invoice automatically..."
pay_invoice_non_interactive "$invoice" >/dev/null 2>&1 & pay_pid=$!

progress "Waiting for mint completion..."
if wait "$mint_pid"; then
  success "Mint successful"
else
  die "Mint command failed"
fi

if wait "$pay_pid"; then
  success "Payment completed"
else
  warn "Payment process finished with warnings (this may be normal)"
fi

# ---------- 2) Test 1: Happy path ----------
test_header "1" "Cairo send + receive with prime proof (11)"

progress "Creating Cairo token with spending condition..."
SEND_OUT="$(printf "0\n%s\n" "$AMOUNT_TOKEN" | "$CLI" send \
  --cairo-executable "$CAIRO_EXECUTABLE_PATH" \
  --cairo-executable 1 \
  --cairo-executable 1 2>&1)" || die "Cairo send failed"

printf '%s\n' "$SEND_OUT" | into

TOKEN="$(printf '%s\n' "$SEND_OUT" | extract_token)"
[[ -n "$TOKEN" ]] || die "Failed to extract token from send output"

info "Token created successfully"
progress "Attempting to receive with prime proof (input: 11)..."

set +e
RECV_OUT="$("$CLI" receive --cairo "$CAIRO_EXECUTABLE_PATH" 1 11 -- "$TOKEN" 2>&1)"
RECV_CODE=$?
set -e

printf '%s\n' "$RECV_OUT" | into

AMT="$(printf '%s\n' "$RECV_OUT" | received_amount)"
if [[ $RECV_CODE -eq 0 && "${AMT:-0}" -eq "$AMOUNT_TOKEN" ]]; then
  success "Test 1 passed - received ${AMT} sats with prime proof"
else
  die "Test 1 failed - expected ${AMOUNT_TOKEN} sats (exit=$RECV_CODE, received=${AMT:-<none>})"
fi

# ---------- 3) Test 2: Non-prime should be rejected ----------
test_header "2" "Cairo receive with non-prime input (9) should fail"

progress "Creating another Cairo token..."
SEND_OUT_NP="$(printf "0\n%s\n" "$AMOUNT_TOKEN" | "$CLI" send \
  --cairo-executable "$CAIRO_EXECUTABLE_PATH" \
  --cairo-executable 1 \
  --cairo-executable 1 2>&1)" || die "Cairo send (non-prime test) failed"

printf '%s\n' "$SEND_OUT_NP" | into

TOKEN_NP="$(printf '%s\n' "$SEND_OUT_NP" | extract_token)"
[[ -n "$TOKEN_NP" ]] || die "Failed to extract token (non-prime test)"

info "Token created successfully"
progress "Attempting to receive with non-prime proof (input: 9)..."

set +e
RECV_OUT_NP="$("$CLI" receive --cairo "$CAIRO_EXECUTABLE_PATH" 1 9 -- "$TOKEN_NP" 2>&1)"
RECV_CODE_NP=$?
set -e

printf '%s\n' "$RECV_OUT_NP" | into

AMT_NP="$(printf '%s\n' "$RECV_OUT_NP" | received_amount 2>/dev/null || true)"
if [[ $RECV_CODE_NP -eq 0 && -n "$AMT_NP" ]]; then
  die "Test 2 failed - expected rejection for non-prime 9, but received ${AMT_NP} sats"
else
  success "Test 2 passed - correctly rejected non-prime input"
fi

trap - EXIT

# ---------- Final Results ----------
echo
printf "${BOLD}All tests passed successfully.${RESET}\n"
printf "${DIM}Completed at %s${RESET}\n" "$(date)"

if [[ "${CI:-}" == "true" || "${INTEGRATION_TEST:-}" == "true" ]]; then
  echo "Integration test completed successfully"
  exit 0
fi