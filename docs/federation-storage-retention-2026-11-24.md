# Federation storage contract and checkpoint retention

Status: prerequisites 1, 3 and 4 implemented (validity window, Bark readers,
state transfer); prerequisites 2, 5 and 6 open. Follows package 6 of
`bls-federation-architecture-review-plan-2026-11-23.md`.

Decision taken: checkpoints may truncate logs and history, and also terminal
consensus entities.

## Finding: nothing can be truncated safely yet

The plan assumed checkpoints could become truncation points directly. Three
facts in the current code rule that out:

1. **No state transfer.** A member that is behind always replays journal
   entries and finalized items from its own frontier, one scheduled checkpoint
   at a time (`cdk-federation/src/sync_loop.rs:659`, `:1300`). A wiped member
   replays from genesis (`cdk-federation/src/bootstrap.rs:452`). Peers serve
   catch-up only from journal rows they still hold
   (`cdk-axum/src/federation/catch_up_routes.rs:115`). If peers deleted rows
   below a checkpoint, a member behind that checkpoint could never rejoin.
2. **Replay protection depends on journal rows.** Operation envelopes carry no
   expiry and no session binding (`cdk-common/src/federation/operation.rs:1929`).
   A resubmitted envelope is recognised as a duplicate by its durable journal
   row (operation id) or its conflict keys
   (`cdk-sql-common/src/mint/federation/journal.rs:1038`,
   `cdk-federation/src/operation_pipeline.rs:862`). Swaps, signed transactions,
   broadcast intents and broadcast observations have no conflict keys. Without
   the row, an old envelope would be ordered and appended again, and only the
   mint database and projection checks would stop it from applying.
3. **Terminal entities are replay guards and digest inputs.** A spent UTXO
   stops a replayed deposit observation from crediting the same output again.
   An allocation turns a replayed allocation into `Existing` instead of a new
   index. Every terminal entity is hashed into the materialized-state digest
   that each checkpoint signs (`cdk/src/mint/federation/projection.rs:1917`).
   Members prune at different local times, so pruning by local promotion would
   make their digests differ and no checkpoint quorum would form.

In addition, Bark reconciliation, the Bark recovery driver and the operator Bark
reconcile route read the whole journal from index 0
(`cdk-federation/src/bootstrap.rs:631`, `cdk-bark/src/node/projection.rs:43`,
`cdk-axum/src/federation/operator_routes.rs:142`, `recovery_driver.rs:97`).

## Storage contract

| Store | Purpose | Authority | Readers | Retention target |
|---|---|---|---|---|
| `federation_journal_entries` | Ordered operations, status, rolling digests | Consensus order | Startup suffix, catch-up serving, point lookups at apply and signing, Bark readers | Bodies below the retention floor deleted once state transfer exists |
| `federation_journal_conflict_keys` | Duplicate quote and resource claims | Consensus order | Journal append, finalization pre-check | Kept, or bounded by the replay window (decision A) |
| `federation_aleph_bft_finalized_items` | Finalized consensus items | AlephBFT | Startup suffix, catch-up serving, checkpoint capture | Below the retention floor deleted once state transfer exists |
| `federation_aleph_bft_backups` | Local session backup | Local | Startup | Already pruned before the active session |
| `federation_checkpoints`, signatures, quorums | Signed frontiers | Quorum | Startup, catch-up, promotion | Already pruned: recovery checkpoints covered by a scheduled quorum. Keep the trusted one and the one at the retention floor |
| `federation_payment_observations` | Member payment evidence | Consensus order | Apply (quorum finalization), reports, digest | Deleted when the quote is terminal below the floor |
| `federation_signature_shares` | Member shares of blind signatures | Consensus order | NUT-09 restore, retried mint and swap responses | Kept: restore must work for any past output |
| `federation_materialized_state_keys` | Index of rows in the materialized digest | Consensus order | Snapshot, keyset rotation rebuild | Follows the entities it indexes |
| Bark actions, vtxos, transitions, observations | Bark projection | Consensus order | Bark drivers, digest | Transitions and observations of terminal actions deleted below the floor |
| `transaction` rows | Intents, accepted transactions, lineage heads, reservations | Consensus order | Apply, signing authorization, broadcast, digest | Terminal transactions and their intents deleted below the floor |
| `chain_observation` rows | Wallet UTXOs and evidence | Consensus order | Apply, transaction policy, quote payment, digest | Spent UTXOs reduced to an outpoint tombstone below the floor |
| `address_allocation` rows | Quote to derivation index | Consensus order | Apply, deposit attribution | Kept: an address can receive another deposit at any time |
| `bark_recovery` rows | Recovery input reservations | Consensus order | Apply, digest | Published reservations deleted below the floor (the spent UTXO tombstone covers the input) |
| Payment scheduler entries | Local retry timing | Local | Payment observer | Already deleted when a quote completes |

