# Federation storage contract and checkpoint retention

Status: prerequisites 1 to 6 implemented (validity window, spent-output
tombstones, Bark readers, state transfer, terminal indexes with the digest
exclusion rule, deletion); duplicate and conflict detection bounded by the
window; deletion of journal entries, conflict keys, finalized items and old
scheduled checkpoints (see "History deletion: built") and of terminal
entities (see "Terminal entities: built"). Follows package 6 of
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
| `federation_payment_observations` | Member payment evidence | Consensus order | Apply (quorum finalization), reports, digest | Retired when the quote is final (issued Bolt11 mint quote, paid melt quote); deleted below the floor |
| `federation_signature_shares` | Member shares of blind signatures | Consensus order | NUT-09 restore, retried mint and swap responses | Kept: restore must work for any past output |
| `federation_materialized_state_keys` | Index of rows in the materialized digest | Consensus order | Snapshot, keyset rotation rebuild | Follows the entities it indexes |
| Bark actions, vtxos, transitions, observations | Bark projection | Consensus order | Bark drivers, digest | Transitions and observations of retired actions deleted below the floor; the action stays as the replay guard; VTXOs, allocations and VTXO reservations kept |
| `transaction` rows | Intents, accepted transactions, lineage heads, reservations | Consensus order | Apply, signing authorization, broadcast, digest | A lineage whose transaction confirmed and whose melt quotes are all paid is deleted whole below the floor, with its intents |
| `chain_observation` rows | Wallet UTXOs and evidence | Consensus order | Apply, transaction policy, quote payment, digest | Spent UTXOs reduced to a permanent tombstone below the floor |
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
member has promoted checkpoint *n*. Every terminal entity records the
operation index at which it became terminal, and operations stop reading it
at that index, so a member that deleted it decides every later operation as
one that did not (see "Terminal entities: built").

### Prerequisites, in order

1. **Replay guard without journal bodies: validity window (decided, implemented).**
   See "Validity window" below.
2. **Outpoint tombstones for spent UTXOs (implemented)**, kept forever, so a
   replayed deposit observation cannot recreate a pruned UTXO.
3. **Bark readers off the full journal.** Reconcile, the recovery driver and the
   operator route read the Bark projection instead.
4. **State transfer (decision B, implemented; see "State transfer: built").**
   A member below a peer's floor downloads the
   checkpoint's materialized state as paged rows, verifies them against the
   signed `materialized_state_digest`, installs them, and replays the suffix.
   This covers the mint database rows the digest already enumerates (quotes,
   proof states, blind signatures, completed operations, keyset rotations). It
   is a new protocol path with its own limits and tests.
5. **Terminal-index fields and the digest exclusion rule (implemented)** (see
   above).
6. **Deletion (implemented).** Journal and finalized bodies below the floor,
   then terminal entities, payment observations and Bark history, each in
   bounded batches in its own transaction.

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
| `TransactionSigned`, `TransactionBroadcastIntent` | Any member | `Existing` for a known transaction; rejected once the lineage is terminal, on every member alike |

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
observation rows; once those rows of a retired action may be deleted, the
action row itself is the guard (see "Terminal entities: built").

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

## History deletion: built

### Floor

`prune_federation_history` (SQL, in one transaction per batch) first advances
the retention floor, stored as `FederationRetentionFloor` in
`federation_checkpoint_state.retention_floor_json`:

- Take the two latest scheduled checkpoints with a stored quorum that the
  trusted checkpoint covers. With fewer than two there is no floor.
- The floor is the earlier one's journal and finalized-item positions, or the
  start of the latest one's validity window
  (`federation_state_transfer_window_start`, including the one older item that
  proves the window complete) if that is earlier.
- It is lowered to the earliest journal entry live state still reads by
  operation id: a melt saga's operation, an unfinished Bark action's origin,
  and (passed in by `Mint::federation_transaction_retention_pin`) an outgoing
  transaction that is not confirmed, abandoned or replaced.
