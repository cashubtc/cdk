# Federation storage contract and checkpoint retention

Status: design, not implemented. Follows package 6 of
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
4. **State transfer (decision B).** A member below a peer's floor downloads the
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

## Not retained-history work

Signature shares, address allocations and the mint's proof and blind-signature
tables stay. They are needed for restore and deposit attribution regardless of
checkpoints.