## Design

### The retention floor is a consensus position

The floor is the `journal_next_index` and `finalized_next_item_index` of the
scheduled checkpoint *before* the latest one. It is a function of the
scheduled checkpoint sequence, not of when a member promoted, so every member
computes the same floor. Keeping one full checkpoint interval of bodies above it
lets a member one interval behind catch up with today's protocol.

### Terminal pruning is defined, not performed

The materialized-state digest of checkpoint *n* excludes every entity whose
terminal operation index is below the floor of checkpoint *n*. Exclusion is
part of the digest definition, so members agree whatever their local deletion
timing. Physical deletion can then run in the background at any time after the
member has promoted checkpoint *n*. This needs every terminal entity to record
the operation index at which it became terminal; spent UTXOs, confirmed,
abandoned and replaced transactions, and completed and failed intents do not
all record that yet.

### Prerequisites, in order

1. **Replay guard without journal bodies: validity window (decided, implemented).**
   See "Validity window" below.
2. **Outpoint tombstones for spent UTXOs**, kept forever, so a replayed deposit
   observation cannot recreate a pruned UTXO.
3. **Bark readers off the full journal.** Reconcile, the recovery driver and the
   operator route read the Bark projection instead.
4. **State transfer (decision B, implemented; see "State transfer: built").**
   A member below a peer's floor downloads the
   checkpoint's materialized state as paged rows, verifies them against the
   signed `materialized_state_digest`, installs them, and replays the suffix.
   This covers the mint database rows the digest already enumerates (quotes,
   proof states, blind signatures, completed operations, keyset rotations). It
   is a new protocol path with its own limits and tests.
5. **Terminal-index fields and the digest exclusion rule** (see above).
6. **Deletion.** Journal and finalized bodies below the floor, then terminal
   entities, payment observations and Bark history, each in bounded batches in
   its own transaction.

Steps 1 to 3 are useful on their own. Steps 4 to 6 must land together: no
member may delete bodies until every member can install a state transfer.

## Validity window

Decision A was the validity window. Implemented:

- `FederationOperationEnvelope` has a `session` field. It is not part of the
  operation id, so a retried client request keeps its id and its idempotent
  response, but it is covered by the author's authorization signature
  (authorization transcript `v2`). The envelope version is now 7.
- The submission funnel (`cdk-federation/src/operation_pipeline.rs`) stamps
  every envelope this member authored with the active AlephBFT session and signs
  it again before it enters the mempool. Envelopes authored by other members
  keep their stamp.
- Finalization marks an envelope `RejectedInvalidOperation` unless the unit's
  session is within `[session, session + FEDERATION_OPERATION_VALIDITY_SESSIONS]`
  (16). Every member computes this from the finalized bytes, and startup
  recovery checks the stored status with the same rule.

Consequences:

- Duplicate detection by operation id only needs operations ordered in the last
  `FEDERATION_OPERATION_VALIDITY_SESSIONS` sessions. The retention floor must
  therefore also stay at or below the first journal index of session
  `current - FEDERATION_OPERATION_VALIDITY_SESSIONS`.
- An operation still pending when its window closes is rejected at finalization.
  Its waiter receives the rejection, and the caller submits again with a fresh
  stamp.
- Peer-to-peer operation submission is test-only. In production every envelope
  enters consensus through its author's funnel, so it is always stamped.

