# Federated CDK signed-checkpoint implementation plan

Status: proposed  
Scope: federation-only, cut-through replacement of catch-up certificates  
Primary goals: remove history-proportional steady-state work and make restart
recovery proportional to the uncertified tail rather than the federation's
complete lifetime history

Compatibility policy: no backward compatibility is required for existing
experimental federations. All wire, configuration, database, status, and
runtime changes are cut-through. Database schema changes are allowed, but they
must be squashed into the existing initial federation schema migrations. Do not
add an upgrade path from the certificate schema, compatibility tables, dual
reads/writes, protocol negotiation, legacy aliases, or mixed-version federation
behavior. Existing federation databases and generated configurations are
discarded and recreated for the checkpoint protocol.

## Executive review

The current catch-up-certificate design preserves an important safety property:
a member must not accept journal history supplied by one peer or reopen
authoritative routes after restart unless a federation quorum attests to the
same order and materialized state. The problem is not that this evidence
exists. The problem is its range-oriented representation and the way the node
reconstructs the safety conclusion.

Every member currently accumulates member signatures over journal ranges. To
determine the certified height, `catch_up_certificate_quorum_status` loads the
complete journal and every stored certificate, groups certificates by exact
range contents, reconstructs those ranges, recomputes digest chains, verifies
all signatures, and then searches for a contiguous quorum-certified prefix
starting at index zero. The peer-sync loop consults that result on its
five-second cadence, certificate collection consults it repeatedly inside one
round, and the local status/health path consults it again. Before caching, the
same unchanged historical fact was therefore re-proven repeatedly.

This work grows with federation age rather than with new activity. It is
multiplied by every member, status polling, and peer evidence collection. It
also makes restart expensive because startup loads the full journal, all
retained AlephBFT finalized items, and all stored certificates, reconstructs
the consensus runtime from genesis, and verifies the certificate chain before
readiness can open.

The replacement should use one cumulative quorum-signed checkpoint proof with
two creation policies:

- **scheduled checkpoints** at deterministic, consensus-configured history
  intervals, retained as reusable certificates for historical batches; and
- **recovery checkpoints** created on demand at an exact converged stable tip
  to authenticate the finalized but not-yet-scheduled tail.

Each checkpoint commits to the cumulative accepted-operation order, the
cumulative finalized-item stream, the materialized application state, and the
recovery cursors at a stable AlephBFT unit boundary. Members derive and sign a
checkpoint once, persist its individual signatures and assembled quorum, and
serve that stored proof without rescanning history or signing it again. A node
verifies a checkpoint quorum once, persists that trusted recovery frontier,
initializes the runtime at that nonzero frontier, and processes only the suffix
after it.

The target complexity is:

- steady state with no new checkpoint: O(1), with no signatures and no journal
  or certificate scan;
- scheduled checkpoint creation: O(new batch + federation threshold), once per
  deterministic boundary;
- recovery checkpoint creation: O(tail + federation threshold), only when
  recovery needs a stable post-scheduled frontier;
- catch-up: O(number of missing operations/finalized items), not O(total
  history) per poll;
- startup at an exact trusted checkpoint: O(federation threshold + current
  materialized-state validation), independent of historical journal length;
- startup with a tail after the checkpoint: O(federation threshold + tail),
  with the tail bounded by checkpoint policy in the normal case.

Signed checkpoints do not replace AlephBFT and must never become an alternate
ordering path. AlephBFT continues to decide and order operations. A checkpoint
is durable quorum evidence about an already-finalized prefix, used only for
recovery, catch-up authorization, readiness, and operational status.

The distinction between a scheduled checkpoint and a recovery checkpoint is a
creation and retention policy, not a second cryptographic protocol. Both use
the same canonical payload, signature verification, quorum rules, persistence,
and catch-up validation path. The existing range-certificate protocol is
deleted after cutover.

## Symptoms and evidence observed

### Load-test symptoms

The federation receive stress test recorded raw journal throughput around
0.5-0.57 operations per second and sustained wallet-confirmed throughput around
0.27 transactions per second in the overloaded multi-wave cases. Increasing
concurrency did not produce a corresponding service-rate increase. Instead:

- client latency increased substantially;
- wallets sometimes received only one or two of the required three blind
  signature shares even though the operations eventually committed;
- members exposed `lagging`, `catching_up`, and
  `verifying quorum catch-up certificates` responses during ordinary load;
- the load left recovery pressure after the timed interval, with later
  sequential operations remaining slow;
- logs contained repeated catch-up/certificate transitions, range-content
  mismatches against a moving tip, and temporary inability to collect the
  configured catch-up evidence threshold.

The wallet fanout and catch-up-waiting defects are separate issues documented
in `docs/federated-cashu-nocean-stress-test.md`. They explain why temporary mint
unavailability became visible to clients, but they do not explain or justify
the mint CPU cost.

### Regression after the milestone-3 deployment

The milestone-3 mint change stopped healthy live AlephBFT members from treating
normal one-operation propagation skew as a reason to start REST journal
catch-up. That removed one false catch-up trigger, but the subsequent deployment
did not meaningfully improve measured TPS. CPU usage became dramatically
higher, and federation logs became excessively noisy.

The code-level hot path explains the CPU symptom. Every peer-sync round could
revalidate the complete durable certificate history even when the journal and
certificate table were unchanged. The health/status path could repeat the same
work. On a multi-member deployment, each process independently performed this
history-proportional work.

There are additional amplifiers:

- certificate collection loads the entire journal before deciding whether any
  evidence is missing;
- a nonempty private REST `/federation/v1/journal` response computes a current
  materialized-state digest and creates a new signature on demand;
- the requester asks multiple peers for the same exact range to assemble a
  threshold and persists every received certificate;
- a moving head can cause attempted ranges and materialized-state digests to
  change while evidence is being collected;
- each node stores roughly a threshold of signatures per range, so retained
  evidence and verification work grow continuously;
- startup separately reconstructs the full AlephBFT finalized-item and journal
  histories, so fixing only the certificate status cache does not fix restart
  scaling.

