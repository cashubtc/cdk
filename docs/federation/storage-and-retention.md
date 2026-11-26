# Federation storage and retention

This document is the storage contract for a federation member: what each store
holds, who is authoritative for it, how long it is kept, and how a member that
lost its history rejoins. Consensus and checkpoints are described in
[consensus and checkpoints](consensus-and-checkpoints.md).

## Backends and traits

Federation mint storage is implemented for SQLite (including the in-memory
test database) and PostgreSQL, through `cdk-sql-common`. `cdk-redb` is a wallet
database and stores no federation state; wallets that talk to a federation use
any wallet backend.

The federation storage surface is ten traits in
`crates/cdk-common/src/database/mint/federation.rs`: a transaction and a
connection trait each for the journal, application state, BLS/FROST DKG, the
FROST wallet and the setup ceremony. Families of reads that differ only by
filter or range are single keyed or ranged operations
(`FederationIndexRange`, `FederationJournalEntryKey`,
`FederationCheckpointSelector`, `FederationSignatureShareKey`,
`FederationPaymentObservationSelection`). Write families of immutable records
take one record enum (`FederationDkgRecord`, `FederationSetupRecord`). Which
setup record may accompany which session transition is decided in the domain
(`FederationSetupStep::validate_transition`), not in storage.

The Bark projection has its own typed store, `FederationBarkStore`, defined in
`cdk-federation-bark` and implemented in `cdk-sql-common`; the mint receives it
through `MintBuilder::with_federation_bark_store`.

The consensus projections (`transaction`, `chain_observation`,
`address_allocation`, `bark_recovery`) are stored one key-value row per entity
through `FederationStateTable` (`crates/cdk-common/src/federation/state_table.rs`).
Each apply loads the projection inside the apply transaction and writes back
only the rows that changed. Derived indexes are rebuilt on load, not stored.

Lifecycle transitions (DKG ceremony, setup session, invitations) lock and read
the row, accept an identical row as an idempotent retry, and otherwise require
the domain's transition function before writing.

Federation creation SQL is not checksum-locked; under the cut-through policy it
is edited in place. The standalone migration history is frozen by the
migration checksum regression.

## One apply transaction per operation

Every accepted operation applies in one mint database transaction that also
marks its journal entry applied (`finish_federation_apply_transaction`). This
includes melt payment evidence: `finalize_federation_melt_in_transaction`
finalizes a paid melt (quote paid, inputs spent, change shares and signatures
stored, completed operation recorded, melt request and saga deleted) inside the
apply transaction, and events are published only after commit. A Bark operation
that carries melt evidence writes its application rows, its observations and
the melt outcome in that same transaction.

Runtime locks are taken in one order: operation application, then the
consensus runtime, then the database transaction. Network requests complete
before any of these locks are taken (see `crates/cdk-federation/src/runtime_components.rs`).

## Storage contract

| Store | Purpose | Authority | Retention |
|---|---|---|---|
| `federation_journal_entries` | Ordered operations, status, rolling digests | Consensus order | Deleted below the retention floor |
| `federation_journal_conflict_keys` | Duplicate and conflict guards | Consensus order | Deleted with their entries |
| `federation_aleph_bft_finalized_items` | Finalized consensus items | AlephBFT | Deleted below the retention floor |
| `federation_aleph_bft_backups` | Local AlephBFT session backup | Local | Only the active session is kept |
| `federation_checkpoints`, signatures, quorums | Signed frontiers | Quorum | Recovery checkpoints pruned once a scheduled quorum covers them; scheduled checkpoints below the floor checkpoint deleted, except the trusted and audited ones |
| `federation_state_snapshots`, `federation_state_snapshot_rows` | Frozen state at scheduled checkpoints, for state transfer | Quorum-signed digest | Latest two scheduled snapshots |
| `federation_payment_observations` | Member payment evidence | Consensus order | Retired when the quote is final; deleted below the floor |
| `federation_signature_shares` | Member shares of blind signatures | Consensus order | Never deleted (NUT-09 restore) |
| `federation_materialized_state_keys` | Index of rows covered by the state digest | Consensus order | Follows the entities it indexes |
| Bark actions, VTXOs, transitions, observations, allocations, reservations | Bark projection | Consensus order | Transitions and observations of retired actions deleted below the floor; the action row stays as replay guard; VTXOs, allocations and VTXO reservations kept |
| `transaction` rows | Payment intents, accepted transactions, lineage heads, UTXO reservations | Consensus order | A settled lineage is deleted whole below the floor |
| `chain_observation` rows | Wallet UTXOs and evidence | Consensus order | Spent UTXOs reduced to a permanent tombstone below the floor |
| `address_allocation` rows | Quote to derivation index | Consensus order | Kept: an address can receive another deposit at any time |
| `bark_recovery` rows | Recovery input reservations | Consensus order | Published reservations deleted below the floor |
| Mint quotes, proofs, blind signatures, completed operations | Mint business state | Consensus order | Never deleted |
| Payment scheduler entries (KV, one per quote) | Local retry timing | Local | Deleted when a quote completes; corrupt or unknown entries are rebuilt |

