# Append-only mint event log with Merkle transparency proofs

* Status: proposed
* Authors: @asmogo
* Date: 2026-07-01
* Targeted modules: `cashu`, `cdk-common`, `cdk-sql-common`, `cdk` (mint), `cdk-axum`, `cdk-mintd`, `cdk-sigsum` (new)
* Associated tickets/PRs: supersedes/merges [#2173](https://github.com/cashubtc/cdk/pull/2173) ("ADR for append-only change log in mint database")

## Context and Problem Statement

The mint database uses mutable tables for current state. After an `UPDATE` the previous value is gone; after a `DELETE` the row disappears. #2173 proposes fixing this with per-entity typed log tables (`melt_quote_log`, `proof_log`, `keyset_log`). That solves *internal* auditability and makes state replayable, but it has two problems:

1. **Maintenance burden without a ceiling.** A typed log table must mirror every mutable column of its source table, by hand, in two SQL dialects, forever. Review of #2173 already surfaced two instances of this schema drifting out of sync on the very first pass (a missed `UPDATE` path for `blind_signature` in `add_blind_signatures`, and missing `request_lookup_id_kind`/`estimated_blocks`/`fee_reserve` columns in `melt_quote_log`). Every future migration to a logged table now has to remember to also touch its log table, in both dialects, or the audit trail silently goes stale.
2. **It doesn't deliver transparency.** "Auditability for external parties" and "playback" require more than a record of changes — they require that an external party can trust the record wasn't rewritten *after the fact* by the operator who controls the database. A plain SQL log table gives the mint operator the same `UPDATE`/`DELETE` power over `melt_quote_log` as over `melt_quote`. There is no cryptographic commitment, no way for a wallet or auditor to detect a retroactive edit, and no way for independent observers to cross-check that two views of "the log" are consistent.

This ADR proposes a design that satisfies both the original replay/auditability goal *and* external verifiability, while removing the schema-duplication burden.

## Decision Drivers

* **Auditability** — every financially meaningful state transition must be recoverable.
* **External verifiability** — a party who does not trust the mint operator must be able to detect a retroactively rewritten history, not just read a self-reported log.
* **Playback** — a third party must be able to deterministically reconstruct current-state tables from the log alone.
* **Minimal, bounded maintenance burden** — adding a column to a source table should not require a parallel, hand-maintained change somewhere else.
* **Atomicity** — log entries live in the same transaction as the mutation they describe.
* **Portability** — SQLite and PostgreSQL.
* **Minimal disruption** — existing read/write paths and business logic do not change.

## Considered Options

#### Per-table typed log tables (#2173 as written)

One `_log` table per mutable entity, with typed columns mirroring the source table's mutable columns.

**Pros:** schema-validated, directly queryable with plain SQL, no JSON parsing.
**Cons:** N schemas × 2 dialects to hand-maintain in lockstep with the source tables (already demonstrated to drift); gives replay but no tamper-evidence; three independently-keyed logs would need to be re-merged into one global order anyway if a transparency layer is ever added on top.

#### Single generic event log, no cryptography

One table, `entity_type` / `entity_id` / `op` / JSON or CBOR `payload`. Simpler than per-table logs, fixes the schema-drift problem, still gives replay.

**Pros:** one schema, no drift, sufficient for playback.
**Cons:** still no external verifiability — an operator can edit rows undetected.

#### Single generic event log + Merkle transparency layer (chosen)

Same single generic log as above, but every appended entry is also folded into an append-only Merkle tree, and the mint periodically publishes signed checkpoints (root hash + tree size + signature) that let any third party verify inclusion and consistency without trusting the mint's live API responses. This is the architecture behind Certificate Transparency (RFC 6962/9162), Sigsum, Sigstore/Rekor, Go's sumdb, and Signal/`warg`'s key/package transparency logs.

**Pros:** replay (same as option 2) *and* cryptographic tamper-evidence; well-understood, widely deployed design; can be built with primitives the workspace already depends on (`bitcoin::secp256k1` for Schnorr signatures — no new crypto dependency).
**Cons:** more moving parts than a plain log table; requires a decision about checkpoint publication and (optionally) witnessing; unbounded log growth needs a retention story, same as option 1/2 but now also constrained by proof validity.

#### Externally hosted transparency infrastructure (Trillian / Tessera / Sigstore-Rekor / Sigsum)

This option splits into two genuinely different things that are easy to conflate:

* **Self-hosted server software** — Trillian, and its modern successor [Tessera](https://github.com/transparency-dev/tessera) (transparency.dev's own recommendation over Trillian as of 2024). Both are Go libraries/services *you run yourself*; there is no shared public multi-tenant instance to submit to. Adopting either means operating a second, Go-based process per mint, plus its own storage. Rejected for the mint's own local tree (§5) — running a second language runtime per mint for something `sha2` + ~100 lines of Rust already does is not a good trade.
* **Already-running public services** — [Sigsum](https://www.sigsum.org/) (a stable production log, `seasalp`, operated by Glasklar Teknik, with independent stable witnesses run by Glasklar and Mullvad — see [sigsum.org/services](https://www.sigsum.org/services/)) and the public [Rekor](https://rekor.sigstore.dev) instance (operated by the Sigstore project, 99.5% availability SLO). These are not something to run — they're something to *submit to*. Sigsum in particular is explicitly designed to be content-agnostic (submit any signed 32-byte checksum) and to be embedded into third-party systems exactly like this one.

**Pros (public services):** real, operating infrastructure with independent witnesses already in place today, at effectively zero operational cost to a mint — no new service to run, just an HTTP client. Solves the operator-equivocation gap in §9 that this design otherwise leaves unsolved.
**Cons:** these are shared, general-purpose, and at least partially rate-limited services (Sigsum's stable log requires DNS-domain-based registration, see §7); they are appropriate for anchoring a small number of periodic checkpoint hashes, not for hosting a mint's entire event stream. Rekor's public instance is governed and resourced for software-supply-chain use cases, so depending on it for payment-system checkpoints, while technically supported by its generic "hashed artifact" entry type, is a judgment call worth raising with that community rather than assuming indefinitely.

**Decision:** adopt the public-service half of this option — see §7 — while continuing to reject the self-hosted-server half for the mint's own tree.

#### Blockchain/timestamp anchoring only (OpenTimestamps, `OP_RETURN`)

**Pros:** free, decentralized, Bitcoin-anchored timestamping of a single hash; thematically the most natural fit for a Bitcoin e-cash mint of anything in this list.
**Cons:** anchors a *point in time* for one hash, not a queryable, replayable log, and gives no inclusion-among-many-entries proof machinery by itself. Kept as a complementary, low-cost redundant anchor for the checkpoint hash (§7), not a replacement for Sigsum's witnessed inclusion proofs or for the log/tree design itself.

## Decision Outcome

Chosen option: **single generic event log + Merkle transparency layer, with periodic checkpoints anchored to already-running public Sigsum/OpenTimestamps infrastructure**. It fully subsumes #2173's replay requirement (folding the generic log in `seq` order reconstructs current state exactly as the per-table logs would have), removes the schema-duplication failure mode already observed in review, and — by anchoring checkpoints externally instead of only self-reporting them — is the only option that satisfies "external parties should be able to trust a playback, not just perform one" without requiring the cashu ecosystem to build and operate new public transparency-log infrastructure.

---

## Design

### 1. What gets logged

Same scope analysis as #2173 — only entities that are mutated or deleted after creation need logging. Insert-only tables (`mint_quote_payments`, `mint_quote_issued`, `completed_operation`) and ephemeral tables (`melt_request`, `blinded_message`, `saga_state`) are unaffected.

| Entity | `entity_type` | Triggered by |
|---|---|---|
| melt quote | `melt_quote` | `update_melt_quote_state`, `update_melt_quote_request_lookup_id` |
| proof | `proof` | `update_proofs_state`, `remove_proofs` |
| keyset | `keyset` | `set_active_keyset` |
| blind signature | `blind_signature` | `add_blind_signatures` (the fill-in-`c`/`dleq` update path — this is the case flagged in review of #2173; it is a real mutation and must be logged) |

Adding a fifth logged entity later is a one-line change at the call site — no new table, no new migration.

### 2. Canonical event log (single table, one migration, both dialects)

```sql
-- migrations/sqlite/20260701000000_create_mint_event_log.sql
-- migrations/postgres/20260701000000_create_mint_event_log.sql

CREATE TABLE mint_event_log (
    seq         BIGINT PRIMARY KEY,   -- monotonic, gap-tolerant, see "sequencing" below
    entity_type TEXT   NOT NULL,      -- 'melt_quote' | 'proof' | 'keyset' | 'blind_signature'
    entity_id   TEXT   NOT NULL,      -- quote id / Y hex / keyset id / blinded message hex
    op          SMALLINT NOT NULL,    -- 0 = Insert, 1 = Update, 2 = Delete
    payload     BLOB   NOT NULL,      -- canonical CBOR of the new field values
    leaf_hash   BLOB   NOT NULL,      -- RFC 6962 leaf hash, precomputed at insert time
    created_time BIGINT NOT NULL
);
CREATE INDEX idx_mint_event_log_entity ON mint_event_log(entity_type, entity_id, seq);
```

`payload` is not an unstructured blob in the pejorative sense — it's a canonical (deterministic field order, deterministic number encoding) CBOR encoding of exactly the fields the mutation call site is writing. This is the same information a typed `_log` table would store, just not pre-split into per-entity SQL schemas. If a specific entity ever needs indexed SQL queries over one field (e.g. "all melt quotes that transitioned to `Failed`"), add a single derived/materialized table for *that* field, driven by an actual need — not three tables built speculatively.

### 3. `cdk-common`: log entry types

```rust
// crates/cdk-common/src/database/event_log.rs

/// Entity kinds that participate in the append-only transparency log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoggedEntity {
    MeltQuote,
    Proof,
    Keyset,
    BlindSignature,
}

/// The kind of mutation an event log entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventOp {
    Insert = 0,
    Update = 1,
    Delete = 2,
}

/// A single append-only log entry, prior to sequencing.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub entity_type: LoggedEntity,
    pub entity_id: String,
    pub op: EventOp,
    /// Canonical CBOR of the fields written by this mutation.
    pub payload: Vec<u8>,
}

/// A sequenced, hashed log entry as persisted in `mint_event_log`.
#[derive(Debug, Clone)]
pub struct SequencedLogEntry {
    pub seq: u64,
    pub entry: LogEntry,
    pub leaf_hash: [u8; 32],
    pub created_time: u64,
}
```

### 4. Sequencing (the part that needs care)

The tree requires a single, strictly ordered, gap-free sequence of leaves. Two deployment shapes:

* **Single mint process (SQLite, or Postgres with one writer)** — an in-process `AtomicU64` sequencer, seeded from `MAX(seq)` at startup, assigns `seq` synchronously inside the same transaction as the mutation. Simple, matches #2173's original assumption, no contention concerns.
* **Multiple mint processes sharing one Postgres database (HA)** — do **not** let every process claim `seq` via a shared `SEQUENCE`/`nextval()` directly for tree leaves. Postgres sequences don't block, but they also don't guarantee commit order matches allocation order (a transaction that grabs `seq=105` can commit before the one holding `seq=104`), which breaks the tree's "no gaps, no reordering after publication" invariant. Instead: mutation transactions still write their row (with a Postgres `SEQUENCE`-assigned `seq`, gaps from aborted transactions are harmless), but a single dedicated **appender** task per mint (leader-elected, or simply "the one process configured as the log writer" in a typical active/passive mint HA setup) is the only thing that reads committed rows in `seq` order and folds them into the Merkle tree — advancing the tree only over a contiguous prefix it has observed. This mirrors what CT logs, Sigsum, and Rekor all do in practice: the write-serving frontend scales horizontally, but there is exactly one sequencer for the tree itself. This is called out explicitly here rather than glossed over, since it's the one place a naive implementation breaks.

This also resolves the `change_id`-generation debate from the #2173 review thread: the row's `seq` no longer has to double as a globally meaningful, collision-resistant, timestamp-embedding identifier (as the PR's bit-packed `change_id` scheme tried to do) — it only has to be a stable per-row identity for querying. The identifier that actually matters cryptographically is the **leaf's position in the tree**, assigned once by the single appender, never renumbered.

### 5. Merkle tree layer (RFC 6962 hashing, Merkle Mountain Range storage)

No new dependency required — SHA-256 is already in the dependency tree (`sha2`, used elsewhere in `cashu`).

```rust
// crates/cdk-common/src/database/transparency.rs

use sha2::{Digest, Sha256};

/// RFC 6962 leaf hash: domain-separated so a leaf can never collide with an
/// interior node hash.
pub fn leaf_hash(seq: u64, entity_type: LoggedEntity, entity_id: &str, op: EventOp, payload: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x00]); // leaf domain separator
    hasher.update(seq.to_be_bytes());
    hasher.update([entity_type as u8]);
    hasher.update(entity_id.as_bytes());
    hasher.update([op as u8]);
    hasher.update(payload);
    hasher.finalize().into()
}

/// RFC 6962 interior node hash.
fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x01]); // interior domain separator
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

/// Append-only Merkle tree, stored as a Merkle Mountain Range so that
/// appending a leaf costs O(log n) hashes instead of a full rehash.
#[derive(Debug, Clone, Default)]
pub struct MerkleTreeState {
    pub tree_size: u64,
    /// One root hash per set bit in `tree_size`'s binary representation,
    /// ordered from the tallest (leftmost) peak to the shortest.
    pub peaks: Vec<[u8; 32]>,
}

impl MerkleTreeState {
    /// Appends a leaf and returns the new root hash.
    pub fn append(&mut self, leaf: [u8; 32]) -> [u8; 32] {
        let mut carry = leaf;
        let mut new_peaks = Vec::with_capacity(self.peaks.len() + 1);
        // Merge the new leaf with existing peaks the same way binary addition
        // carries: a peak of height h only survives if bit h of tree_size is 0.
        for &peak in self.peaks.iter().rev() {
            if self.tree_size & 1 == 1 {
                carry = node_hash(&peak, &carry);
                self.tree_size >>= 1;
                continue;
            }
            new_peaks.push(peak);
            self.tree_size >>= 1;
        }
        new_peaks.push(carry);
        new_peaks.reverse();
        self.peaks = new_peaks;
        self.tree_size += 1;
        self.root()
    }

    /// Combines all peaks into a single root hash (bagging the peaks, MMR-style).
    pub fn root(&self) -> [u8; 32] {
        self.peaks
            .iter()
            .rev()
            .copied()
            .reduce(|acc, peak| node_hash(&peak, &acc))
            .unwrap_or([0u8; 32])
    }
}
```

Persisted as a single-row table:

```sql
CREATE TABLE transparency_tree_state (
    id        INTEGER PRIMARY KEY CHECK (id = 1),
    tree_size BIGINT NOT NULL,
    peaks     BLOB    NOT NULL   -- concatenated 32-byte hashes
);
```

Inclusion and consistency proofs (`prove_inclusion(seq, tree_size)`, `prove_consistency(old_size, new_size)`) follow the standard RFC 6962 algorithms over the same peak/hash primitives; omitted here for brevity but are ~40 lines each and well-specified. (If preferred over hand-rolling, `warg-transparency`, `ct-merkle`, or `merkle-log` on crates.io implement the same math and could be vendored instead of the sketch above — see prior discussion.)

### 6. Signed checkpoints

Reuse the existing `cashu::nuts::nut01` Schnorr signing primitives (`SecretKey::sign` / `PublicKey::verify`, already backed by `bitcoin::secp256k1`) with a **dedicated log-signing keypair**, separate from the mint's minting key, so a compromised log key cannot mint.

```rust
// crates/cdk-common/src/database/transparency.rs

/// A signed commitment to the state of the transparency log at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub tree_size: u64,
    pub root_hash: [u8; 32],
    pub timestamp: u64,
    pub log_key_id: String,
    pub signature: bitcoin::secp256k1::schnorr::Signature,
}

impl Checkpoint {
    /// Message signed: `origin || tree_size || root_hash || timestamp`, following
    /// the c2sp.org/checkpoint text-checkpoint convention so third-party witness
    /// tooling (e.g. Sigsum witnesses) can cosign it unmodified.
    fn signing_payload(origin: &str, tree_size: u64, root_hash: &[u8; 32], timestamp: u64) -> Vec<u8> { /* ... */ }

    #[instrument(skip_all)]
    pub fn sign(origin: &str, tree: &MerkleTreeState, key: &SecretKey, timestamp: u64) -> Result<Self, Error> { /* ... */ }

    #[instrument(skip_all)]
    pub fn verify(&self, origin: &str, log_key: &PublicKey) -> Result<(), Error> { /* ... */ }
}
```

Persisted append-only (checkpoints are themselves never mutated):

```sql
CREATE TABLE transparency_checkpoint (
    id         BIGINT PRIMARY KEY,
    tree_size  BIGINT NOT NULL,
    root_hash  BLOB   NOT NULL,
    timestamp  BIGINT NOT NULL,
    signature  BLOB   NOT NULL
);
```

The mint signs a new checkpoint on a cadence similar to CT's Maximum Merge Delay — e.g. every N appended entries or every M seconds, whichever comes first (configurable in `cdk-mintd`).

### 7. Wallet/auditor-facing API (new NUT)

A new NUT ("Mint transparency log") exposes, served by `cdk-axum`:

| Endpoint | Purpose |
|---|---|
| `GET /v1/audit/pubkey` | The mint's log-signing public key. |
| `GET /v1/audit/checkpoint` | Latest signed checkpoint, including its external anchors (§7.1). |
| `GET /v1/audit/checkpoint/{tree_size}` | Historical checkpoint, for consistency proofs. |
| `GET /v1/audit/entries?start=&end=` | Raw `mint_event_log` rows in `[start, end)`, for bulk replay. |
| `GET /v1/audit/proof/inclusion?seq=&tree_size=` | Merkle audit path for entry `seq` under checkpoint `tree_size`. |
| `GET /v1/audit/proof/consistency?first=&second=` | Merkle consistency proof between two checkpoints. |

#### 7.1. External anchoring (new `cdk-sigsum` crate)

Never submit `mint_event_log` itself externally — it's a shared, general-purpose, partially rate-limited public resource, and a mint's full event stream is unnecessary load regardless of whether the mint minds the data being public. Only the periodic checkpoint (§6) — a root hash, a size, a timestamp; a few dozen bytes — is anchored externally, on the same cadence the checkpoint itself is published:

1. **Primary anchor: [Sigsum](https://www.sigsum.org/)'s stable public log, `seasalp`** (operated by Glasklar Teknik). Sigsum logs are content-agnostic — a submitter signs and submits `H(H(checkpoint_bytes))`, the log doesn't interpret it at all — which is exactly the right shape for anchoring a commitment without needing the log operator's cooperation on data model. This is implemented in a new crate, `cdk-sigsum` (added alongside this ADR), a from-scratch client for the [Sigsum log server protocol](https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md) built only on `reqwest`, `ed25519-dalek`, and `bitcoin::hashes::sha256` (no dependency on Sigsum's own Go tooling). It implements all five log endpoints (`get-tree-head`, `get-inclusion-proof`, `get-consistency-proof`, `get-leaves`, `add-leaf`), the domain-based rate-limit token required by public logs (a `_sigsum_v1.<domain>` DNS TXT record the mint publishes under a domain it already controls — its own mint domain works fine), and assembles a self-contained, offline-verifiable ["sigsum proof"](https://git.glasklar.is/sigsum/core/sigsum-go/-/blob/main/doc/sigsum-proof.md) for each anchored checkpoint. See `crates/cdk-sigsum/README.md` for usage; the core call is:

   ```rust
   let client = SigsumClient::new(Url::parse("https://seasalp.glasklar.is/")?);
   let proof = cdk_sigsum::anchor(&client, &log_public_key, &submit_key, Some(&token), &checkpoint_bytes).await?;
   ```

   The resulting `SigsumProof` (log key hash, leaf, cosigned tree head, inclusion proof) is stored alongside the mint's own `transparency_checkpoint` row and served back via `GET /v1/audit/checkpoint`. Because `seasalp` already has independent witnesses (Glasklar and Mullvad, per [sigsum.org/services](https://www.sigsum.org/services/)) cosigning its tree heads, this is also how §9's equivocation gap gets closed, for free, without the cashu ecosystem operating anything.
2. **Secondary anchor: OpenTimestamps.** The same checkpoint hash is additionally submitted to the free, decentralized, Bitcoin-anchored [OpenTimestamps](https://opentimestamps.org) calendar servers (Rust support: the `opentimestamps` and `opentimestamps-cli` crates). This gives a redundant, independently-secured "this existed at time T" proof that doesn't depend on Sigsum's or Glasklar's continued operation, and is thematically the most natural fit for a Bitcoin e-cash mint of anything considered.
3. Both anchors are best-effort and asynchronous relative to checkpoint publication — a mint's own `/v1/audit/*` endpoints keep working even if `seasalp` or an OpenTimestamps calendar is temporarily unreachable; the external anchors are attached to a checkpoint once available, not required to serve it.

### 8. Playback / replay procedure

Any third party with a copy of `mint_event_log` (via `/v1/audit/entries`) and the mint's initial keyset generation parameters can:

1. **Reconstruct state** — fold `op`/`entity_type`/`entity_id`/`payload` over `seq` order to rebuild the current-state tables (`melt_quote`, `proof`, `keyset`, `blind_signature`). This alone fully replaces #2173's replay use case.
2. **Verify the replay is authentic**, not merely self-consistent — recompute `leaf_hash` for each entry, rebuild the `MerkleTreeState` incrementally, and compare the resulting root at `tree_size = N` against a `Checkpoint` signed by the mint for that same `tree_size`. A match proves the replayed entries are exactly, and only, the ones the mint publicly committed to.
3. **Verify the mint itself couldn't have quietly rewritten that checkpoint** — check the checkpoint's Sigsum inclusion proof against a `seasalp` tree head that carries the Glasklar/Mullvad witness cosignatures (using the [`sigsum`](https://docs.rs/sigsum) crate for offline verification), and/or check its OpenTimestamps proof against the Bitcoin blockchain. Steps 1–2 alone still trust the mint's own signature; step 3 is what makes the trust independently checkable.

Step 3 is the piece #2173 cannot provide on its own, and — now that it's backed by already-running, independently-witnessed public infrastructure rather than deferred future work — is the actual "transparency" deliverable behind the original ask.

### 9. Witnessing

Because the mint operator controls both the database and the log-signing key, an operator could in principle still equivocate — sign two different checkpoints at the same `tree_size` and show each to a different audience. §7.1 already closes most of this gap today, for free, by anchoring checkpoints to Sigsum's `seasalp` log, which is independently witnessed by Glasklar and Mullvad: as long as any auditor or wallet checks a checkpoint's Sigsum inclusion proof against a witnessed tree head (rather than trusting the mint's bare signature), an equivocating mint gets caught the moment two witnessed views diverge.

What's left, and genuinely still ecosystem-level future work rather than something one mint's codebase can solve alone:

* **Wallet-side gossip/pinning** — wallets should remember the highest Sigsum-witnessed checkpoint seen per mint and refuse any earlier-looking checkpoint, the standard CT/Sigsum client-side mitigation. Not yet implemented in `cdk`'s wallet code.
* **A witness built into `cdk-mintd` itself** — the [C2SP tlog-witness protocol](https://github.com/C2SP/C2SP/blob/main/tlog-witness.md) that Sigsum (and Tessera) already speak is a small, standardized, implementable-in-a-few-hundred-lines protocol: "receive a checkpoint, check it's consistent with the last one seen for that log, cosign it." Shipping one alongside `cdk-mintd` would let any two mints running CDK opt in to witnessing each other's checkpoints, turning the cashu mint ecosystem into its own mutual-witness network for free, using a standard protocol rather than a new bespoke one, and without needing a central authority or new shared infrastructure. Not implemented yet; tracked as follow-up to this ADR, not a blocker for it, since Sigsum's own witnesses already cover the same threat in the meantime.

### Invariants

1. Existing tables remain the source of current state; the log is additive.
2. `mint_event_log` and `transparency_checkpoint` are append-only — no `UPDATE`, no `DELETE`, enforced by DB-level `REVOKE UPDATE, DELETE` grants where the backend supports it, and by code review elsewhere.
3. Every log entry is appended in the same transaction as the mutation it describes.
4. Exactly one appender advances `transparency_tree_state` at a time; leaves are never reordered once included in a published checkpoint.
5. A checkpoint's `root_hash` is reproducible by anyone from `mint_event_log` entries `[0, tree_size)` alone.

## Positive Consequences

* Full audit trail *and* cryptographic tamper-evidence, not just one or the other.
* One schema instead of N — the exact review-flagged failure mode from #2173 (drifting typed log tables) structurally cannot recur.
* Third parties can verify a replay instead of merely trusting one, and can do so against already-witnessed public checkpoints (§7.1) rather than only the mint's own signature.
* Built entirely on primitives already in the dependency tree (`bitcoin::hashes::sha256`, `bitcoin::secp256k1`) plus one small new crate (`cdk-sigsum`, ~600 lines, no dependency on Trillian/Rekor/immudb or any Go tooling); no new public infrastructure needs to be built or operated by the cashu ecosystem.

## Negative Consequences

* More moving parts than a plain log table (tree state, checkpoints, a new NUT, an external anchoring client).
* Requires a decision, per deployment, about who runs the single "appender" role in an HA Postgres setup (§4) — not automatic.
* Log and tree growth are unbounded; pruning must preserve provability for still-referenced ranges (RFC 9162-style "expired" shards), which is a harder retention story than deleting old rows from a plain log table. This ADR does not solve retention, only flags that it's a harder problem than in #2173 and should be its own follow-up before this ships to mints with high transaction volume.
* External anchoring (§7.1) depends on the continued operation of Sigsum's `seasalp` log and Glasklar's rate-limit registration process for it; both are outside CDK's control. This is mitigated, not eliminated, by also anchoring to OpenTimestamps, and by the mint's own `/v1/audit/*` endpoints remaining fully functional (just not externally witnessed) if both anchors are temporarily unavailable.
* Wallet-side checkpoint pinning/gossip and a built-in `cdk-mintd` witness (§9) are not implemented yet. Until then, external verifiability meaningfully protects against tampering by anyone *without* the mint's log key (§7.1's Sigsum witnesses already cover most of the operator-equivocation case too), but a fully closed-loop "wallet detects an equivocating mint entirely on its own" story is still follow-up work.

## Links

* Supersedes / incorporates the entity-scope analysis from [#2173](https://github.com/cashubtc/cdk/pull/2173)
* [RFC 6962 — Certificate Transparency](https://www.rfc-editor.org/rfc/rfc6962)
* [RFC 9162 — Certificate Transparency v2](https://www.rfc-editor.org/rfc/rfc9162)
* [c2sp.org/checkpoint](https://c2sp.org/checkpoint) — witness-cosignable checkpoint text format
* [Russ Cox, "Transparent Logs for Skeptical Clients"](https://research.swtch.com/tlog)
* [Sigsum](https://www.sigsum.org/) — minimal transparency log with witness cosigning, designed to be embedded into third-party systems
* [Sigsum log server protocol](https://git.glasklar.is/sigsum/project/documentation/-/blob/main/log.md) — wire format implemented by `crates/cdk-sigsum`
* [Sigsum services](https://www.sigsum.org/services/) — the `seasalp` stable log and its Glasklar/Mullvad witnesses used for external anchoring (§7.1)
* [`sigsum` crate](https://docs.rs/sigsum) (Mullvad) — offline verification of Sigsum proofs of logging, for the wallet/auditor side
* [C2SP tlog-witness protocol](https://github.com/C2SP/C2SP/blob/main/tlog-witness.md) — protocol a future built-in `cdk-mintd` witness (§9) would speak
* [Rekor / Sigstore transparency log](https://docs.sigstore.dev/logging/overview/) — considered as an additional public anchor, not adopted in this revision
* [OpenTimestamps](https://opentimestamps.org) — secondary, Bitcoin-anchored checkpoint anchor (§7.1)
* [Tessera](https://github.com/transparency-dev/tessera) — modern self-hosted alternative to Trillian, considered and not adopted for the mint's own tree (§5) since it would require running a second, Go-based process per mint
* [`warg-transparency`](https://docs.rs/warg-transparency) — Rust verifiable log + verifiable map, used in production by the Bytecode Alliance's `warg` package registry
