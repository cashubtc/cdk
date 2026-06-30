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

* **Single generic `event_log` table** -- one shared table with `table_name`, `entity_id`, `change_type`, and a JSON `data` blob. Simple to set up, but the untyped JSON blob prevents column-level queries and schema validation, and mixing heterogeneous entities in one table makes indexing awkward.

* **Per-table log tables with typed columns** -- each tracked entity gets its own `_log` table containing only identity columns and the columns that actually change. More tables to maintain, but each one is small, focused, schema-validated, and directly queryable. Insert-only entities need no log table at all.

* **Database triggers** -- automatic capture at the SQL level with no application code changes. Not viable because SQLite and PostgreSQL have incompatible trigger syntax, triggers cannot access the application-level `change_id` generation logic, and they are harder to test and debug.

## Proposed Decision

Per-table log tables with typed columns.

This approach is preferred over a single generic event log because each log table carries only the columns that actually change for that entity, making the schema self-documenting and queryable without JSON parsing. It is preferred over database triggers because the mint must support both SQLite and PostgreSQL, and triggers have incompatible syntax, cannot call application-level ID generation, and are harder to test.

The scope is deliberately narrow: only 3 of the mint's 12+ tables need log tables. Most entities are insert-only (their source table is already an immutable record) or ephemeral (deleted after use, with no audit value). By limiting logging to the entities whose rows actually mutate after creation, the system avoids duplicating data that is already preserved elsewhere.

## Design

### Which entities need log tables

Only entities whose rows are **updated or deleted** after creation need a log table. The rest are either immutable (source table is the complete record) or ephemeral (no audit value).

| Entity | Mutable columns | Log table |
|--------|----------------|-----------|
| **melt_quote** | `state`, `request_lookup_id`, `payment_proof`, `paid_time` | `melt_quote_log` |
| **proof** | `state` (+ deletions on compensation) | `proof_log` |
| **keyset** | `active` | `keyset_log` |

### Entities that do NOT need log tables

| Entity | Reason |
|--------|--------|
| **mint_quote** | Already append-only. `update_mint_quote` writes to `mint_quote_payments` and `mint_quote_issued` detail tables. The `amount_paid`/`amount_issued` on the quote row are materialized sums. |
| **blind_signature** | Insert-only, never updated or deleted. |
| **completed_operation** | Insert-only, never updated or deleted. |
| **melt_request** | Ephemeral staging data, always deleted on operation completion. The melt quote state transition captures the meaningful event. |
| **blinded_message** | Pending-signature placeholders. Cleaned up after signing. |
| **saga_state** | Process-private crash recovery state. Always deleted after completion. |
| **kv_store** | Generic key-value store. Too heterogeneous for a typed log. |
| **protected_endpoint** | Configuration, not a financial event. |
| **auth tables** | Same patterns as non-auth counterparts. Deferred to follow-up. |

### Log table schemas

#### `melt_quote_log`

Triggered by `update_melt_quote_state` and `update_melt_quote_request_lookup_id`.

```sql
CREATE TABLE melt_quote_log (
    change_id           BIGINT PRIMARY KEY,
    quote_id            TEXT NOT NULL,
    state               TEXT,
    request_lookup_id   TEXT,
    payment_proof       TEXT,
    paid_time           BIGINT
);
CREATE INDEX idx_melt_quote_log_quote ON melt_quote_log(quote_id, change_id);
```

Lifecycle: Unpaid -> Pending (inputs locked) -> Paid (payment confirmed) or Failed (can retry). Each transition appends one row.

#### `proof_log`

Triggered by `update_proofs_state` and `remove_proofs`.

```sql
CREATE TABLE proof_log (
    change_id       BIGINT PRIMARY KEY,
    change_type     SMALLINT NOT NULL,  -- 0 = Updated, 1 = Deleted
    ys              TEXT NOT NULL,      -- JSON array of Y hex values
    state           TEXT,               -- new state (Updated) or final state (Deleted)
    count           INTEGER NOT NULL
);
CREATE INDEX idx_proof_log_change ON proof_log(change_id);
```

Lifecycle: Unspent -> Pending -> Spent. On compensation, pending proofs may be deleted. Each batch operation appends one row.

#### `keyset_log`

Triggered by `set_active_keyset`.

```sql
CREATE TABLE keyset_log (
    change_id       BIGINT PRIMARY KEY,
    keyset_id       TEXT NOT NULL,
    active          BOOLEAN NOT NULL
);
CREATE INDEX idx_keyset_log_keyset ON keyset_log(keyset_id, change_id);
```

Lifecycle: Created (active=true) -> rotated (active=false). Each rotation produces two entries (old deactivated, new activated).

### `change_id`

Application-generated `i64`:

```
Bit 63:      always 0 (positive signed i64)
Bits 62..22: 41 bits of millisecond timestamp since custom epoch
Bits 21..0:  22 bits of hash(changeset)
```

The hash is derived from `(table_name, entity_id, change_type)`. Time-sortable by high bits, content-aware by low bits. No separate `changed_at` column needed.

### Implementation approach

Each mutation method in `crates/cdk-sql-common/src/mint/*.rs` appends an `INSERT INTO <entity>_log` after the primary mutation, on the same database connection/transaction. If the transaction rolls back, the log entry rolls back too.

This is the same pattern already used for `keyset_amounts` updates piggy-backed onto proof/signature mutations.

#### Affected methods

| File | Method | Log table |
|------|--------|-----------|
| `quotes.rs` | `update_melt_quote_state` | `melt_quote_log` |
| `quotes.rs` | `update_melt_quote_request_lookup_id` | `melt_quote_log` |
| `proofs.rs` | `update_proofs_state` | `proof_log` |
| `proofs.rs` | `remove_proofs` | `proof_log` |
| `keys.rs` | `set_active_keyset` | `keyset_log` |

#### New files

| File | Purpose |
|------|---------|
| `crates/cdk-common/src/database/event_log.rs` | `ChangeType` enum, `generate_change_id()` |
| `crates/cdk-sql-common/src/mint/event_log.rs` | Per-table `append_*_log()` SQL helpers |
| `migrations/sqlite/20260629000000_create_log_tables.sql` | 3 log tables |
| `migrations/postgres/20260629000000_create_log_tables.sql` | 3 log tables |

### Invariants

1. Existing tables remain the source of current state
2. Log tables are append-only. No updates, no deletes
3. Every logged state change is appended in the same transaction as the mutation
4. Events are ordered by `change_id` (primarily by embedded timestamp)
5. The database enforces uniqueness via `change_id` primary key

### Positive Consequences

* Full audit trail of every financially meaningful state transition
* Enables replay, reconciliation, and debugging without changing operational model
* Typed columns allow schema-validated, indexable queries
* Minimal scope: only 3 tables, 5 methods

### Negative Consequences

* Small write amplification (one extra INSERT per state change)
* Log tables grow without bound and will eventually need a retention or archival policy