Member-local secrets are not federation state and are not in this table: the
member identity key, BLS and FROST key shares, Bark operational and receive
secrets, and the FROST nonce files.

## Validity window

Every operation envelope carries the AlephBFT `session` its author stamped it
in. The stamp is outside the operation ID, so a client retry keeps its ID and
idempotent response, but it is covered by the author's authorization signature,
so only the author can re-stamp it. The submission funnel stamps each envelope
this member authored with the active session and signs it again before it
enters the mempool.

At finalization an envelope is `RejectedInvalidOperation` unless the unit's
session lies in `[session, session + FEDERATION_OPERATION_VALIDITY_SESSIONS]`
(16). Every member computes this from the finalized bytes; startup recovery
checks stored statuses with the same rule. An operation still pending when its
window closes is rejected, and its caller submits again with a fresh stamp.

Duplicate and conflict detection is bounded by the same window on every member:
an entry guards a unit finalized in session `S` only if it was ordered in
session `S - FEDERATION_OPERATION_VALIDITY_SESSIONS` or later
(`federation_operation_guards_session`). Journal rows record `ordered_session`
and `ordered_item_index` for this purpose; an operation ID may repeat in the
journal, and lookups use the latest entry. A member holding only the window and
a member holding all history therefore decide alike.

Once an operation ID falls outside the window, a replay is stopped by the
entity it would change rather than by the journal:

| Kind | Who can re-stamp | Guard after the window |
|---|---|---|
| `MintQuote`, `MeltQuote` | Any member | Existing quote row |
| `MintQuotePayment`, `MeltQuotePayment` | Observer | Observation row by operation ID; quote state |
| `Mint` | Any member | Issued quote (rejected) |
| `Melt` | Any member | Melt quote state |
| `Swap` | Any member | Spent proofs and existing signatures for the operation |
| `KeysetRotation` | Any member | Keyset state |
| `AddressAllocation` | Any member | Quote mapping (`Existing`) |
| `ChainObservation` | Observer | Per-observer evidence epoch and depth; spent-output tombstones |
| `TransactionProposal` | Proposer | Known proposal; lineage reservations |
| `TransactionSigned`, `TransactionBroadcastIntent` | Any member | Existing record; rejected once the lineage is terminal |
| `TransactionBroadcastObservation` | Observer | Exact recorded observation; latest attempt per observer |
| Bark `Application` operations | Author | Operation ID recorded in the Bark projection; retired action row |

Each replay must end applied (with no state change) or in a definitive
rejection, never `Failed`, which would stall application.
`crates/cdk/src/mint/federation/tests/replay_window.rs` covers every kind
except the Bark `ReceivePrepared`, `ActionPlan`, `ActionTransition` and
`EvidenceCertificate` replays, which take the same Bark no-op path but have no
fixture in `cdk`.

## Retention floor

`FederationRetentionFloor` is stored with the checkpoint state and advanced by
`prune_federation_history`:

- Take the two latest scheduled checkpoints with a stored quorum that the
  trusted checkpoint covers. With fewer than two there is no floor.
- The floor is the earlier one's journal and finalized-item positions, or the
  start of the latest one's validity window (including the one older item that
  proves the window complete) if that is earlier.
- It is lowered to the earliest journal entry that live state still reads by
  operation ID: a melt saga's operation, an action that is not retired, and an
  outgoing transaction that is not confirmed, abandoned or replaced
  (`Mint::federation_transaction_retention_pin`).
- It never moves back. The order, finalized-item and application digests just
  below it are recorded while those rows exist, and digest lookups fall back to
  them.

Without the pins the floor is a function of the scheduled checkpoint sequence
and the finalized items, so every member that promoted the same checkpoints
computes the same floor. Pins only lower it. One full checkpoint interval above
the floor stays available, so a member one interval behind catches up by
ordinary replay.

## Deletion

After every checkpoint promotion,
`FederationNodeRuntime::prune_history_below_retention_floor` runs up to 64
batches of `FEDERATION_RETENTION_PRUNE_BATCH_ROWS` (500) rows, each in its own
transaction:

1. journal entries with their conflict keys, finalized items, and scheduled
   checkpoints (with signatures and quorums) older than the floor checkpoint,
   except the trusted and audited ones;
2. terminal entities (`Mint::prune_federation_terminal_state`): each projection
   in its own transaction, oldest first, a lineage always whole; then retired
   payment observations and the transitions and observations of retired Bark
   actions.

Later promotions continue where a run stopped. A failure is logged and retried
at the next promotion.

## Terminal means retired

An entity is terminal from the operation that made it final, and from that
operation on no operation reads it. Projections retire terminal rows when they
load (`FederationStateTable::retire_terminal_rows`), live payment-observation
reads skip retired rows, and an operation that names a retired Bark action
applies as a no-op. Terminality is decided by consensus state, so every member
retires at the same journal index, and a member that deleted retired rows
decides every later operation exactly like one that kept them.

| Entity | Terminal when | Recorded index | Permanent guard |
|---|---|---|---|
| Payment observation | Its quote is final (Bolt11 mint quote issued, melt quote paid) | `terminal_operation_index` | Quote state; observations of a final quote are not recorded |
| Spent wallet UTXO | Spent by a terminal CDK lineage or a Bark recovery publication | `spent_operation_index` | Tombstone with outpoint and, when it counted, the quote and output |
| Outgoing transaction lineage and its intents | One transaction confirmed, every melt quote it pays is paid, and its inputs are still reserved | `settled_operation_index`, `terminal_operation_index` | Spent-output tombstones; melt quote state |
| Bark recovery reservation | Published | `published_operation_index` | Spent-output tombstone |
| Bark action | Final phase and its quote settled (mint paid, melt paid or rolled back), or no quote | `retired_operation_index` | The action row |

The spent-output tombstone keeps the quote and amount it contributed, because
an on-chain mint quote's payment is cumulative over every output paid to its
address; `quote_payment_at_or_before` merges tombstones with live outputs, so
the paid amount is the same whether the full record exists or not.

Not pruned: lineages that never confirm or whose quotes are never paid;
unspent, reorged and quarantined outputs; observations of Bolt12 and on-chain
mint quotes (they can be paid again) and of unpaid melts; Bark actions bound to
a quote that never settles (they also keep pinning their journal entries); Bark
VTXOs, allocations and VTXO reservations.

## Digest rule

Every checkpoint signs a `materialized_state_digest`. Deletion must not change
it, so exclusion is part of its definition:
`get_federation_materialized_state_floor(checkpoint)` is the floor the
checkpoint sets as the latest scheduled checkpoint (before pins), and the
checkpoint's digest leaves out payment observations retired below it, lineages
terminal below it, recovery reservations published below it, and transitions
and observations of Bark actions retired below it. Outputs spent below it
appear as their tombstone row.