- It never moves back. The digests just before it (order, finalized-item,
  application) are recorded while those rows still exist, and the
  `federation_*_digest_at` lookups fall back to them.

A state-transfer install records its window start as the installed member's
floor.

Without the pins, the floor is a function of the scheduled checkpoint sequence
and the finalized items, so members that promoted the same checkpoints agree.
The pins only hold a floor lower; they never let a member serve less.

### Deletion

After a promotion (`checkpoint_manager.rs`, `checkpoint_catch_up.rs`),
`FederationNodeRuntime::prune_history_below_retention_floor` runs up to 64
batches of `FEDERATION_RETENTION_PRUNE_BATCH_ROWS` (500), each in its own
transaction: journal entries with their conflict keys, finalized items, and
scheduled checkpoints (with signatures and quorums) older than the floor
checkpoint, except the trusted and audited ones. Later promotions continue
where it stopped. A failure is logged and retried at the next promotion.

### Readers

- **Catch-up serving.** A request whose journal or finalized-item cursor is
  below the floor gets `FederationCheckpointError::BelowRetentionFloor`, sent
  as HTTP 410. The peer client turns 410 into `PeerHistoryPruned`.
- **State-transfer window serving.** A checkpoint whose window reaches below
  the floor gets the same answer; the latest scheduled checkpoint's window is
  always kept.
- **Startup suffix verification** starts at the trusted checkpoint, which is at
  or above the floor, and registers only entries inside each item's window.
- **Point lookups at apply.** `require_prior_applied_quote_operation` accepts a
  stored quote whose creating entry lies below the floor, as it already did
  below a state-transfer base. Bark plan lookups, transaction signing and melt
  change provenance read entries of live entities, which the pins keep.
- **Bark readers** read from their replay marker, so their reads stay bounded.
  A marker below the floor (a new Bark scope, or a runtime whose replay kept
  failing for a whole checkpoint interval) reads from the floor and skips the
  deleted entries. Rebuilding such a Bark wallet from the Bark projection
  instead of the journal is not done.

### Automatic state transfer

When every peer attempted in a sync round answers a catch-up with
`PeerHistoryPruned`, the round calls `install_state_transfer_below_peer_floors`:
it fetches a peer's latest scheduled quorum, downloads the snapshot and window,
installs them, and restarts the consensus runtime at that checkpoint
(`adopt_installed_state_transfer`). The next rounds catch up the suffix with the
ordinary protocol. Because a member with no stored next scheduled checkpoint
never sent a catch-up request, `sync_once_from_member` first probes the peer
toward the latest known scheduled quorum
(`probe_peer_history_to_known_checkpoint`) so the pruned answer is seen.

The installer requires empty federation state. A member that holds state below
every peer's floor halts instead (decision 9, see "Stale member: halt and
reset").

### Tests (`cdk-axum`, `federation::tests::retention`)

- `pruning_keeps_the_floor_checkpoint_and_the_validity_window`: the floor equals
  the previous scheduled checkpoint and is at or below the latest window start,
  on both members; exactly the rows from the floor up remain; older scheduled
  checkpoints and quorums are gone, the floor and latest are kept; pruning is
  idempotent; catch-up below the floor gets 410 and from the floor returns
  everything; the latest window serves and installs, an older one is refused;
  a later checkpoint captured on pruned history keeps members' digests equal.
- `unfinished_melt_saga_pins_the_retention_floor`: a pending melt saga holds the
  floor at its entry until it is gone.
- `member_below_every_peer_floor_installs_a_transfer_then_catches_up`: two peers
  prune; an empty third member gets `PeerHistoryPruned` from both, installs a
  transfer in one sync round and catches up a later suffix to the peers' tip
  and digests.

### Not deleted yet

