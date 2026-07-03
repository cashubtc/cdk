# Transparency log: implementation & testing notes

Status: implemented, partially verified live. This is a knowledge dump of
everything built so far for the append-only transparency log feature (see
`0001-append-only-transparency-log.md` for the design ADR and `nut-xx.md` for
the wallet-facing protocol draft), plus open questions from live testing.
Written to let anyone pick this up in a fresh session without re-deriving
context.

## 1. What exists, where

### New crates

**`crates/cdk-tlog`** — pure logic, no DB/HTTP dependency. Three modules:

- `merkle.rs` — RFC 6962 hashing, `TreeHead` (Merkle Mountain Range
  append/root in O(log n)), `inclusion_proof`/`verify_inclusion`,
  `consistency_proof`/`verify_consistency`. Verified against RFC 6962's own
  worked example (`PROOF(3, D[7]) = [c, d, g, l]`) as unit tests, not just
  internal round-trips. One real bug (MMR peak-carry logic) was caught this
  way and fixed.
- `checkpoint.rs` — [c2sp.org/tlog-checkpoint](https://c2sp.org/tlog-checkpoint),
  [c2sp.org/signed-note](https://c2sp.org/signed-note), and
  [c2sp.org/tlog-cosignature](https://c2sp.org/tlog-cosignature) formats:
  `Checkpoint`, `SignedCheckpoint`, `SignatureLine`, `sign_checkpoint`,
  `cosign`, `verify_checkpoint_signature`, `verify_cosignature_with_timestamp`.
  Ed25519 throughout (not secp256k1 — see §6).
- `witness.rs` — pure decision logic for the
  [C2SP tlog-witness `add-checkpoint`](https://github.com/C2SP/C2SP/blob/main/tlog-witness.md)
  call: `AddCheckpointRequest::parse`/`to_body`, `consider_checkpoint`,
  `WitnessError` (maps 1:1 to the spec's HTTP status codes: 404 unknown
  origin, 403 no trusted signature, 400 bad old-size, 409 size/root
  conflict, 422 bad consistency proof).

29 unit tests total in this crate, all passing.

**`crates/cdk-sigsum`** — client for Sigsum's log protocol (submit leaf,
poll tree head, fetch inclusion proof, assemble offline-verifiable proof).
Built earlier in this work; `TransparencyLogService` can optionally anchor
checkpoints to it (feature `sigsum-anchor`). Not wired into any live test —
untested against a real Sigsum log.

### Database layer

**`crates/cdk-common/src/database/mint/transparency.rs`** (new) — additive
trait, does **not** touch the existing `Database`/`Transaction` trait
hierarchy:

```rust
pub enum LoggedEntity { MeltQuote, Proof, Keyset, BlindSignature }
pub enum EventOp { Insert = 0, Update = 1, Delete = 2 }
pub struct MintEventLogEntry { seq, entity_type, entity_id, op, payload, leaf_hash, created_time }
pub trait TransparencyLogDatabase {
    // Assigns dense zero-based leaf indices to committed-but-unindexed
    // rows (in row-id order) and returns them. Only the single appender
    // task calls this.
    async fn assign_leaf_indices(&self, max: u64) -> Result<Vec<MintEventLogEntry>, Self::Err>;
    // Range query over *leaf index* (MintEventLogEntry.seq), not row id.
    async fn get_event_log_range(&self, start: u64, end: u64) -> Result<Vec<MintEventLogEntry>, Self::Err>;
}
pub type DynTransparencyLogDatabase = Arc<dyn TransparencyLogDatabase<Err = Error> + Send + Sync>;
```

**Two numberings, deliberately.** The table's auto-increment row id
(`seq` column) is only a commit-order hint; the Merkle tree position is a
separate nullable `leaf_index` column, assigned densely (zero-based) by the
single appender task when it folds committed rows into the tree — the same
sequencer-assigns-the-index model CT/Sigsum use. This is what makes
row-id gaps harmless: on Postgres, `IDENTITY` values burned by rolled-back
transactions are *permanent* gaps, and an appender keyed on row id would
stall at the first one forever. `MintEventLogEntry.seq` as exposed
everywhere (trait, service, HTTP) is the leaf index, which also matches
NUT-XX's zero-based `seq` with no translation.

`event_leaf_preimage(entity_type, entity_id, op, payload, created_time)` is
the canonical byte preimage hashed to get `leaf_hash`. **`seq` is
deliberately excluded** from the hash — RFC 6962 leaves never encode their
own tree position; it's implicit from where they land. This also avoids a
chicken-and-egg problem: `seq` isn't known until the `INSERT` that assigns
it has already happened.

**`crates/cdk-sql-common/src/mint/event_log.rs`** (new) — `append_event()`,
called directly (same connection, same transaction) from the mutation
sites. As of 2026-07-03 creation is logged too (`op=Insert`), so the tree
commits to row *existence*, replay works from the log alone, and issuance
is no longer path-dependent:

| File | Method | Logs |
|---|---|---|
| `quotes.rs` | `add_melt_quote` | `op=Insert`: `{"amount","unit","fee_reserve","state","expiry","payment_method","request_lookup_id","request_lookup_id_kind"}` — **never** the full `request` (reveals payment destination) |
| `quotes.rs` | `update_melt_quote_request_lookup_id` | `{"request_lookup_id", "request_lookup_id_kind"}` |
| `quotes.rs` | `update_melt_quote_state` | `{"state","fee_reserve","estimated_blocks","selected_fee_index"[,"paid_time","payment_proof"]}` |
| `proofs.rs` | `add_proofs` | one event per Y, `op=Insert`: `{"amount","keyset_id","state"}` — never `secret`/`c`/`witness` |
| `proofs.rs` | `update_proofs_state` | one event per Y: `{"state"}` |
| `proofs.rs` | `remove_proofs` | one event per Y: `{"state":"removed"}`, `op=Delete` |
| `keys.rs` | `add_keyset_info` | `op=Insert` (first time only — the call is an upsert re-run at startup, guarded by an existence check): `{"unit","active","valid_from","valid_to","input_fee_ppk"}` |
| `keys.rs` | `set_active_keyset` | one event per keyset touched: `{"active"}` |
| `signatures.rs` | `add_blind_signatures`, complete-insert branch | `op=Insert`: `{"amount","keyset_id","c","dleq_e","dleq_s","signed_time"}` |
| `signatures.rs` | `add_blind_signatures`, fill-in-`c`/`dleq` branch | `op=Update`: `{"c","dleq_e","dleq_s","signed_time","amount"}` |

(The NULL-`c` placeholder rows written by `add_blinded_messages` and
cleaned up by `delete_blinded_messages` are ephemeral staging, not
issuance — deliberately unlogged.)

Also implements `TransparencyLogDatabase for SQLMintDatabase<RM>` (read
side), generic over both SQLite and Postgres via `RM: DatabasePool`.

**Migrations** (both dialects): `20260701000000_create_mint_event_log.sql`
in `crates/cdk-sql-common/src/mint/migrations/{sqlite,postgres}/`. Single
table:

```sql
CREATE TABLE mint_event_log (
    seq INTEGER PRIMARY KEY AUTOINCREMENT, -- IDENTITY on Postgres; commit-order hint only
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    op SMALLINT NOT NULL,
    payload BLOB NOT NULL,       -- JSON, see table above
    leaf_hash BLOB NOT NULL,
    created_time BIGINT NOT NULL,
    leaf_index BIGINT            -- added by 20260702000000; zero-based tree
                                 -- position, NULL until the appender folds
                                 -- the row (see "Two numberings" above)
);
CREATE INDEX idx_mint_event_log_entity ON mint_event_log(entity_type, entity_id, seq);
CREATE UNIQUE INDEX idx_mint_event_log_leaf_index ON mint_event_log(leaf_index);
```

A third migration (`20260703000000_enforce_append_only_event_log.sql`,
both dialects) enforces the ADR's append-only invariant with triggers:
DELETE always aborts, and the only permitted UPDATE is the appender's
one-time `leaf_index` assignment (NULL → value, all other columns
unchanged). Covered by `event_log_is_append_only_at_db_level` in
`cdk-sqlite`.

No new tables for tree state or checkpoints — both live in the mint's
**existing generic KV store** (see §3), since they're small/single-valued
and don't need relational range queries.

Confirmed live (see §4): both migrations apply cleanly on top of a
pre-existing, long-lived `cdk-mintd.sqlite` (all prior migrations plus this
new one show up in the `migrations` table with matching timestamps, no
gaps, no failures).

### Background services (`crates/cdk`)

**`crates/cdk/src/mint/transparency.rs`** — `TransparencyLogService`:

- `load_or_create(log_db, kv_db, origin)` — loads or generates a dedicated
  Ed25519 signing key (never the mint's NUT-01 key), loads persisted tree
  state.
- `run_once()` — first drains rows already carrying a leaf index at
  `[tree.size, ...)` (crash recovery: index assignment is durable in the
  event log table itself, so replaying after a crash folds the same leaves
  into the same positions), then calls `assign_leaf_indices(BATCH)` to
  sequence newly committed rows, folds everything into the `TreeHead`, and
  signs a checkpoint if the tree grew. Tree state + checkpoint note +
  latest-checkpoint pointer are persisted in **one KV transaction**, so a
  crash can never persist the tree ahead of or behind its leaves.
- `spawn(shutdown, interval)` — background loop, ticks every `interval`
  (30s as wired in `cdk-mintd`).
- Query methods for HTTP: `latest_checkpoint`, `checkpoint_at(size)`,
  `entries(start, end)`, `inclusion_proof(seq, tree_size)`,
  `consistency_proof(first, second)`.
- `anchor_to_sigsum` (feature `sigsum-anchor`) — best-effort, never fails
  the tick.

KV storage layout (namespace `cdk_transparency`):

| secondary/key | contents |
|---|---|
| `keys/signing_key` | raw 32-byte Ed25519 seed |
| `state/tree_state` | `tree_size(8 BE) \|\| peaks(32 bytes each)` |
| `state/latest_checkpoint_size` | 8-byte BE u64 |
| `checkpoint/size_{tree_size:020}` | full C2SP signed-note text, UTF-8 |

(`state/next_seq` existed in an earlier revision; the tree-advancement
cursor is now just `tree_size` itself, with the durable `leaf_index`
column as the source of truth for which rows are already sequenced.)

**`crates/cdk/src/mint/witness.rs`** — `Witness`:

- Separate identity from the transparency log: its own Ed25519 key, KV
  namespace `cdk_witness`, keyed by `keys/signing_key`.
- `TrustedLog { origin, public_key }` — static trust policy passed in at
  construction. **No dynamic config surface yet** (see §5).
- `handle_add_checkpoint(body)` → parses, checks trust + consistency via
  `cdk_tlog::witness::consider_checkpoint`, persists new
  `(size, root)` per origin under KV key `state/{sha256(origin)_hex}`,
  returns the cosignature line.

Both are attached to `Mint` via `set_transparency_log`/`set_witness`
(interior-mutable `ArcSwapOption` fields, same pattern as the existing
`keysets: ArcSwap<...>` field) and spawned/joined from `Mint::start()`/
`stop()`, sharing the existing `shutdown_notify: Arc<Notify>` with the
payment-processor supervisor task.

Tests: `mint::transparency::tests` (2 tests: full run_once → verified
checkpoint signature → verified inclusion/consistency proofs, using a real
`cdk-sqlite` in-memory DB and real mutation calls) and `mint::witness::tests`
(3 tests: cosigns first submission, rejects untrusted origin, rejects stale
`old_size`).

### HTTP (`crates/cdk-axum`)

**`src/transparency.rs`** — `/v1/audit/*` (all GET, all proxy to
`TransparencyLogService`, 404 if none attached):

| Route | |
|---|---|
| `/v1/audit/pubkey` | `{origin, pubkey (base64), signature_scheme: "ed25519"}` |
| `/v1/audit/checkpoint` | `{checkpoint: "<C2SP note text>"}` |
| `/v1/audit/checkpoint/{tree_size}` | same shape, 404 if none at that size |
| `/v1/audit/entries?start=&end=` | `{start, end, entries: [...]}`, capped at 1000/request |
| `/v1/audit/proof/inclusion?seq=&tree_size=` | `{seq, tree_size, leaf_hash, proof: [hex...]}` |
| `/v1/audit/proof/consistency?first=&second=` | `{first, second, proof: [hex...]}` |

**`src/witness.rs`** — `POST /witness/add-checkpoint` (note: **not** nested
under `/v1`, mounted at the router root), raw `text/plain` body in/out per
spec, exact status-code mapping from `WitnessError`.

### `cdk-mintd`

`setup_database`/`initial_setup` now additionally return a
`DynTransparencyLogDatabase` (cloned from the same `Arc<MintSqliteDatabase>`/
`Arc<MintPgDatabase>` before it's fully erased into `DynMintDatabase`).
Right after `let mint = Arc::new(mint);`, `TransparencyLogService` is
constructed with origin `"{settings.info.url}/transparency-log"` and
attached via `mint.set_transparency_log(...)`. **No witness is wired into
cdk-mintd yet** — `Witness` exists and is tested at the `cdk` crate level,
but nothing in `cdk-mintd` constructs one or calls
`mint.set_witness(...)`. **No config toggle exists** — the transparency log
is unconditionally on whenever the binary is built with the `mint` feature
(which always pulls in `transparency-log`).

### Cargo features (all additive, default paths unaffected)

```
cdk:        transparency-log = ["dep:cdk-tlog", "dep:ed25519-dalek", "dep:base64", "dep:rand_core", "dep:hex"]
            sigsum-anchor    = ["transparency-log", "dep:cdk-sigsum"]
            mint             = [..., "transparency-log"]   # always on with mint
```

## 2. How to build & run it locally

```bash
# Build the mint daemon (fake Lightning backend, no real LN node needed)
cargo build -p cdk-mintd --bin cdk-mintd

# crates/cdk-mintd/example.config.toml already uses ln_backend = "fakewallet".
# Minimal changes for a throwaway local instance:
cp crates/cdk-mintd/example.config.toml /tmp/mintd-config.toml
# edit /tmp/mintd-config.toml: set listen_port and info.url to match, e.g.
#   listen_port = 18085
#   url = "http://127.0.0.1:18085"

CDK_MINTD_WORK_DIR=/tmp/mintd-workdir \
  ./target/debug/cdk-mintd --config /tmp/mintd-config.toml
```

The sqlite DB defaults to `<work_dir>/cdk-mintd.sqlite` (or
`~/.cdk-mintd/cdk-mintd.sqlite` if no work dir / `--config`-relative dir is
given — this is where the **already-running instance found during this
session** keeps its data, started via
`./target/debug/cdk-mintd --config crates/cdk-mintd/config.toml --seed-file seed.txt`).

Inspect the log directly:

```bash
sqlite3 ~/.cdk-mintd/cdk-mintd.sqlite \
  "SELECT seq, entity_type, op, entity_id, created_time FROM mint_event_log ORDER BY seq;"

# Correct way to see per-entity-type counts (see the note in §4 about
# GROUP BY entity_id) — count *distinct* entity ids, not `entity_id` itself:
sqlite3 ~/.cdk-mintd/cdk-mintd.sqlite \
  "SELECT entity_type, op, count(DISTINCT entity_id), count(*) FROM mint_event_log GROUP BY entity_type, op;"
```

Or via HTTP once the mint has processed at least one tick (30s interval):

```bash
curl -s http://127.0.0.1:18085/v1/audit/pubkey | jq
curl -s http://127.0.0.1:18085/v1/audit/checkpoint | jq
curl -s "http://127.0.0.1:18085/v1/audit/entries?start=0&end=50" | jq
```

Run the automated tests for everything touched:

```bash
cargo test -p cdk-tlog
cargo test -p cdk-common --features mint
cargo test -p cdk-sql-common --features mint
cargo test -p cdk-sqlite --features mint       # includes the wiring integration test
cargo test -p cdk --features mint --lib        # includes TransparencyLogService + Witness tests
cargo test -p cdk-axum
cargo test -p cdk-mintd --lib
```

All of the above passed, repeatedly, as of the last full run in this
session (681+ tests across these crates, zero failures, clippy clean under
the workspace's `-D warnings` + `unwrap_used = deny` lints).

**Not verified**: Postgres migration was never run against a live Postgres
server in this sandbox (no server available) — only compile-checked.

## 3. Design decisions worth remembering

- **Row id ≠ tree position.** The auto-increment row id is not gap-free
  (an aborted `INSERT` permanently burns the value on Postgres) and is
  never used as the Merkle leaf position. The appender assigns a separate,
  dense, zero-based `leaf_index` to committed rows in row-id order — so a
  burned row id can never stall tree advancement, and a transaction that
  commits late simply gets a later leaf index (observation order, exactly
  like CT/Sigsum sequencers). Per-entity event order is still preserved,
  because two mutations of the same entity are serialized by row locks and
  the second's event `INSERT` (and hence row id) happens after the first
  commits.
- **Tree state and checkpoints live in the KV store, not new tables.**
  Only the append-only event log itself (which needs real range queries)
  got a real SQL table. This was a deliberate scope-reduction after
  discussing that #2173's per-entity-typed-table design was overengineered.
- **Checkpoint/signature formats are the real C2SP standard**
  (`tlog-checkpoint`/`signed-note`/`tlog-cosignature`/`tlog-witness`), not a
  bespoke format — this is what lets a mint's checkpoint be cosigned by
  Sigsum's or Tessera's existing infrastructure, or by another mint's
  built-in witness, without protocol translation. This required diverging
  from an earlier, unrelated NUT-XX draft found mid-session that used
  secp256k1/BIP-340 and a bespoke signing preamble; `docs/adr/nut-xx.md` was
  rewritten to match the C2SP-based implementation (user's explicit choice).
- **Witness and transparency-log identities are deliberately separate**
  keys/namespaces — a mint witnessing others should not sign with the same
  key it uses for its own checkpoints.

## 4. Live testing session — findings so far

An existing, already-running `cdk-mintd` instance was found during this
session (`crates/cdk-mintd/config.toml` + `seed.txt`, two `[[ln]]` fakewallet
backends for `sat` and `eur`, DB at `~/.cdk-mintd/cdk-mintd.sqlite`). It was
**not restarted or modified** — only inspected read-only.

Findings from inspecting that live DB:

- Migrations table shows the new `20260701000000_create_mint_event_log.sql`
  applied cleanly alongside all prior migrations, same batch timestamp, no
  gaps. **Migration application itself is not implicated in any bug.**
- `mint_event_log` contains 28 rows: 2 `keyset`/`Update` (one per unit —
  `sat` and `eur` each have their own keyset, confirmed via
  `SELECT unit, count(*) FROM keyset GROUP BY unit` → `eur|1`, `sat|1`) and
  26 `blind_signature`/`Update`.
- **False alarm, corrected**: an earlier diagnostic query in this session
  (`SELECT entity_type, op, entity_id, count(*) ... GROUP BY entity_type, op`)
  appeared to show all 26 `blind_signature` rows sharing one `entity_id`.
  That query selects `entity_id` without grouping by it — SQLite silently
  returns an arbitrary value from one row per group in that case, it does
  **not** mean all 26 rows share an ID. Re-running with
  `count(DISTINCT entity_id)` confirms all 26 are genuinely distinct
  blinded messages, and both `keyset` rows are genuinely distinct keyset
  ids. **The event log itself shows no evidence of a bug** — it looks
  exactly like what 26 independent blind signature backfills and 2
  independent keyset activations should look like.
- `proof` and `melt_quote` tables were empty at inspection time (0 rows),
  so no proof-state or melt-quote events are expected yet either — not a
  bug, just no swap/melt activity yet on this instance.
- An attempt to query `SELECT final_expiry FROM keyset` failed with
  "no such column". This was **my own mistaken query**, not a real schema
  problem: the actual column is named `valid_to` (confirmed via
  `.schema keyset`); `final_expiry` is only the Rust-side field name on
  `MintKeySetInfo`, mapped to `valid_to` in the SQL layer. No transparency-
  log change touches this table's schema or this mapping at all.

### Open, unresolved: "keyset id incorrect" when adding the mint to a wallet

This is the one item **not yet explained**. What's confirmed *not* to be
the cause, based on the above:

- Not a migration failure.
- Not a corrupted/missing `keyset` row — both configured units have exactly
  one keyset each, as expected.
- Not directly caused by the `set_active_keyset` code change in
  `crates/cdk-sql-common/src/mint/keys.rs` — that change only *adds* a
  `SELECT` before the existing `UPDATE` statements and *appends* event-log
  writes after them; the actual `UPDATE keyset SET active=...` statements
  are byte-for-byte unchanged from before this work.

What's genuinely unknown, and needs a live check against the running
instance (not yet done — investigation was paused here at the user's
request):

1. Whether the ID stored in `keyset.id` for either unit actually
   recomputes correctly via NUT-02's `Id::v2_from_data(&keys, &unit,
   input_fee_ppk, valid_to)` (or `v1_from_keys` if it's a V0 id) — i.e.
   whether this is a **pre-existing** keyset-ID-drift bug unrelated to this
   session's work (e.g. from `valid_to`/`input_fee_ppk` being mutated after
   the ID was originally derived, on this specific long-lived DB), not
   something introduced here.
2. What the *live* `/v1/keys` HTTP response actually contains for both
   units, and whether the wallet doing the "keyset id incorrect" check is
   pointed at the `sat` or `eur` unit, and which wallet/client raised the
   error.
3. Whether this reproduces on a **freshly created** mint DB (no prior
   history), which would strongly implicate something in this session's
   changes; or only on this specific old DB, which would point to
   pre-existing drift or a stale wallet-side cache instead.

**Suggested next steps, in order of how much they isolate the cause:**

```bash
# 1. Compare the live keys response's declared id against a fresh keyset
#    fetched right now, for both units:
curl -s http://127.0.0.1:8085/v1/keys | jq '.keysets[] | {id, unit}'

# 2. Recompute the id client-side from the returned keys/unit/fee/expiry
#    (see cashu::nuts::nut02::KeySet::verify_id) and diff against what was
#    returned, per unit.

# 3. Reproduce on a throwaway fresh DB (recipe in §2) with the exact same
#    config (two fakewallet units, sat + eur) and see if a brand new wallet
#    add still fails. If it does NOT fail on a fresh DB, the bug is in
#    this specific old DB's history/drift, not in this session's code.

# 4. If it also fails fresh, bisect by checking out this session's changes
#    one crate at a time (git stash / git diff -- crates/cdk-sql-common)
#    against the same fresh-DB repro, starting with keys.rs since that's
#    the only touched file in the keyset code path.
```

## 5. Known gaps / honest limitations (carried over + updated)

1. **HA/multi-writer mints**: exactly one appender may call
   `assign_leaf_indices` (leaf-index assignment is not designed for
   concurrent assigners — no leader election exists yet). Row-id gaps no
   longer stall anything, but multi-process deployments still need to
   designate a single log-writer process. Not load-tested under real
   concurrency.
2. **Postgres path is compile-checked only**, never run against a live
   server in this environment.
3. ~~No outbound witness client~~ — **done**: `TransparencyLogService`
   requests cosignatures from `[[transparency_log.witnesses]]` on every
   publish (10s timeout per witness, 409 old-size resync, cosignature
   verified against the witness key before being appended and re-persisted
   to the stored note).
4. ~~`cdk-mintd` has no config toggle / witness wiring~~ — **done**:
   `[transparency_log]` (enabled/origin/checkpoint_interval_secs/witnesses)
   and `[witness]` (enabled/name/trusted_logs) both exist; the default
   origin is now schema-less (`<host[:port]>/transparency-log`) per NUT-XX,
   and the witness prints its name + base64 public key at startup for other
   operators to copy.
5. **Proof/consistency-proof generation is O(tree size)** — loads every leaf
   hash on every request. Flagged as a scaling follow-up in the original
   ADR, still unaddressed. (Requests for a `tree_size`/`second` beyond the
   current published tree now get a clean HTTP 400 via
   `TransparencyLogService::tree_size()` instead of a 500.)
6. `cdk-sigsum` anchoring **verified live against the public `barreleye`
   test log** (leaf included under a signed, witness-cosigned tree head;
   independently re-verified with a from-scratch Python verifier). The
   proof is persisted and served as `sigsum_proof` on
   `/v1/audit/checkpoint`. Production `seasalp` (rate-limited) untested.
7. **The "keyset id incorrect" report is still open** — see §4.
8. **Wallet-side pinning implemented** (`Wallet::verify_transparency_log`,
   feature `transparency-log`): TOFU-pins the log key + origin, requires
   consistency proofs on growth, reports `RollbackDetected` /
   `RootMismatch` / `InconsistentHistory` / `IdentityChanged` as typed
   statuses. Since 2026-07-03 also:
   `verify_transparency_log_with_witnesses(&[TrustedWitness], min)` gates
   pin advancement on verified witness cosignatures
   (`InsufficientCosignatures` status), and
   `verify_transparency_log_replay()` performs the full NUT-XX
   entry-by-entry replay audit (recompute every leaf hash via
   `GET /v1/audit/entries` pagination, rebuild the tree, compare roots;
   typed `ReplayStatus`). Not yet exposed over the cdk-swift FFI.
   Sigsum-proof verification wallet-side (offline, via the `sigsum` crate)
   remains follow-up.
9. **Cosignature wire format fixed** to match c2sp tlog-cosignature: the
   signature-line payload is `key_id(4) || timestamp(8 BE) || sig(64)` —
   an earlier revision omitted the embedded timestamp and would not have
   interoperated with external C2SP verifiers.
10. **Discovery**: no NUT-06 `nuts` entry until a NUT number is assigned;
    wallets probe `GET /v1/audit/pubkey` (404 = unsupported).
11. **Insert events added 2026-07-03** (`op=Insert`, consensus-critical —
    changes leaf hashes for newly created rows; fine pre-release, would be
    a fork after a NUT number is assigned). See the call-site table in §1.
12. **Log-key rotation is unimplemented** — NUT-XX says a mint that loses
    its log key MUST rotate and publish an event identifying the new key;
    today the wallet just reports `IdentityChanged`. Needs protocol design
    (signed rotation event in the log itself) — deliberate follow-up, see
    the ADR's Negative Consequences.
13. **Genesis snapshot** for mints enabling the log on pre-existing state:
    still an open NUT-XX question; insert events only cover rows created
    after enablement.

## 6. Reference material used while building this

- RFC 6962 (Certificate Transparency) — Merkle tree hash, audit path,
  consistency proof algorithms and worked example, used to validate
  `cdk-tlog::merkle`.
- [c2sp.org/tlog-checkpoint](https://c2sp.org/tlog-checkpoint),
  [c2sp.org/signed-note](https://c2sp.org/signed-note),
  [c2sp.org/tlog-cosignature](https://c2sp.org/tlog-cosignature),
  [C2SP tlog-witness](https://github.com/C2SP/C2SP/blob/main/tlog-witness.md) —
  exact wire formats implemented in `cdk-tlog::checkpoint`/`witness`.
- [sigsum.org](https://www.sigsum.org/) — public log/witness infrastructure
  `cdk-sigsum` targets (`seasalp`, run by Glasklar Teknik; witnesses run by
  Glasklar and Mullvad).
- `docs/adr/0001-append-only-transparency-log.md` — the design ADR this
  implementation follows.
- `docs/adr/nut-xx.md` — wallet/auditor-facing protocol draft, kept in sync
  with this implementation (field names, leaf-hash formula, checkpoint
  format) during this session.
