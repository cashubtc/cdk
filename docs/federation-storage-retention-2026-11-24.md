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

1. **Replay guard without journal bodies (decision A).** Either:
   - *permanent compact index*: keep operation ids and conflict keys forever in a
     narrow table (about 40 bytes per operation), or
   - *validity window*: envelopes carry the AlephBFT session they were created
     in, admission rejects envelopes older than a fixed number of sessions, and
     ids and keys older than the window are deleted. This bounds the guard but
     changes the envelope format (cut-through) and makes slow clients resubmit.
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

## Not retained-history work

Signature shares, address allocations and the mint's proof and blind-signature
tables stay. They are needed for restore and deposit attribution regardless of
checkpoints.
