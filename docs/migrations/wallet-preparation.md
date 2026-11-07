# Wallet storage preparation API

Wallet preparation uses a required `WalletDatabase::prepare_operation` method.
The change improves wallet storage reliability. Custom database implementers and
binding consumers must upgrade together.

## Transaction contract

`OperationPreparation` carries the complete recovery record, quote snapshots,
existing or explicit inputs, and an optional deterministic output counter range.
The backend validates and commits these changes in one transaction:

- A new saga ID must be unused; a later melt transition must match its persisted
  saga version and prepared state.
- Quotes must belong to the operation's mint and currency and match the expected
  versions. Initial acquisition requires no owner. Later melt transitions require
  the same operation owner. Initial acquisition increments the quote version.
- Initial inputs must be unspent and unowned. Explicit inputs may be imported;
  an existing proof retains its provenance and derivation index and must match the
  supplied proof. Later melt inputs must already be reserved by that operation.
- Counter ranges use compare-and-swap on the starting counter. The saga stores
  the exact range and outputs used by the operation. Committed ranges are never
  rewound during compensation.
- Nested melt swaps transfer input ownership to their own durable saga and
  advance the parent version in the same transaction. Parent recovery waits for
  an incomplete child before compensating.

A conflict or rejected write rolls back the entire transition. A conflicting
caller must reload state and rebuild its preparation; it cannot reuse an old
counter range or quote snapshot. A transport failure after an RPC commit can
have an ambiguous result: consult the persisted saga before retrying.

The transaction covers local persistence. Requests to mints, Lightning nodes,
and the Bitcoin network remain separate operations governed by the persisted
saga state and existing reconciliation rules.

## Backend and path coverage

| Area | Implementation |
| --- | --- |
| SQLite, including the in-memory implementation | One SQL transaction |
| PostgreSQL | One SQL transaction with conditional ownership/version updates |
| Redb | One write transaction |
| Supabase | One `prepare_wallet_operation` RPC transaction, authenticated wallet isolation, encrypted proof imports |
| Ordinary and explicit-proof/token melts | Shared atomic preparation for BOLT11, BOLT12, and on-chain |
| Melt request preparation | Pending input states, change counter range and requested saga state committed together |
| Mint issuance | Single and same-method batch preparation, quote ownership and output range committed together |
| Nested swaps | Recovery record, counter allocation and owned input handoff committed together |
| Maximum cross-mint transfer | Existing BOLT11 quoting API; execution uses explicit-proof melt preparation |

Mixed-method and mixed-currency mint batches remain unsupported. The maximum
cross-mint quote API remains BOLT11-only. This change does not add remote mint or
payment backend capabilities. Normal protocol, fee and signature requirements
continue to apply to each method.

Supabase administrators must apply the new migration before upgrading clients.
The required schema version is **11**. Existing SQLite/PostgreSQL wallet schemas
need no migration. Existing serialized swap records without parent metadata
remain readable.

## Swift, Kotlin and custom stores

The Rust database trait gains a required method without a sequential default.
UniFFI's `WalletDatabase` callback interface gains
`prepare_operation(preparation_json)` (Swift/Kotlin: `prepareOperation`). This is
a source and generated-binding compatibility change for custom store providers.
Regenerate all bindings and ship matching native libraries and generated code.

A wrapper around CDK's native SQLite database must forward the complete callback
once:

```swift
func prepareOperation(preparationJson: String) async throws {
    try await sqlite.prepareOperation(preparationJson: preparationJson)
}
```

```kotlin
override suspend fun prepareOperation(preparationJson: String) {
    sqlite.prepareOperation(preparationJson)
}
```

Do not implement this callback by calling the legacy reservation, counter and
saga callbacks separately. A store implemented entirely outside Rust must
implement the same transactional preconditions itself. The JSON value uses the
canonical Rust `OperationPreparation` representation.

Downstream wallet apps must update their SDK/native artifacts and custom store
implementations. No downstream app or release is updated by this repository
change. The binding regression tests exercise foreign callbacks forwarding into
CDK SQLite for all three supported payment methods, including failed
preparation, reopening storage, retry, cancellation and repeated recovery.

## Previously orphaned state

Proofs without a corresponding durable operation remain reserved. Recovery also
preserves associated melt quote ownership when such proofs exist. Local absence
of a saga or transaction does not prove that a remote request was never submitted.

Reconciliation needs an operation-specific record or other trusted evidence of
submission history, authoritative quote/payment status and the mint's current
state for all affected inputs. Pending or unavailable remote status is not enough
to release funds. If an earlier swap or issuance may have completed, its outputs
and deterministic recovery range also need reconciliation. This change does not
automatically repair legacy orphaned proofs or reconstruct missing history.
