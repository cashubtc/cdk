# NUT-XX: Mint Transparency Log

`optional`

`depends on: NUT-06`

`uses: NUT-00 NUT-01 NUT-02 NUT-05 NUT-07 NUT-12`

This NUT defines an append-only transparency log for mint events. A mint that
implements this NUT publishes a sequence of protocol events, folds them into a
Merkle tree, and periodically signs checkpoints that commit to the tree size and
root hash.

Wallets, auditors, and other third parties can use the log to:

1. replay the mint's protocol-visible state transitions,
2. verify that an event was included in a signed checkpoint,
3. verify that two checkpoints are append-only consistent, and
4. detect retroactive edits when checkpoints are witnessed, anchored, pinned, or
   compared between observers.

This NUT does not change mint, melt, or swap flows. It only adds audit endpoints.

Checkpoints, their signatures, and witness cosignatures use the same wire
formats as [c2sp.org/tlog-checkpoint][tlog-checkpoint],
[c2sp.org/signed-note][signed-note], and [c2sp.org/tlog-cosignature][tlog-cosig]
— the formats already spoken by [Sigsum](https://www.sigsum.org/),
[Tessera](https://github.com/transparency-dev/tessera), and Sunlight-family
static-CT logs. A mint's checkpoint can therefore be submitted to, and cosigned
by, any already-running witness that speaks the
[C2SP tlog-witness protocol][tlog-witness] — including another mint's own
built-in witness (see [Witnessing](#witnessing)) — without inventing a new
format. Reference implementation: [`cdk-tlog`][cdk-tlog] (Merkle math and
checkpoint/note formats) and [`cdk-sigsum`][cdk-sigsum] (client for anchoring
checkpoints to Sigsum's public log), both in the [cashubtc/cdk][cdk] repository.
See `docs/adr/0001-append-only-transparency-log.md` in that repository for the
design rationale.

[tlog-checkpoint]: https://c2sp.org/tlog-checkpoint
[signed-note]: https://c2sp.org/signed-note
[tlog-cosig]: https://c2sp.org/tlog-cosignature
[tlog-witness]: https://github.com/C2SP/C2SP/blob/main/tlog-witness.md
[cdk-tlog]: https://github.com/cashubtc/cdk/tree/main/crates/cdk-tlog
[cdk-sigsum]: https://github.com/cashubtc/cdk/tree/main/crates/cdk-sigsum
[cdk]: https://github.com/cashubtc/cdk

---

## Overview

A mint maintains a log of `MintLogEntry` leaves. Entries are assigned a
monotonically increasing `seq` at append time. `seq` is a stable ordering key
for querying, not part of what gets hashed (see [Leaf Hash](#leaf-hash)) — RFC
6962 leaves never encode their own tree position, since the position is
implicit from where the leaf lands in the tree. If the latest checkpoint has
`tree_size = n`, valid entries are in the range `0 <= seq < n`.

Each log entry contains:

```json
{
  "seq": 0,
  "entity_type": "proof",
  "op": "update",
  "entity_id": "02599b9ea0a1ad4143706c2a5a4a568ce442dd4313e1cf1f7f0b58a317c1a355ee",
  "payload": {"state": "SPENT"},
  "created_time": 1782920900,
  "leaf_hash": "37d2b94f85c645e97fd63d8edfa5ab31ccdc9fb85b06e3c02b45d221f2a8cf61"
}
```

Where:

* `seq` is the zero-based leaf index.
* `entity_type` is the event kind.
* `op` is `"update"` or `"delete"`. Entities in scope for this NUT are never
  logged as `"insert"` — see [Event Kinds](#event-kinds).
* `entity_id` is the event's stable entity identifier.
* `payload` is a JSON object containing *only the fields the mutation actually
  wrote*, not a full snapshot of the entity. This keeps the log's shape tied
  to actual write paths instead of a hand-maintained schema that can silently
  drift out of sync with them.
* `created_time` is a Unix timestamp in seconds.
* `leaf_hash` is the lowercase hex-encoded SHA-256 leaf hash.

Receivers MUST ignore unknown fields in `MintLogEntry`.

### Event Kinds

Mints implementing this NUT MUST log every financially meaningful mutation that
affects the following protocol-visible entities after the log is enabled:

| `entity_type` | `entity_id` | Purpose |
|---|---|---|
| `proof` | `Y`, the compressed point from `hash_to_curve(secret)` | Records proof state changes using the states from NUT-07, and removals on compensation. |
| `blind_signature` | `B_`, the blinded message being signed | Records a blind signature's DLEQ fields being filled in after being initially stored with placeholders. |
| `keyset` | keyset `id` | Records keyset activation/deactivation from NUT-02. |
| `melt_quote` | quote id | Records melt quote state and payment-lookup changes from NUT-05 and payment-method NUTs. |

Insert-only entities (mint quotes, blind signatures at creation time, completed
operations) are already their own complete history and are out of scope for
this log — logging them would duplicate data without adding auditability.

Mints MAY add implementation-specific event kinds. Implementations that replay
the log MUST preserve unknown event kinds but MAY ignore them when reconstructing
protocol state.

### Payloads

Payloads are JSON objects containing only the fields the triggering mutation
wrote. For readability, the exact shapes a conformant implementation produces
are shown below (not full entity snapshots).

#### `proof`

On a state change (`op: "update"`):

```json
{ "state": "SPENT" }
```

`state` MUST be one of `"UNSPENT"`, `"PENDING"`, or `"SPENT"` as defined in
NUT-07. On removal during compensation (`op: "delete"`):

```json
{ "state": "removed" }
```

#### `blind_signature`

Logged only when a previously-placeholder row is filled in with its DLEQ
fields (`op: "update"`):

```json
{
  "c": "0277d1de806ed177007e5b94a8139343b6382e472c752a74e99949d511f7194f6c",
  "dleq_e": "...",
  "dleq_s": "...",
  "signed_time": 1782920900,
  "amount": 8
}
```

`dleq_e`/`dleq_s` MAY be `null` if the mint did not return a DLEQ proof.

#### `keyset`

```json
{ "active": false }
```

#### `melt_quote`

On a request-lookup-id change:

```json
{ "request_lookup_id": "...", "request_lookup_id_kind": "..." }
```

On a state change:

```json
{
  "state": "PAID",
  "fee_reserve": 10,
  "estimated_blocks": null,
  "selected_fee_index": null,
  "paid_time": 1782920900,
  "payment_proof": "..."
}
```

`paid_time` and `payment_proof` are only present when `state` is `"PAID"`.
Fields that would reveal a wallet secret MUST NOT be included.

## Leaf Hash

The leaf preimage is the concatenation of:

```text
utf8(entity_type) || 0x00 ||
utf8(entity_id) || 0x00 ||
uint8(op) ||
uint64_be(created_time) ||
payload
```

Where `payload` is the canonical JSON-encoded bytes of the `payload` object
above, and `op` is encoded as:

| `op` | Value |
|---|---:|
| `"update"` | `1` |
| `"delete"` | `2` |

The leaf hash is the RFC 6962 leaf hash:

```text
leaf_hash = SHA256(0x00 || leaf_preimage)
```

Mints MUST return `leaf_hash` as lowercase hex. Verifiers MUST recompute
`leaf_hash` and reject entries whose returned hash does not match. Note that
`seq` and `created_time`'s position notwithstanding, `seq` itself is never part
of the preimage.

## Merkle Tree

The tree hash is the RFC 6962 Merkle Tree Hash using SHA-256, with `d[i]`
being the leaf hash (not the preimage) for entry `i`:

```text
MTH({}) = SHA256()
MTH({d[0]}) = d[0]
MTH(D[n]) = SHA256(0x01 || MTH(D[0:k]) || MTH(D[k:n]))
```

Where `k` is the largest power of two smaller than `n`.

Implementations MAY store the tree in any form, including as a Merkle Mountain
Range, as long as inclusion proofs, consistency proofs, and checkpoint roots
verify against the RFC 6962 tree hash.

## Checkpoints

A checkpoint is a [c2sp.org/tlog-checkpoint][tlog-checkpoint]: an origin, a
tree size, and a root hash, signed as a [c2sp.org/signed-note][signed-note].
The **origin** is the mint's log identity — a schema-less string such as
`<mint-host>/transparency-log` — used both as the note's first line and as the
`name` in its signature line.

```text
<origin>
<tree_size>
<base64(root_hash)>

— <origin> <base64(4-byte key ID || Ed25519 signature)>
```

Example:

```text
mint.example.com/transparency-log
12345
GPDU9QyPLS0jybea1MOPmcE7bZFyGewvcqVgqFmi4qc=

— mint.example.com/transparency-log Az3grlgtzPICa5OS8npVmf1Myq/5IZniMp+ZJurmRDeOoRDe4URYN7u5/Zhcyv2q1gGzGku9nTo+zyWE+xeMcTOAYQ8=
```

The signature is a plain Ed25519 signature (signed-note type `0x01`) directly
over the checkpoint's note text (the three lines above plus their trailing
newlines). The key ID is
`SHA256(origin || 0x0A || 0x01 || 32-byte Ed25519 public key)[:4]`, per
signed-note. The signing key MUST be a dedicated log-signing key. Mints MUST
NOT use their NUT-01 mint signing keys as log-signing keys.

Any witness cosignatures (see [Witnessing](#witnessing)) are additional
signature lines appended to the same note, using Ed25519 checkpoint
cosignatures (signed-note type `0x04`, per
[c2sp.org/tlog-cosignature][tlog-cosig]).

### Witnessing

A signed checkpoint proves the mint's log key committed to a tree root. It
does not by itself prove the mint showed the same checkpoint to everyone — see
[Security Considerations](#security-considerations). Mints SHOULD have their
checkpoints cosigned by one or more witnesses speaking the
[C2SP tlog-witness protocol][tlog-witness] (`add-checkpoint`), which already
has independent, publicly-reachable operators (see
[sigsum.org/services](https://www.sigsum.org/services/)) as well as the option
for mints to witness each other: a mint MAY expose its own witness endpoint
implementing the same protocol, cosigning checkpoints from other mints and/or
other transparency logs it has an opinion about, purely by tracking, per
origin, the largest checkpoint size it has already cosigned.

### External Anchors

Mints MAY additionally submit a checkpoint's `SHA256(SHA256(note_text))` to a
content-agnostic public transparency log, such as Sigsum's `seasalp`
([sigsum.org/services](https://www.sigsum.org/services/)), or to a Bitcoin-
anchored timestamping service such as [OpenTimestamps](https://opentimestamps.org).
These anchors are recorded as an opaque, out-of-band record associated with
the checkpoint (e.g. served alongside it by the mint) and are not required for
inclusion/consistency verification against the mint's own tree — they only
help third parties detect equivocation, per [Witnessing](#witnessing).

## Mint Info

Until this NUT is assigned a number, wallets discover support by probing
`GET /v1/audit/pubkey` — a mint that implements this NUT MUST answer it, and
a 404 means the log is not enabled. (The reference implementation does not
advertise a `nuts` entry yet for exactly this reason: NUT-06 `nuts` keys are
NUT numbers, and claiming an unassigned one would collide once assigned.)

Once numbered, mints advertise support in the NUT-06 info response:

```json
{
  "nuts": {
    "XX": {
      "supported": true,
      "origin": "mint.example.com/transparency-log",
      "pubkey": "MtN3XdxeMTUmzeUpFcQGHz4TZ8DPmgocpe0oXPZlm+8=",
      "checkpoint": "/v1/audit/checkpoint",
      "max_entries": 1000
    }
  }
}
```

Where:

* `origin` is the checkpoint origin line this mint signs (see
  [Checkpoints](#checkpoints)).
* `pubkey` is the base64-encoded 32-byte Ed25519 log-signing public key.
* `checkpoint` is the endpoint for the latest checkpoint.
* `max_entries` is the maximum number of entries returned by one entries
  request.

## Endpoints

### Get Log Public Key

```http
GET https://mint.host:3338/v1/audit/pubkey
```

Response:

```json
{
  "origin": "mint.example.com/transparency-log",
  "pubkey": "MtN3XdxeMTUmzeUpFcQGHz4TZ8DPmgocpe0oXPZlm+8=",
  "signature_scheme": "ed25519"
}
```

### Get Latest Checkpoint

```http
GET https://mint.host:3338/v1/audit/checkpoint
```

Response:

```json
{
  "checkpoint": "mint.example.com/transparency-log\n12345\nGPDU9QyPLS0jybea1MOPmcE7bZFyGewvcqVgqFmi4qc=\n\n\u2014 mint.example.com/transparency-log Az3grlgtzPICa5OS8npVmf1Myq/5IZniMp+ZJurmRDeOoRDe4URYN7u5/Zhcyv2q1gGzGku9nTo+zyWE+xeMcTOAYQ8=\n"
}
```

`checkpoint` is the full C2SP signed note (checkpoint text, a blank line, then
one signature line per key that has signed or cosigned it), so that a generic
C2SP-aware verifier can consume the string directly without knowing anything
about Cashu.

### Get Historical Checkpoint

```http
GET https://mint.host:3338/v1/audit/checkpoint/{tree_size}
```

Same response shape as above, for exactly `tree_size`. If the mint has no
checkpoint at that size, it MUST return an error.

### Get Entries

```http
GET https://mint.host:3338/v1/audit/entries?start=0&end=100
```

`start` is inclusive and `end` is exclusive. The mint MAY return fewer entries
than requested (bounded by `max_entries` from NUT-06), but it MUST NOT return
entries outside the requested range.

Response:

```json
{
  "start": 0,
  "end": 2,
  "entries": [
    {
      "seq": 0,
      "entity_type": "keyset",
      "op": "update",
      "entity_id": "009a1f293253e41e",
      "payload": {"active": true},
      "created_time": 1782920800,
      "leaf_hash": "..."
    },
    {
      "seq": 1,
      "entity_type": "proof",
      "op": "update",
      "entity_id": "02599b9ea0a1ad4143706c2a5a4a568ce442dd4313e1cf1f7f0b58a317c1a355ee",
      "payload": {"state": "SPENT"},
      "created_time": 1782920900,
      "leaf_hash": "..."
    }
  ]
}
```

`end` in the response is the exclusive end of the returned range. If no entries
are available, `entries` MUST be an empty array and `start` MUST equal `end`.

### Get Inclusion Proof

```http
GET https://mint.host:3338/v1/audit/proof/inclusion?seq=1&tree_size=12345
```

Response:

```json
{
  "seq": 1,
  "tree_size": 12345,
  "leaf_hash": "37d2b94f85c645e97fd63d8edfa5ab31ccdc9fb85b06e3c02b45d221f2a8cf61",
  "proof": [
    "a2ad5b7f3d87...",
    "9c6fd22cba31..."
  ]
}
```

`proof` is the RFC 6962 inclusion audit path from the leaf to the root, ordered
from leaf level to root level. The verifier derives whether each sibling is on
the left or the right from `seq` and `tree_size`.

The mint MUST reject a request where `seq >= tree_size`.

### Get Consistency Proof

```http
GET https://mint.host:3338/v1/audit/proof/consistency?first=100&second=12345
```

Response:

```json
{
  "first": 100,
  "second": 12345,
  "proof": [
    "ad7f9c1b...",
    "70e30f1e..."
  ]
}
```

`proof` is the RFC 6962 consistency proof showing that the tree at size
`second` is an append-only extension of the tree at size `first`.

The mint MUST reject a request where `first > second`.

## Verification

To verify the latest state of the log, a wallet or auditor:

1. Fetches the log public key and origin from `/v1/audit/pubkey` or NUT-06.
2. Fetches the latest checkpoint from `/v1/audit/checkpoint`.
3. Verifies the checkpoint's Ed25519 signature (and any cosignatures from
   witnesses it trusts) per signed-note.
4. Fetches entries in `[0, tree_size)` using `/v1/audit/entries`.
5. Recomputes every `leaf_hash`.
6. Rebuilds the RFC 6962 Merkle tree and verifies the root equals the
   checkpoint's root hash.
7. Optionally verifies any known external anchors.

To verify that a specific entry is included in a checkpoint, a verifier:

1. Fetches the entry and recomputes its `leaf_hash`.
2. Fetches `/v1/audit/proof/inclusion?seq={seq}&tree_size={tree_size}`.
3. Verifies the inclusion proof against the checkpoint root.

To verify append-only behavior between two checkpoints, a verifier:

1. Verifies both checkpoint signatures.
2. Fetches `/v1/audit/proof/consistency?first={old_size}&second={new_size}`.
3. Verifies the consistency proof against both checkpoint roots.

Wallets SHOULD remember the largest verified, witness-cosigned checkpoint per
mint and require new checkpoints to be append-only consistent with it — the
same gossip/pinning pattern used by Certificate Transparency and Sigsum
clients.

## Privacy Considerations

This NUT intentionally makes more mint state observable. Mints MUST NOT include
wallet secrets, proof `secret` values, blinding factors, private keys, access
tokens, or authentication credentials in log payloads.

For proof events, mints MUST identify proofs by `Y = hash_to_curve(secret)`,
not by `secret`.

Mints SHOULD document whether their transparency log is public, authenticated,
rate-limited, or shared only with selected auditors. A mint that restricts access
to entries can still produce verifiable checkpoints, but public observers will
not be able to perform full replay unless they can obtain the entries.

Mints MUST NOT submit raw log entries to any external anchor or witness — only
the checkpoint (a root hash, a size, a timestamp) ever leaves the mint's own
`/v1/audit/*` surface for that purpose. External services are shared,
general-purpose infrastructure sized for anchoring periodic commitments, not
for hosting a mint's full event stream.

## Security Considerations

A signed checkpoint proves that the mint's log-signing key committed to a tree
root. By itself, it does not prove that the mint showed the same checkpoint to
everyone. Wallet pinning, gossip, independent witnesses (see
[Witnessing](#witnessing)), or external anchors are needed to detect
equivocation by the mint operator.

A mint that loses its log-signing key MUST rotate to a new log key and publish
a log event and checkpoint that identifies the new key. Wallets and auditors
SHOULD treat unexplained log key changes as suspicious.

Mints MUST append log entries in the same transaction as the state mutation they
describe, or otherwise ensure that no committed financial mutation can be
omitted from the log.

## Open Questions

* Should there be a standardized genesis snapshot event for mints enabling this
  NUT after they already have live state?
* Which error-code range should be reserved for audit endpoint failures?
* Should this NUT specify a discovery mechanism for a mint's chosen witnesses,
  or leave witness selection entirely to wallet/auditor policy (as Sigsum
  policy files do)?

[00]: 00.md
[01]: 01.md
[02]: 02.md
[05]: 05.md
[06]: 06.md
[07]: 07.md
[12]: 12.md