A generation-aware cache around the current quorum-status computation is a
valid emergency mitigation because it avoids revalidating unchanged evidence.
It is not the desired end state: it retains range certificates, full-history
startup reconstruction, growing evidence tables, range mismatch behavior, and
the coupling between routine peer polling and recovery evidence.

## How catch-up certificates work today

`FederationCatchUpCertificate` signs one exact response range. Its transcript
binds the federation, signer, first and last indexes, preceding and resulting
order digests, current journal-tip digest, current materialized-state digest,
and every operation ID in that range. A quorum is a set of matching individual
member identity signatures reaching the federation's consensus threshold.

Certificates are currently used in four ways:

1. **REST catch-up authorization.** A lagging member fetches a journal range
   from one peer and then asks additional peers for the same range. It applies
   that response only after a threshold of certificates matches exactly.
2. **Proactive durable coverage.** Every successful background peer-sync round
   attempts to ensure that the local journal suffix has local and remote
   certificates, even when the member learned those operations directly from
   healthy live AlephBFT.
3. **Startup readiness.** A nonempty restarted member remains fail-closed until
   the complete local journal is covered by a contiguous certificate quorum and
   the quorum-certified materialized-state digest matches the local projection.
4. **Status and monitoring.** Local status, the mintd status command, smoke
   tests, and Prometheus expose the highest certificate-covered operation count
   and its signer set.

The certificates are persisted in
`federation_catch_up_certificates`, keyed by range start and signer. They travel
inside authenticated private REST journal responses. They are not sent through
AlephBFT.

The excessive policy is proactive certification of every range on every node
combined with repeated reconstruction of the same contiguous prefix. The
minimum safety requirement is smaller: retain a quorum-authenticated recovery
frontier and authenticate only the suffix that advances that frontier.

## Safety requirements that must not change

The implementation is complete only if all of these invariants continue to
hold:

- AlephBFT remains the only production ordering/finality mechanism.
- A checkpoint can cover only fully finalized consensus items at a complete
  AlephBFT unit boundary.
- A checkpoint can cover only the contiguous operation prefix whose
  post-consensus application states are terminal and durably materialized.
- The signed transcript is bound to the federation and the exact public
  configuration/identity-key epoch.
- A quorum counts distinct configured identity keys and uses the federation's
  consensus threshold, not the wallet blind-signature threshold.
- The accepted-operation order digest is cumulative from genesis.
- A second cumulative digest commits to every finalized AlephBFT item,
  including invalid, duplicate, and non-operation items that do not appear in
  the accepted-operation journal.
- The materialized-state digest is distinct from both ordering commitments and
  remains in the checkpoint.
- A member signs only a checkpoint that exactly matches its own fully applied,
  durable state at that checkpoint boundary.
- Scheduled checkpoint eligibility and boundary selection are deterministic
  consensus configuration. A checkpoint never cuts through an AlephBFT unit.
- Recovery checkpoints cover only an exact, quorum-observed converged stable
  tip and never a moving or caller-selected arbitrary range.
- A stored checkpoint quorum is transferable: any member may serve the bounded
  payload and signature set, and the receiver verifies the configured signers
  without contacting them again.
- Checkpoint validity requires the configured consensus threshold, not every
  member. Every honest member should eventually persist the quorum, but one
  offline or Byzantine member must not block checkpoint formation.
- Missing, mismatched, unknown-signer, duplicate-signer, stale-configuration,
  or bad-signature evidence fails closed.
- Catch-up data from one peer remains untrusted until it reaches a known
  quorum checkpoint. It must not open public authoritative routes early.
- A persisted checkpoint must never point ahead of the journal, finalized-item
  stream, or materialized state committed in the same database.
- A materialized-state mismatch remains latched/fail-closed across restart.
- Starting from a checkpoint must preserve old-operation idempotency and old
  finalized-unit replay rejection without rebuilding unbounded in-memory sets.
- Standalone mint behavior and public Cashu wallet APIs remain unchanged.

## Target checkpoint model

### Canonical checkpoint payload

Introduce a canonical `FederationCheckpoint` with at least these fields:

```text
version
federation_id
federation_config_digest
checkpoint_kind
scheduled_checkpoint_sequence
scheduled_anchor_id
journal_next_index
journal_order_digest
finalized_next_item_index
finalized_item_digest
aleph_bft_session
next_unit_finalization_index
next_unit_index_by_member
materialized_state_digest
```

`journal_next_index` is the operation count and therefore the first operation
index after the checkpoint. It avoids special empty-history arithmetic.
`finalized_next_item_index` is the equivalent cursor for every finalized
AlephBFT item. `next_unit_index_by_member` is common federation state derivable
from finalized units; the restarting member selects its own entry to initialize
its local data provider without replaying all historical units.

`checkpoint_kind` is `Scheduled` or `Recovery`. Scheduled checkpoints increment
`scheduled_checkpoint_sequence` and set `scheduled_anchor_id` to the preceding
scheduled checkpoint. Recovery checkpoints retain the current scheduled
sequence and anchor to the latest scheduled checkpoint. This prevents an
on-demand recovery checkpoint from changing the identity of the next scheduled
checkpoint or creating competing checkpoint chains. A locally trusted frontier
may advance only to a checkpoint with non-regressing cursors and matching
cumulative digest continuation, which prevents rollback to an older valid
recovery checkpoint.

The checkpoint ID is a domain-separated hash of the canonical payload. Each
member identity key signs that ID. `FederationCheckpointQuorum` carries the
payload and a bounded map of distinct member signatures. Scheduled and recovery
checkpoints use this same type and verification path.

The exact payload should be finalized in milestone 1. Fields may be removed
only with an explicit proof that neither runtime recovery nor safety validation
needs them. Local-only data such as pending submissions or raw AlephBFT backup
bytes must not be placed in the quorum transcript.

### Checkpoint creation policies

Scheduled checkpoint intervals are consensus configuration. Start with:

- `checkpoint_interval_journal_operations = 10_000`; and
- `checkpoint_interval_finalized_items = 10_000`.

