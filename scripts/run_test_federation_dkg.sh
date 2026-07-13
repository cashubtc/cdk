#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

FED_DIR="${FED_DIR:-target/manual-production-dkg-20of14-release}"
WALLET_DIR="${WALLET_DIR:-target/manual-production-dkg-20of14-release-wallet}"
RECEIVER_DIR="${RECEIVER_DIR:-target/manual-production-dkg-20of14-release-receiver}"
MEMBERS="${MEMBERS:-20}"
THRESHOLD="${THRESHOLD:-14}"
PORT_START="${PORT_START:-58185}"
STATUS_RETRIES="${STATUS_RETRIES:-1800}"
STATUS_SLEEP_SECONDS="${STATUS_SLEEP_SECONDS:-1}"
DKG_EXPORT_JOBS="${DKG_EXPORT_JOBS:-20}"

MINTD=target/release/cdk-mintd
CLI=target/release/cdk-cli
SETUP_PUBLIC_CONFIG="$FED_DIR/dkg-setup-public.json"
CEREMONY_CONFIG="$FED_DIR/dkg-ceremony.toml"
FINAL_PUBLIC_CONFIG="$FED_DIR/member-1/dkg-final/federation-public.json"
MINT_URL="http://127.0.0.1:${PORT_START}"

DKG_PIDS=()
RUNTIME_PIDS=()

cleanup() {
  set +e
  local pid

  for pid in "${DKG_PIDS[@]:-}" "${RUNTIME_PIDS[@]:-}"; do
    [[ -n "$pid" ]] || continue
    kill "$pid" 2>/dev/null || true
  done

  for pid in "${DKG_PIDS[@]:-}" "${RUNTIME_PIDS[@]:-}"; do
    [[ -n "$pid" ]] || continue
    wait "$pid" 2>/dev/null || true
  done
}

handle_signal() {
  trap - INT TERM
  exit 130
}

fail_with_member_log() {
  local member="$1"
  local message="$2"
  printf '%s\n' "$message" >&2
  tail -n 120 "$FED_DIR/member-$member/work/mintd.log" >&2 || true
  return 1
}

wait_for_process() {
  local member="$1"
  local pid="$2"
  local description="$3"
  local attempt

  for attempt in $(seq 1 "$STATUS_RETRIES"); do
    if ! kill -0 "$pid" 2>/dev/null; then
      fail_with_member_log "$member" "member $member exited while waiting for $description"
      return 1
    fi
    if "$MINTD" \
      --work-dir "$FED_DIR/member-$member/work" \
      --config "$FED_DIR/member-$member/config.toml" \
      federation dkg-status \
      --public-config "$SETUP_PUBLIC_CONFIG" \
      --private-config "$FED_DIR/member-$member/dkg-setup-private.json" \
      --from-database \
      --progress-only 2>/dev/null \
      | grep -q "keyset_shares=1 .*transcript_signatures=$MEMBERS .*activations=$MEMBERS"
    then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done

  fail_with_member_log "$member" "member $member did not complete DKG"
}

wait_for_runtime() {
  local member="$1"
  local pid="$2"
  local attempt

  for attempt in $(seq 1 "$STATUS_RETRIES"); do
    if ! kill -0 "$pid" 2>/dev/null; then
      fail_with_member_log "$member" "member $member exited during runtime activation"
      return 1
    fi
    if "$MINTD" federation status \
      --public-config "$FINAL_PUBLIC_CONFIG" \
      --private-config "$FED_DIR/member-$member/dkg-final/federation-private.json" \
      --member "$member" \
      --require-ready >/dev/null 2>&1
    then
      return 0
    fi
    sleep "$STATUS_SLEEP_SECONDS"
  done

  fail_with_member_log "$member" "member $member did not activate its finalized DKG config"
}

export_final_config() {
  local member="$1"
  local member_dir="$FED_DIR/member-$member"

  "$MINTD" \
    --work-dir "$member_dir/work" \
    --config "$member_dir/config.toml" \
    federation dkg-status \
    --public-config "$SETUP_PUBLIC_CONFIG" \
    --private-config "$member_dir/dkg-setup-private.json" \
    --from-database \
    --require-ready \
    --final-config-dir "$member_dir/dkg-final" \
    --force
}

