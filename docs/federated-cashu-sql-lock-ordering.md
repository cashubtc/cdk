# Federated Cashu SQL Lock Ordering

This note documents the lock ordering contract for SQL-backed federation state.
It applies to SQLite and Postgres mint storage, and to any future
`federation_signature_share` or member-status tables.

The current consensus path serializes accepted operations before persistence,
but SQL code still needs a stable rule for concurrent catch-up, replay,
admin repair, and future SQL-level conflict-key tables. When a transaction must
touch more than one federation resource, it must derive all logical lock labels
before acquiring any row lock, sort the labels lexicographically, and acquire
locks only in that sorted order.

## General Rule

For every federation write transaction:

1. Compute the operation ID and all `FederationOperationConflictKey` values
   before opening or mutating SQL state.
2. Convert every lockable resource to a stable ASCII label.
3. Sort the full label set lexicographically.
4. Acquire locks or perform lock-equivalent writes in that exact order.
5. Re-check duplicate operation IDs and conflicting conflict keys while holding
   the locks.
6. Insert or update journal rows, sequence watermarks, catch-up certificates,
   payment-observation rows, materialized mint rows, and signature-share rows.
7. Commit only after all rows for the accepted operation are durable.

Do not lock in request order. Batch requests can carry the same resources in
different orders, and request-order locking can deadlock two otherwise valid
transactions.

## Consensus Lock Labels

Use these logical labels when SQL needs to lock accepted-operation resources.
The `conflict_key_label` value is a canonical JSON rendering of one typed
`FederationOperationConflictKey`. It is a transient lock label, not a singular
journal column; journal rows persist only the complete `conflict_keys` vector.

| Resource | Label |
| --- | --- |
| Exact operation ID | `operation_id:{operation_id}` |
| Conflict key | `conflict_key:{conflict_key_label}` |

If a transaction checks both exact duplicates and conflict keys, include the
operation ID label and every conflict-key label in one sorted list. Do not lock
the operation ID first in one path and the conflict key first in another path.

## Current Conflict-Key Set

The authoritative source is `FederationOperation::conflict_keys()` in
`crates/cdk-common/src/federation/operation.rs`.

| Operation | Conflict keys |
| --- | --- |
| `MintQuote` | One `MintQuote { quote }` key. |
| `MintQuotePayment` | One `MintQuotePayment { quote, state, observer }` key. |
| `Mint` | One `Mint { quote }` key for each quote ID in the mint request. Batch minting can produce multiple keys. |
| `MeltQuote` | One `MeltQuote { quote }` key. |
| `MeltQuotePayment` | One `MeltQuotePayment { quote, state, observer }` key. |
| `Melt` | One `Melt { quote }` key. |
| `KeysetRotation` | One `KeysetRotation { keyset_id }` key. |
| `Swap` | No journal conflict key. Spend conflicts are enforced by deterministic apply against proof state. |
| `Restore` | No journal conflict key. Restore is a lookup/signature-reconstruction path for already accepted outputs. |

For multi-key operations, such as batch minting, sort the canonical labels before
locking. Never rely on the wallet-provided quote order.

## Materialized Apply Locks

Accepted journal order decides which operation is valid. Apply code then
materializes that decision into mint tables. If an apply transaction must lock
several materialized rows, derive stable labels and lock them after the
consensus labels, also sorted lexicographically:

| Materialized row | Label |
| --- | --- |
| Mint quote row | `mint_quote:{quote}` |
| Melt quote row | `melt_quote:{quote}` |
| Proof state row | `proof:{y}` |
| Blinded output/signature row | `blinded_message:{keyset_id}:{blinded_secret}` |
| Future federation signature-share row | `signature_share:{operation_id}:{output_index}:{member_id}` |
| Future member-status row | `member_status:{member_id}` |

When a single SQL statement can lock all rows in one deterministic query, prefer
that shape. Existing melt quote helpers already use a single atomic related-quote
lock to avoid circular waits.

## Operation-Specific Notes

- Mint issuance: lock all `Mint { quote }` conflict keys, then quote rows, then
  blinded output/signature rows sorted by keyset and blinded secret.
- Batch mint issuance: include every quote ID in the same sorted lock set before
  checking payment state or writing shares.
- Swap: no journal conflict key exists today, so apply must lock proof rows by
  sorted `proof:{y}` labels before writing spent state and output shares.
- Melt: lock the `Melt { quote }` conflict key, the melt quote row, then proof
  rows and change-output rows in sorted order.
- Payment observations: lock the member-scoped observation conflict key before
  writing observation evidence.
- Keyset rotation: lock the `KeysetRotation { keyset_id }` conflict key before
  writing keyset rows or changing active flags.
- Catch-up and startup replay: replay entries in journal index order. For each
  entry, use the same per-operation lock ordering as live application.

## Review Checklist

Any change that adds a federation SQL table or row lock should answer these
questions before merge:

- Does the transaction compute all lock labels before the first row lock?
- Are labels sorted once and used consistently across live submit, catch-up, and
  startup replay?
- Does a batch or multi-output operation avoid request-order locking?
- Are exact duplicate checks and conflict-key checks performed under the same
  sorted lock discipline?
- If a future `federation_signature_share` table is added, can a reviewer trace
  each share row by `operation_id`, `output_index`, and `member_id` without
  changing the lock order?
- Does the existing concurrent federation journal write stress test still pass?