A scheduled checkpoint becomes eligible when either accepted-journal progress
or complete finalized-item progress since the preceding scheduled checkpoint
crosses its configured interval. Using both counters prevents rejected,
duplicate, or non-operation AlephBFT traffic from creating an unbounded runtime
recovery tail when accepted-operation volume is low.

The canonical scheduled boundary is the first complete finalized-unit boundary
after eligibility at which every accepted operation through that unit has a
terminal, durably materialized application state. The actual journal boundary
may therefore be 10,003 rather than exactly 10,000. What is consensus-stable is
the interval policy and deterministic boundary rule, not an unsafe partial-unit
cut. Boundary digests and cursors must be captured before later finalization can
change the materialized state being certified.

At an eligible boundary each member:

- derives and persists the identical checkpoint payload;
- persists one local signature for that checkpoint ID and never re-signs it;
- exchanges stored signatures over authenticated private REST until the
  consensus threshold is reached;
- persists the assembled quorum and makes it available as a transferable proof;
- disseminates the assembled quorum so every reachable honest member can store
  it; and
- performs no checkpoint work while neither interval has crossed a boundary.

After the latest scheduled checkpoint, catch-up may require a recovery
checkpoint for the smaller finalized tail. The requester first discovers an
exact converged stable frontier from quorum peer summaries. A member signs the
recovery checkpoint only when that complete payload equals its own fully
applied durable tip and its scheduled anchor is the member's latest scheduled
quorum. It never signs a caller-provided digest blindly, a lower arbitrary
historical state for which it has no stored state commitment, or a moving tip.

Recovery checkpoints are debounced and deduplicated by checkpoint ID. They are
persisted and reusable until a later scheduled quorum covers their frontier.
They replace, rather than coexist with, the old arbitrary range certificate.

### Persistence and retention

Persist checkpoint payloads, individual signatures, assembled quorum proofs,
and the locally trusted frontier. Retain all scheduled quorum checkpoints by
default. At intervals of 10,000 operations or finalized items, this is orders
of magnitude smaller than per-range certificates and lets a very old member
verify and apply recovery one bounded batch at a time without staging the
entire missing history.

Individual non-quorum proposals may be pruned once they are older than the
latest compatible quorum checkpoint and cannot contribute to its deterministic
checkpoint ID. Recovery quorums may be pruned after a later scheduled quorum
covers their journal and finalized cursors and no local trusted frontier still
depends on them. Scheduled quorum checkpoints must not be pruned until a
separately designed backup-retention policy defines the oldest supported
recovery point.

Every reachable honest member should eventually persist the assembled quorum,
but forming or trusting a checkpoint requires only the configured consensus
threshold. Serving a stored checkpoint performs no history scan, digest
reconstruction, or new signature operation.

A checkpoint is an attestation of a state already stored in the mint database;
it is not a physical database snapshot. Backups must include the mint database
and a matching trusted checkpoint. Restoring an empty member from an arbitrary
checkpoint would require a separate state-snapshot transfer protocol and is not
part of this plan.

### Transport policy

Checkpoint proposals/signatures use authenticated private REST, not AlephBFT.
Add a small checkpoint endpoint or endpoints under `/federation/v1` for:

- latest local scheduled and recovery checkpoint summaries;
- fetching stored checkpoint payloads and assembled quorums after a sequence or
  finalized cursor;
- requesting the local member's signature for an exact locally derivable
  checkpoint; and
- publishing or fetching an assembled quorum so other honest members can
  persist the transferable proof.

Journal catch-up remains paginated REST and continues to carry the finalized
items needed for exact runtime reconstruction. It stops carrying
`catch_up_certificate`. Peers advertise scalar checkpoint IDs/cursors in
status; they do not embed complete signature sets in every status response.

### Startup and catch-up flow

1. Load the newest locally persisted scheduled or recovery checkpoint quorum
   compatible with the configured federation.
2. Verify its bounded signature set once and compare its journal/finalized
   cursors and digests with scalar database metadata.
3. Initialize the journal and AlephBFT runtime from the checkpoint's nonzero
   base digests and cursors.
4. Load and validate only local journal/finalized-item rows after the checkpoint.
5. Recompute the current materialized-state digest once before readiness. This
   preserves restart mismatch detection while avoiding historical certificate
   and finalized-item replay.
6. Ask peers for their newest stored scheduled checkpoint quorums, recovery
   checkpoint quorum, and scalar consensus tips.
7. If local state is behind, fetch each missing scheduled-checkpoint-bounded
   batch from one selected healthy source, validate both rolling digest chains
   against the already stored quorum proof, apply the batch, and persist the
   advanced trusted checkpoint transactionally. The serving member does not
   recalculate a signature and the requester does not contact historical
   signers again.
8. If the local durable tail already matches a newer quorum checkpoint, promote
   it without reapplying it.
9. Continue until the latest scheduled quorum checkpoint. For the small
   post-scheduled tail, use the live AlephBFT session or discover a converged
   stable target and form/fetch one recovery checkpoint quorum using the same
   checkpoint proof type. Download the tail from one source and promote it only
   after the recovery quorum verifies.
10. Open authoritative routes only when local order, finalized cursor,
    materialized-state digest, peer tip, and live-session readiness satisfy the
    existing fail-closed convergence rules.

The unavoidable startup work is verifying one quorum and validating current
materialized state. Historical journal length must not determine startup time
when the database is exactly at a trusted checkpoint.

## Milestone dependency map

| Milestone | Result |
| --- | --- |
| 0 | Reproducible CPU/startup baseline and work counters |
| 1 | Canonical checkpoint, finalized-item digest, and quorum verification |
| 2 | SQLite/Postgres checkpoint persistence and atomic promotion |
| 3 | Event-driven scheduled/recovery checkpoint production and reusable quorum exchange |
| 4 | Scheduled-batch and stable-tail catch-up without range certificates |
| 5 | Nonzero-base runtime recovery and fast startup |
| 6 | Checkpoint-native readiness, status, metrics, and operator commands |
| 7 | Cut-through deletion of the certificate implementation |
| 8 | Adversarial, restart, and performance proof on local and deployed federation |