Payment observations of terminal quotes and terminal entities are deleted by
stage 3 ("Terminal entities: built"). The transaction pin now has a test.
Apply-time journal reads of final entities are covered there too: a Bark
action pins its entries until it is retired, and nothing reads a retired one.

## Terminal entities: built

### One rule: terminal means retired

An entity is terminal from the operation that made it final. From that
operation on, no operation reads it: the projections retire it when they load
(`FederationStateTable::retire_terminal_rows`, applied by
`FederationStoredState::load` and `read_federation_state`), live payment
observation reads skip retired rows, and an operation naming a retired Bark
action is applied as a no-op. Terminality is decided by consensus state (quote
state, projection rows), so every member retires at the same journal index.
The stored rows stay until history retention deletes them, and a member that
deleted them and one that did not load the same projections and decide every
later operation alike.

| Entity | Terminal when | Terminal index | What later operations see | Deleted below the floor | Permanent guard |
|---|---|---|---|---|---|
| Payment observation | Its quote is final: a Bolt11 mint quote issued, a melt quote paid | The mint or payment observation that made the quote final | Nothing: live reads skip retired rows, and an observation of a final quote is not recorded | Yes | Quote state |
| Spent wallet UTXO | Spent: a Bark recovery publication, or a CDK lineage becoming terminal | The spending operation | Its tombstone: outpoint, and quote and output when it counted toward the quote's payment | Full record replaced by the tombstone | Tombstone, never deleted |
| Outgoing transaction lineage (transactions, batch head, input reservations) and its intents | One transaction confirmed, every melt quote it pays is paid (`settle_completed_intent`) and its inputs are still reserved (`retire_settled_lineage`) | The payment observation that paid the last quote | Nothing; its inputs are spent at that index | Yes, whole | Spent-output tombstones and melt quote state |
| Bark recovery reservation | Published | The `ExitPublished` operation | Nothing | Yes | Spent-output tombstone |
| Bark action | A final phase and its quote settled (mint quote paid, melt quote paid or rolled back), or no quote | The later of the final transition and the settlement (`retired_operation_index`) | Any operation naming it is an applied no-op | Its transitions and observations | The action row |

Never deleted: signature shares, address allocations, proofs, blind
signatures, quotes and completed operations.

### Digest exclusion rule

`get_federation_materialized_state_floor(checkpoint)` is the retention floor
the checkpoint sets as the latest scheduled checkpoint, before live-state
pins: the latest scheduled checkpoint below it, or the start of its validity
window if that is earlier; zero when no scheduled checkpoint lies below it. It
depends only on the checkpoint sequence and finalized items. The checkpoint's
materialized-state digest leaves out:

- payment observations retired below the floor;
- spent UTXOs spent below the floor, which appear as their tombstone row
  (`retire_rows_below`);
- transaction lineages terminal below the floor;
- recovery reservations published below the floor;
- transitions and observations of Bark actions retired below the floor.

Scheduled and recovery capture compute the floor of the checkpoint being
captured; the audit uses the floor of the trusted checkpoint. The live digest
(`federation_materialized_state_snapshot`, operator status) uses the floor of
the latest scheduled checkpoint. A member deletes only below its stored
retention floor, which is never above the floor of any checkpoint it captures
later, so deleted rows are always rows the digest leaves out. Payment
observation digest rows carry `terminal_operation_index`; the action record
carries `quote_settled_operation_index`; wallet UTXOs carry
`spent_operation_index`; intents carry `settled_operation_index`;
transactions carry `terminal_operation_index`; recovery reservations carry
`published_operation_index`. All are consensus-fixed.

### State transfer

The frozen snapshot holds the rows at the checkpoint's floor, so an installed
member holds no entity retired below it, and tombstones arrive as ordinary
projection rows. The installer installs retired payment observations with
their terminal index, verifies the recomputed digest with no exclusion (there
is nothing left to exclude), and rebuilds a mint quote's payments from the
installed observations; when a final quote's observations were left out, the
signed paid amount is recorded as one payment.