Scheduled and recovery capture use the floor of the checkpoint being captured,
the audit uses the trusted checkpoint's floor, and the live digest (operator
status) uses the latest scheduled checkpoint's floor. A member deletes only
below its stored floor, which is never above the floor of any later checkpoint,
so deleted rows are always rows the digest leaves out.

The digest covers every field a state transfer installs, as consensus-fixed
values: quote rows include the payment lookup ID and NUT-20 key (mint) or the
expiry, options, request, fee options and selected fee index (melt); proof rows
include the full proof, quote ID and operation; blind-signature rows include
the quote, operation, output position and accepted index but not the signature,
which each member recomputes from its own share; completed operations include
the melt payment amount and fee. Fields that can differ between members
(creation time and extra fields on the originating member, a melt's processor
lookup ID, paid time) are left out.

## State transfer

A member that is behind a peer's floor cannot replay; it installs the state of
a scheduled checkpoint instead.

- **Snapshot.** Scheduled capture stores the digest rows it already builds in
  the candidate's transaction (`federation_state_snapshots`,
  `federation_state_snapshot_rows`) and keeps the latest two
  (`FEDERATION_STATE_SNAPSHOT_RETAINED`). One source read
  (`get_federation_materialized_state_source`) and one row builder
  (`crates/cdk/src/mint/federation/state_transfer.rs`) serve the live digest,
  the snapshot and the installer's check.
- **Transfer.** `POST /federation/v1/state-transfer` pages the snapshot (500
  rows by default, at most 5,000, never above the 8 MiB catch-up page limit);
  `POST /federation/v1/state-transfer/window` pages the validity window before
  the checkpoint. Requests are signed peer requests; responses are unsigned,
  because rows are trusted only once they hash to the quorum-signed digest, so
  one honest peer suffices. A whole transfer is capped at
  `FEDERATION_MAX_STATE_TRANSFER_BYTES` (1 GiB), because the installer holds it
  in memory.
- **Window.** The transfer carries every finalized item and journal entry
  ordered in the `FEDERATION_OPERATION_VALIDITY_SESSIONS` sessions before the
  checkpoint, plus one older item that proves the window complete. Before
  anything is written, the items must chain to the signed finalized-item
  digest, the entries must be exactly the accepted items and chain to the
  signed order and application digests. Without the window an installed member
  would treat a recent duplicate as new and fork.
- **Install.** `Mint::install_federation_state_transfer` verifies the quorum,
  requires a scheduled checkpoint and strictly sorted rows that hash to the
  signed digest, decodes every row, and then in one transaction requires empty
  federation state, writes the rows through the apply path's own storage calls
  (payments rebuilt from observations, the member's own shares recomputed from
  its key share), recomputes the digest and commits only on equality. In the
  same transaction it stores the window as history and sets the checkpoint as
  frontier, trusted and audited checkpoint and state-transfer base. The window
  start becomes the member's retention floor.
- **Trigger.** When every peer attempted in a sync round answers catch-up with
  `PeerHistoryPruned` (HTTP 410 for `BelowRetentionFloor`), an empty member
  runs `install_state_transfer_below_peer_floors`: it installs a peer's latest
  scheduled checkpoint and restarts the consensus runtime there
  (`adopt_installed_state_transfer`); later rounds catch up the suffix. A
  member with no stored next checkpoint first probes the peer toward the latest
  known quorum so that it sees the pruned answer.

State transfer refuses, or does not restore:

- a snapshot with an in-flight melt (a pending melt quote or proof); the member
  retries from a later checkpoint;
- unsigned blinded messages of finished melts, which are not in the digest;
- per-keyset fee totals (`keyset_amounts.fee_collected`), which start from
  zero;
- Bark journal lookups below the base: the Bark projection is installed, but an
  action that spans the checkpoint and needs a journal read by operation ID
  cannot advance on the installed member;
- member-local secrets (identity, BLS and FROST shares, Bark wallet and receive
  secrets), which come only from the member's own backup.

## Stale member: halt and reset

