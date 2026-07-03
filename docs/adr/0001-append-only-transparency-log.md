# Append-only mint event log with Merkle transparency proofs

* Status: accepted, implemented on `feat/append-only`
* Authors: @asmogo
* Date: 2026-07-01 (design §§1–9 updated 2026-07-03 to match the implementation and the NUT-XX draft; see `nut-xx.md` for the normative wire formats)
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

The four entities that are mutated or deleted after creation (the same scope analysis as #2173) are logged across their whole lifecycle — creation included. Insert events exist so the Merkle tree commits to a row's *existence*, not only its transitions: without them, replay from the log alone is impossible (update payloads carry only changed fields, so the starting point of every entity would be missing), and an operator could retroactively invent or vanish rows that were never updated without any checkpoint breaking. Truly insert-only tables that participate in no logged transition (`mint_quote_payments`, `mint_quote_issued`, `completed_operation`) and ephemeral staging tables (`melt_request`, the placeholder blinded-message rows, `saga_state`) remain unaffected.

| Entity | `entity_type` | Triggered by |
|---|---|---|
| melt quote | `melt_quote` | `add_melt_quote` (insert), `update_melt_quote_state`, `update_melt_quote_request_lookup_id` |
| proof | `proof` | `add_proofs` (insert), `update_proofs_state`, `remove_proofs` (delete) |
| keyset | `keyset` | `add_keyset_info` (insert, first time only — the call is an upsert re-run at startup), `set_active_keyset` |
| blind signature | `blind_signature` | `add_blind_signatures` — both branches: the complete-row insert, and the fill-in-`c`/`dleq` update path (the case flagged in review of #2173). Every issued signature is logged exactly once, on whichever path issued it. |

Adding a fifth logged entity later is a one-line change at the call site — no new table, no new migration.

### 2. Canonical event log (single table, one migration, both dialects)

```sql
-- migrations/{sqlite,postgres}/20260701000000_create_mint_event_log.sql
-- migrations/{sqlite,postgres}/20260702000000_add_leaf_index_to_mint_event_log.sql

CREATE TABLE mint_event_log (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT, -- IDENTITY on Postgres; commit-order hint only
    entity_type TEXT   NOT NULL,      -- 'melt_quote' | 'proof' | 'keyset' | 'blind_signature'
    entity_id   TEXT   NOT NULL,      -- quote id / Y hex / keyset id / blinded message hex
    op          SMALLINT NOT NULL,    -- 0 = Insert, 1 = Update, 2 = Delete
    payload     BLOB   NOT NULL,      -- canonical JSON of the new field values (see below)
    leaf_hash   BLOB   NOT NULL,      -- RFC 6962 leaf hash, precomputed at insert time
    created_time BIGINT NOT NULL,
    leaf_index  BIGINT                -- zero-based Merkle tree position; NULL until the
                                      -- single appender folds the row (see §4)
);
CREATE INDEX idx_mint_event_log_entity ON mint_event_log(entity_type, entity_id, seq);
CREATE UNIQUE INDEX idx_mint_event_log_leaf_index ON mint_event_log(leaf_index);
```

`payload` is not an unstructured blob in the pejorative sense — it's a canonical JSON encoding (compact, keys sorted; `serde_json`'s default map behavior, pinned by a unit test in `cdk-sql-common`'s `event_log.rs` so a dependency-feature flip can't silently change the bytes) of exactly the fields the mutation call site is writing. JSON was chosen over the CBOR considered in an earlier draft of this ADR because the NUT-XX audit endpoints serve entries as JSON anyway, and one encoding end-to-end means an external verifier can re-serialize the served payload object and get byte-identical input to the leaf hash. This is the same information a typed `_log` table would store, just not pre-split into per-entity SQL schemas. If a specific entity ever needs indexed SQL queries over one field (e.g. "all melt quotes that transitioned to `Failed`"), add a single derived/materialized table for *that* field, driven by an actual need — not three tables built speculatively.

### 3. `cdk-common`: log entry types

```rust
// crates/cdk-common/src/database/mint/transparency.rs

/// Entity kinds that participate in the append-only transparency log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoggedEntity {
    MeltQuote,
    Proof,
    Keyset,
    BlindSignature,
}

/// The kind of mutation an event log entry records. `Insert` exists so the
/// tree commits to a row's *existence*, not just its later transitions —
/// without it, replay from the log alone is impossible and an operator
/// could silently invent or vanish rows that were never updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventOp {
    Insert = 0,
    Update = 1,
    Delete = 2,
}

/// One durable, sequenced row of the log, as read back by the checkpoint
/// publisher and the audit HTTP endpoints. `seq` here is the zero-based
/// Merkle leaf index (`leaf_index` in SQL, NUT-XX's public `seq`), not the
/// row's auto-increment id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintEventLogEntry {
    pub seq: u64,
    pub entity_type: LoggedEntity,
    pub entity_id: String,
    pub op: EventOp,
    /// Canonical JSON of the fields written by this mutation.
    pub payload: Vec<u8>,
    pub leaf_hash: [u8; 32],
    pub created_time: u64,
}
```

The leaf-hash preimage (`event_leaf_preimage`) is
`utf8(entity_type) || 0x00 || utf8(entity_id) || 0x00 || uint8(op) ||
uint64_be(created_time) || payload`. **`seq` is deliberately excluded** —
RFC 6962 leaves never encode their own tree position (it's implicit from
where the leaf lands), and excluding it also sidesteps a chicken-and-egg
problem: the row's id isn't known until the `INSERT` that assigns it has
already happened. This is normative in NUT-XX's "Leaf Hash" section.

### 4. Sequencing (the part that needs care)

The tree requires a single, strictly ordered, gap-free sequence of leaves. Two deployment shapes:

The implementation uses **two numberings, deliberately**, on both backends:

* The table's auto-increment row id (`seq` column) is only a commit-order hint. Gaps are harmless and expected — on Postgres, `IDENTITY` values burned by rolled-back transactions are *permanent* gaps, and any consumer keyed on row id contiguity would stall at the first one forever.
* The Merkle tree position is a separate, nullable `leaf_index` column, assigned densely (zero-based) by a single **appender** task (`TransparencyLogDatabase::assign_leaf_indices`) when it folds committed rows into the tree, in row-id order. A transaction that commits late simply gets a later leaf index — observation order, exactly like CT/Sigsum sequencers. Per-entity event order is still preserved, because two mutations of the same entity are serialized by row locks, so the second's event `INSERT` happens after the first commits. Assignment is durable in the event log table itself, which is what makes crash recovery trivial: an appender that dies between assigning and folding just re-reads the already-indexed-but-unfolded suffix on restart.

In an HA multi-process Postgres deployment, exactly one process may run the appender (leader-elected, or simply "the one process configured as the log writer" in a typical active/passive setup). The write-serving frontend scales horizontally; there is exactly one sequencer for the tree itself — the same shape CT logs, Sigsum, and Rekor all use. This is called out explicitly here rather than glossed over, since it's the one place a naive implementation breaks.

This also resolves the `change_id`-generation debate from the #2173 review thread: the row's id no longer has to double as a globally meaningful, collision-resistant, timestamp-embedding identifier (as the PR's bit-packed `change_id` scheme tried to do) — it only has to be a stable per-row identity for querying. The identifier that actually matters cryptographically is the **leaf's position in the tree** (`leaf_index`, exposed as NUT-XX's `seq`), assigned once by the single appender, never renumbered.

### 5. Merkle tree layer (RFC 6962 hashing, Merkle Mountain Range storage)

Implemented in a new dependency-light crate, **`crates/cdk-tlog`** (`merkle.rs`), pure logic with no DB/HTTP dependency, hashing via `bitcoin::hashes::sha256` (already in the tree):

* `leaf_hash(data)` — the RFC 6962 leaf hash `SHA256(0x00 || data)` over the preimage from §3 (note: the leaf's tree position is *not* part of the preimage).
* `node_hash` — RFC 6962 interior hash `SHA256(0x01 || left || right)`.
* `TreeHead` — append-only accumulator stored as a Merkle Mountain Range (one peak per set bit of `size`), so appending a leaf costs O(log n) hashes instead of a full rehash. Deliberately unable to produce proofs on its own — proof generation needs the actual leaves, which the durable event log already stores.
* `inclusion_proof` / `verify_inclusion`, `consistency_proof` / `verify_consistency` — the standard RFC 6962 algorithms, validated against RFC 6962 §2.1.3's own worked examples (`PROOF(3, D[7]) = [c, d, g, l]` etc.) as unit tests, not just internal round-trips.

Tree state (size + peaks) is persisted through the mint's **existing generic KV store** (namespace `cdk_transparency`, key `state/tree_state`), not a bespoke SQL table — it's a single small record with no need for relational queries. Only the event log itself, which needs real range queries, got a table (§2). An earlier draft of this ADR sketched `transparency_tree_state`/`transparency_checkpoint` tables; the KV store subsumed both.

Known scaling limitation, accepted for now: generating an inclusion or consistency proof loads every leaf hash in `[0, tree_size)` — O(n) per request. Fine at current volumes; a peak-cache or tile-based store (à la sumdb tiles) is follow-up work before this ships to high-volume mints.

### 6. Signed checkpoints

Checkpoints are **not** a bespoke format: they are [c2sp.org/tlog-checkpoint](https://c2sp.org/tlog-checkpoint) text checkpoints, signed as [c2sp.org/signed-note](https://c2sp.org/signed-note) notes with **Ed25519** (signed-note type `0x01`), with witness cosignatures as timestamped Ed25519 cosignature lines (type `0x04`, per [c2sp.org/tlog-cosignature](https://c2sp.org/tlog-cosignature)). Implemented in `crates/cdk-tlog` (`checkpoint.rs`, `witness.rs`).

An earlier draft of this ADR proposed reusing the mint's existing secp256k1/BIP-340 Schnorr primitives with a bespoke signing payload. That was dropped deliberately: the C2SP formats are what Sigsum, Tessera, and Sunlight-family logs and witnesses actually speak, and Ed25519 is the only signature type their signed-note profile defines — using them unmodified is what lets a mint's checkpoint be cosigned by already-running third-party witnesses (and by another mint's built-in witness, §9) with zero protocol translation. The log-signing key is still a **dedicated keypair** (never the mint's NUT-01 minting key), so a compromised log key cannot mint; it just happens to be an Ed25519 key.

```text
<origin>            e.g. mint.example.com/transparency-log (schema-less, per NUT-XX)
<tree_size>
<base64(root_hash)>

— <origin> base64(4-byte key ID || Ed25519 signature)
— <witness name> base64(4-byte key ID || 8-byte BE timestamp || Ed25519 signature)   (cosignatures)
```

Checkpoint notes are persisted append-only through the mint's KV store (namespace `cdk_transparency`, key `checkpoint/size_{tree_size:020}`, plus a `state/latest_checkpoint_size` pointer), written **in the same KV transaction as the advanced tree state** so a crash can never persist the tree ahead of or behind its checkpoint. The only later write to a stored note is *appending* witness cosignature lines to it — the checkpoint body and the mint's own signature never change.

The mint signs a new checkpoint whenever the tree has advanced, on a periodic tick (default 30s, configurable as `[transparency_log].checkpoint_interval_secs` in `cdk-mintd`) — the same cadence role as CT's Maximum Merge Delay.

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

1. **Reconstruct state** — fold `op`/`entity_type`/`entity_id`/`payload` over `seq` order to rebuild the protocol-visible state of the logged entities (`melt_quote`, `proof`, `keyset`, `blind_signature`): insert events establish each row's existence and initial fields, update/delete events its transitions. Secret-bearing columns (`proof.secret`/`c`/`witness`, `melt_quote.request`) are deliberately absent from payloads, so the replayed view is the auditable one, not a byte-identical database copy. This replaces #2173's replay use case.
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
2. `mint_event_log` is append-only: no `DELETE`, and the only permitted `UPDATE` is the appender's one-time `leaf_index` assignment (`NULL` → a value, all other columns unchanged). Enforced by database triggers on both backends (`20260703000000_enforce_append_only_event_log.sql`), not just code review. Checkpoint notes in the KV store are append-only by construction: the only rewrite is appending witness cosignature lines to an existing note.
3. Every log entry is appended in the same transaction as the mutation it describes.
4. Exactly one appender assigns leaf indices and advances the persisted tree state at a time; leaves are never reordered once included in a published checkpoint.
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
* Wallet-side verification is implemented (`Wallet::verify_transparency_log[_with_witnesses]` — TOFU pinning, rollback/rewrite/equivocation detection, witness-cosignature gating — and `Wallet::verify_transparency_log_replay` for the full NUT-XX entry-by-entry replay audit), as is a built-in witness in `cdk-mintd` (§9). Remaining follow-ups, deliberately out of scope for this revision:
  * **Log-key rotation.** NUT-XX requires a mint that loses its log key to rotate and publish an event identifying the new key; no rotation mechanism exists yet — a wallet currently just reports `IdentityChanged` and a human has to adjudicate. Needs a protocol-level design (a signed rotation event in the log itself) before implementation.
  * **Genesis snapshot.** A mint enabling the log with pre-existing state has no logged `insert` events for rows created before enablement, so replay only covers post-enablement history. NUT-XX tracks this as an open question (a standardized snapshot event).
  * **O(n) proof generation** (§5) and log retention/pruning — both must be addressed before high-volume mints ship this.
  * **Wallet-side gossip** between independent observers (comparing pinned checkpoints out of band) remains ecosystem-level work; per-wallet pinning and witness cosignatures already cover the single-observer case.

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