### Replay guard per operation kind, once ids outside the window are forgotten

Only a member can stamp an envelope. For kinds bound to an author, only that
author can; for the others, any member can, so a Byzantine member could re-wrap
an old operation in a fresh session. What then stops it applying twice:

| Kind | Re-stampable by | Guard after the window |
|---|---|---|
| `ChainObservation`, `MintQuotePayment`, `MeltQuotePayment`, `TransactionBroadcastObservation`, `Application` with an author | Author only | Projection state per observer, plus outpoint tombstones for deposits (prerequisite 2) |
| `TransactionProposal` | Proposer only | `accept_transaction` returns `Existing` for a known proposal; batch lineage heads |
| `Mint` | Any member | Issued-quote check and already-signed blinded messages in the mint database |
| `Melt` | Any member | Melt quote state in the mint database |
| `Swap` | Any member | Spent proofs in the mint database |
| `MintQuote`, `MeltQuote` | Any member | Existing quote rows in the mint database |
| `KeysetRotation` | Any member | Keyset state |
| `AddressAllocation` | Any member | `apply` returns `Existing` for a known quote |
| `TransactionSigned`, `TransactionBroadcastIntent` | Any member | `Existing` for a known transaction; rejected once the transaction is pruned |

Each row must be confirmed by a test that replays the operation with a fresh
stamp before step 6 deletes any journal row. Conflict keys follow the same
window: they can be deleted together with the operation ids.

### Window-bounded duplicate and conflict detection (implemented)

Every member now decides duplicates and conflicts from the same data: only
entries ordered in session `S - FEDERATION_OPERATION_VALIDITY_SESSIONS` or
later guard a unit finalized in session `S`
(`federation_operation_guards_session`). The bound comes from the finalized
unit's session and each entry's consensus evidence, never from local timing
or from how much history a member holds.

- **Finalization handler.** Accepted operation ids keep the session that
  ordered them; an id only marks an item `DuplicateOperation` while it guards
  the unit's session. Ids that no longer guard are dropped when a session
  begins, so the set is bounded by the window.
- **Journal.** `prepare_finalized_appends` treats an id or a conflict key as
  taken only if its latest holder guards the unit's session. Conflict keys map
  to their latest reserving entry. The in-memory hydrated guards are dropped
  with the window.
- **Durable lookups.** The finalization path and startup suffix verification
  register only stored entries that guard the item's session.
- **Storage.** An operation id may now repeat in `federation_journal_entries`
  (no unique constraint); lookups and application-state updates use the
  latest entry. `federation_journal_conflict_keys` keeps every holder, keyed by
  `(conflict_key_json, operation_index)`, and lookups take the latest. Journal
  rows carry `ordered_session` and `ordered_item_index` columns from their
  evidence, which the window bounds use instead of joining on operation id.
- **Content-addressed rows keep their first position.** A payment observation
  re-inserted with the same operation id, an identical signature share, and a
  materialized-state key re-recorded by the same operation id keep their
  first ordering position, so an already applied replay changes nothing.

Replay tests (`cdk`, `mint::federation::tests::replay_window`) apply each kind,
then a copy re-stamped in the first session after its window by a member
allowed to stamp it, appended at a later index with the same operation id.
Each replay must end `Applied` or in a definitive rejection (never `Failed`,
which would stall application), and the materialized-state rows must not
change:

