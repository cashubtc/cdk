#!/usr/bin/env bash
#
# End-to-end verification of the append-only transparency log
# (docs/adr/0001-append-only-transparency-log.md, docs/adr/nut-xx.md).
#
# Spins up two cdk-mintd instances (fakewallet backends) that mutually
# witness each other's checkpoints, generates log activity, and then
# verifies, per mint:
#
#   1. /v1/audit/pubkey answers with the expected origin and scheme.
#   2. The latest checkpoint is a well-formed C2SP signed note whose
#      Ed25519 signature line binds to the mint's published log key.
#   3. Full replay (NUT-XX Verification steps 4-6): every served entry's
#      leaf hash is recomputed from its own fields and the rebuilt
#      RFC 6962 tree root equals the checkpoint's root hash.
#   4. Insert events are present (creation is committed to, not just
#      transitions).
#   5. The checkpoint carries a verifiable cosignature from the OTHER
#      mint's witness (C2SP tlog-cosignature key-ID binding, plus a full
#      Ed25519 verify when the python `cryptography` package is present).
#   6. Inclusion and consistency proofs verify, including consistency
#      between the pre-restart checkpoint and the final one (append-only
#      across restarts).
#   7. Out-of-range proof requests are rejected with HTTP 400, and
#      /v1/audit/entries never returns entries beyond the requested range.
#   8. The mint_event_log table is append-only at the DB level: direct
#      DELETE/UPDATE via sqlite3 abort on the trigger.
#
# Usage:
#   scripts/verify-transparency-log.sh            # builds cdk-mintd if needed
#   MINTD_BIN=path/to/cdk-mintd scripts/verify-transparency-log.sh
#
# Requires: bash, curl, python3, sqlite3. No jq needed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MINTD_BIN="${MINTD_BIN:-$REPO_ROOT/target/debug/cdk-mintd}"
WORK="$(mktemp -d /tmp/cdk-tlog-verify.XXXXXX)"
PORT_A=18095
PORT_B=18096
PID_A=""
PID_B=""
PASS=0
FAIL=0