### Deletion

`Mint::prune_federation_terminal_state(floor, limit)` runs after history
pruning in `prune_history_below_retention_floor`, below the stored retention
floor, up to 64 batches of 500 per promotion: each projection in its own
transaction (oldest terminal entities first, a lineage always whole), then
retired payment observations and the transitions and observations of retired
Bark actions in one SQL transaction.

### Spent outputs and quote payments

An on-chain mint quote's payment is cumulative over every output ever paid to
its address, spent or not, and payments are evaluated at the applying entry's
index. A spent output never receives evidence again, so its contribution is
fixed when it is spent; the tombstone keeps exactly that contribution (quote,
amount, script), and `quote_payment_at_or_before` merges tombstones with live
outputs in the same outpoint order. The amount and the payment commitment are
the same whether the full record exists or not. This is why the tombstone is
not the bare outpoint.

### Replay guards that survive deletion

- **Spent outputs.** A deposit or reorg observation of a spent outpoint
  returns `Existing(Spent)`; reservation and spending refuse it with the same
  error as for a spent record.
- **Payment observations.** An observation of a final quote is not recorded,
  so replaying a deleted one changes nothing on any member.
- **Transaction lineages.** A terminal lineage is unknown to every member:
  signing, broadcast intents and observations for it are rejected alike; a
  replayed proposal finds its inputs spent. A melt cannot add a new intent for
  a paid quote (`persist_pending_onchain_intent` requires a pending quote).
- **Bark.** `apply_bark_operation` (SQL) and `federation_bark_operation_recorded`
  treat an operation naming a retired action as already applied, before any
  read of its transitions or observations.

### Apply-time journal reads

- Bark: payment templates for observations of a finished action read the plan
  entry. The live-state pin now holds every action that is not retired (it
  held only non-final ones), and nothing reads a retired action.
- Outgoing transactions: signing reads the accepted proposal entry; the
  transaction pin holds unfinished transactions, and a terminal lineage is
  never read.
- Melt change provenance reads the melt entry of a live saga (saga pin).
- `require_prior_applied_quote_operation` already accepts a quote created
  below the floor (stage 2).

### Behaviour changes

- A spent output is frozen: later deposit or reorg evidence for it is not
  recorded (before, it could mark the output reorged or quarantined and set
  the liability alarm).
- A CDK on-chain lineage's inputs stay reserved after confirmation until every
  quote it pays is paid; then they are spent.
- Observations of a final quote are not recorded (before, late observations
  were stored for audit). `get_federation_payment_observation_history` returns
  every stored observation, retired ones included.
- Operations naming a retired Bark action are no-ops, so late observations of
  it are ignored.
- A paid melt quote whose saga is still finalizing after a crash completes from
  the saga on any later observation, without a new quorum.

### Not pruned, and why

- Lineages that never confirm, whose melt quotes are never paid, or one of
  whose inputs left its reservation (a reorg deeper than the confirmation),
  and their inputs: something may still read them.
- Unspent, reorged and quarantined outputs: they can still change.
- Payment observations of Bolt12 and on-chain mint quotes (they can be paid
  again) and of melt quotes that are not paid.
- Bark actions bound to a quote that never settles (for example a receive
  whose quote is never paid). They also keep pinning their journal entries.
- Bark VTXOs, derivation allocations and VTXO reservations: not terminal
  entities in this design.

### Tests

- `cdk-axum`, `federation::tests::retention::retired_payment_observations_digest_alike_whether_or_not_deleted`:
  three members; a Bolt11 quote is created, paid and minted; one member keeps
  terminal entities (`keep_terminal_entities`), two delete them. After the
  floor passes the mint, the stored rows differ but every scheduled checkpoint
  is identical on all three, the live digest equals the latest checkpoint's,
  and each member passes the audit of its trusted checkpoint. A replay of the
  deleted observation, re-stamped by its author after its window, is applied
  as a no-op everywhere. The snapshot served by a member that deleted the rows
  equals the one served by the member that kept them, installs into a fresh
  member and reaches the signed digest.