| Kind (replay author) | Result | Guard |
|---|---|---|
| `MintQuote` (any) | Already applied | Existing quote row |
| `MintQuotePayment` (observer) | Already applied, paid amount unchanged | Observation row by operation id; quorum per observer |
| `Mint` (any) | Rejected | Issued quote |
| `MeltQuote` (any) | Already applied | Existing quote row |
| `Melt` (any), after payment | Already applied | Paid melt quote |
| `MeltQuotePayment` (observer), after payment | Already applied | Observation row; paid quote |
| `Swap` (any) | Already applied, shares keep their first index | Spent proofs and existing signatures for the operation |
| `KeysetRotation` (any) | Already applied | Keyset state |
| `AddressAllocation` (any) | Already applied | Quote mapping (`Existing`) |
| `ChainObservation::Deposit` (observer), also after the output was reserved and after a reorg and reconfirmation | Already applied | Per-observer evidence: same epoch and depth, or an older epoch |
| `ChainObservation::Reorg` (observer), also after reconfirmation in another block | Already applied (fix) | Stale epoch or the exact recorded reorg, checked before the block anchor |
| `TransactionProposal` (proposer), also after replacement and confirmation | Already applied | Known proposal; reservations stay with the lineage |
| `TransactionSigned` (any), after confirmation | Already applied | Equal signed transaction |
| `TransactionBroadcastIntent` (any), after Confirmed, Replaced, Abandoned | Already applied (fix) | Existing broadcast record with the same policy |
| `TransactionBroadcastObservation` (observer) | Already applied (fix); a superseded attempt is rejected | Exact recorded observation; latest attempt per observer |
| Bark `ReceiveIntent`, `MemberObservation` (author) | Already applied (fix) | Operation id recorded in the Bark projection |

The three fixes: a replayed reorg used to fail the block-anchor check and
quarantine a reconfirmed output; a replayed broadcast intent or observation
failed with a non-definitive error once the lineage moved on, which stalls
application; and a replayed Bark operation failed validation against the
action's current phase before the projection's own no-op check. The
remaining Bark kinds (`ReceivePrepared`, `ActionPlan`, `ActionTransition`,
`EvidenceCertificate`) take the same Bark no-op path but have no fixture in
`cdk` that builds them. The Bark projection guard reads transition and
observation rows, so deleting those rows for terminal actions (stage 3) needs
a guard that survives the deletion.

## Bark readers (prerequisite 3, implemented)

Startup, readiness, the operator reconcile route and the post-apply hook all
replayed the whole journal into the member's Bark runtime. They now call one
function, `reconcile_federation_bark_runtime(mint, runtime)`, which keeps a
durable marker per Bark scope (`cdk_bark` / `replay_next_index`, keyed by a
digest of the scope) holding the first journal index not yet replayed. A call
reads only entries from the marker on. After a successful replay the marker moves
past every finished entry and stops at the first entry still awaiting
application; a failed replay leaves it in place, so the same entries are tried
again. Replay is idempotent against the durable Bark wallet. A new Bark scope has
no marker and replays from the start, as a fresh wallet must.

## State transfer (prerequisite 4): design

### What the signed digest can and cannot vouch for

Every checkpoint signs `materialized_state_digest`, so a member that installs a
state whose recomputed digest equals the signed one has the federation's state,
provided the digest covers every byte it installs. Today it does not. The digest
rows are an audit projection and leave fields out:

- `mint_quotes` rows hold amount, unit, request, expiry, paid and issued
  amounts, state and method, but not the NUT-20 public key or payment
  identifiers;
- `blind_signatures` rows hold amount and keyset id, but not the signature;
- `proof_states` rows hold the state only, which is enough, because the key is Y;
- the consensus projections (transactions, UTXOs, allocations, Bark) are
  already complete encodings.

A field outside the digest is either added to it or verified on its own. Blind
signatures can be verified with a pairing check against the keyset's aggregate
public key. Quote public keys and payment identifiers must be added to the
digest rows. That changes the digest definition, which is acceptable
(cut-through), and is the first implementation step.

### Digest coverage, table by table