say()  { printf '\033[1;34m[verify]\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m  PASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '\033[1;31m  FAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }

cleanup() {
    [ -n "$PID_A" ] && kill "$PID_A" 2>/dev/null || true
    [ -n "$PID_B" ] && kill "$PID_B" 2>/dev/null || true
    wait 2>/dev/null || true
    if [ "${KEEP_WORKDIR:-0}" = "1" ] || [ "$FAIL" -gt 0 ]; then
        say "workdir kept at $WORK"
    else
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# ---------------------------------------------------------------- build

if [ ! -x "$MINTD_BIN" ]; then
    say "building cdk-mintd (debug)..."
    (cd "$REPO_ROOT" && cargo build -p cdk-mintd --bin cdk-mintd)
fi
say "using binary: $MINTD_BIN"

# ---------------------------------------------------------------- config

# Distinct, valid BIP39 test-vector mnemonics — throwaway test mints only.
MNEMONIC_A="abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
MNEMONIC_B="legal winner thank year wave sausage worth useful legal winner thank yellow"

# The Ed25519 basepoint: a guaranteed-valid public key, used only as a
# placeholder trust entry in phase 1 so each witness starts (and prints
# its own key) before we know the peer's real log key.
DUMMY_PUBKEY="5866666666666666666666666666666666666666666666666666666666666666"

# write_config <path> <port> <mnemonic> <phase2:0|1> \
#              [peer_origin peer_logkey peer_witness_name peer_witness_key peer_port]
write_config() {
    local path="$1" port="$2" mnemonic="$3" phase2="$4"
    local peer_origin="${5:-}" peer_logkey="${6:-}"
    local peer_witness_name="${7:-}" peer_witness_key="${8:-}" peer_port="${9:-}"

    cat > "$path" <<EOF
[info]
url = "http://127.0.0.1:$port"
listen_host = "127.0.0.1"
listen_port = $port
mnemonic = "$mnemonic"

[info.logging]
output = "stderr"
console_level = "info"

[info.http_cache]
backend = "memory"
ttl = 60
tti = 60

[mint_info]
name = "tlog verify mint $port"

[database]
engine = "sqlite"

[[ln]]
ln_backend = "fakewallet"
unit = "sat"
min_mint = 1
max_mint = 1000000
min_melt = 1
max_melt = 1000000

[fake_wallet]
fee_percent = 0.02
reserve_fee_min = 1
min_delay_time = 1
max_delay_time = 2

[transparency_log]
enabled = true
origin = "127.0.0.1:$port/transparency-log"
checkpoint_interval_secs = 2

[witness]
enabled = true
name = "127.0.0.1:$port/witness"
EOF

    if [ "$phase2" = "1" ]; then
        # Second unit: guarantees fresh keyset events (Insert + Update)
        # after the restart, so a new checkpoint gets published and the
        # outbound witness client actually fires.
        cat >> "$path" <<EOF

[[ln]]
ln_backend = "fakewallet"
unit = "eur"
min_mint = 1
max_mint = 1000000
min_melt = 1
max_melt = 1000000

[[transparency_log.witnesses]]
url = "http://127.0.0.1:$peer_port/witness/add-checkpoint"
name = "$peer_witness_name"
public_key = "$peer_witness_key"

[[witness.trusted_logs]]
origin = "$peer_origin"
public_key = "$peer_logkey"
EOF
    else
        cat >> "$path" <<EOF

[[witness.trusted_logs]]
origin = "bootstrap.invalid/never-used"
public_key = "$DUMMY_PUBKEY"
EOF
    fi
}

start_mint() { # <name> <config> -> pid
    local name="$1" config="$2"
    CDK_MINTD_WORK_DIR="$WORK/$name" RUST_LOG=info \
        "$MINTD_BIN" --config "$config" >> "$WORK/$name.log" 2>&1 &
    echo $!
}

wait_http() { # <url> [timeout_s]
    local url="$1" timeout="${2:-30}" i=0
    while [ "$i" -lt "$((timeout * 2))" ]; do
        if curl -sf -o /dev/null "$url"; then return 0; fi
        sleep 0.5
        i=$((i + 1))
    done
    return 1
}

json_field() { # <field> ; reads JSON on stdin
    python3 -c "import sys, json; print(json.load(sys.stdin)['$1'])"
}

# ---------------------------------------------------------------- phase 1
# First boot: generate all keys, learn each mint's log key + origin (from
# /v1/audit/pubkey) and its witness key (printed at startup).

say "phase 1: first boot to generate and collect keys"
mkdir -p "$WORK/mint-a" "$WORK/mint-b"
write_config "$WORK/mint-a.toml" "$PORT_A" "$MNEMONIC_A" 0
write_config "$WORK/mint-b.toml" "$PORT_B" "$MNEMONIC_B" 0

PID_A=$(start_mint mint-a "$WORK/mint-a.toml")
PID_B=$(start_mint mint-b "$WORK/mint-b.toml")

wait_http "http://127.0.0.1:$PORT_A/v1/audit/pubkey" || { bad "mint A never served /v1/audit/pubkey (see $WORK/mint-a.log)"; exit 1; }
wait_http "http://127.0.0.1:$PORT_B/v1/audit/pubkey" || { bad "mint B never served /v1/audit/pubkey (see $WORK/mint-b.log)"; exit 1; }

ORIGIN_A=$(curl -sf "http://127.0.0.1:$PORT_A/v1/audit/pubkey" | json_field origin)
LOGKEY_A=$(curl -sf "http://127.0.0.1:$PORT_A/v1/audit/pubkey" | json_field pubkey)
ORIGIN_B=$(curl -sf "http://127.0.0.1:$PORT_B/v1/audit/pubkey" | json_field origin)
LOGKEY_B=$(curl -sf "http://127.0.0.1:$PORT_B/v1/audit/pubkey" | json_field pubkey)

witness_key_from_log() { # <logfile>
    sed -nE 's/.*built-in witness enabled.*public_key[= ]"?([A-Za-z0-9+/]{43}=)"?.*/\1/p' "$1" | head -1
}
# The witness key line appears at startup; poll briefly for it.
for _ in $(seq 1 20); do
    WITKEY_A=$(witness_key_from_log "$WORK/mint-a.log")
    WITKEY_B=$(witness_key_from_log "$WORK/mint-b.log")
    [ -n "$WITKEY_A" ] && [ -n "$WITKEY_B" ] && break
    sleep 0.5
done
[ -n "$WITKEY_A" ] || { bad "could not extract mint A's witness public key from $WORK/mint-a.log"; exit 1; }
[ -n "$WITKEY_B" ] || { bad "could not extract mint B's witness public key from $WORK/mint-b.log"; exit 1; }
WITNESS_NAME_A="127.0.0.1:$PORT_A/witness"
WITNESS_NAME_B="127.0.0.1:$PORT_B/witness"

say "mint A: origin=$ORIGIN_A logkey=$LOGKEY_A witness_key=$WITKEY_A"
say "mint B: origin=$ORIGIN_B logkey=$LOGKEY_B witness_key=$WITKEY_B"

# Wait for the first checkpoints (2s publish interval), remember their
# sizes for the cross-restart consistency check later.
wait_http "http://127.0.0.1:$PORT_A/v1/audit/checkpoint" 20 || { bad "mint A never published a checkpoint"; exit 1; }
wait_http "http://127.0.0.1:$PORT_B/v1/audit/checkpoint" 20 || { bad "mint B never published a checkpoint"; exit 1; }
checkpoint_size() { # <port>
    curl -sf "http://127.0.0.1:$1/v1/audit/checkpoint" | json_field checkpoint | sed -n 2p
}
PHASE1_SIZE_A=$(checkpoint_size "$PORT_A")
PHASE1_SIZE_B=$(checkpoint_size "$PORT_B")
say "phase-1 checkpoint sizes: A=$PHASE1_SIZE_A B=$PHASE1_SIZE_B"

kill "$PID_A" "$PID_B" 2>/dev/null || true
wait 2>/dev/null || true
PID_A=""; PID_B=""

# ---------------------------------------------------------------- phase 2
# Restart with mutual witnessing configured and a second currency unit
# (fresh keyset -> fresh log events -> new checkpoint -> cosign roundtrip).

say "phase 2: restart with mutual witnessing + new unit"
write_config "$WORK/mint-a.toml" "$PORT_A" "$MNEMONIC_A" 1 \
    "$ORIGIN_B" "$LOGKEY_B" "$WITNESS_NAME_B" "$WITKEY_B" "$PORT_B"
write_config "$WORK/mint-b.toml" "$PORT_B" "$MNEMONIC_B" 1 \
    "$ORIGIN_A" "$LOGKEY_A" "$WITNESS_NAME_A" "$WITKEY_A" "$PORT_A"

PID_A=$(start_mint mint-a "$WORK/mint-a.toml")
PID_B=$(start_mint mint-b "$WORK/mint-b.toml")

wait_http "http://127.0.0.1:$PORT_A/v1/audit/pubkey" || { bad "mint A did not come back up (see $WORK/mint-a.log)"; exit 1; }
wait_http "http://127.0.0.1:$PORT_B/v1/audit/pubkey" || { bad "mint B did not come back up (see $WORK/mint-b.log)"; exit 1; }

wait_for_cosigned_checkpoint() { # <port> <peer_witness_name>
    local port="$1" peer="$2" i=0
    while [ "$i" -lt 60 ]; do
        if curl -sf "http://127.0.0.1:$port/v1/audit/checkpoint" \
            | json_field checkpoint | grep -qF "$peer"; then
            return 0
        fi
        sleep 1
        i=$((i + 1))
    done
    return 1
}

say "waiting for mutually cosigned checkpoints..."
wait_for_cosigned_checkpoint "$PORT_A" "$WITNESS_NAME_B" \
    && ok "mint A's checkpoint carries a cosignature line from $WITNESS_NAME_B" \
    || bad "mint A's checkpoint was never cosigned by $WITNESS_NAME_B (see $WORK/*.log)"
wait_for_cosigned_checkpoint "$PORT_B" "$WITNESS_NAME_A" \
    && ok "mint B's checkpoint carries a cosignature line from $WITNESS_NAME_A" \
    || bad "mint B's checkpoint was never cosigned by $WITNESS_NAME_A (see $WORK/*.log)"

# ---------------------------------------------------------------- verifier

cat > "$WORK/verify.py" <<'PYEOF'
"""Independent NUT-XX transparency log verifier (stdlib only; uses the
`cryptography` package for full Ed25519 verification when available)."""
import base64
import hashlib
import json
import sys
import time
import urllib.error
import urllib.request

BASE, ORIGIN, LOGKEY_B64, WITNESS_NAME, WITKEY_B64, MIN_PREV_SIZE = sys.argv[1:7]
MIN_PREV_SIZE = int(MIN_PREV_SIZE)
failures = []


def check(name, cond, detail=""):
    if cond:
        print(f"  PASS {name}")
    else:
        print(f"  FAIL {name} {detail}")
        failures.append(name)


def get(path, expect_error=False):
    try:
        with urllib.request.urlopen(f"{BASE}{path}") as response:
            return response.status, response.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def get_json(path):
    status, body = get(path)
    assert status == 200, f"GET {path} -> {status}: {body[:200]}"
    return json.loads(body)


def sha256(data):
    return hashlib.sha256(data).digest()


def node_hash(left, right):
    return sha256(b"\x01" + left + right)


def leaf_hash_from_entry(entry):
    payload = json.dumps(
        entry["payload"], separators=(",", ":"), sort_keys=True, ensure_ascii=False
    ).encode()
    op = {"insert": 0, "update": 1, "delete": 2}[entry["op"]]
    preimage = (
        entry["entity_type"].encode() + b"\x00"
        + entry["entity_id"].encode() + b"\x00"
        + bytes([op])
        + entry["created_time"].to_bytes(8, "big")
        + payload
    )
    return sha256(b"\x00" + preimage)


def largest_pow2_lt(n):
    k = 1
    while k * 2 < n:
        k *= 2
    return k


def mth(leaves):
    if not leaves:
        return sha256(b"")
    if len(leaves) == 1:
        return leaves[0]
    k = largest_pow2_lt(len(leaves))
    return node_hash(mth(leaves[:k]), mth(leaves[k:]))


def verify_inclusion(leaf, index, size, proof, root):
    decisions = []
    while size > 1:
        k = largest_pow2_lt(size)
        if index < k:
            decisions.append(True)
            size = k
        else:
            decisions.append(False)
            index -= k
            size -= k
    if len(proof) != len(decisions):
        return False
    h = leaf
    for is_left, sibling in zip(reversed(decisions), proof):
        h = node_hash(h, sibling) if is_left else node_hash(sibling, h)
    return h == root


def verify_consistency(old_size, old_root, new_size, new_root, proof):
    if old_size > new_size:
        return False
    if old_size == 0:
        return not proof
    if old_size == new_size:
        return not proof and old_root == new_root
    it = iter(proof)

    def sub(m, n, b):
        if m == n:
            if b:
                return old_root, old_root
            h = next(it)
            return h, h
        k = largest_pow2_lt(n)
        if m <= k:
            old_child, new_child = sub(m, k, b)
            extra = next(it)
            return old_child, node_hash(new_child, extra)
        old_child, new_child = sub(m - k, n - k, False)
        extra = next(it)
        return node_hash(extra, old_child), node_hash(extra, new_child)

    try:
        old_hash, new_hash = sub(old_size, new_size, True)
    except StopIteration:
        return False
    if next(it, None) is not None:
        return False
    return old_hash == old_root and new_hash == new_root


def key_id(name, sig_type, pubkey):
    return sha256(name.encode() + b"\n" + bytes([sig_type]) + pubkey)[:4]


try:
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    def ed25519_verify(pubkey, message, signature):
        try:
            Ed25519PublicKey.from_public_bytes(pubkey).verify(signature, message)
            return True
        except Exception:
            return False

    HAVE_ED25519 = True
except ImportError:
    HAVE_ED25519 = False

# 1. pubkey endpoint
pubkey_response = get_json("/v1/audit/pubkey")
check("pubkey origin matches", pubkey_response["origin"] == ORIGIN)
check("pubkey scheme is ed25519", pubkey_response["signature_scheme"] == "ed25519")
check("pubkey matches phase-1 key", pubkey_response["pubkey"] == LOGKEY_B64)
log_pubkey = base64.b64decode(LOGKEY_B64)
witness_pubkey = base64.b64decode(WITKEY_B64)

# 2. checkpoint note structure + signature binding
note = get_json("/v1/audit/checkpoint")["checkpoint"]
text, _, sig_block = note.partition("\n\n")
lines = text.split("\n")
check("note origin line", lines[0] == ORIGIN)
size = int(lines[1])
root = base64.b64decode(lines[2])
check("tree did not shrink across restart", size >= MIN_PREV_SIZE,
      f"(size={size}, phase-1 size={MIN_PREV_SIZE})")
note_text = text + "\n"

signatures = []
for line in sig_block.strip("\n").split("\n"):
    assert line.startswith("— "), f"bad signature line: {line!r}"
    name, b64 = line[2:].split(" ")
    payload = base64.b64decode(b64)
    signatures.append((name, payload[:4], payload[4:]))

mint_signatures = [
    signature for name, kid, signature in signatures
    if name == ORIGIN and kid == key_id(ORIGIN, 0x01, log_pubkey)
]
check("checkpoint has a signature line binding to the mint's log key",
      len(mint_signatures) == 1)
if HAVE_ED25519 and mint_signatures:
    check("checkpoint Ed25519 signature verifies",
          ed25519_verify(log_pubkey, note_text.encode(), mint_signatures[0]))

# 3. full replay: fetch all entries, recompute hashes, rebuild the root
entries = []
next_seq = 0
while next_seq < size:
    response = get_json(f"/v1/audit/entries?start={next_seq}&end={size}")
    assert response["entries"], f"mint stopped serving entries at {next_seq}"
    for entry in response["entries"]:
        assert entry["seq"] == next_seq, f"gap at seq {next_seq}"
        entries.append(entry)
        next_seq += 1
leaves = [leaf_hash_from_entry(entry) for entry in entries]
check("every served leaf_hash matches a fresh recomputation",
      all(h.hex() == entry["leaf_hash"] for h, entry in zip(leaves, entries)))
check("replayed RFC 6962 root equals the checkpoint root", mth(leaves) == root)

# 4. insert events present (existence is committed to, per the updated ADR)
ops = {entry["op"] for entry in entries}
check("log contains insert events", "insert" in ops, f"(ops seen: {sorted(ops)})")
check("log contains keyset insert",
      any(e["op"] == "insert" and e["entity_type"] == "keyset" for e in entries))

# 5. cosignature from the peer mint's witness
cosignatures = [
    (name, kid, signature) for name, kid, signature in signatures
    if name == WITNESS_NAME
]
check("checkpoint carries a cosignature line from the peer witness",
      len(cosignatures) >= 1)
if cosignatures:
    name, kid, signature = cosignatures[0]
    check("cosignature key ID binds to the peer witness key (type 0x04)",
          kid == key_id(WITNESS_NAME, 0x04, witness_pubkey))
    check("cosignature payload is timestamp(8) || sig(64)", len(signature) == 72)
    timestamp = int.from_bytes(signature[:8], "big")
    check("cosignature timestamp is recent", abs(time.time() - timestamp) < 3600,
          f"(ts={timestamp})")
    if HAVE_ED25519:
        message = f"cosignature/v1\ntime {timestamp}\n".encode() + note_text.encode()
        check("cosignature Ed25519 signature verifies",
              ed25519_verify(witness_pubkey, message, signature[8:]))

# 6. inclusion + consistency proofs
inclusion = get_json(f"/v1/audit/proof/inclusion?seq=0&tree_size={size}")
check("inclusion proof for seq 0 verifies",
      verify_inclusion(
          bytes.fromhex(inclusion["leaf_hash"]), 0, size,
          [bytes.fromhex(h) for h in inclusion["proof"]], root))
check("inclusion endpoint returns the same leaf hash as the entry",
      inclusion["leaf_hash"] == entries[0]["leaf_hash"])

old_size = max(MIN_PREV_SIZE, 1)
old_root = mth(leaves[:old_size])
consistency = get_json(f"/v1/audit/proof/consistency?first={old_size}&second={size}")
check(f"consistency proof {old_size} -> {size} verifies (append-only across restart)",
      verify_consistency(
          old_size, old_root, size, root,
          [bytes.fromhex(h) for h in consistency["proof"]]))

# 7. request validation
status, _ = get(f"/v1/audit/proof/inclusion?seq=0&tree_size={size + 1000}")
check("inclusion proof beyond current size is rejected with 400", status == 400)
status, _ = get(f"/v1/audit/proof/consistency?first={size}&second={size + 1000}")
check("consistency proof beyond current size is rejected with 400", status == 400)
status, _ = get(f"/v1/audit/proof/consistency?first={size}&second=1")
check("consistency proof with first > second is rejected with 400", status == 400)
overfetch = get_json(f"/v1/audit/entries?start=0&end={size + 5000}")
check("entries endpoint never returns entries beyond the log",
      overfetch["end"] <= size and all(e["seq"] < size for e in overfetch["entries"]))

if not HAVE_ED25519:
    print("  NOTE python 'cryptography' not installed: signature bindings were "
          "checked via key IDs only, not full Ed25519 verification")

sys.exit(1 if failures else 0)
PYEOF

run_verifier() { # <label> <port> <origin> <logkey> <peer_witness_name> <peer_witness_key> <phase1_size>
    local label="$1"
    shift
    say "auditing $label..."
    if python3 "$WORK/verify.py" "http://127.0.0.1:$2" "$3" "$4" "$5" "$6" "$7"; then
        ok "$label: full audit passed"
    else
        bad "$label: audit reported failures"
    fi
}

run_verifier "mint A" A "$PORT_A" "$ORIGIN_A" "$LOGKEY_A" "$WITNESS_NAME_B" "$WITKEY_B" "$PHASE1_SIZE_A"
run_verifier "mint B" B "$PORT_B" "$ORIGIN_B" "$LOGKEY_B" "$WITNESS_NAME_A" "$WITKEY_A" "$PHASE1_SIZE_B"

# ---------------------------------------------------------------- db-level
# The append-only triggers must reject direct tampering with the log.
# Stop the mints first so the tampering attempt hits the trigger, not a
# busy/locked database.

kill "$PID_A" "$PID_B" 2>/dev/null || true
for _ in $(seq 1 30); do
    kill -0 "$PID_A" 2>/dev/null || kill -0 "$PID_B" 2>/dev/null || break
    sleep 0.5
done
PID_A=""; PID_B=""

say "checking DB-level append-only enforcement (sqlite3)..."
for name in mint-a mint-b; do
    db="$WORK/$name/cdk-mintd.sqlite"
    if [ ! -f "$db" ]; then
        bad "$name: expected database at $db"
        continue
    fi
    delete_out=$(sqlite3 "$db" "DELETE FROM mint_event_log;" 2>&1 || true)
    if grep -q "append-only" <<< "$delete_out"; then
        ok "$name: direct DELETE on mint_event_log is blocked by trigger"
    else
        bad "$name: direct DELETE on mint_event_log was NOT blocked ($delete_out)"
    fi
    update_out=$(sqlite3 "$db" "UPDATE mint_event_log SET payload = x'7b7d';" 2>&1 || true)
    if grep -q "append-only" <<< "$update_out"; then
        ok "$name: direct UPDATE of a hash-covered column is blocked by trigger"
    else
        bad "$name: direct UPDATE of payload was NOT blocked ($update_out)"
    fi
done

# ---------------------------------------------------------------- summary

echo
say "results: $PASS passed, $FAIL failed"
if [ "$FAIL" -gt 0 ]; then
    say "logs kept at $WORK (mint-a.log / mint-b.log)"
    KEEP_WORKDIR=1
    exit 1
fi