Milestones 1 and 2 establish the durable security primitive. Milestone 3 may
temporarily coexist with certificates only in tests or an implementation branch.
Production must not accept a mixture. Milestone 7 is a cut-through switch:
members using range certificates and members using either scheduled or recovery
checkpoints are not wire- or database-compatible and must not form one
federation.

No milestone includes a database-upgrade or compatibility phase. Required
schema changes are folded into the existing initial federation migrations.
Development and deployment validation use newly generated federation databases
and configurations only.

## Milestone 0: Freeze correctness and performance baselines

### Objective

Make the excessive work measurable before changing the protocol and preserve
the existing safety regressions as checkpoint requirements.

### Checklist

- [ ] Add counters/timers for full journal rows loaded, finalized items loaded,
      certificate/checkpoint signatures verified, startup replayed entries,
      materialized-state digest duration, and readiness duration.
- [ ] Keep instrumentation bounded and free of operation payloads or secrets.
- [ ] Add synthetic histories at 1,000, 10,000, and at least 100,000 operations
      without requiring public networking.
- [ ] Record idle CPU/work-counter behavior across at least twelve five-second
      peer-sync rounds with unchanged history.
- [ ] Record cold-start time and rows/signatures processed at each history size.
- [ ] Run the existing receive stress benchmark and preserve its raw output
      outside the repository; record only aggregate results.
- [ ] Turn the startup materialized-state mismatch regression, partial evidence
      regression, and restart catch-up regressions into named gates for this plan.

### Files and references

- `docs/federated-cashu-nocean-stress-test.md`
- `scripts/benchmark_nocean_federation.sh`
- `scripts/reproduce_nocean_federation_stress_test.sh`
- `crates/cdk-axum/src/federation.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-axum/src/federation/tests.rs`
- `crates/cdk-axum/src/lib.rs`
- `crates/cdk-prometheus/src/metrics.rs`

### Success criteria

- The baseline demonstrates whether unchanged sync rounds perform historical
  database reads or signature verification.
- Startup measurements distinguish journal reconstruction, finalized-item
  reconstruction, certificate verification, and materialized-state projection.
- All existing fail-closed restart and catch-up tests pass unchanged.

### Test conditions

- Run focused `cdk-axum` federation tests in debug mode.
- Run synthetic startup measurements in release mode with SQLite.
- Run one five-member AlephBFT process test with no faults and one with a member
  offline long enough to require REST catch-up.
- Run the reproducible deployment stress script only after local gates pass.

## Milestone 1: Define and verify the checkpoint protocol

### Objective

Introduce a cumulative checkpoint primitive that can authenticate both the
application journal and the complete AlephBFT finalized-item prefix with a
bounded quorum proof.

### Checklist

- [ ] Add `FederationCheckpoint`, `FederationCheckpointId`,
      `FederationCheckpointSignature`, and `FederationCheckpointQuorum` in a
      dedicated federation checkpoint module.
- [ ] Add one signed `FederationCheckpointKind` discriminator for `Scheduled`
      and `Recovery`; do not create a second range-certificate proof type.
- [ ] Define a versioned, domain-separated canonical signature transcript.
- [ ] Bind the checkpoint to the federation config digest/identity-key epoch.
- [ ] Add `FederationFinalizedItemDigest` and define canonical hashing for every
      finalized item, including status and consensus/unit provenance.
- [ ] Persist or derive the rolling finalized-item digest during finalization;
      never recompute it from genesis for normal checkpoint creation.
- [ ] Define deterministic checkpoint eligibility at complete unit and terminal
      application boundaries.
- [ ] Verify distinct known signers, reject duplicates, and require the
      configured consensus threshold.
- [ ] Define scheduled sequence/anchor rules so recovery checkpoints do not
      change the next scheduled checkpoint ID or fork the scheduled chain.
- [ ] Reject an older scheduled sequence, broken scheduled anchor, cursor
      regression, incompatible recovery frontier, config mismatch, unknown
      signer, wrong federation, modified digest, modified cursor, and bad
      signature.
- [ ] Define monotonic trusted-frontier promotion for multiple recovery
      checkpoints anchored to the same scheduled checkpoint.
- [ ] Document explicitly that checkpoint quorum is recovery evidence, not
      consensus finality.

### Files and references

- Add `crates/cdk-common/src/federation/checkpoint.rs`.
- `crates/cdk-common/src/federation.rs`
- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/consensus/tests.rs`
- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-common/src/federation/config.rs`

### Success criteria

- One quorum proof has size O(number of federation members), independent of
  journal length.
- Verifying the newest checkpoint does not require journal entries, historical
  checkpoints, or finalized-item bodies.
- One peer holding an assembled quorum can transfer the proof without any
  signer recomputing a signature or the receiver contacting those signers.
- Any mutation to a committed field invalidates the checkpoint signature.
- Two honest members at the same durable unit/application boundary derive the
  same checkpoint ID.
- An on-demand recovery checkpoint cannot change the ID that honest members
  derive for the next scheduled checkpoint.

### Test conditions

- Unit-test canonical encoding and fixed digest vectors.
- Property-test or table-test every committed field for mutation rejection.
- Test 1-of-1, threshold-of-N, duplicate signer, unknown signer, and insufficient
  quorum cases.
- Test accepted, rejected-invalid, duplicate, and non-operation finalized items
  in the finalized digest.
- Test session rotation and a checkpoint exactly on the first/last unit boundary.
- Test scheduled checkpoints with multiple recovery checkpoints between them,
  including stale recovery rollback rejection and deterministic next-scheduled
  checkpoint derivation.

## Milestone 2: Add checkpoint persistence and atomic promotion

### Objective

Store sparse checkpoint proposals and quorums without allowing a trusted
checkpoint to move ahead of durable consensus or application state.

### Checklist

- [ ] Replace the fresh federation schema's
      `federation_catch_up_certificates` table with checkpoint payload,
      individual-signature, assembled-quorum, and trusted-frontier tables.
- [ ] Persist checkpoint kind, scheduled sequence, and scheduled anchor without
      duplicating the verification schema for recovery checkpoints.
