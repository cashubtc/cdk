# Append-only change log for the mint database

* Status: proposed
* Authors: [@crodas](https://github.com/crodas)
* Date: 2026-06-29
* Targeted modules: `cdk-common`, `cdk-sql-common`
* Associated tickets/PRs: none yet

## Context and Problem Statement

The mint database uses mutable tables for current state. After an update the previous value is gone; after a delete the row disappears. This makes auditing, debugging, replay, and reconciliation difficult. How can we preserve the history of state changes without replacing the existing storage model?

## Decision Drivers

* Auditability: every financially meaningful state transition must be recoverable
* Minimal disruption: the existing read and write paths must not change
* Atomicity: log entries must live in the same transaction as the mutation they describe
* Portability: must work across both SQLite and PostgreSQL backends

## Considered Options

* **Single append-only delta table with a typed delta enum** -- one shared insert-only table keyed by a global record id (`table_name:pk`). Each change is stored as a `delta` value, a typed enum with one variant per writable field, serialized to JSON or a compact binary form. The variants are derived from the mutation method signatures: every parameter a method writes becomes a variant. The enum is forward compatible: a new writable field is a new variant, and existing rows never need rewriting. One migration and one insert path, at the cost of losing column-level SQL queries over the delta payload.

* **Per-table log tables with typed columns** -- each tracked entity gets its own `_log` table containing only identity columns and the columns that actually change. Each table is small, focused, schema-validated, and directly queryable, but every new tracked field is a schema migration, and the write path fans out across several tables and helpers.

* **Single generic `event_log` table** -- one shared table with `table_name`, `entity_id`, `change_type`, and a JSON `data` blob. Simple to set up, but the untyped JSON blob prevents schema validation and forward-compatible typing of the payload.

* **Database triggers** -- automatic capture at the SQL level with no application code changes. Not viable because SQLite and PostgreSQL have incompatible trigger syntax, triggers cannot access the application-level `id` generation logic, and they are harder to test and debug.

## Proposed Decision

Single append-only delta table with a typed delta enum.

This approach is preferred over per-table log tables because it keeps the write path and schema small: one table, one insert helper, and one `Delta` enum. A new tracked field is a new enum variant, not a schema migration and a new `_log` table. Forward compatibility comes from the enum and its serialization rather than from `ALTER TABLE`.

It is preferred over the single generic `event_log` table because the `delta` is a typed, versioned enum rather than an untyped JSON blob, so the set of possible changes is defined in code and derived directly from the mutation method signatures.

It is preferred over database triggers because the mint must support both SQLite and PostgreSQL, and triggers have incompatible syntax, cannot call application-level id generation, and are harder to test.

The scope is deliberately narrow: only 3 of the mint's 12+ tables have rows that mutate after creation, so only their mutations are logged. Most entities are insert-only (their source table is already an immutable record) or ephemeral (deleted after use, with no audit value). By logging only the mutations that actually change or remove existing rows, the system avoids duplicating data that is already preserved elsewhere.

## Design

### Which mutations are logged

Only entities whose rows are **updated or deleted** after creation contribute to the log. The rest are either immutable (source table is the complete record) or ephemeral (no audit value).

| Entity | Mutable columns | Logged |
|--------|----------------|--------|
| **melt_quote** | `state`, `request_lookup_id`, `payment_proof`, `paid_time` | yes |
| **proof** | `state` (+ deletions on compensation) | yes |
| **keyset** | `active` | yes |

Entities that are NOT logged:

| Entity | Reason |
|--------|--------|
| **mint_quote** | Already append-only. `update_mint_quote` writes to `mint_quote_payments` and `mint_quote_issued` detail tables. The `amount_paid`/`amount_issued` on the quote row are materialized sums. |
| **blind_signature** | Insert-only, never updated or deleted. |
| **completed_operation** | Insert-only, never updated or deleted. |
| **melt_request** | Ephemeral staging data, always deleted on operation completion. The melt quote state transition captures the meaningful event. |
| **blinded_message** | Pending-signature placeholders. Cleaned up after signing. |
| **saga_state** | Process-private crash recovery state. Always deleted after completion. |
| **kv_store** | Generic key-value store. Too heterogeneous for a typed delta. |
| **protected_endpoint** | Configuration, not a financial event. |
| **auth tables** | Same patterns as non-auth counterparts. Deferred to follow-up. |

### Schema

A single insert-only table records every change as a serialized delta.

```sql
CREATE TABLE change_log (
    id          BIGINT PRIMARY KEY,   -- application-generated, time-sortable
    record      TEXT NOT NULL,        -- global record id: "table_name:pk"
    delta       BLOB NOT NULL,        -- serialized Delta enum (JSON or compact)
    reason      TEXT,                 -- human-readable why the change happened
    created_at  BIGINT NOT NULL       -- wall-clock millis at insert time
);
CREATE INDEX idx_change_log_record ON change_log(record, id);
```

* `id` is a time-sortable application-generated `i64` (see [id generation](#id-generation)). Ordering by `id` gives a global timeline.
* `record` identifies the mutated row across all tables in a single string, `table_name:pk` (for example `melt_quote:0f3a...` or `proof:<Y hex>`). All changes to one row share the same `record`, so the index on `(record, id)` reconstructs that row's history in order.
* `delta` is the serialized `Delta` enum described below.
* `reason` is optional free text for the operator or the calling code to explain the change (for example `"melt payment confirmed"` or `"compensation rollback"`).
* `created_at` is stored explicitly rather than folded into `id`, so replay and auditing do not have to decode the id to get a timestamp.

### The `Delta` enum

`Delta` is a forward-compatible enum with **one variant per writable field**. The variants are read straight off the mutation method signatures: every parameter a method writes to a row becomes one variant carrying only that field's new value. The `record` column already identifies which row changed, so the delta only has to say which field moved and to what.

Deriving the variants from the current signatures:

| Method (from the trait) | Parameters written | Field variant |
|-------------------------|--------------------|---------------|
| `update_melt_quote_state(_, new_state: MeltQuoteState, payment_proof: Option<String>)` | `state`, `payment_proof` | `MeltQuoteState`, `MeltQuotePaymentProof` |
| `update_melt_quote_request_lookup_id(_, new_request_lookup_id: &PaymentIdentifier)` | `request_lookup_id` | `MeltQuoteRequestLookupId` |
| `update_proofs_state(_, new_state: State)` | `state` | `ProofState` |
| `remove_proofs(ys, _)` | row deletion | `ProofRemoved` |
| `set_active_keyset(unit, id)` | `active` | `KeysetActive` |

```rust
/// One field-level state change. Serialized into `change_log.delta`.
///
/// Each variant maps to exactly one writable field and carries only that
/// field's new value. Variants come directly from the mutation method
/// signatures: a method that writes N fields appends N rows, one per field.
///
/// Forward compatible: a new writable field is a new variant, added without
/// migrating existing rows. Deserialization tolerates unknown variants so older
/// binaries can read logs written by newer ones.
#[non_exhaustive]
#[derive(Serialize, Deserialize)]
#[serde(tag = "field", content = "value")]
enum Delta {
    /// melt_quote.state
    MeltQuoteState(MeltQuoteState),
    /// melt_quote.payment_proof
    MeltQuotePaymentProof(Option<String>),
    /// melt_quote.request_lookup_id
    MeltQuoteRequestLookupId(PaymentIdentifier),
    /// proof.state
    ProofState(State),
    /// proof row removed (tombstone; carries no value)
    ProofRemoved,
    /// keyset.active
    KeysetActive(bool),
}
```

The `#[serde(tag = "field", content = "value")]` tagging keeps the payload self-describing, so a change written today stays readable after the enum grows. `#[non_exhaustive]` signals that callers must handle future variants.

### Serialization format

The `delta` column is a `BLOB`, so the enum's wire format is an internal choice that can be picked for compactness without changing the schema. The format must preserve forward compatibility: an old reader must be able to decode, or at least skip, a variant written by a newer writer. Options, from most to least self-describing:

| Format | Size | Forward compatibility | Dependency | Notes |
|--------|------|-----------------------|------------|-------|
| **JSON** (`serde_json`) | largest | excellent (named tags, skips unknown keys) | already in tree | Human-readable, easiest to debug, verbose on the wire. |
| **CBOR** (`ciborium`) | small | excellent (self-describing, tolerates unknown map keys and integer tags) | already in tree | Already used for Cashu token encoding. Binary JSON: same forward-compat model at a fraction of the size. |
| **Protobuf** (`prost`) | small | excellent (field numbers, unknown fields preserved) | already in tree | Requires a `.proto` schema and codegen. Field-number discipline gives strong forward compatibility. |
| **Postcard / bincode** | smallest | fragile | new crate | Non-self-describing. Enum variants are encoded by positional index, so reordering or removing a variant breaks old data; unknown variants cannot be skipped without a length prefix. |
| **Hand-rolled tag byte + fields** | smallest | manual | none | One discriminant byte per variant, then the value. Fully controlled and dependency-free, but every variant's encode/decode and skip logic is written and tested by hand. |

Recommended: **CBOR via `ciborium`**. It is already a workspace dependency (it encodes Cashu tokens), it is roughly as forward compatible as JSON because it is self-describing, and it is several times more compact. Representing the enum as a single-key map (`{tag: value}`), or as `#[serde(tag = "field")]` where the tag is a small integer rather than a string, keeps each row down to a few bytes while still letting a reader skip a tag it does not recognize.

Postcard and bincode are the most compact but are a poor fit here: their positional enum encoding is not forward compatible, which is the whole point of the `Delta` enum. Reach for the hand-rolled tag byte only if a dependency-free binary format becomes a hard requirement; it buys a little more compactness than CBOR at the cost of hand-written, per-variant skip logic.

### id generation

`id` is an application-generated Snowflake `i64`, implemented in-house with no external crate:

```
Bit 63:      always 0 (positive signed i64)
Bits 62..22: 41 bits of millisecond timestamp since a custom epoch
Bits 21..12: 10 bits of node id
Bits 11..0:  12 bits of per-millisecond sequence
```

The generator holds the last timestamp and a sequence counter behind an atomic/mutex. On each call it reads the clock: if the millisecond is unchanged it increments the sequence, and if the sequence overflows 12 bits (4096 ids in one millisecond) it spins until the next millisecond. This yields monotonic, time-sortable ids that stay unique across concurrent writers and across mint instances (distinguished by the 10-bit node id, from config). The custom epoch maximizes the usable timestamp range.

`created_at` is still stored as its own column so readers do not have to decode the id to get a timestamp.

### Implementation approach

Every mutation method appends one `INSERT INTO change_log` per field it writes, on the same database connection/transaction as the primary mutation. If the transaction rolls back, the log entries roll back too. All methods target the same table and build `Delta` values instead of writing typed columns.

This is the same piggy-backing pattern already used for `keyset_amounts` updates on proof/signature mutations.

Because `record` is a single row id, a method that touches several rows appends one entry per row, and a method that writes several fields on one row appends one entry per field. The history of any one row is the entries sharing its `record`, ordered by `id`.

| File | Method | Rows/fields written | Delta variant(s) appended |
|------|--------|---------------------|---------------------------|
| `quotes.rs` | `update_melt_quote_state` | one row, `state` + `payment_proof` | `MeltQuoteState`, and `MeltQuotePaymentProof` when `Some` |
| `quotes.rs` | `update_melt_quote_request_lookup_id` | one row, `request_lookup_id` | `MeltQuoteRequestLookupId` |
| `proofs.rs` | `update_proofs_state` | one row per `Y` | `ProofState` per affected `Y` |
| `proofs.rs` | `remove_proofs` | one row per `Y` | `ProofRemoved` per removed `Y` |
| `keys.rs` | `set_active_keyset` | activated keyset, plus the previously active one | `KeysetActive(true)`, `KeysetActive(false)` |

### Using the log as an event stream (poor man's Kafka)

Because the table is append-only and every row carries a monotonic, time-sortable `id`, `change_log` doubles as a durable, ordered event stream. Any process that wants to react to state changes (replication to a read replica, cache invalidation, analytics, feeding an external system, cross-instance synchronization) can consume it like a single-partition Kafka topic, without adding a message broker.

The consumption pattern is a cursor over `id`:

```sql
SELECT id, record, delta, reason, created_at
FROM change_log
WHERE id > :last_seen_id
ORDER BY id
LIMIT :batch;
```

Each consumer stores its own `last_seen_id` cursor and advances it after processing a batch. Because `id` is monotonic and the table is insert-only, this gives:

* **Total order.** `ORDER BY id` is a stable global sequence; the 10-bit node id in the Snowflake keeps ids unique across mint instances even under concurrent writes.
* **At-least-once delivery.** A consumer that crashes before persisting its cursor simply re-reads from the last committed `id`. Consumers must be idempotent (keying off `id` or `(record, id)` makes this trivial).
* **Independent consumers.** Cursors are per-consumer state, so many readers can consume at their own pace, similar to Kafka consumer groups reading one partition.
* **Replay.** Resetting a cursor to `0` (or any `id`) replays history from that point, which is the same mechanism used for reconciliation and debugging.

Two caveats to note for a real deployment:

* **Visibility ordering under concurrency.** Ids are assigned before commit, so a transaction with a lower `id` can commit after one with a higher `id`. A naive `id > cursor` poll could then skip a row that committed late. Consumers that need strict completeness should either read only up to a watermark that lags the newest `id` by a safety margin, or track gaps explicitly. This is the same read-committed race that log-based CDC systems handle with a low-watermark.
* **Retention.** Stream consumers and the audit-trail retention policy interact: history cannot be pruned past the slowest consumer's cursor without losing events for it.

### New files

| File | Purpose |
|------|---------|
| `crates/cdk-common/src/database/event_log.rs` | `Delta` enum, `generate_id()`, `record` helpers |
| `crates/cdk-sql-common/src/mint/event_log.rs` | Single `append_delta()` SQL helper |
| `migrations/sqlite/20260629000000_create_change_log.sql` | `change_log` table |
| `migrations/postgres/20260629000000_create_change_log.sql` | `change_log` table |

### Invariants

1. Existing tables remain the source of current state
2. The `change_log` table is append-only. No updates, no deletes
3. Every logged state change is appended in the same transaction as the mutation
4. Events are ordered by `id` (primarily by embedded timestamp)
5. The database enforces uniqueness via the `id` primary key

### Positive Consequences

* Full audit trail of every financially meaningful state transition
* Enables replay, reconciliation, and debugging without changing the operational model
* Single table and single insert path keep the change surface small
* A new tracked field is a new enum variant, with no schema migration
* Doubles as an ordered, durable event stream for replication, cache invalidation, and cross-instance sync, consumed by a cursor over `id` with no separate message broker

### Negative Consequences

* Small write amplification (one extra INSERT per written field)
* The table grows without bound and will eventually need a retention or archival policy
* The `delta` payload is opaque to SQL: column-level filtering (for example, by state or amount) requires deserializing rows in the application, or backend-specific JSON functions whose support differs between SQLite and PostgreSQL
* No database-level schema validation of the payload; validity of a `delta` lives entirely in the enum and its serialization