| Digest table | Covers what a transfer installs | Change |
|---|---|---|
| `transaction`, `chain_observation`, `address_allocation`, `bark_recovery` | Yes | Digest rows are the stored rows themselves (canonical JSON), so the digest and the transfer use one encoding |
| Bark actions, VTXOs, derivation allocations, reservations, transitions, observations | Yes | Already carried each record's full JSON |
| `payment_observations` | Yes, via a hash of the full observation | The snapshot row carries the observation as an attachment; the installer re-encodes the row from it and requires equality, which checks the evidence hash |
| `mint_quotes` | Yes, for consensus-fixed fields | Payment lookup id and NUT-20 public key added. Creation time and extra fields can differ on the originating member and stay out; the installer writes 0 and none. Payments are rebuilt from the payment-observation rows with the apply path's quorum rule |
| `melt_quotes` | Yes, for consensus-fixed fields | Expiry, options, creation time, extra fields and fee options added in step 1. Now also the request as canonical JSON (its display form does not parse back), the estimated blocks and the selected fee index. The payment lookup id is rewritten by the paying member's processor and stays out; paid time is local and stays out |
| `proof_states` | Yes | The full stored proof (step 1), and now the quote id and the stored mint operation (UUID and kind), which the apply path sets from the operation. A paid melt's duplicate-request check finds its inputs by quote id |
| `blind_signatures` | Yes, except the signature itself | Each member stores its own share, so the signature stays out. Added the quote id and the share's operation id, output position and accepted index, all consensus-fixed. The installer recomputes the member's share from its key share |
| `completed_operations` | Yes | Added the melt payment amount and fee, which the apply path fixes from the accepted observation. Per-keyset fee totals are kept as running sums in `keyset_amounts`, not per operation, and are not restored (issued and redeemed totals are rebuilt by the inserts the apply path uses) |
| `federation_keysets` | Yes | Unchanged: the row carries the whole keyset and the rotation's operation position |
| FROST root public configs, FROST application configs | Yes | Unchanged: each row carries the whole config JSON |

Every added field was checked in multi-member tests that compare members'
digests after real flows (`aleph_value_flow`, `catch_up`, `recovery`,
`checkpoint`): members still agree.

The key registry (`federation_materialized_state_keys`) is not hashed, only
enumerated. Its rows are rebuilt from the digest rows. Their operation position
(the operation that last wrote the row) is not consensus-checked, so it stays
out of the digest; installed records name the checkpoint position instead.
`Application` registry keys are not hashed or read by anything and are not
restored.

### A snapshot has to be taken at the checkpoint

A peer's live database is already past the checkpoint, so it cannot export the
state *at* the checkpoint later. The exporter must freeze the rows when it
captures the scheduled checkpoint, which already walks every row to compute the
digest. It stores them, full fidelity, as one snapshot per scheduled
checkpoint, and keeps the latest two (the same two the retention floor keeps).

### Protocol

1. A member below a peer's retention floor asks for the snapshot of the latest
   scheduled checkpoint it has a verified quorum for.
2. It downloads the snapshot in bounded pages (the existing catch-up page
   limits) from any peer. One honest peer suffices; the quorum signature is the
   trust anchor.
3. It installs the rows into empty federation tables in one transaction,
   recomputes the digest, and commits only if it equals the signed digest and
   every self-verifying field checks out.
4. It promotes the checkpoint as its trusted frontier and catches up the
   journal suffix with the existing protocol.

### Not transferable

Member-local secrets (DKG and FROST key shares, receive secrets, the Bark
wallet) are not federation state. A member that lost them must restore them
from its own backup; state transfer does not replace that. Signature shares
are not in the digest; each can be verified against its member's verification
key, so they may be transferred too, but they are optional for consensus.

### Size of the work

Full-fidelity rows and an installer for each table in the digest (about
fifteen tables across the mint, the consensus projections and Bark), the
snapshot store and its retention, the paged route and client, and tests that a
transferred member reaches the same digest and serves restore. This is the
largest piece of the retention work.

## State transfer: built

### One source, one row builder