- [ ] Use a unique key on `(checkpoint_id, signer_member_id)` and indexed lookup
      by checkpoint sequence/finalized cursor.
- [ ] Add database APIs to insert an idempotent local/remote signature, load
      checkpoints after a cursor, store/fetch an assembled quorum, load the
      newest scheduled and recovery quorums, and atomically promote a verified
      quorum.
- [ ] Add scalar metadata queries for journal next index/order digest and
      finalized next index/finalized digest so startup does not load bodies.
- [ ] Persist rolling finalized-item digests with finalized items or in an
      atomically updated frontier record.
- [ ] Ensure catch-up promotion persists the journal/finalized batch and trusted
      checkpoint in one transaction, or leaves the checkpoint behind the data.
- [ ] Define crash-safe retention: keep all scheduled quorum checkpoints; prune
      superseded non-quorum proposals and prune recovery quorums only after a
      scheduled quorum covers them and no trusted frontier depends on them.
- [ ] Preserve the cut-through policy: edit the existing fresh federation
      schema migrations in place; do not add an upgrade migration from the
      certificate schema, compatibility tables, or dual reads/writes.

### Files and references

- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-common/src/database/mint/test/federation.rs`
- `crates/cdk-sql-common/src/mint/federation.rs`
- `crates/cdk-sql-common/src/mint/federation/journal.rs`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260712000000_create_federation_schema.sql`
- `crates/cdk-sql-common/src/mint/migrations/postgres/20260712000000_create_federation_schema.sql`
- `crates/cdk-sqlite/src/mint/mod.rs`
- `crates/cdk-postgres/src/lib.rs`

### Success criteria

- SQLite and Postgres round-trip identical checkpoint/quorum records.
- Re-inserting a signature is idempotent and one signer cannot occupy two rows
  for the same checkpoint ID.
- Re-inserting or transferring an assembled quorum is idempotent and requires
  no signature regeneration.
- No transaction failure can leave a trusted checkpoint ahead of either durable
  journal/application state or the finalized-item cursor.
- Loading the latest trusted checkpoint is an indexed bounded query.

### Test conditions

- Shared database-trait tests for checkpoint insert, assembled-quorum transfer,
  scheduled/recovery lookup, quorum promotion, restart, duplicate insert,
  conflicting payload, retention, and ordered lookup.
- SQLite and Postgres transaction-failure tests at every promotion write step.
- Concurrent signature insert/promotion tests.
- SQLite `EXPLAIN QUERY PLAN` and Postgres schema/index assertions for the new
  hot-path queries.

## Milestone 3: Produce and exchange scheduled and recovery checkpoints

### Objective

Make scheduled checkpoint work event-driven and proportional to newly finalized
batches, make recovery checkpoints on-demand at a converged stable tip, and do
no certificate/checkpoint computation or journal catch-up request on an
unchanged peer-sync tick.

### Checklist

- [ ] Replace `catch_up_certificate_chunk_size` with consensus-configured
      `checkpoint_interval_journal_operations` and
      `checkpoint_interval_finalized_items`; document initial defaults of
      10,000 for both.
- [ ] Add runtime dirty/eligible frontiers advanced only after durable finalized
      unit application and trigger scheduled eligibility when either interval
      crosses its next target.
- [ ] Select the first terminal, fully materialized, complete finalized-unit
      boundary after eligibility and capture its exact boundary commitments
      before later finalization advances state.
- [ ] Derive, persist, and sign each scheduled local checkpoint exactly once.
- [ ] Add authenticated private REST request/response types and message kinds for
      checkpoint summary, fetch, and signature request.
- [ ] Add checkpoint routes under `/federation/v1` and matching peer-client
      methods with existing request admission, authentication, replay-window,
      timeout, and backoff rules.
- [ ] Have peers return an already stored local signature when available. A peer
      may create one only when the exact checkpoint equals its own deterministic
      scheduled boundary or exact fully applied stable tip; never sign arbitrary
      caller data.
- [ ] Collect stored signatures until threshold, promote the quorum once, persist
      the assembled transferable proof, and disseminate it to reachable honest
      members.
- [ ] Serve a stored assembled quorum directly so a recovering member does not
      contact its historical signers or cause any new signing.
- [ ] Add the recovery-checkpoint stable-tip path with quorum tip discovery,
      scheduled-anchor validation, debounce, checkpoint-ID deduplication, and no
      periodic work while the tip is unchanged.
- [ ] Ensure a recovery checkpoint cannot alter the derivation of the next
      scheduled checkpoint.
- [ ] Keep checkpoint traffic off AlephBFT and out of the operation journal.
- [ ] Ensure ordinary status polling transfers only scalar checkpoint summary
      fields, not full signature sets.
- [ ] When the local journal and finalized-item cursors already match the peer,
      issue no `/federation/v1/journal` request. Healthy idle peer synchronization
      is limited to scalar status/checkpoint-summary polling.
- [ ] Call the journal catch-up endpoint only during startup/recovery or after a
      peer summary proves that the local journal, finalized-item cursor, or
      trusted checkpoint frontier is behind a selected target checkpoint.

### Files and references

- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-axum/src/federation.rs`
- Add `crates/cdk-axum/src/federation/checkpoint_routes.rs`.
- Add `crates/cdk-axum/src/federation/checkpoint.rs` if orchestration does not fit
  cleanly in `node_runtime.rs`.
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/peer_client.rs`
- `crates/cdk-axum/src/federation/sync_config.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-mintd/src/config.rs`
- `crates/cdk-mintd/src/lib.rs`
- `crates/cdk-mintd/example.config.toml`

### Success criteria

- Twelve unchanged sync rounds produce zero checkpoint signatures, zero
  checkpoint DB writes, zero full journal reads, zero historical signature
  verifications, and zero journal catch-up requests.
- A batch produces at most one local signature per member per checkpoint ID.
- A quorum checkpoint is eventually formed with the minimum threshold of
  available honest members.
- One member can transfer a stored assembled quorum to another member with zero
  new signatures and zero journal reads by the original signers.