A member that still holds federation state but finds every peer's history
pruned below its frontier cannot install a transfer. It stops its AlephBFT live
session and sets `FederationSyncStatus::ResetRequired { journal_next_index }`:
operator status shows `sync_state = "reset_required"`, public requests get
`NodeHalted`, and sync rounds return at once. No other status transition clears
it. The halt is not persisted: a restarted member without a reset halts again
at its first sync round.

`cdk-mintd federation reset-member --confirm-wipe`, run while the daemon is
stopped, deletes in one transaction (`reset_federation_member_state`):

- consensus state: journal, conflict keys, finalized items, AlephBFT backups,
  pending operations and session, checkpoints with signatures, quorums,
  snapshots and frontier (so also the retention floor and state-transfer base);
- application state: key registry, payment observations, signature shares,
  consensus projections, Bark projection, payment schedule and Bark replay
  markers;
- mint business rows: quotes and their payments and issuances, melt requests,
  proofs, blind signatures, sagas, completed operations, per-keyset totals.

It keeps the federation config snapshot, DKG and setup records, FROST key
material, the mint's keysets and configuration, and does not touch the FROST
nonce files. It refuses without `--confirm-wipe` before opening anything, and
it cannot detect a running daemon. The next start is the ordinary empty-member
path. See [operations](operations.md#member-below-every-peers-retention-floor).

## Readers and retained history

With terminal rows retired and deleted below the floor, a reader of live rows
is bounded by unfinished work plus at most one checkpoint interval of retired
rows.

Bounded readers:

| Reader | Reads | Bounded by |
|---|---|---|
| Startup suffix (`bootstrap.rs`) | Journal and finalized items from the trusted checkpoint | One checkpoint interval (everything before the first checkpoint, when nothing is pruned yet) |
| Catch-up and window serving | One page | The page limit |
| Bark replay (`reconcile_federation_bark_runtime`) | Journal from a durable per-scope marker | Entries not yet replayed |
| Operator Bark reconcile route | `bark_projection_frontier`, one indexed `MAX` per Bark table | Constant |
| Bark custody signing wait | The action and the plan's input VTXOs by key | The plan |
| Bark locator allocation on apply | `MAX(locator_index)` | One index entry |
| Effective config rebuild | Registry rows of kind `keyset_rotation` | Keyset rotations |
| Apply-path payment observations | Live observations of one quote | That quote |
| Payment-observation operator report | Live observations, once | Live observations (Bolt12, on-chain mint and unpaid melt observations never retire) |
| `transaction` and `bark_recovery` projections | Every row of the namespace | Live entities plus one interval |

Readers that still grow with retained history, and why:

- **Chain-observation and address-allocation projections**, loaded whole on
  the apply path: tombstones and allocations are permanent, and validation of
  allocations, deposits, reorgs, proposals and on-chain quote payments works on
  the whole in-memory projection. Bounding them needs a projection that fetches
  outputs, tombstones and allocations by key inside the apply transaction, a
  change to the validation code.
- **Bark drivers and recovery driver** (`bark_projection_snapshot`): the
  snapshot is passed to about twenty runtime functions that select over every
  VTXO and allocation and find actions by quote across final phases. Bounding
  them needs a store-backed projection view and a decision about which retired
  actions and spent VTXOs a driver may stop seeing.
- **Bark operator status**: it counts VTXOs by state, spent ones included, and
  receive intents over all retained transitions.
- **Bark recovery FROST authorization**: needs the chain-observation projection
  whole.
- **Checkpoint materialization** (live digest, capture, audit, snapshot): the
  digest covers tables that are never deleted (quotes, proofs, blind
  signatures, completed operations, signature shares, allocations). Streaming
  it would need every table in the digest's Rust string order rather than SQL
  collation order, and capture stores every row as the snapshot anyway, so it
  stays one in-memory build per capture, audit or requested status digest.

`federation_hot_reads_stay_bounded_as_history_grows` (storage conformance,
SQLite and PostgreSQL) checks that the bounded reads return the same rows as
retained history grows, and the SQLite plan test checks that they use indexed
searches.