`get_federation_materialized_state_source` (`FederationApplicationDatabase` and
`FederationApplicationTransaction`) reads every stored row the digest covers
through one connection or one transaction: the key registry and the rows it
names (quotes, completed operations, proofs, signed blind signatures and every
member's shares for them), all payment observations, the FROST configs and the
Bark projection rows. `cdk/src/mint/federation/state_transfer.rs` turns it, plus
the key-value projections, into sorted `FederationStateSnapshotRow`s (digest
row plus optional attachment). The live digest, the snapshot frozen at capture
and the installer's check inside its transaction all use this builder.

### Snapshot storage

Scheduled capture (`checkpoint_manager.rs`) already built the rows to get the
digest. It now stores them in the same transaction as the candidate:
`federation_state_snapshots` (checkpoint id, sequence, row count, digest) and
`federation_state_snapshot_rows` (one JSON row per index). The store checks
that the rows hash to the checkpoint digest, keeps the latest
`FEDERATION_STATE_SNAPSHOT_RETAINED` (two) scheduled snapshots and deletes older
ones in that transaction. A recovered capture of the same checkpoint is a no-op.

### Paged transfer

- Route: `POST /federation/v1/state-transfer`, a signed peer request with the
  new REST kind `StateTransfer` and the catch-up request byte limit. The
  response is not signed: the rows are trusted only once they hash to the
  quorum-signed digest, so any peer holding the snapshot can serve it.
- Pages: `FederationStateTransferRequest { checkpoint_id, next_row_index,
  limit }`, default 500 rows, at most 5,000, and never above the 8 MiB
  catch-up page limit (the server halves the page until it fits).
- Client: `FederationRestPeerClient::fetch_state_transfer_page` checks the
  version, federation, checkpoint, cursor, row limit, progress and row order.
  `download_state_snapshot_from_member` pages through a snapshot, requires a
  stable row count and caps the whole download at
  `FEDERATION_MAX_STATE_TRANSFER_BYTES` (1 GiB), because the installer holds the
  snapshot in memory. `transfer_state_from_member` downloads and installs.

### Installer

`Mint::install_federation_state_transfer(quorum, rows)` (`state_install.rs`):

1. Verifies the quorum against the member's config, requires a scheduled
   checkpoint, requires strictly sorted rows and checks that they hash to the
   signed digest before anything is written.
2. Decodes every row into what installs it, refusing unknown tables, rows
   that do not decode, payment-observation attachments that do not re-encode
   to their row, and state it cannot install (below).
3. In one transaction: requires empty federation state (no registry keys,
   observations, projection or Bark rows), then writes the FROST configs
   (identical configs already held as key material are no-ops), the
   projection rows, the Bark rows (inserted by the Bark store's own inserts),
   completed operations, payment observations, quotes (payments rebuilt from
   the observations, issuance from the row), spent proofs grouped by their
   operation, the member's own signature shares recomputed with
   `MemberConfigPrivate::sign_transferred_blinded_messages` (keyset rotations
   from the snapshot applied first), the blind signatures and the registry.
4. Recomputes the digest from what it wrote, inside the transaction, and
   commits only if it equals the signed digest; otherwise it rolls back and
   the member stays empty.
5. In the same transaction, `install_federation_state_transfer_frontier`
   requires a genesis frontier and no journal or finalized-item rows, stores
   the quorum and sets the checkpoint as journal, finalized-item and
   application frontier, trusted checkpoint, audited checkpoint and
   state-transfer base, and sets the active AlephBFT session.

After the install the caller restarts the runtime. Startup finds a trusted
checkpoint at the exact frontier and needs no rows below it. Catch-up then
fetches the suffix with the existing protocol.

Two readers learned about the base:

- The SQL checkpoint digest lookups (`federation_*_digest_at`) answer at the
  base from its signed cursors, since no local row lies below it.
- `require_prior_applied_quote_operation` accepts a quote with no journal row
  if a base exists at or below the operation and the quote row is installed.

### The validity window

Duplicate and conflict checks at finalization read durable journal rows by
operation id and conflict key (`operation_pipeline.rs`, then
`register_historical_journal_entry`). A member holding only the checkpoint's
state would append an operation its peers treat as a duplicate and fork its
journal. So the transfer also carries the validity window before the
checkpoint, and the installer stores it as history:

- **Bounds.** The window is every finalized item and journal entry ordered in
  session `checkpoint session - FEDERATION_OPERATION_VALIDITY_SESSIONS` or
  later, plus the one finalized item before them
  (`federation_state_transfer_window_start` in SQL). It comes with the
  finalized-item and application digests just before it.
- **Transfer.** `POST /federation/v1/state-transfer/window` (same REST kind)
  returns items and entries from two cursors, row-limited and under the page
  byte limit; `download_state_transfer_window_from_member` pages it and
  requires stable bounds. It is read from the exporter's journal at serve
  time: entries below a checkpoint are terminal and never change.
- **Verification** (`FederationStateTransferWindow::verify`, before anything
  is written):
  - the items are contiguous to the checkpoint and chain from their base to
    its signed finalized-item digest;
  - the window starts at item 0, or its first item is older than the window,
    so a shortened window is refused; the item sessions are bound by that
    signed chain, the entries' own evidence is not trusted;
  - the entries are exactly the accepted operations among the items, in
    order, with matching consensus evidence;
  - the entries chain to the signed journal order digest
    (`InMemoryFederationJournal::from_base_and_entries`, which also checks
    operation ids and conflict keys against the envelopes);
  - their terminal application states chain from the base to the signed
    application digest.
- **Install.** In the install transaction,
  `install_federation_state_transfer_frontier` stores the items with their
  cumulative digests, the entries with their application digests and
  statuses, and the conflict keys the entries reserve, and records the
  bounds. Checkpoint digest lookups answer at the window's start from the
  recorded bases, so a transferred member can serve the same window onward.

### Tests

- `state_transfer::wiped_member_installs_checkpoint_state_then_catches_up_and_restores`
  (`cdk-axum`): two members apply six operations (two quotes, payments, a mint,
  a swap); the scheduled checkpoint stores its snapshot; a member with only its
  key material downloads it over the real route in several pages, is refused a
  tampered row and a tampered attachment without writing anything, installs,
  reaches the signed digest with identical rows, restarts from the checkpoint,
  catches up a suffix that mints a quote created below the checkpoint, reaches
  the peers' digest, and serves NUT-09 restore and check-state on both sides
  of the checkpoint. Its recomputed shares combine with a peer's share into the
  same signatures as any other pair. A tampered window (an altered application
  result) and a truncated one (base digests recomputed so every chain closes)
  are refused with nothing written. After the install all three members order
  a unit that repeats a swap from the window and adds a quote conflicting with
  one created in the window: all three record the swap as a duplicate and
  consensus-reject the quote, and their journal, application, finalized-item
  and materialized digests stay equal. Without the window rows the installed
  member appends the swap as a new entry and rejects the quote differently;
  the test fails.
- `state_transfer::scheduled_capture_keeps_the_latest_two_snapshots`.
- Page validation and digest order unit tests in `cdk-common`.

### Left out, and what must follow

1. **Duplicates and conflicts older than the validity window (resolved by
   window-bounded detection above).** The window
   (above) makes the installed member decide like its peers for every
   operation ordered in the last `FEDERATION_OPERATION_VALIDITY_SESSIONS`
   sessions before the checkpoint. Peers still keep their whole journal and
   conflict keys, so an operation id or conflict key older than the window is
   still a duplicate or a conflict to them but not to the installed member.
   Reaching one needs an author to re-stamp an operation older than the window
   or a member to reuse an old quote id or observation key. The fix belongs
   with deletion: make duplicate and conflict lookups ignore entries older
   than the window on every member, so the per-kind guards in "Replay guard
   per operation kind" decide alike everywhere.
2. **In-flight melts.** A snapshot with a pending melt quote or a pending proof
   is refused; the member retries from a later checkpoint. Installing one needs
   the melt request, the pending blinded messages and the saga.
3. **Unsigned blinded messages** of finished melts (rows without a signature)
   are not in the digest and are not installed.
4. **Per-keyset fee totals** (`keyset_amounts.fee_collected`) start from zero.
5. **Bark lookups below the base.** The Bark projection is installed, but the
   receive-intent lookup and exit-publication validation read journal entries
   by operation id, so a Bark action that spans the checkpoint cannot advance
   on the installed member. Member-local Bark wallet state is out of scope.
6. **No automatic trigger.** Nothing calls the transfer in production yet:
   peers still keep full history, so a lagging member replays. The trigger
   (a peer's floor above the member's frontier) belongs with deletion
   (prerequisite 6).
7. **Cost.** Every scheduled capture now writes the whole state as snapshot
   rows (it already read all of it for the digest), and the snapshot is held
   in memory during capture and install.

## Not retained-history work

Signature shares, address allocations and the mint's proof and blind-signature
tables stay. They are needed for restore and deposit attribution regardless of
checkpoints.
