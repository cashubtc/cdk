# Append-only journal for the mint database

* Status: proposed
* Authors: [@crodas](https://github.com/crodas)
* Date: 2026-06-29 (revised 2026-07-04)
* Targeted modules: `cdk-common`, `cdk-sql-common`, `cdk`, `cdk-signatory`
* Associated tickets/PRs: reference implementation on branch
  [`prototype/append-only`](https://github.com/cashubtc/cdk/tree/prototype/append-only)

> This document is a proposal. A working reference implementation exists on the
> `prototype/append-only` branch and is described throughout so the design can
> be evaluated against real code, but nothing here is committed to `main`. The
> record keys, entity coverage, and serialization choices are all open to
> change during review.

## Context and Problem Statement

The mint database uses mutable tables for current state. After an update the
previous value is gone; after a delete the row disappears. This makes auditing,
debugging, replay, and reconciliation difficult. How can we preserve the
history of state changes, and reconstruct current state from that history,
without replacing the existing storage model?

## Decision Drivers

* Auditability: every financially meaningful state transition must be
  recoverable
* Replayability: current state must be reconstructable from the log alone,
  starting from an empty database
* Minimal disruption: the existing read and write paths must keep working
* Atomicity: log entries must live in the same transaction as the mutation they
  describe
* Portability: must work across both SQLite and PostgreSQL backends

## Considered Options

* **Single append-only journal of snapshots and deltas** -- one shared
  insert-only table keyed by a compound `(entity, record)`: a small `entity`
  discriminant naming the source table plus the row's primary key. Each row
  stores a serialized `Event`, which is either a `Snapshot` (the full base
  object, written when an entity is created) or a `Delta` (one field-level
  change, written when an entity is mutated). Replaying a row's events in
  `id` order, snapshot first then deltas, reconstructs its current state, so
  the journal is self-contained and does not depend on the source tables to be
  replayable. One migration and one append path, at the cost of losing
  column-level SQL queries over the event payload.

* **Delta-only change log** -- the same single table, but storing only
  field-level deltas and relying on the existing source tables for the base
  object. Smaller rows, but the log is not replayable on its own: reconstructing
  state needs both the log and a consistent snapshot of the source tables at
  some point in time. This was the design in the first draft of this ADR; the
  reference implementation moved to snapshots plus deltas so the journal alone
  is sufficient.

* **Per-table log tables with typed columns** -- each tracked entity gets its
  own `_log` table containing only identity columns and the columns that
  change. Each table is small, focused, schema-validated, and directly
  queryable, but every new tracked field is a schema migration, and the write
  path fans out across several tables and helpers.

* **Single generic `event_log` table** -- one shared table with `table_name`,
  `entity_id`, `change_type`, and a JSON `data` blob. Simple to set up, but the
  untyped JSON blob prevents typing the payload in code.

* **Database triggers** -- automatic capture at the SQL level with no
  application code changes. Not viable because SQLite and PostgreSQL have
  incompatible trigger syntax, triggers cannot access the application-level `id`
  generation logic, and they are harder to test and debug.

## Proposed Decision

Single append-only journal of snapshots and deltas, with the mint layer
deciding which events to emit and the SQL layer providing only the durable
append.

This is preferred over the delta-only change log because storing a creation
snapshot makes the journal replayable from an empty database. A consumer loads
each entity's snapshot and applies its deltas in `id` order to rebuild current
state, with no dependency on the mutable source tables.

It is preferred over per-table log tables because it keeps the write path and
schema small: one table, one append primitive, and one `Event` enum. A new
tracked field is a new enum variant, not a schema migration and a new `_log`
table.

It is preferred over the single generic `event_log` table because the `Event`
is a typed enum rather than an untyped blob, so the set of possible changes is
defined in code and derived directly from the domain types and mutation method
signatures.

It is preferred over database triggers because the mint must support both
SQLite and PostgreSQL, and triggers have incompatible syntax, cannot call
application-level id generation, and are harder to test.

## Design

### Snapshots and deltas

An `Event` is either a `Snapshot` or a `Delta`.

* A **snapshot** is the full base object, captured once when an entity is
  created (a new melt quote, a new proof, an issued blind signature, a new
  keyset).
* A **delta** is one writable field's new value, captured when an existing
  entity is mutated. The `(entity, record)` key already says which row changed,
  so a delta only carries which field moved and to what.

Replaying a row means loading its snapshot and applying its deltas in `id`
order. Because every entity that later mutates also has a creation snapshot,
the journal reconstructs current state on its own.

### Which entities are journaled

| Entity | Snapshot at creation | Deltas on mutation |
|--------|----------------------|--------------------|
| **mint_quote** | `MintQuote` | `MintQuotePayment`, `MintQuoteIssuance` |
| **melt_quote** | `MeltQuote` | `MeltQuoteState`, `MeltQuotePaymentProof`, `MeltQuoteRequestLookupId` |
| **proof** | `Proof` (initial state `Unspent`) | `ProofState`, `ProofRemoved` (tombstone) |
| **blind_signature** | `BlindSignature` | none (immutable once created) |
| **keyset** | `MintKeySetInfo` | `KeysetActive(bool)` |

The delta-only draft logged only the three entities that mutate after creation
(melt_quote, proof, keyset) and treated the source tables as the base object.
The reference implementation adds creation snapshots for those, plus mint_quote
and blind_signature, so the issuance side is replayable too. `blind_signature`
has a snapshot but no deltas: it is immutable, so its creation event is the
whole story.

Still not journaled, because they are ephemeral or process-private:

| Entity | Reason |
|--------|--------|
| **melt_request** | Ephemeral staging data, deleted on completion. The melt quote transitions capture the meaningful event. |
| **blinded_message** | Pending-signature placeholders, cleaned up after signing. |
| **saga_state** | Process-private crash recovery state, deleted after completion. |
| **kv_store** | Generic key-value store. Too heterogeneous for a typed event. |
| **protected_endpoint** | Configuration, not a financial event. |
| **auth tables** | Same patterns as their non-auth counterparts. Deferred to a follow-up. |

### Schema

A single insert-only table records every event. The row it refers to is
identified by a compound `(entity, record)` key: `entity` is a small integer
discriminant naming the source table, and `record` is the primary key within
that table.

```sql
-- Append-only journal of entity creations (snapshots) and mutations (deltas).
-- Replaying a row's events in id order reconstructs its current state.
CREATE TABLE IF NOT EXISTS journal (
    id          INTEGER PRIMARY KEY,   -- Snowflake i64, time-sortable
    entity      INTEGER NOT NULL,      -- Entity enum discriminant (source table)
    record      TEXT    NOT NULL,      -- primary key within that entity
    event       BLOB    NOT NULL,      -- serialized Event (Snapshot | Delta)
    created_at  INTEGER NOT NULL       -- unix seconds at insert time
);

CREATE INDEX IF NOT EXISTS idx_journal_entity_record ON journal(entity, record, id);
```

The PostgreSQL migration is the same shape with `BIGINT` for `id`/`created_at`,
`SMALLINT` for `entity`, and `BYTEA` for `event`.

* `id` is a time-sortable application-generated `i64` (see
  [id generation](#id-generation)). Ordering by `id` gives a global timeline.
* `(entity, record)` identifies the mutated row. `entity` is the `Entity` enum
  discriminant (a small int, so the source-table name is not repeated as text
  on every row), and `record` is that row's primary key. All events for one row
  share the same `(entity, record)`, so the index on `(entity, record, id)`
  reconstructs that row's history in order, and querying one entity type
  (`WHERE entity = :e`) is an indexed range scan.
* `event` is the serialized `Event` (a snapshot or a delta).
* `created_at` is unix seconds at insert time, stored as its own column so
  replay and auditing do not have to decode the `id` to get a timestamp.

The compound key is a typed `entity` discriminant plus the bare primary key,
rather than a single `record TEXT` holding a concatenated `"table_name:pk"`
string (for example `melt_quote:0f3a...`). The split avoids repeating the table
name as text on every row, makes "all events for entity X" a clean indexed
predicate, and removes the ambiguity of parsing a string whose primary key may
itself contain the delimiter. The reference implementation uses this compound
key: the migration has the `entity` column and callers pass the bare primary
key. (One stale doc comment on `add_journal` still describes `record` as a
`"table_name:pk"` string; the actual value passed is the bare primary key.)

The first draft also had a `reason` free-text column; the reference
implementation dropped it to keep the append primitive minimal. It can be added
back if operators need per-event annotations.

### The `Entity` enum

`entity` is stored as the discriminant of a small `#[repr(u8)]` enum, one
variant per journaled source table. The stored value is a stable integer, so
variants must keep their discriminants for old rows to stay readable.

```rust
/// The source table a journal row refers to. Stored as its `u8` discriminant
/// in `journal.entity`; the row's primary key goes in `journal.record`.
#[non_exhaustive]
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Entity {
    MintQuote = 1,
    MeltQuote = 2,
    Proof = 3,
    BlindSignature = 4,
    Keyset = 5,
}
```

Every `Event` variant belongs to exactly one `Entity`, so the enum can be
derived from the event (`event.entity()`) rather than passed separately. Storing
it as its own indexed column is still worthwhile: it lets a consumer filter or
range-scan by entity type without deserializing the `event` blob.

### The `Event`, `Snapshot`, and `Delta` types

`Event` wraps either a `Snapshot` or a `Delta`. All three are
`#[non_exhaustive]` so callers must handle future variants, and a new writable
field or tracked entity is a new variant added without migrating existing rows.

```rust
/// A single journal event: either a full-object snapshot or a field delta.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Full base object, written when an entity is created. Boxed because a
    /// snapshot is far larger than a delta.
    Snapshot(Box<Snapshot>),
    /// One field-level change, written when an entity is mutated.
    Delta(Delta),
}

/// Full base object captured at creation time.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Snapshot {
    MeltQuote(MeltQuote),
    MintQuote(MintQuote),
    Proof(Proof),
    BlindSignature(BlindSignature),
    Keyset(MintKeySetInfo),
}

/// One writable field's new value. The journal's `(entity, record)` key
/// identifies which row changed; each variant carries only that field's new value.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Delta {
    MeltQuoteState(MeltQuoteState),
    MeltQuotePaymentProof(Option<String>),
    MeltQuoteRequestLookupId(PaymentIdentifier),
    /// A payment received for a mint quote (increments `amount_paid`).
    MintQuotePayment(IncomingPayment),
    /// An issuance recorded against a mint quote (increments `amount_issued`).
    MintQuoteIssuance(Amount),
    ProofState(State),
    /// A proof row was removed (tombstone, carries no value).
    ProofRemoved,
    KeysetActive(bool),
}
```

`From` conversions on `Event` let call sites write `value.into()` instead of
spelling out `Event::Delta(Delta::Variant(value))` or the snapshot boxing. For
example `new_state.into()` produces a `ProofState` delta and `quote.into()`
produces a `MeltQuote` snapshot.

### Serialization format

`Event` is serialized to the `event` column with **JSON** (`serde_json`), the
same encoding the mint already uses to persist these domain types (quote
requests, options, keyset amounts).

The first draft recommended CBOR via `ciborium` for compactness. The reference
implementation found that several cashu types serialize differently under a
non-human-readable serializer and do not round-trip through CBOR, so JSON is
used instead. JSON is verbose on the wire but human-readable, easy to debug,
and forward compatible: named tags let an old reader skip unknown fields. The
`event` column stays a `BLOB`/`BYTEA`, so the wire format remains an internal
choice that can be revisited without a schema change if the round-trip issues
are resolved.

To support snapshots, a few domain types (`MintQuote`, `IncomingPayment`,
`Issuance`) gained serde derives; they were previously persisted by column
mapping only. This reuses the existing `amount_currency_serde` helper plus a
new `amount_currency_serde_opt` module for the optional amount field.

### Orchestration: mint decides, SQL appends

Journaling is a method on the mint write-transaction trait:

```rust
/// Append-only journal writer.
#[async_trait]
pub trait JournalTransaction {
    type Err: Into<Error> + From<Error>;

    /// Appends one Event for the row identified by `(entity, record)` within
    /// the current transaction. `record` is that row's bare primary key;
    /// `entity` is derived from the event (`event.entity()`), so it is not
    /// passed separately and is always consistent with the payload.
    async fn add_journal(&mut self, record: String, event: Event) -> Result<(), Self::Err>;
}
```

`JournalTransaction` is a supertrait shared by both the main write transaction
and the keyset transaction. The SQL layer implements only the append: one
`INSERT INTO journal` on the transaction's own connection, so it commits or
rolls back with the mutation it records.

Emission is driven by a `JournaledDatabase` decorator that wraps the mint
database (`cdk-common/src/database/journaled.rs`). Each entity-mutation method
on the wrapped transaction (`add_mint_quote`, `update_melt_quote_state`,
`add_proofs`, `update_proofs_state`, `remove_proofs`, `add_blind_signatures`,
`set_active_keyset`, and the rest) emits the relevant snapshot or delta on the
inner transaction as part of the same mutation. Direct `add_journal` calls on
the decorator are rejected with `Error::JournalNotPermitted`, so journaling is
only ever driven from inside a known mutation rather than by ad hoc callers. The
decorator is installed once around the database: `cdk/src/mint/mod.rs` for the
mint and `cdk-signatory/src/db_signatory.rs` for keyset rotation.

This centralizes "what to log" in one place, a deliberate move away from the
first draft, which piggy-backed the inserts inside the database-layer mutation
methods and would have scattered emission across each flow. Keeping emission in
the decorator and "how to append" in the SQL layer means the log reflects
business-meaningful events rather than raw column writes, and the append
primitive stays backend-agnostic.

Representative call sites from the reference implementation:

| Flow | Event appended | `(entity, record)` |
|------|----------------|--------------------|
| mint quote created | `MintQuote` snapshot | `(MintQuote, id)` |
| mint quote paid | `MintQuotePayment` delta | `(MintQuote, id)` |
| mint quote issued | `MintQuoteIssuance` delta | `(MintQuote, id)` |
| melt quote created | `MeltQuote` snapshot | `(MeltQuote, id)` |
| melt quote paid | `MeltQuoteState` + `MeltQuotePaymentProof` deltas | `(MeltQuote, id)` |
| proof state change | `ProofState` delta per `Y` | `(Proof, y_hex)` |
| proof removed (compensation/rollback) | `ProofRemoved` tombstone per `Y` | `(Proof, y_hex)` |
| blind signature issued | `BlindSignature` snapshot | `(BlindSignature, blinded_secret_hex)` |
| keyset rotation | `Keyset` snapshot, `KeysetActive(true)`, `KeysetActive(false)` on the previous | `(Keyset, id)` |

### id generation

`id` is an application-generated Snowflake `i64`, implemented in-house with no
external crate:

```
Bit 63:      always 0 (positive signed i64)
Bits 62..22: 41 bits of millisecond timestamp since a custom epoch (2024-01-01)
Bits 21..12: 10 bits of node id
Bits 11..0:  12 bits of per-millisecond sequence
```

The generator is lock-free. A single `AtomicU64` packs the last-used
`(timestamp_ms << SEQ_BITS) | sequence`, and a compare-and-swap loop advances
it: if the clock moved forward the sequence resets to zero, if it is the same
millisecond the sequence increments, and if the sequence overflows 12 bits
(4096 ids in one millisecond) it borrows the next millisecond. This yields
monotonic, time-sortable ids that stay unique across concurrent writers and
across mint instances, distinguished by the 10-bit node id set once at startup.
A clock reading before the unix epoch is clamped to zero rather than panicking.
The custom epoch maximizes the usable timestamp range.

### New and changed files (reference implementation)

| File | Purpose |
|------|---------|
| `crates/cdk-common/src/database/event_log.rs` | `Entity`, `Event`/`Snapshot`/`Delta`, `From` conversions, Snowflake `generate_id()`/`init_event_id_generator()` |
| `crates/cdk-common/src/database/mint/mod.rs` | `JournalTransaction` supertrait |
| `crates/cdk-common/src/database/journaled.rs` | `JournaledDatabase` decorator that drives emission and rejects direct `add_journal` |
| `crates/cdk-sql-common/src/mint/event_log.rs` | `add_journal()` append primitive |
| `crates/cdk-sql-common/src/mint/migrations/{sqlite,postgres}/20260702000000_create_journal.sql` | `journal` table |
| `crates/cdk-common/src/mint.rs`, `.../amount_currency_serde_opt.rs` | serde derives for snapshotted types |
| `crates/cdk/src/mint/mod.rs` | installs `JournaledDatabase` around the mint store |
| `crates/cdk-signatory/src/db_signatory.rs` | installs `JournaledDatabase` for keyset rotation |

### Invariants

1. Existing tables remain the source of current state for the read path
2. The `journal` table is append-only. No updates, no deletes
3. Every journaled event is appended in the same transaction as the mutation
4. Every entity that can mutate has a creation snapshot, so replaying the
   journal from empty reconstructs current state
5. Events are ordered by `id` (primarily by embedded timestamp)
6. The database enforces uniqueness via the `id` primary key

### Using the journal as an event stream (poor man's Kafka)

This is a design sketch: the reference implementation is write-only today and
ships no consumer. Because the table is append-only and every row carries a
monotonic, time-sortable `id`, `journal` doubles as a durable, ordered event
stream. Any process that wants to react to state changes (replication to a read
replica, cache invalidation, analytics, cross-instance synchronization) could
consume it like a single-partition Kafka topic, without a message broker.

The consumption pattern is a cursor over `id`:

```sql
SELECT id, entity, record, event, created_at
FROM journal
WHERE id > :last_seen_id
ORDER BY id
LIMIT :batch;
```

Each consumer stores its own `last_seen_id` cursor and advances it after a
batch. This gives total order, at-least-once delivery (a consumer that crashes
before persisting its cursor re-reads from the last committed `id`; consumers
must be idempotent), independent per-consumer cursors, and replay by resetting a
cursor.

Two caveats for a real deployment:

* **Visibility ordering under concurrency.** Ids are assigned before commit, so
  a transaction with a lower `id` can commit after one with a higher `id`. A
  naive `id > cursor` poll could skip a row that committed late. Consumers that
  need strict completeness should read up to a watermark that lags the newest
  `id` by a safety margin, or track gaps explicitly. This is the same
  read-committed race that log-based CDC systems handle with a low-watermark.
* **Retention.** History cannot be pruned past the slowest consumer's cursor
  without losing events for it.

### Testing

The reference implementation is covered end to end by tests that drive a real
swap, a real melt, and a keyset rotation and assert that the emitted journal
rows replay to the expected state
(`crates/cdk/src/mint/{melt,swap}/tests/journal_tests.rs`,
`crates/cdk/src/mint/keyset_journal_tests.rs`, plus an issuance test in
`issue/mod.rs`). Unit tests in `event_log.rs` cover delta round-tripping,
entity discriminant stability, event-to-entity mapping, and id
monotonicity/uniqueness (`delta_round_trip`, `entity_discriminant_round_trips`,
`events_map_to_expected_entity`, `ids_are_monotonic_and_unique`,
`ids_are_unique_across_threads`). Replay itself is exercised only by test
helpers that load the `journal` rows and decode them back into `Event`s; there
is no production read/replay path in the reference yet.

### Positive Consequences

* Full audit trail of every financially meaningful state transition
* The journal is self-contained: current state replays from an empty database,
  which enables reconciliation and debugging without the source tables
* Single table and single append primitive keep the change surface small
* A new tracked field or entity is a new enum variant, with no schema migration
* Mint-orchestrated emission means the log records business events, not raw
  column writes, and the append primitive stays backend-agnostic
* Doubles as an ordered, durable event stream consumed by a cursor over `id`,
  with no separate message broker

### Negative Consequences

* Write amplification: a snapshot at creation plus one insert per mutated field,
  and snapshots are larger than deltas
* The table grows without bound and will eventually need a retention or archival
  policy, which interacts with stream consumers' cursors
* The `event` payload is opaque to SQL: column-level filtering requires
  deserializing rows in the application, or backend-specific JSON functions
  whose support differs between SQLite and PostgreSQL
* JSON is verbose; a more compact binary format is blocked until the affected
  cashu types round-trip through a non-human-readable serializer
* No database-level schema validation of the payload; validity of an `Event`
  lives entirely in the enum and its serialization