- A moving tip cannot change the contents of an already identified scheduled
  checkpoint.
- A recovery checkpoint forms only for an exact converged stable tip and is
  superseded by a later scheduled quorum that covers it.
- Checkpoint production does not change readiness of a healthy live member.

### Test conditions

- Accelerated peer-sync test with unchanged history and exact work counters.
- Five-member threshold test with signatures arriving in different orders.
- One member offline, duplicate REST delivery, response loss, and reconnect
  tests.
- Competing payload test at the same sequence: neither conflicting minority may
  be promoted.
- Scheduled-boundary tests for an interval crossed mid-unit, rejected-only
  finalized progress, and simultaneous journal/finalized interval crossing.
- Recovery-checkpoint tests for quiet tip, moving tip, disagreement, repeated
  identical requests, stored-quorum transfer, and next-scheduled derivation.

## Milestone 4: Replace range-certified catch-up with scheduled-batch and stable-tail catch-up

### Objective

Download scheduled historical batches and the post-scheduled stable tail from
one selected source while preserving reusable quorum authentication for every
frontier that becomes authoritative.

### Checklist

- [ ] Extend catch-up discovery so the requester selects the next stored
      scheduled quorum checkpoint before downloading its batch.
- [ ] Allow any peer holding the assembled scheduled quorum to serve it; do not
      contact or wake the original signers.
- [ ] Request finalized items and compact journal entries strictly after the
      current trusted checkpoint and no farther than the target checkpoint.
- [ ] Initialize rolling operation and finalized-item digests from the trusted
      checkpoint and update them only across received suffix items.
- [ ] Require exact equality with the target checkpoint's journal cursor/order
      digest, finalized cursor/finalized digest, session cursors, and projected
      materialized-state digest.
- [ ] Apply and promote one checkpoint-bounded batch atomically; do not expose
      public routes between an unverified batch write and promotion.
- [ ] Resume interrupted catch-up from the last promoted checkpoint without
      replaying earlier batches.
- [ ] If one source serves invalid data, discard that batch, retain the trusted
      base, back off the source, and retry another peer.
- [ ] Fetch journal/finalized bodies from one healthy source. Fetch only a stored
      bounded quorum proof from another source when the first source does not
      already carry it; never fetch the same historical bodies from every
      signer.
- [ ] After the newest scheduled checkpoint, use live AlephBFT or discover an
      exact quorum-observed stable tip and form/fetch a recovery checkpoint
      anchored to that scheduled checkpoint.
- [ ] Download the stable tail from one source, validate rolling commitments to
      the recovery checkpoint, and promote it through the same atomic checkpoint
      path; never trust a single REST peer's uncertified tail.
- [ ] Remove all request-time range certificate creation and verification from
      journal responses.

### Files and references

- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-axum/src/federation/catch_up_routes.rs`
- `crates/cdk-axum/src/federation/peer_client.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/lib.rs`
- `crates/cdk-axum/src/federation/tests.rs`

### Success criteria

- Catch-up from checkpoint C to checkpoint C+1 reads/verifies only that batch.
- The requester transfers journal/finalized bodies from one healthy source and
  a stored bounded checkpoint quorum from any peer that has it.
- Historical signers perform no work when a later member downloads a scheduled
  batch.
- No malformed or minority-authenticated batch can mutate authoritative state.
- After a crash at any write boundary, restart resumes from C or C+1, never an
  intermediate trusted frontier.
- A very old member can advance across multiple retained checkpoints without
  loading or revalidating the prefix before its starting checkpoint.

### Test conditions

- Tampered operation, reordered operation, missing finalized item, altered
  finalized status, truncated unit, wrong session, and wrong state-digest tests.
- Source-peer failure between pages and between data persistence/quorum
  promotion.
- Long partition spanning several checkpoints followed by recovery.
- Multiple candidate source peers, including one Byzantine response and one
  honest response.
- Catch-up through multiple scheduled checkpoints followed by a recovery
  checkpoint tail, including a moving-tip attempt that must wait rather than
  produce incompatible signatures.
- Verify public mint/swap/melt/restore/check-state routes remain fail-closed
  until the final convergence gate passes.

## Milestone 5: Start the consensus runtime from a nonzero checkpoint

### Objective

Eliminate full-history AlephBFT runtime and journal reconstruction when a valid
local scheduled or recovery checkpoint exists.

### Checklist

- [ ] Add base-aware constructors for `InMemoryFederationJournal`, the
      finalization handler, and `FederationConsensusRuntime` using checkpoint
      cursors/digests.
- [ ] Represent runtime counts as checkpoint base plus retained suffix; status
      must not confuse suffix vector length with total federation height.
- [ ] Load journal and finalized-item rows with `..._from` queries beginning at
      the checkpoint cursors.
- [ ] Stop calling the full-history
      `authenticated_aleph_bft_from_finalized_items` path when a checkpoint fast
      path is available.
- [ ] Move old-operation duplicate/idempotency checks to existing indexed
      durable operation lookup where an unbounded historical in-memory set was
      previously required.
- [ ] Reject replayed pre-checkpoint finalized units using signed cursor/session
      watermarks rather than a set reconstructed from every historical unit.
- [ ] Restore the local member's next unit index from the signed per-member
      cursor map and restore pending local operations through the existing
      pending-operation table.
- [ ] Validate only the local suffix against the checkpoint base and compare
      the current materialized-state digest before opening readiness.
- [ ] Fall back fail-closed to genesis/full recovery only for an empty federation
      or an explicitly supported database with no checkpoint; do not silently
      trust an invalid checkpoint.

### Files and references

- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/consensus/tests.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/federation/journal.rs`
- `crates/cdk-axum/src/lib.rs` (`create_federation_runtime_state`)
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/custom_handlers.rs`
- `crates/cdk-axum/src/federation/tests.rs`

### Success criteria

- At an exact trusted checkpoint, startup loads no pre-checkpoint journal or
  finalized-item bodies and verifies only one bounded checkpoint quorum.
- With a suffix of S items, startup work is O(S), independent of checkpoint
  height.
- Old operation retries remain idempotent and old finalized-unit replays remain
  rejected.
- Materialized-state corruption at or after the checkpoint cannot result in
  Healthy readiness.
- Session rotation, local unit indexes, pending operations, and response replay
  continue from the correct nonzero cursors.

### Test conditions

- Synthetic equal-tail startup at checkpoint heights 1,000, 10,000, and 100,000
  with assertions on rows loaded and signatures verified, not only elapsed time.
- Startup with suffix lengths 0, 1, batch-size minus 1, and multiple batches.
- Restart immediately before and after checkpoint promotion.
- Restart from both a scheduled checkpoint and a later recovery checkpoint
  anchored to it.
- Restart all members simultaneously from matching checkpoints.
- Restart with one member's state altered, checkpoint signature corrupted,
  checkpoint missing, or database cursor ahead/behind the checkpoint.
- Replay a pre-checkpoint operation ID and finalized unit after restart.

## Milestone 6: Make readiness and observability checkpoint-native

### Objective

Remove certificate reconstruction from health/status paths and expose the
trusted checkpoint frontier as scalar runtime state.

### Checklist

- [ ] Replace `VerifyingCatchUpCertificate` with a checkpoint-specific readiness
      state carrying the trusted and target checkpoint cursors.
- [ ] Replace certificate error variants with checkpoint mismatch, insufficient
      quorum, unavailable checkpoint, and checkpoint-tail catch-up errors.
- [ ] Replace local status certificate fields with latest local scheduled
      checkpoint, latest scheduled quorum, latest recovery quorum, latest
      trusted frontier, checkpoint kind/lag, and signer count/IDs.
- [ ] Read status fields from in-memory/durable scalar frontier state without
      scanning journal or signature rows.
- [ ] Replace certificate Prometheus metrics and add scheduled/recovery
      checkpoint creation, stored-quorum transfer, promotion, catch-up batch,
      signature-verification, and startup-tail counters/timers.
- [ ] Rename the mintd status guard to
      `--require-checkpoint-quorum` and update digest comparison output.
- [ ] Update smoke scripts to wait for checkpoint coverage only at explicit
      recovery/checkpoint gates, not after every lightweight operation.
- [ ] Keep logs transition-based and bounded; unchanged checkpoint polls must
      remain debug-silent.

### Files and references

- `crates/cdk-common/src/federation.rs`
- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/status_routes.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-prometheus/src/metrics.rs`
- `crates/cdk-mintd/src/cli.rs`
- `crates/cdk-mintd/src/federation_status.rs`
- `crates/cdk-mintd/src/federation_status/tests.rs`
- `crates/cdk-mintd/src/federation_digest_compare/tests.rs`
- `crates/cdk-mintd/dev/federation/smoke-local-federation.sh.in`