- `cdk`, `melt::accepted_frost_onchain_melt_materializes_one_pending_payment_intent`
  (extended): paying the quote retires the lineage and spends its input; the
  digest at a floor above the settlement is identical before and after
  deletion; deletion leaves the tombstone and no transaction rows; a replayed
  payment observation changes nothing.
- `cdk`, `transaction_signing::unfinished_outgoing_transaction_pins_retained_history`.
- `cdk-common`: `spent_output_tombstone_decides_alike_whether_or_not_the_record_was_deleted`
  (replayed deposit and reorg, reservation, spending and quote payment on a
  kept and a deleted record), `settled_lineage_retires_whole_and_reads_alike_after_deletion`.
- `cdk-federation-bark`: `recovery_reservations_are_conflict_safe_and_replay_safe`
  (extended with retirement).
- Storage conformance (`cdk-sqlite`, `cdk-postgres`),
  `federation_bark_retired_action_guard`: settling a quote retires the
  finished action; a late observation is a no-op; only its rows below the
  floor are deleted, the action stays; a replay of a deleted observation does
  not recreate it.

## Stale member: halt and reset

Decision 9: a member that still holds federation state but lies below every
peer's retention floor halts with a clear status, and only an explicit
operator action deletes anything.

- **Halt.** When every peer answers a sync round with `PeerHistoryPruned` and
  the local frontier is not genesis, `install_state_transfer_below_peer_floors`
  stops the AlephBFT live session and sets the typed sync status
  `FederationSyncStatus::ResetRequired { journal_next_index }`. Operator status
  shows `sync_state = "reset_required"`, the member is not ready (public
  requests get `NodeHalted` with the instruction), and later sync rounds return
  at once. No other status transition (healthy, lagging, catching up, a
  generic halt or startup revalidation) clears it; restarting without a reset
  halts again at the first sync round.
- **Reset.** `cdk-mintd federation reset-member --confirm-wipe`, run while the
  daemon is stopped, opens the configured database and, in one transaction
  (`reset_federation_member_state`), deletes the federation consensus state
  (journal, conflict keys, finalized items, AlephBFT backups, pending
  operations and session, checkpoints with signatures, quorums, snapshots and
  frontier, so also the retention floor and state-transfer base), the
  application state (key registry, payment observations, signature shares,
  consensus projections, Bark projection, payment schedule and Bark replay
  markers) and the mint business rows (quotes and their payments and
  issuances, melt requests, proofs, blind signatures, sagas, completed
  operations, per-keyset totals). It keeps the federation config snapshot,
  DKG and setup records, FROST key material, the mint's keysets and
  configuration, and does not touch the FROST nonce redb file. Without
  `--confirm-wipe` it refuses before opening anything.
- **Rejoin.** The next start finds a genesis frontier and empty federation
  state, so the ordinary empty-member path installs a peer's latest scheduled
  checkpoint state and catches up the suffix.
- **Tests.** `cdk-axum`,
  `retention::member_holding_state_below_every_peer_floor_halts_then_rejoins_after_reset`:
  a member that took part in the first checkpoint goes offline while its
  peers prune past it; two sync rounds leave it in `ResetRequired` with its
  frontier unchanged, and marking it healthy does not clear it; after the
  reset (key material and config kept, frontier genesis, journal empty) it
  restarts over the same database, installs the peers' latest checkpoint,
  catches up a later suffix and matches their digests. `cdk-mintd`:
  `reset_member_refuses_without_confirmation`,
  `reset_keeps_key_material_and_leaves_an_empty_genesis_member`.

## Not retained-history work

Signature shares, address allocations and the mint's proof and blind-signature
tables stay. They are needed for restore and deposit attribution regardless of
checkpoints.