export_final_configs() {
  local member index
  local -a export_members=()
  local -a export_pids=()

  for member in $(seq 1 "$MEMBERS"); do
    mkdir -p "$FED_DIR/member-$member/dkg-final"
    export_final_config "$member" \
      > "$FED_DIR/member-$member/dkg-final/export.log" 2>&1 &
    export_members+=("$member")
    export_pids+=("$!")

    if (( ${#export_pids[@]} >= DKG_EXPORT_JOBS )); then
      for index in "${!export_pids[@]}"; do
        if ! wait "${export_pids[$index]}"; then
          member="${export_members[$index]}"
          echo "member $member failed to export its final DKG config" >&2
          cat "$FED_DIR/member-$member/dkg-final/export.log" >&2
          return 1
        fi
      done
      export_members=()
      export_pids=()
    fi
  done

  for index in "${!export_pids[@]}"; do
    if ! wait "${export_pids[$index]}"; then
      member="${export_members[$index]}"
      echo "member $member failed to export its final DKG config" >&2
      cat "$FED_DIR/member-$member/dkg-final/export.log" >&2
      return 1
    fi
  done
}

trap cleanup EXIT
trap handle_signal INT TERM

if ! [[ "$MEMBERS" =~ ^[1-9][0-9]*$ && "$THRESHOLD" =~ ^[1-9][0-9]*$ ]]; then
  echo "MEMBERS and THRESHOLD must be positive integers" >&2
  exit 1
fi
if (( THRESHOLD > MEMBERS )); then
  echo "THRESHOLD cannot exceed MEMBERS" >&2
  exit 1
fi
if (( PORT_START + (2 * MEMBERS) - 2 > 65535 )); then
  echo "member and Prometheus port range exceeds 65535" >&2
  exit 1
fi
if ! [[ "$DKG_EXPORT_JOBS" =~ ^[1-9][0-9]*$ ]]; then
  echo "DKG_EXPORT_JOBS must be a positive integer" >&2
  exit 1
fi
if ! command -v jq >/dev/null 2>&1; then
  echo "jq is required to select the explicit local-development payment policy" >&2
  exit 1
fi

cargo build --release \
  -p cdk-mintd \
  --features federation,fakewallet,sqlite,prometheus

cargo build --release \
  -p cdk-cli \
  --features federation

just fed-regtest-clean "$FED_DIR"
rm -rf "$WALLET_DIR" "$RECEIVER_DIR"
mkdir -p "$FED_DIR" "$WALLET_DIR" "$RECEIVER_DIR"
printf '# Generated local federation material. Do not commit.\n*\n' > "$FED_DIR/.gitignore"

ROSTER_ARGS=()
for member in $(seq 1 "$MEMBERS"); do
  member_dir="$FED_DIR/member-$member"
  member_port=$((PORT_START + member - 1))
  member_url="http://127.0.0.1:${member_port}"
  mkdir -p "$member_dir"

  "$MINTD" federation dkg-member \
    --member-id "$member" \
    --public-mint-url "$member_url" \
    --federation-api-url "$member_url" \
    --public-output "$member_dir/enrollment.json" \
    --private-output "$member_dir/identity-private.json" \
    --force

  ROSTER_ARGS+=(--member "$member_dir/enrollment.json")
done

"$MINTD" federation dkg-roster \
  "${ROSTER_ARGS[@]}" \
  --threshold "$THRESHOLD" \
  --unit sat \
  --amounts 1,2,4,8,16,32,64,128 \
  --public-output "$SETUP_PUBLIC_CONFIG" \
  --ceremony-config-output "$CEREMONY_CONFIG" \
  --force

# The production roster command intentionally emits member-quorum payment
# verification. These local fakewallet processes have independent in-memory
# payment state, so use the explicit development policy for this payment smoke.
# This changes neither the member roster nor the dealer-free DKG ceremony.
policy_config="$SETUP_PUBLIC_CONFIG.tmp"
jq '
  .payment_observation_policy.mint_quotes = {"type":"development_single_observation"}
  | .payment_observation_policy.melt_quotes = {"type":"development_single_observation"}
' "$SETUP_PUBLIC_CONFIG" > "$policy_config"
mv "$policy_config" "$SETUP_PUBLIC_CONFIG"

for member in $(seq 1 "$MEMBERS"); do
  member_dir="$FED_DIR/member-$member"
  member_port=$((PORT_START + member - 1))
  prometheus_port=$((PORT_START + MEMBERS + member - 1))
  member_url="http://127.0.0.1:${member_port}"

  "$MINTD" federation dkg-bind \
    --public-config "$SETUP_PUBLIC_CONFIG" \
    --private-identity "$member_dir/identity-private.json" \
    --output "$member_dir/dkg-setup-private.json" \
    --force

  cat > "$member_dir/config.toml" <<EOF
# Local dealer-free DKG member config. Contains test secrets; do not commit.
[info]
url = "$member_url"
listen_host = "127.0.0.1"
listen_port = $member_port
seed = "manual-production-dkg-member-$member"
enable_info_page = true

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "CDK DKG federation member $member"
description = "Local dealer-free DKG member $member"

[prometheus]
enabled = false
address = "127.0.0.1"
port = $prometheus_port

[database]
engine = "sqlite"

[ln]
ln_backend = "fakewallet"
unit = "sat"
min_mint = 1
max_mint = 500000
min_melt = 1
max_melt = 500000

[fake_wallet]
supported_units = ["sat"]
fee_percent = 0.0
reserve_fee_min = 1
min_delay_time = 0
max_delay_time = 0

[federation]
enabled = true
allow_development_payment_observation_policy = true
public_config_path = "$SETUP_PUBLIC_CONFIG"
private_config_path = "$member_dir/dkg-setup-private.json"

EOF
  cat "$CEREMONY_CONFIG" >> "$member_dir/config.toml"
  chmod 600 "$member_dir/config.toml" "$member_dir/identity-private.json" \
    "$member_dir/dkg-setup-private.json"
done

echo "Generated $MEMBERS independent member identities and the public DKG roster."

for member in $(seq 1 "$MEMBERS"); do
  member_dir="$FED_DIR/member-$member"
  mkdir -p "$member_dir/work"

  env \
    CDK_MINTD_PROMETHEUS_ENABLED=false \
    CDK_FEDERATION_DISABLE_PEER_SYNC=1 \
    "$MINTD" \
      --work-dir "$member_dir/work" \
      --config "$member_dir/config.toml" \
      > "$member_dir/work/mintd.log" 2>&1 &

  DKG_PIDS+=("$!")
  echo "[member-$member] DKG started"
done

for member in $(seq 1 "$MEMBERS"); do
  echo "[member-$member] waiting for DKG completion"
  wait_for_process "$member" "${DKG_PIDS[$((member - 1))]}" "DKG completion"
done

echo "Exporting final member configs with up to $DKG_EXPORT_JOBS parallel jobs."
export_final_configs

for member in $(seq 1 "$MEMBERS"); do
  cmp -s \
    "$FINAL_PUBLIC_CONFIG" \
    "$FED_DIR/member-$member/dkg-final/federation-public.json" \
    || {
      echo "member $member produced a different final public config" >&2
      exit 1
    }
done

echo "All members produced identical final DKG public configs."

for pid in "${DKG_PIDS[@]}"; do
  kill "$pid" 2>/dev/null || true
done
for pid in "${DKG_PIDS[@]}"; do
  wait "$pid" 2>/dev/null || true
done
DKG_PIDS=()

for member in $(seq 1 "$MEMBERS"); do
  member_dir="$FED_DIR/member-$member"

  env \
    CDK_MINTD_PROMETHEUS_ENABLED=false \
    CDK_MINTD_FEDERATION_OPERATION_SUBMISSION_TIMEOUT_SECS=180 \
    "$MINTD" \
      --work-dir "$member_dir/work" \
      --config "$member_dir/config.toml" \
      >> "$member_dir/work/mintd.log" 2>&1 &

  RUNTIME_PIDS+=("$!")
  echo "[member-$member] AlephBFT runtime started"
done

for member in $(seq 1 "$MEMBERS"); do
  echo "[member-$member] waiting for runtime readiness"
  wait_for_runtime "$member" "${RUNTIME_PIDS[$((member - 1))]}"
done

"$MINTD" federation compare-digests \
  --public-config "$FINAL_PUBLIC_CONFIG" \
  --private-config "$FED_DIR/member-1/dkg-final/federation-private.json"

"$CLI" --work-dir "$WALLET_DIR" --non-interactive \
  federation add \
  --public-config "$FINAL_PUBLIC_CONFIG" \
  --local-mint-url "$MINT_URL" \
  --quote-coordinator-member-id 1 \
  --member-request-timeout-secs 180 \
  --member-fallback-timeout-secs 180 \
  --quote-status-member-timeout-secs 180 \
  --fanout-concurrency "$MEMBERS"

"$CLI" --work-dir "$WALLET_DIR" --non-interactive mint-info "$MINT_URL"

"$CLI" --work-dir "$RECEIVER_DIR" --non-interactive \
  federation add \
  --public-config "$FINAL_PUBLIC_CONFIG" \
  --local-mint-url "$MINT_URL" \
  --quote-coordinator-member-id 1 \
  --member-request-timeout-secs 180 \
  --member-fallback-timeout-secs 180 \
  --quote-status-member-timeout-secs 180 \
  --fanout-concurrency "$MEMBERS"

"$CLI" --work-dir "$WALLET_DIR" --non-interactive \
  --unit sat mint "$MINT_URL" 50 --wait-duration 300

TOKEN="$(
  "$CLI" --work-dir "$WALLET_DIR" --non-interactive \
    --unit sat send \
    --mint-url "$MINT_URL" \
    --amount 20 | tail -n 1
)"

"$CLI" --work-dir "$RECEIVER_DIR" --non-interactive \
  --unit sat receive "$TOKEN"

"$CLI" --work-dir "$RECEIVER_DIR" --non-interactive \
  --unit sat balance

INVOICE="$("$MINTD" federation fake-invoice --amount-msat 10000)"

"$CLI" --work-dir "$RECEIVER_DIR" --non-interactive \
  --unit sat melt \
  --mint-url "$MINT_URL" \
  --method bolt11 \
  --invoice "$INVOICE"

"$CLI" --work-dir "$RECEIVER_DIR" --non-interactive \
  --unit sat balance

cat <<EOF

$MEMBERS-of-$THRESHOLD dealer-free DKG federation and fakewallet operation smoke passed.
Public config: $FINAL_PUBLIC_CONFIG
Logs: $FED_DIR/member-N/work/mintd.log

Member identities, roster assembly, private identity binding, and BLS shares use
the production dealer-free DKG path. Payment observations use the explicit
development single-observation policy because each fakewallet is process-local.
EOF

read -r -p "Press Enter to stop all federation members..."