### Success criteria

- A status request is O(1) with respect to history and performs zero signature
  verification.
- Idle status and sync polling produce no repetitive warning/info logs.
- Operators can distinguish local tip, latest scheduled checkpoint, recovery
  checkpoint, trusted/target frontier, suffix length, and whether live AlephBFT
  owns the remaining tail.
- Existing alerting semantics remain fail-closed under checkpoint mismatch.

### Test conditions

- Status serialization and CLI formatting tests for empty, exact scheduled
  checkpoint, recovery checkpoint, suffix, catching-up, mismatch, and halted
  states.
- Metrics tests proving gauges are replaced rather than duplicated.
- Repeated status request test asserting no database history read or signature
  verification.
- Log-capture test asserting stable idle rounds do not emit repeated recovery
  transitions.

## Milestone 7: Delete the catch-up-certificate implementation cut-through

### Objective

Finish with one recovery-evidence model and no dual wire, database, status, or
configuration behavior.

### Checklist

- [ ] Delete `FederationCatchUpCertificate`,
      `FederationCatchUpCertificateQuorum`, transcript construction, range-key
      grouping, and range quorum verification.
- [ ] Remove `catch_up_certificate` from
      `FederationJournalCatchUpResponse` and delete response signing.
- [ ] Delete `catch_up_certificate_quorum_status`, contiguous-prefix rebuilding,
      its cache/invalidation plumbing, and certificate collection functions.
- [ ] Delete certificate database methods, codecs, table/index names, and tests.
- [ ] Confirm both scheduled and recovery policies use only
      `FederationCheckpointQuorum`; do not retain a renamed range-certificate
      compatibility path.
- [ ] Delete certificate readiness/error/status/metric/CLI/config fields and all
      compatibility aliases.
- [ ] Update current architecture, implementation, security-review, stress-test,
      and operator documentation to describe checkpoints.
- [ ] Squash the checkpoint schema into the existing initial federation
      migrations and regenerate federation development databases/configs; no
      existing federation database or upgrade bridge is supported after the
      cut-through change.
- [ ] Use `rg` to prove no production references to catch-up certificates remain;
      historical review text may retain clearly marked provenance only.

### Files and references

