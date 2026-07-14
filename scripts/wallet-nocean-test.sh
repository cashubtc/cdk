#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

MINT_URL="${MINT_URL:-https://cdk-1.nocean.xyz}"
SSH_HOST="${SSH_HOST:-nocean}"
MINT_AMOUNT="${MINT_AMOUNT:-50}"
SEND_AMOUNT="${SEND_AMOUNT:-20}"
MELT_AMOUNT_MSAT="${MELT_AMOUNT_MSAT:-10000}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)"
RUN_DIR="${RUN_DIR:-target/nocean-wallet-test/$RUN_ID}"
SENDER_DIR="$RUN_DIR/sender"
RECEIVER_DIR="$RUN_DIR/receiver"
PUBLIC_CONFIG="$RUN_DIR/federation-public.json"
TOKEN_FILE="$RUN_DIR/pending-token.cashu"

for amount_name in MINT_AMOUNT SEND_AMOUNT MELT_AMOUNT_MSAT; do
  amount_value="${!amount_name}"
  if [[ ! "$amount_value" =~ ^[1-9][0-9]*$ ]]; then
    echo "$amount_name must be a positive integer, got: $amount_value" >&2
    exit 1
  fi
done

find_cli() {
  local candidate
  for candidate in target/release/cdk-cli target/debug/cdk-cli; do
    if [[ -x "$candidate" ]] \
      && "$candidate" federation --help >/dev/null 2>&1
    then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  echo "cdk-cli is not built with federation support" >&2
  echo "Build it with: cargo build --release -p cdk-cli --features federation" >&2
  return 1
}

pause() {
  local message="$1"
  printf '\n%s' "$message"
  read -r
}

balance() {
  local wallet_dir="$1"
  "$CLI" \
    --work-dir "$wallet_dir" \
    --non-interactive \
    --unit sat \
    balance
}

add_federation() {
  local wallet_dir="$1"
  "$CLI" \
    --work-dir "$wallet_dir" \
    --non-interactive \
    federation add \
    --public-config "$PUBLIC_CONFIG" \
    --local-mint-url "$MINT_URL" \
    --primary-member-id 1 \
    --member-request-timeout-secs 90 \
    --member-fallback-timeout-secs 90 \
    --quote-status-member-timeout-secs 90 \
    --fanout-concurrency 5
}

CLI="$(find_cli)"

if [[ -e "$RUN_DIR" ]]; then
  echo "run directory already exists: $RUN_DIR" >&2
  echo "Choose another RUN_DIR or leave it unset for a timestamped directory." >&2
  exit 1
fi

mkdir -p "$SENDER_DIR" "$RECEIVER_DIR"
chmod 700 "$RUN_DIR" "$SENDER_DIR" "$RECEIVER_DIR"

cat <<EOF
Nocean federated Cashu wallet test

Mint:       $MINT_URL
Mint amount: $MINT_AMOUNT sat
Send/swap:   $SEND_AMOUNT sat
Melt amount: $MELT_AMOUNT_MSAT msat
Wallet data: $RUN_DIR
CLI:         $CLI
EOF

echo
echo "Checking all five public federation members..."
for member in 1 2 3 4 5; do
  member_url="https://cdk-$member.nocean.xyz/v1/info"
  curl --fail --silent --show-error "$member_url" >/dev/null
  echo "  member $member: HTTPS ready"
done

echo
echo "Fetching the current public federation config from $SSH_HOST..."
scp -q \
  "$SSH_HOST:cdk-federation/member-1/dkg-final/federation-public.json" \
  "$PUBLIC_CONFIG"
chmod 600 "$PUBLIC_CONFIG"

echo "Importing the federation into fresh sender and receiver wallets..."
add_federation "$SENDER_DIR"
add_federation "$RECEIVER_DIR"

echo
echo "Federation mint information:"
"$CLI" \
  --work-dir "$SENDER_DIR" \
  --non-interactive \
  mint-info "$MINT_URL"

pause "Press Enter to mint $MINT_AMOUNT sat into the sender wallet..."
mint_started="$SECONDS"
"$CLI" \
  --work-dir "$SENDER_DIR" \
  --non-interactive \
  --unit sat \
  mint "$MINT_URL" "$MINT_AMOUNT" \
  --wait-duration 180
echo "Mint completed in $((SECONDS - mint_started)) seconds."
echo "Sender balance:"
balance "$SENDER_DIR"

pause "Press Enter to swap and send $SEND_AMOUNT sat from sender to receiver..."
send_output="$(
  "$CLI" \
    --work-dir "$SENDER_DIR" \
    --non-interactive \
    --unit sat \
    send \
    --mint-url "$MINT_URL" \
    --amount "$SEND_AMOUNT"
)"
token="$(printf '%s\n' "$send_output" | tail -n 1)"
case "$token" in
  cashu*) ;;
  *)
    echo "send did not return a Cashu token" >&2
    printf '%s\n' "$send_output" >&2
    exit 1
    ;;
esac
printf '%s\n' "$token" > "$TOKEN_FILE"
chmod 600 "$TOKEN_FILE"
echo "Swap/send succeeded. The token is temporarily stored at $TOKEN_FILE."
echo "Sender balance after send:"
balance "$SENDER_DIR"

pause "Press Enter to receive the swapped token into the receiver wallet..."
"$CLI" \
  --work-dir "$RECEIVER_DIR" \
  --non-interactive \
  --unit sat \
  receive "$token"
rm -f "$TOKEN_FILE"
unset token
echo "Receive succeeded; the temporary token file was removed."
echo "Receiver balance:"
balance "$RECEIVER_DIR"

pause "Press Enter to create and melt a $MELT_AMOUNT_MSAT msat fakewallet invoice..."
invoice="$(
  ssh "$SSH_HOST" \
    "~/cdk-federation/bin/cdk-mintd federation fake-invoice --amount-msat $MELT_AMOUNT_MSAT"
)"
melt_started="$SECONDS"
"$CLI" \
  --work-dir "$RECEIVER_DIR" \
  --non-interactive \
  --unit sat \
  melt \
  --mint-url "$MINT_URL" \
  --method bolt11 \
  --invoice "$invoice"
unset invoice
echo "Melt completed in $((SECONDS - melt_started)) seconds."

echo
echo "Final sender balance:"
balance "$SENDER_DIR"
echo "Final receiver balance:"
balance "$RECEIVER_DIR"

pause "Press Enter to verify all federation members still converge..."
ssh "$SSH_HOST" '
  cd ~/cdk-federation
  timeout 30 bin/cdk-mintd federation compare-digests \
    --public-config member-1/dkg-final/federation-public.json \
    --private-config member-1/dkg-final/federation-private.json
'

cat <<EOF

Nocean wallet test completed successfully.
The fresh wallet state remains at: $RUN_DIR
EOF