- `crates/cdk-common/src/federation.rs`
- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/federation.rs`
- `crates/cdk-sql-common/src/mint/federation/journal.rs`
- SQLite/Postgres fresh federation schema migrations
- `crates/cdk-axum/src/federation.rs`
- `crates/cdk-axum/src/federation/catch_up_routes.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-axum/src/federation/status_routes.rs`
- `crates/cdk-axum/src/federation/tests.rs`
- `crates/cdk-axum/src/lib.rs`
- `crates/cdk-axum/src/custom_handlers.rs`
- `crates/cdk-prometheus/src/metrics.rs`
- `crates/cdk-mintd/src/cli.rs`
- `crates/cdk-mintd/src/config.rs`
- `crates/cdk-mintd/src/federation_status.rs`
- `crates/cdk-mintd/dev/federation/smoke-local-federation.sh.in`
- `docs/federated-cashu-implementation-plan.md`
- `docs/federated-cashu-implementation-plan-completed.md`
- `docs/federated-cashu-code-and-security-review.md`
- `docs/federated-cashu-nocean-stress-test.md`

### Success criteria

- There is one checkpoint protocol, one database representation, and one
  readiness path for both scheduled history and recovery tails.
- A nonempty REST catch-up response is not signed per range and does not require
  matching range responses from every quorum member.
- No steady-state task can invoke historical certificate verification because
  the code and schema no longer exist.
- Standalone and federation feature combinations compile without dead code or
  compatibility branches.

### Test conditions

- `rg` production sources for `CatchUpCertificate`,
  `catch_up_certificate`, and the old metric/config names.
- Run formatting, federation-focused clippy/checks, `cdk-common`, `cdk-axum`,
  SQLite, Postgres compile/tests, and mintd status/setup tests.
- Build without federation features and run affected standalone mint/wallet
  tests.
- Start only fresh generated federation databases; confirm an obsolete
  experimental database is unsupported and is never partially accepted through
  a migration or compatibility path.

## Milestone 8: Prove safety, liveness, and performance

### Objective

Demonstrate that checkpoints remove the observed CPU/startup pathology without
weakening catch-up, restart, or Byzantine fault handling.

### Checklist

- [ ] Run the full restart matrix: one member, threshold members, all members,
      clean shutdown, crash before checkpoint, crash during promotion, and long
      offline interval.
- [ ] Run partition tests below and above consensus threshold and heal them.
- [ ] Run scheduled-boundary agreement tests where the journal interval is
      crossed mid-unit and where only finalized-item progress triggers the
      checkpoint.
- [ ] Run stable-tail recovery checkpoint tests with exact agreement, a moving
      tip, a stale recovery frontier, and a later scheduled checkpoint that
      supersedes it.
- [ ] Run malicious source/checkpoint-signature tests from milestone 4 in the
      real multi-member harness.
- [ ] Measure idle CPU and work counters for at least ten minutes with a long
      synthetic history.
- [ ] Measure cold start at multiple history sizes with suffix zero and a fixed
      suffix; fit/report whether work scales with history or suffix.
- [ ] Repeat the federation receive stress matrix and compare confirmed TPS,
      journal TPS, latency, CPU, log rate, catch-up transitions, and checkpoint
      traffic with milestone 0.
- [ ] Redeploy a fresh development federation and exercise member restart from
      scheduled and recovery checkpoints after creating several batches.
- [ ] Recover a long-offline member using checkpoint quorums fetched from one
      peer and assert that historical signers perform zero new signature or
      journal-scan work.
- [ ] Record aggregate findings without credentials, private URLs, machine
      identifiers, private keys, database copies, or bearer tokens.

### Files and references

- `crates/cdk-axum/src/federation/tests.rs`
- `crates/cdk-axum/src/federation/tests/aleph_bft_routes.rs`
- `crates/cdk-mintd/dev/federation/smoke-local-federation.sh.in`
- `crates/cdk-mintd/src/federation_setup/tests.rs`
- `scripts/benchmark_nocean_federation.sh`
- `scripts/reproduce_nocean_federation_stress_test.sh`
- `scripts/deploy_nocean_federation.sh`
- `docs/federated-cashu-nocean-stress-test.md`
- this implementation plan

### Success criteria

- Idle sync/status polling performs zero history scans and zero checkpoint
  signature verification.
- With zero suffix, rows loaded and signatures verified at startup are constant
  as historical height increases.
- With fixed suffix S, startup work remains constant as historical height
  increases and grows only with S.
- A restarted member catches up from its trusted checkpoint and reaches the
  consensus tip without replaying the prefix before that checkpoint.
- A member can download every scheduled historical batch from one source and
  authenticate each with a previously assembled transferable quorum; the
  original signers do no recovery-time work.
- A post-scheduled tail becomes authoritative only through live AlephBFT or one
  exact stable-tip recovery checkpoint quorum.
- CPU returns to an idle baseline after load; there is no recurring five-second
  CPU spike correlated with peer sync.
- Log volume remains bounded and transition-based.
- Throughput is not lower than the milestone-0 baseline; any remaining TPS limit
  is profiled separately from checkpoint work.
- Every tampering, minority, mismatch, and crash-window test remains fail-closed.
- Five-member mint, swap, melt, restore, check-state, session rotation, and
  restart smoke tests pass.

### Test conditions

- Use both in-memory test transport and real localhost private REST/AlephBFT
  transport.
- Exercise SQLite as the deployment-representative backend and compile/run the
  shared checkpoint persistence suite for Postgres.
- Run release binaries for performance conclusions.
- Compare equal operation counts and checkpoint intervals; do not compare a
  fresh short history with an old long-history baseline.
- Capture process CPU and request/log counters separately for each federation
  member so one lagging member cannot be hidden by an aggregate.

## Final definition of done

The replacement is complete when every honest member deterministically creates
and persists scheduled checkpoint quorums at consensus-configured journal or
finalized-item intervals, and any member can later obtain those stored proofs
from one peer without causing historical rescans or new signatures. A member
can restart from a quorum-signed checkpoint at a large nonzero history,
initialize the journal and AlephBFT runtime from checkpoint cursors,
validate/catch up only the suffix, recompute current materialized state once,
and become healthy without loading or verifying the pre-checkpoint history.

In steady state, scheduled checkpoint creation is a sparse consequence of
finalized progress. An unchanged five-second peer-sync or status round performs
no checkpoint cryptography, no history scan, and no journal catch-up request.
The journal endpoint is used only when startup/recovery or a newer peer summary
shows that the local journal, finalized-item cursor, or trusted checkpoint is
behind. REST catch-up transfers each scheduled batch from one selected peer and
verifies a previously assembled bounded quorum separately. The post-scheduled
tail is handled by live AlephBFT or the same checkpoint proof created once at an
exact converged stable tip. All old catch-up range-certificate types, tables,
routes, status fields, metrics, configuration, and compatibility behavior have
been removed.

Most importantly, the safety argument becomes simpler: AlephBFT orders the
history; cumulative digests commit to its finalized and accepted prefixes; a
configured quorum signs sparse durable checkpoints; and recovery advances from
one verified checkpoint to the next without repeatedly re-proving genesis.
