# Federated CDK signed-checkpoint implementation plan

Status: in progress (checkpoint protocol, persistence, exchange, bounded
page-streamed catch-up, nonzero startup, certificate cut-through,
checkpoint-native operator tooling, crash-safe checkpoint retention, ordered
recovery barriers, and bounded work instrumentation are implemented;
checkpoint production is event-driven and stable checkpoint polls are work/log
silent; consolidation and final validation remain)
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
- **recovery checkpoints** created on demand at the complete terminal boundary
  of an AlephBFT-ordered recovery barrier to authenticate the finalized but
  not-yet-scheduled tail even while valid traffic continues.

Each checkpoint commits to the cumulative accepted-operation order, the
cumulative finalized-item stream, a cumulative terminal-application digest, a
full audit digest of the authoritative materialized application state, and the
recovery cursors at a stable AlephBFT unit boundary. Members derive and sign a
checkpoint once, persist its individual signatures and assembled quorum, and
serve that stored proof without rescanning history or signing it again. A node
verifies a checkpoint quorum once, persists that trusted recovery frontier,
initializes the runtime at that nonzero frontier, and processes only the suffix
after it. Normal startup trusts the transactionally persisted checkpoint and
application frontiers; it does not recompute the full materialized-state audit
digest.

The target complexity is:

- steady state with no new checkpoint: O(1), with no signatures and no journal
  or certificate scan;
- scheduled checkpoint creation: O(current authoritative materialized state +
  new batch + federation threshold), once per deterministic boundary, because
  version 1 deliberately performs one full state audit instead of maintaining
  a Merkle tree;
- recovery checkpoint creation: O(current authoritative materialized state +
  tail + federation threshold), only when recovery needs a stable
  post-scheduled frontier;
- catch-up: O(number of missing operations/finalized items), not O(total
  history) per poll;
- startup at an exact trusted checkpoint: O(federation threshold + bounded
  current-session AlephBFT backup), independent of historical journal and
  materialized-state size;
- startup with a tail after the checkpoint: O(federation threshold + tail +
  bounded current-session AlephBFT backup), with the tail bounded by checkpoint
  policy in the normal case.

This design intentionally optimizes the startup critical path rather than
checkpoint creation. Detecting arbitrary out-of-band corruption of every
materialized database row before readiness would require either a full state
scan on every startup or an incrementally maintained authenticated state tree.
Version 1 does neither. It relies on database transaction integrity for crash
consistency, checks the full authoritative state when a checkpoint is created,
and provides an explicit operator audit that can recompute the full state
digest and durably halt the member on mismatch.

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
- A third cumulative application digest commits in accepted-operation order to
  every terminal application result. Business-state writes, terminal journal
  state, the applied cursor, and this digest must advance atomically.
- The full materialized-state audit digest is distinct from all cumulative
  ordering/application commitments and remains in the checkpoint. It covers
  only consensus-authoritative state and is computed from database state
  directly, without discovering rows by scanning historical journal entries.
- Member-local, transient, uncommitted, or diagnostic state must not influence
  either signed application commitment.
- A member signs only a checkpoint that exactly matches its own fully applied,
  durable state at that checkpoint boundary.
- Scheduled checkpoint eligibility and boundary selection are deterministic
  consensus configuration. A checkpoint never cuts through an AlephBFT unit.
- Recovery checkpoints cover only the complete terminal boundary selected by a
  durably ordered recovery barrier and never a caller-selected boundary,
  digest, materialized-state claim, or arbitrary historical range.
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
  stream, terminal application frontier, or materialized state committed in the
  same database.
- A materialized-state mismatch remains latched/fail-closed across restart.
- Starting from a checkpoint must preserve old-operation idempotency and old
  finalized-unit replay rejection without rebuilding unbounded in-memory sets.
- Checkpoint boundaries do not need to coincide with AlephBFT session
  boundaries. The current live-session backup remains a separate, bounded
  recovery input and must never become an alternate source of authoritative
  application state.
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
application_digest
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
bytes must not be placed in the quorum transcript. `application_digest` is a
domain-separated cumulative chain over canonical terminal application results.
`materialized_state_digest` is the full authoritative-state audit captured at
checkpoint creation. Normal restart verifies the signed stored value and its
atomic frontier metadata but does not recompute that full audit.

### Application and materialized-state commitments

`application_digest` starts from a domain-separated genesis value and advances
exactly once for each accepted-journal operation when that operation reaches a
terminal application state. Its append transcript includes at least the prior
application digest, operation index, operation ID, and canonical terminal
`FederationApplicationState`, including deterministic rejection details. A
valid checkpoint requires the terminal application frontier to equal
`journal_next_index`. Retries or idempotent re-observation of an already
terminal operation do not append the digest again.

The application digest is a cumulative commitment to deterministic application
results, not a Merkle root over current SQL rows. Its safety depends on committing
the operation's business-state writes, terminal journal state, applied frontier,
and next application digest in one database transaction. Startup recomputes
this chain only over the retained suffix after its checkpoint base and compares
the result with scalar durable metadata.

`materialized_state_digest` retains the stronger whole-state comparison. It is
computed at checkpoint creation from a canonical direct enumeration of every
consensus-authoritative materialized table. It must not use the full journal to
discover row keys, and it excludes local payment/backend metadata, transient
work queues, private signing material, and other member-local state. Version 1
does not maintain an incremental authenticated state tree. Consequently this
audit is proportional to current authoritative state, but normal startup and
status are not.

The same full audit is available only through an explicit operator command
outside checkpoint creation. There is no periodic or background audit and no
normal-startup audit in this plan.

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
cut. Under the federation operation-application lock, the member captures the
rolling digests/cursors, computes the full materialized-state audit directly
from authoritative database tables in a transactionally stable view, and
persists the checkpoint candidate before later finalization may change the
state being certified. Signature exchange happens after this local durable
capture and does not hold the application lock.

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
checkpoint for the smaller finalized tail. The requester signals demand in an
authenticated summary request. A healthy peer proposes a bounded recovery
barrier through ordinary AlephBFT ordering; after its entire unit is terminally
applied, every member derives the checkpoint boundary and payload from local
durable state. A member signs only that locally derived payload with its latest
scheduled quorum as anchor. It never signs a caller-provided digest, a lower
arbitrary historical state, or a caller-selected boundary.

Recovery barriers use stable generation IDs, one-pending admission, a local
proposal lease, and a deterministic bounded-tail suppression rule. Recovery
checkpoint candidates remain deduplicated by checkpoint ID and reusable until
a later scheduled quorum covers their frontier. They replace, rather than
coexist with, the old arbitrary range certificate.

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

The startup fast path assumes the mint database preserves committed
transactions. It detects partial writes by requiring the journal, finalized,
terminal-application, cumulative-digest, and checkpoint-frontier metadata to be
atomically consistent. Arbitrary out-of-band row modification is handled by an
explicit operator materialized-state audit, not by normal startup. An audit
mismatch durably latches the member fail-closed and cannot be cleared by an
ordinary restart or peer agreement.

### AlephBFT live-session backup policy

Checkpoints initialize CDK's journal, finalization handler, duplicate guards,
and recovery cursors. They do not replace upstream AlephBFT's current-session
backup. Checkpoints may occur at any complete finalized-unit boundary,
including in the middle of an AlephBFT session.

After checkpoint and suffix validation, startup loads the local backup for the
active session. If it is missing or stale, the member reuses the existing
authenticated private-REST backup recovery path and fetches the bounded current
session backup from a peer whose session and durable tip match the selected
target. AlephBFT validates the protocol data in the backup; every newly
finalized payload still passes CDK's signed base cursors, per-member unit
watermarks, duplicate checks, and normal deterministic application path. An
invalid backup may fail recovery but must not advance authoritative state; the
member backs off that source and tries another peer.

Session rotation is the backup-compaction boundary. Configuration and runtime
limits must bound current-session rounds, backup chunks, and total recovered
backup bytes independently of federation lifetime. Once a newer session is
durably active and a retained checkpoint/finalized frontier covers older
sessions, obsolete backup sessions may be pruned according to a crash-safe
retention rule. A single recovering member must never force a session rotation
as a checkpoint shortcut.

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
2. Verify its bounded signature set once and compare its journal, finalized,
   terminal-application, and cumulative-digest frontiers with scalar database
   metadata.
3. Initialize the journal and AlephBFT runtime from the checkpoint's nonzero
   base digests and cursors.
4. Load only local journal/finalized-item rows after the checkpoint. Recompute
   the operation, finalized-item, and terminal-application digest chains across
   that suffix, require every application to be terminal, and compare the
   resulting cursors/digests with the transactionally persisted local frontier.
   Do not reapply an already materialized local suffix.
5. Do not recompute the full materialized-state audit on normal startup. The
   signed stored audit digest is trusted only together with the atomic local
   checkpoint/application frontier. Operators may run the explicit full audit
   separately; mismatch durably halts the member.
6. Ask peers for their newest stored scheduled checkpoint quorums, recovery
   checkpoint quorum, and scalar consensus tips.
7. If local state is behind, fetch each missing scheduled-checkpoint-bounded
   batch from one selected healthy source, validate all rolling digest chains
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
10. Load the bounded local backup for the active AlephBFT session. If it is
    missing or stale, fetch it from an authenticated matching peer with source
    failover, then start the live session. Do not fetch or replay backups from
    every historical session.
11. Open authoritative routes only when local order, finalized cursor,
    application digest, peer tip, bounded-backup recovery, and live-session
    readiness satisfy the existing fail-closed convergence rules.

The unavoidable startup work is verifying one quorum and restoring the bounded
current AlephBFT session backup. Historical journal length and materialized
state size must not determine startup time when the database is exactly at a
trusted checkpoint.

## Milestone dependency map

| Milestone | Result |
| --- | --- |
| 0 | Reproducible CPU/startup baseline and work counters |
| 1 | Canonical checkpoint, application/finalized digests, and quorum verification |
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

- [x] Add counters/timers for full journal rows loaded, finalized items loaded,
      certificate/checkpoint signatures verified, startup replayed entries,
      materialized-state digest duration, AlephBFT backup bytes/chunks loaded,
      and readiness duration.
- [x] Keep instrumentation bounded and free of operation payloads or secrets.
- [x] Add synthetic histories at 1,000, 10,000, and at least 100,000 operations
      without requiring public networking.
- [x] Record idle CPU/work-counter behavior across at least twelve five-second
      peer-sync rounds with unchanged history.
- [x] Record cold-start time and rows/signatures processed at each history size.
- [x] Run the existing receive stress benchmark and preserve its raw output
      outside the repository; record only aggregate results.
- [x] Preserve the materialized-state mismatch regression as an explicit
      operator-audit gate, and preserve partial evidence and restart catch-up
      regressions as startup/recovery gates.

### Progress and verification

Checkpoint and restart work now uses two shared Prometheus collectors instead
of one collector per call site:

- `cdk_federation_checkpoint_work_total{member,work}` counts journal/finalized
  rows loaded, signatures verified, pre-checkpoint entries replayed, checkpoint
  suffix rows loaded, backup bytes/chunks loaded, scheduled/recovery candidates
  created, stored quorums transferred, checkpoints promoted, and catch-up
  batches applied;
- `cdk_federation_checkpoint_duration_seconds{member,operation}` measures
  materialized-state projection/digest work, startup readiness, startup-tail
  reconstruction, scheduled/recovery creation, quorum transfer, promotion,
  signature verification, and checkpoint-bounded catch-up.

Both variable labels are closed Rust enums. The only other label is the finite
configured member ID; checkpoint IDs, operation IDs, payloads, peer URLs,
errors, and secrets cannot become metric labels. Instrumentation is behind the
existing optional Prometheus feature and the federation-only build retains the
same behavior without the metrics dependency.

Verified with the fixed-label collector unit test, federation-only and
federation-plus-Prometheus `cdk-axum` checks, and focused Prometheus-enabled
runtime tests for exact-checkpoint startup with no prefix rows,
checkpoint-bounded catch-up/promotion, and persisted live-session backup
replay. Strict `cdk-prometheus` clippy passes. Strict workspace/Axum clippy
still encounters pre-existing unrelated warnings before it can serve as a
repository-wide gate.

The ignored release benchmark
`benchmark_checkpoint_startup_is_history_length_independent` now creates
file-backed SQLite histories of 1,000, 10,000, and 100,000 genuine finalized
operations without networking. Its fixture uses one normally applied mint
quote followed by uniquely authorized deterministic conflicts. This keeps the
authoritative materialized state bounded while exercising the existing
consensus, journal, finalized-item, transaction, rolling-frontier, checkpoint,
and restart code. It periodically reconstructs its fixture runtime at a
transient checkpoint boundary so fixture memory is bounded; those transient
checkpoints are never trusted or measured.

One release run recorded the following restart aggregates. The timed section
begins after closing and reopening the SQLite database and covers
`create_federation_runtime_state`; fixture construction is excluded.

| Finalized operations | Startup | Journal rows | Finalized rows | Signatures | Replay/tail/backup work |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 8.597 ms | 0 | 0 | 2 | 0 |
| 10,000 | 8.329 ms | 0 | 0 | 2 | 0 |
| 100,000 | 8.058 ms | 0 | 0 | 2 | 0 |

The corresponding test asserts that the recovered runtime reports the full
operation and finalized-item cursors while retaining no prefix rows. The
measurements demonstrate the intended bounded behavior: exact-checkpoint
startup verifies the configured two-signature quorum and does not read or
replay history as the retained database grows.

`benchmark_unchanged_checkpoint_idle_rounds` reuses the ordinary fast idle
regression at the configured five-second cadence. Its Milestone 8 form creates
100,000 finalized operations, promotes their exact checkpoint, restarts with no
prefix rows, and runs 120 ordinary status/fetch/peer-exchange rounds. Two
release runs covered 601.924 and 602.498 seconds inside the idle loop. The
checkpoint-work, signature-verification, materialized-state-audit, catch-up,
and emitted-log deltas all remained zero. The OS-timed run used 27.46 seconds
of user CPU and 0.16 seconds of system CPU over 628.19 seconds end to end; those
conservative totals include construction of the 100,000-operation fixture. The
earlier warmed one-operation run used 0.31 seconds of user CPU and 0.09 seconds
of system CPU over 60.50 seconds. Raw benchmark and timing output was kept only
in untracked temporary storage.

The pre-checkpoint five-member receive-stress baseline remains the run recorded
in `federated-cashu-nocean-stress-test.md`: its best 20-request burst confirmed
20/20 at 0.566 transactions per second, while multi-wave confirmed throughput
plateaued near 0.27 transactions per second. The linked document contains only
aggregate evidence; bearer-token and per-request raw artifacts were kept
outside the repository. This is the Milestone 0 comparison point for the
post-checkpoint repetition in Milestone 8.

The explicit operator-audit match/mismatch tests, tampered checkpoint-batch
atomicity test, and both SQLite paid/failed restart-catch-up tests pass with
Prometheus enabled. They preserve the former startup mismatch invariant in the
operator-only audit path and retain fail-closed partial-evidence and restart
recovery gates without adding a background full-state scan.

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
  reconstruction, certificate verification, materialized-state projection, and
  current-session AlephBFT backup loading.
- Existing partial-evidence and restart/catch-up tests remain fail-closed. The
  former startup full-state mismatch regression is preserved through the
  explicit operator-audit path rather than a normal-startup scan.

### Test conditions

- Run focused `cdk-axum` federation tests in debug mode.
- Run synthetic startup measurements in release mode with SQLite.
- Run one five-member AlephBFT process test with no faults and one with a member
  offline long enough to require REST catch-up.
- Run the reproducible deployment stress script only after local gates pass.

## Milestone 1: Define and verify the checkpoint protocol

### Objective

Introduce a cumulative checkpoint primitive that can authenticate the
application journal, terminal application results, the complete AlephBFT
finalized-item prefix, and one checkpoint-time authoritative-state audit with a
bounded quorum proof.

### Checklist

- [x] Add `FederationCheckpoint`, `FederationCheckpointId`,
      `FederationCheckpointSignature`, and `FederationCheckpointQuorum` in a
      dedicated federation checkpoint module.
- [x] Add one signed `FederationCheckpointKind` discriminator for `Scheduled`
      and `Recovery`; do not create a second range-certificate proof type.
- [x] Define a versioned, domain-separated canonical signature transcript.
- [x] Bind the checkpoint to the federation config digest/identity-key epoch.
- [x] Add `FederationFinalizedItemDigest` and define canonical hashing for every
      finalized item, including status and consensus/unit provenance.
- [x] Persist or derive the rolling finalized-item digest during finalization;
      never recompute it from genesis for normal checkpoint creation.
- [x] Add a domain-separated cumulative application digest over canonical
      terminal application results, including deterministic rejection details.
- [x] Require business-state changes, terminal journal application state, the
      applied cursor, and the cumulative application digest to commit in one
      database transaction.
- [x] Define the authoritative materialized-state audit projection. It must
      enumerate database state directly, exclude member-local/transient rows,
      and require no historical journal scan.
- [x] Document the startup integrity model: normal startup trusts atomic
      database frontiers and verifies only the suffix; arbitrary row corruption
      is checked by checkpoint creation or an explicit operator audit, not by a
      startup full-state scan.
- [x] Define deterministic checkpoint eligibility at complete unit and terminal
      application boundaries.
- [x] Verify distinct known signers, reject duplicates, and require the
      configured consensus threshold.
- [x] Define scheduled sequence/anchor rules so recovery checkpoints do not
      change the next scheduled checkpoint ID or fork the scheduled chain.
- [x] Reject an older scheduled sequence, broken scheduled anchor, cursor
      regression, incompatible recovery frontier, config mismatch, unknown
      signer, wrong federation, modified digest, modified cursor, and bad
      signature.
- [x] Define monotonic trusted-frontier promotion for multiple recovery
      checkpoints anchored to the same scheduled checkpoint.
- [x] Document explicitly that checkpoint quorum is recovery evidence, not
      consensus finality.
- [x] Document explicitly that raw AlephBFT backup bytes are not checkpoint
      evidence and cannot authorize application state.

### Progress and verification

The first milestone-1 implementation unit is complete in
`crates/cdk-common/src/federation/checkpoint.rs`. It centralizes the canonical
payload, rolling application/finalized-item digests, checkpoint ID and Schnorr
signature transcripts, bounded quorum verification, and scheduled/recovery
promotion rules. The second unit adds durable contiguous journal,
finalized-item, and terminal-application frontiers in the shared SQL layer.
Every finalized item and terminal application result now receives its rolling
digest exactly once in the transaction that persists it; rollback tests prove
the scalar frontier does not escape the transaction.

The direct authoritative-state projection no longer accepts or scans journal
bodies. Applied operations transactionally maintain a compact typed registry
of the current quote, completed-operation, proof-state, blind-signature, and
keyset rows they write. A full audit enumerates that registry together with the
federation-specific authoritative tables and hashes the current referenced
database rows, including deterministic missing-row markers. The registry is
only a direct row-discovery index; rejection details remain exclusively in the
cumulative application digest. Effective keyset reconstruction reuses the same
rotation records instead of replaying the journal. Deterministic boundary
selection is now centralized in `FederationCheckpointBoundary`. The public
federation config commits to non-zero journal-operation and finalized-item
intervals, both defaulting to 10,000. A boundary is constructible only after a
complete finalized unit with a terminal application frontier, scheduled
eligibility triggers when either configured interval crosses, and the shared
scheduled/recovery builders enforce sequence and anchor rules. Runtime capture,
candidate persistence under the application lock, and signature exchange
remain milestone-3 work rather than protocol-definition work.

Verified with:

- `cargo test -p cdk-common --features federation checkpoint --lib` (10 passed);
- `cargo test -p cdk-sqlite --features federation federation_checkpoint_persistence`
  (1 passed);
- `cargo test -p cdk-common --features federation
  applied_operation_derives_direct_materialized_state_keys --lib` (1 passed);
- `cargo test -p cdk --features mint,federation effective_federation_config`
  (2 passed);
- `cargo test -p cdk --features mint,federation materialized_digest` (1 passed);
- `cargo test -p cdk --features mint,federation
  accepted_member_quorum_materializes_and_catches_up_identically` (1 passed);
- `cargo clippy -p cdk-common --lib --features federation -- -D warnings`;
- `cargo clippy -p cdk-sql-common --lib --features federation -- -D warnings`;
- `cargo fmt --all`.

The broader `cdk-common --all-targets` clippy gate currently stops on the
pre-existing `cloned_ref_to_slice_refs` lint in the federation operation tests;
the checkpoint production-library clippy gate is clean.

### Files and references

- `crates/cdk-common/src/federation/checkpoint.rs`
- `crates/cdk-common/src/federation.rs`
- `crates/cdk-common/src/federation/transport.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/consensus/tests.rs`
- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-common/src/federation/config.rs`
- `crates/cdk/src/mint/federation.rs`
- `crates/cdk/src/mint/federation/projection.rs`

### Success criteria

- One quorum proof has size O(number of federation members), independent of
  journal length.
- Verifying the newest checkpoint does not require journal entries, historical
  checkpoints, or finalized-item bodies.
- At an exact checkpoint, validating the local application frontier requires
  only scalar metadata and does not recompute the materialized-state audit.
- One peer holding an assembled quorum can transfer the proof without any
  signer recomputing a signature or the receiver contacting those signers.
- Any mutation to a committed field invalidates the checkpoint signature.
- Two honest members at the same durable unit/application boundary derive the
  same checkpoint ID.
- An on-demand recovery checkpoint cannot change the ID that honest members
  derive for the next scheduled checkpoint.

### Test conditions

- Unit-test canonical encoding and fixed digest vectors.
- Unit-test application-digest continuation for applied, consensus-rejected,
  deterministically rejected, duplicate, and retry-after-restart cases.
- Property-test or table-test every committed field for mutation rejection.
- Test 1-of-1, threshold-of-N, duplicate signer, unknown signer, and insufficient
  quorum cases.
- Test accepted, rejected-invalid, duplicate, and non-operation finalized items
  in the finalized digest.
- Test session rotation and a checkpoint exactly on the first/last unit boundary.
- Test scheduled checkpoints with multiple recovery checkpoints between them,
  including stale recovery rollback rejection and deterministic next-scheduled
  checkpoint derivation.
- Test that member-local or uncommitted diagnostic/payment state does not alter
  either signed application commitment at a fixed finalized frontier.

## Milestone 2: Add checkpoint persistence and atomic promotion

### Objective

Store sparse checkpoint proposals and quorums without allowing a trusted
checkpoint to move ahead of durable consensus or application state.

### Checklist

- [x] Replace the fresh federation schema's
      `federation_catch_up_certificates` table with checkpoint payload,
      individual-signature, assembled-quorum, and trusted-frontier tables.
- [x] Persist checkpoint kind, scheduled sequence, and scheduled anchor without
      duplicating the verification schema for recovery checkpoints.
- [x] Use a unique key on `(checkpoint_id, signer_member_id)` and indexed lookup
      by checkpoint sequence/finalized cursor.
- [x] Add database APIs to insert an idempotent local/remote signature, load
      checkpoints after a cursor, store/fetch an assembled quorum, load the
      newest scheduled and recovery quorums, and atomically promote a verified
      quorum.
- [x] Add scalar metadata queries for journal next index/order digest,
      finalized next index/finalized digest, terminal applied cursor/application
      digest, and latest audited materialized-state digest so startup does not
      load bodies or project full state.
- [x] Persist rolling finalized-item digests with finalized items or in an
      atomically updated frontier record.
- [x] Persist the rolling application digest and applied cursor in the same
      transaction as every successful or deterministically rejected federation
      operation's business-state writes and terminal journal application state.
- [x] Persist a locally derived checkpoint candidate and its full
      materialized-state audit digest before releasing the boundary capture
      lock; later signature/quorum storage may occur after the database has
      advanced.
- [x] Ensure catch-up promotion persists the journal/finalized batch, business
      application changes, terminal application frontier/digest, and trusted
      checkpoint in one transaction, or leaves the checkpoint behind the data.
- [x] Persist a durable materialized-state-audit mismatch latch that ordinary
      startup, peer agreement, and retry cannot clear.
- [x] Define crash-safe retention: keep all scheduled quorum checkpoints; prune
      superseded non-quorum proposals and prune recovery quorums only after a
      scheduled quorum covers them and no trusted frontier depends on them.
- [x] Preserve the cut-through policy: edit the existing fresh federation
      schema migrations in place; do not add an upgrade migration from the
      certificate schema, compatibility tables, or dual reads/writes.

### Progress and verification

The shared mint database traits and SQL implementation now persist checkpoint
payloads, immutable per-signer rows, normalized transferable quorum proofs, and
one trusted checkpoint ID. A single scalar row holds the three contiguous
rolling frontiers, the latest local checkpoint-time audit, and a one-way audit
mismatch latch. Promotion checks the checkpoint's historical boundary digests
against the locally persisted rows, rejects any frontier ahead of local state,
and updates the trusted ID in the caller's transaction.

The fresh SQLite and PostgreSQL federation schemas now contain only checkpoint
payload, signature, quorum, scalar-frontier, and supporting checkpoint indexes.
The former certificate table and codecs have been removed in place; there is no
upgrade migration, compatibility table, or dual-read/write bridge.

Retention runs inside the same database transactions that store checkpoint
candidates, quorums, and trusted-frontier promotions. All scheduled quorums are
retained. A superseded candidate is removed only when it has no quorum and is
not referenced by either the trusted frontier or latest operator-audit record.
A recovery checkpoint and its signatures/quorum are removed only after a newer
scheduled quorum covers both of its durable cursors and neither scalar record
still references it. The indexed retention query loads only the newest
scheduled quorum and recovery checkpoints eligible for deletion; it does not
scan scheduled checkpoint history and requires no background worker.

Verified with:

- shared SQLite checkpoint persistence, rollback, idempotency, scheduled and
  recovery lookup/promotion, conflicting-signature, audit-latch, superseded
  candidate pruning, scheduled-checkpoint preservation, and covered-recovery
  pruning tests;
- existing SQLite journal persistence, concurrent writes/metadata updates,
  finalized-item persistence, application-state persistence, hot-index, and
  fresh-migration tests;
- PostgreSQL checkpoint and migration test targets compiled with
  `cargo test -p cdk-postgres --features federation
  federation_checkpoint_persistence --no-run`;
- `cargo clippy -p cdk-sql-common --lib --features federation -- -D warnings`;
- `cargo clippy -p cdk-sqlite --tests --features federation -- -D warnings`;
- `cargo check -p cdk-axum --features federation`;
- `cargo check -p cdk-mintd --features federation,sqlite`;
- `cargo fmt --all -- --check`.

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
  journal/application state, application digest, or the finalized-item cursor.
- No transaction failure can expose business-state changes without the matching
  terminal application state, applied cursor, and application digest, or vice
  versa.
- Loading the latest trusted checkpoint is an indexed bounded query.

### Test conditions

- Shared database-trait tests for checkpoint insert, assembled-quorum transfer,
  scheduled/recovery lookup, quorum promotion, restart, duplicate insert,
  conflicting payload, retention, and ordered lookup.
- SQLite and Postgres transaction-failure tests at every promotion write step.
- SQLite and Postgres transaction-failure tests at every terminal-application
  digest/frontier write step.
- Concurrent signature insert/promotion tests.
- Operator-audit mismatch latch round-trip, restart, and ordinary-retry tests.
- SQLite `EXPLAIN QUERY PLAN` and Postgres schema/index assertions for the new
  hot-path queries.

## Milestone 3: Produce and exchange scheduled and recovery checkpoints

### Objective

Make scheduled checkpoint work event-driven, with one explicit full
authoritative-state audit only at an eligible boundary, make recovery
checkpoints on demand at an AlephBFT-ordered complete-unit boundary, and do no
certificate/checkpoint computation or journal catch-up request on an unchanged
peer-sync tick.

### Checklist

- [x] Replace `catch_up_certificate_chunk_size` with consensus-configured
      `checkpoint_interval_journal_operations` and
      `checkpoint_interval_finalized_items`; document initial defaults of
      10,000 for both.
- [x] Add runtime dirty/eligible frontiers advanced only after durable finalized
      unit application and trigger scheduled eligibility when either interval
      crosses its next target.
- [x] Select the first terminal, fully materialized, complete finalized-unit
      boundary after eligibility and capture its exact boundary commitments
      before later finalization advances state.
- [x] Under the operation-application lock and a transactionally stable database
      view, compute the full authoritative materialized-state audit directly
      from database tables, persist it with the checkpoint candidate, and then
      release the lock before signature exchange.
- [x] Ensure checkpoint audit projection performs no historical journal read and
      excludes member-local, transient, and uncommitted state.
- [x] Derive, persist, and sign each scheduled local checkpoint exactly once.
- [x] Add authenticated private REST request/response types and message kinds for
      checkpoint summary, fetch, and signature request.
- [x] Add checkpoint routes under `/federation/v1` and matching peer-client
      methods with existing request admission, authentication, replay-window,
      timeout, and backoff rules.
- [x] Have peers return an already stored local signature when available. A peer
      may create one only when the exact checkpoint equals its own deterministic
      scheduled boundary or fully applied ordered-barrier boundary; never sign
      arbitrary caller data.
- [x] Collect stored signatures until threshold, promote the quorum once, persist
      the assembled transferable proof, and disseminate it to reachable honest
      members.
- [x] Serve a stored assembled quorum directly so a recovering member does not
      contact its historical signers or cause any new signing.
- [x] Add the recovery-checkpoint ordered-barrier path with stable generation
      IDs, scheduled-anchor validation, proposal leasing, checkpoint-ID
      deduplication, and no periodic work while recovery is not requested.
- [x] Ensure a recovery checkpoint cannot alter the derivation of the next
      scheduled checkpoint.
- [x] Keep checkpoint signatures, quorums, and body transfer off AlephBFT and
      out of the operation journal; order only the bounded recovery barrier as
      an ordinary durable AlephBFT item.
- [x] Ensure ordinary status polling transfers only scalar checkpoint summary
      fields, not full signature sets.
- [x] When the local journal and finalized-item cursors already match the peer,
      issue no `/federation/v1/journal` request. Healthy idle peer synchronization
      is limited to scalar status/checkpoint-summary polling.
- [x] Call the journal catch-up endpoint only during startup/recovery or after a
      peer summary proves that the local journal, finalized-item cursor, or
      trusted checkpoint frontier is behind a selected target checkpoint.

### Progress and verification

The live finalized-unit path reuses the durable journal/application frontier
and the consensus complete-unit watermarks as its dirty/eligible frontier; it
does not maintain a parallel cache. After one finalized unit is durably
persisted and fully applied, while the existing operation-application mutex
still excludes later finalization, it checks those bounded rolling frontiers
against the latest scheduled quorum. The first eligible complete boundary is
therefore audited and persisted before a later unit can advance state. A
pending scheduled sequence suppresses later unit events, so the expensive audit
and local signature are not repeated while quorum exchange is pending.
Duplicate finalized-unit retry follows the same hook. A single startup-only
recovery call closes the crash window between durable application and candidate
persistence; status, signature, and peer-sync polling no longer re-enter
checkpoint production.

Authenticated private REST now exposes four bounded checkpoint message classes:
scalar summary, exact-ID fetch, exact-ID local-signature request, and assembled
quorum publication. They use the existing signed envelope, request admission,
kind-isolated replay windows, peer timeout, and reconnect selection. Signature
requests do not contain a checkpoint payload. The receiver returns only its
already persisted signature for that exact locally derived checkpoint ID.

The peer-sync round compares scalar summaries, fetches an advertised assembled
quorum from one holder, requests only missing signatures for an identical local
scheduled candidate, promotes once threshold is reached, and publishes the
stored proof only to peers that do not report it. An ahead quorum is verified
and stored without being promoted before the local durable frontier reaches it.
Unchanged rounds therefore perform no checkpoint signing, audit, quorum write,
full-proof transfer, or stored-proof verification.

Recovery checkpoints now reuse the same candidate, signature, quorum,
promotion, fetch, and dissemination path. A member that is not healthy requests
recovery through its authenticated scalar-summary request. A reachable healthy
member with a live session may propose a versioned `FederationRecoveryBarrier`
bound to the federation, latest scheduled anchor, and its current finalized
cursor. The barrier carries no caller-provided boundary, rolling digest,
application result, or materialized-state claim. AlephBFT orders and durably
persists it like any other consensus item; only after every operation in the
complete unit reaches a terminal application state does each member derive the
same boundary locally and perform the existing audit/candidate/signature work.

Admission permits at most one queued or awaiting-finality barrier per runtime.
A 30-second local proposal lease limits request amplification, while a
deterministic 64-finalized-item minimum advance reuses a recent checkpoint and
keeps the uncovered live tail bounded by one maximum-sized unit. Concurrent
proposals therefore converge to one effective candidate boundary or harmless
duplicates. A stale scheduled anchor requests a replacement barrier. Startup
replays an accepted barrier from the last complete durable unit if candidate
persistence was interrupted. The former two-observation quiet-tip latch and
peer-summary boundary matching are deleted, so sustained finalization no longer
starves recovery.

Focused runtime regressions now prove that the interval-crossing unit is the
captured scheduled boundary, later finalization cannot move a pending
candidate, and startup recovery creates only one signature for durable eligible
progress whose ordinary capture was interrupted. A twelve-round stable-poll
regression also proves zero checkpoint work-counter growth, zero journal
catch-up requests, and no debug-or-higher log output.

Verified with:

- `cargo test -p cdk-common --features federation
  aleph_bft_finalization_handler_emits_ordered_items_with_unit_metadata --lib`;
- `cargo test -p cdk-common --features federation
  consensus_runtime_recovers_from_finalized_items --lib`;
- `cargo test -p cdk-sqlite --features federation federation_checkpoint_persistence`;
- `cargo clippy -p cdk-common --lib --features federation -- -D warnings`;
- `cargo clippy -p cdk-sql-common --lib --features federation -- -D warnings`;
- `cargo check -p cdk-axum --features federation`;
- `cargo test -p cdk-common --features federation
  checkpoint_transport_messages_are_bound_and_bounded --lib`;
- `cargo test -p cdk-common --features federation
  transported_complete_boundary_requires_the_exact_roster --lib`;
- `cargo test -p cdk-axum --features federation recovery_barrier --lib`;
- `cargo test -p cdk-axum --features federation
  concurrent_recovery_requests_queue_one_effective_barrier --lib`;
- `cargo test -p cdk-axum --features federation
  restart_retries_last_ordered_barrier_with_the_same_checkpoint_id --lib`;
- `cargo test -p cdk-axum --features federation
  scheduled_checkpoint_captures_first_complete_eligible_boundary --lib`;
- `cargo test -p cdk-axum --features federation
  startup_recovers_an_interrupted_eligible_checkpoint_capture_once --lib`;
- `cargo test -p cdk-axum --features federation,prometheus
  unchanged_checkpoint_polls_do_not_repeat_signature_verification --lib`.

The current `cdk-axum` clippy rerun remains blocked by existing federation
warnings outside this checkpoint slice; the checkpoint changes are covered by
the successful `cargo check` and targeted tests above.

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
- `crates/cdk/src/mint/federation/projection.rs`

### Success criteria

- Twelve unchanged sync rounds produce zero checkpoint signatures, zero
  checkpoint DB writes, zero materialized-state audits, zero full journal
  reads, zero historical signature verifications, and zero journal catch-up
  requests.
- A batch produces at most one local signature per member per checkpoint ID.
- A quorum checkpoint is eventually formed with the minimum threshold of
  available honest members.
- One member can transfer a stored assembled quorum to another member with zero
  new signatures and zero journal reads by the original signers.
- Continued finalization cannot change the contents of an already identified
  scheduled checkpoint.
- One eligible boundary performs at most one local full materialized-state audit
  per member and that audit does not load historical journal entries.
- A recovery checkpoint forms only for the locally derived complete-unit
  terminal boundary of a durably ordered barrier and is superseded by a later
  scheduled quorum that covers it.
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
- Boundary-capture tests that pause later application until the audited
  checkpoint candidate is durable, then allow signature collection without
  holding the application lock.
- Recovery-checkpoint tests for sustained traffic, concurrent requests,
  invalid future cursors, one unavailable member, restart replay,
  stored-quorum transfer, and next-scheduled derivation.

## Milestone 4: Replace range-certified catch-up with scheduled-batch and stable-tail catch-up

### Objective

Download scheduled historical batches and the post-scheduled stable tail from
one selected source while preserving reusable quorum authentication for every
frontier that becomes authoritative.

### Checklist

- [x] Extend catch-up discovery so the requester selects the next stored
      scheduled quorum checkpoint before downloading its batch.
- [x] Allow any peer holding the assembled scheduled quorum to serve it; do not
      contact or wake the original signers.
- [x] Request finalized items and compact journal entries strictly after the
      current trusted checkpoint and no farther than the target checkpoint.
- [x] Initialize rolling operation, finalized-item, and terminal-application
      digests from the trusted checkpoint and update them only across received
      suffix items.
- [x] Require every complete-unit page to remain covered by the target
      checkpoint's journal cursor/order digest, finalized cursor/finalized
      digest, session cursors, and projected application digest. Verify and
      apply one bounded page at a time under the operation-application lock;
      require exact equality across every committed frontier before promotion.
      Trust the quorum's checkpoint-time materialized-state audit together with
      transactionally consistent local frontiers; do not add a full-state scan
      to normal catch-up.
- [x] Apply each page through existing atomic persistence and business-state
      transactions while retaining the old trusted checkpoint, and promote
      only after the complete checkpoint-bounded suffix verifies. A crash may
      leave durable untrusted data ahead of the trusted checkpoint, never the
      checkpoint ahead of data; public routes remain closed and retry resumes
      from the scalar durable frontier.
- [x] Resume interrupted catch-up from the last promoted checkpoint or a later
      scalar durable frontier without replaying earlier batches.
- [x] If one source serves invalid data, discard that batch, retain the trusted
      base, back off the source, and retry another peer.
- [x] Fetch journal/finalized bodies from one healthy source. Fetch only a stored
      bounded quorum proof from another source when the first source does not
      already carry it; never fetch the same historical bodies from every
      signer.
- [x] After the newest scheduled checkpoint, use live AlephBFT or request an
      AlephBFT-ordered recovery barrier and form/fetch a recovery checkpoint
      anchored to that scheduled checkpoint.
- [x] Download the ordered recovery tail from one source, validate rolling commitments to
      the recovery checkpoint, and promote it through the same checkpoint
      path; never trust a single REST peer's uncertified tail.
- [x] Remove all request-time range certificate creation and verification from
      journal responses.

### Progress and verification

Checkpoint polling requests the exact next scheduled quorum after the locally
trusted scheduled sequence; gaps are not skipped. Exact-ID fetch transfers the
stored checkpoint and assembled quorum from any holder without contacting its
original signers. Once a target proof is stored, the sync loop requests journal
and finalized-item pages carrying that exact checkpoint ID. A body source needs
only the stored checkpoint payload, not its quorum, because the requester
already holds and verifies the transferable proof.

Checkpoint pages use bounded indexed SQL ranges and never read beyond the
target cursors. The requester holds no whole-batch aggregate: it validates,
persists, and deterministically applies one complete-unit page while the old
checkpoint remains trusted. The bounded in-memory suffix runtime carries
rolling order/finalized/application commitments between pages; fail-closed
verification clones that runtime rather than replaying every earlier page.
Durable scalar frontiers let a restart reconstruct the bounded suffix once and
request only the remaining pages. The final page must exactly equal every
checkpoint cursor, digest, and session watermark before promotion. This normal
recovery path does not run the full materialized-state audit. Promotion
immediately rebases the in-memory runtime at the new nonzero frontier, so a
subsequent scheduled or recovery batch retains only its suffix. Targeted and
ordinary journal pages never create or verify request-time range certificates;
checkpoint identity binds the targeted page and the requester separately
verifies the stored bounded checkpoint quorum.

The signed checkpoint policy now triggers a scheduled boundary when any of the
accepted-operation, finalized-item, or canonical-finalized-byte intervals is
crossed. Defaults are 10,000 / 10,000 / 32 MiB. Fixed protocol caps limit one
batch to 20,000 accepted operations, 20,000 finalized items, and 64 MiB of
canonical finalized history; requests are limited to 64 KiB, pages to 8 MiB,
and total encoded transfer work to 256 MiB. Native transports reject oversized
bodies while reading and before JSON decoding. Pages end only at complete
AlephBFT units, with compact journal claims aligned to the accepted items.

Checkpoint observability records fetched/applied pages, encoded bytes,
staged/applied bytes, retries, and peak encoded in-flight page size. Operator
auditing remains explicit only: catch-up, startup, and background sync do not
invoke a full materialized-state audit.

Coverage now includes exact next-checkpoint lookup, checkpoint transport
serialization, pre-decode response limits, complete-unit byte trimming, paged
serving from a source that stores only the checkpoint payload, tamper rejection
before mutation, source failover, crash/resume after every durable page,
one-page/many-page equivalence, catch-up from genesis, and a second checkpoint
suffix after rebasing. Compilation and linting were verified with:

- `cargo test -p cdk-axum --features federation --lib --no-run`;
- `cargo test -p cdk-sqlite --features federation --lib --no-run`;
- `cargo check -p cdk-axum --features federation`.
- `cargo clippy -p cdk-common --lib --features federation -- -D warnings`.
- `cargo check --workspace --all-targets`;
- `cargo clippy --workspace --all-targets -- -D warnings`.

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
  finalized status, altered terminal application result, truncated unit, wrong
  session, wrong application digest, and wrong state-audit-digest tests.
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

Eliminate full-history CDK/AlephBFT finalized-item and journal reconstruction
when a valid local scheduled or recovery checkpoint exists, while restoring
only the independently bounded current AlephBFT live-session backup.

### Checklist

- [x] Add base-aware constructors for `InMemoryFederationJournal`, the
      finalization handler, and `FederationConsensusRuntime` using checkpoint
      cursors/digests.
- [x] Represent runtime counts as checkpoint base plus retained suffix; status
      must not confuse suffix vector length with total federation height.
- [x] Load journal and finalized-item rows with `..._from` queries beginning at
      the checkpoint cursors.
- [x] Stop calling the full-history
      `authenticated_aleph_bft_from_finalized_items` path when a checkpoint fast
      path is available.
- [x] Move old-operation duplicate/idempotency and semantic-conflict checks to
      indexed durable lookups where unbounded historical in-memory sets were
      previously required.
- [x] Reject replayed pre-checkpoint finalized units using signed cursor/session
      watermarks rather than a set reconstructed from every historical unit.
- [x] Install the signed per-member next-unit indexes as lower replay watermarks
      for every configured member, restore the local member's data-provider
      index from its entry, and restore pending local operations through the
      existing pending-operation table.
- [x] Validate only the local suffix against the checkpoint's operation,
      finalized-item, and application-digest bases. Require terminal application
      state and exact equality with the scalar atomic database frontier; do not
      reapply an already materialized local suffix.
- [x] Do not run the full materialized-state audit on the normal startup fast
      path. Reject inconsistent checkpoint/frontier metadata and leave arbitrary
      row-corruption detection to the explicit operator audit.
- [x] Start from a checkpoint at any complete finalized-unit boundary; do not
      require or force an AlephBFT session boundary.
- [x] Load only the backup for the active AlephBFT session. If the local backup
      is missing or stale, reuse authenticated private-REST backup recovery from
      a peer whose session and durable target frontier match, with size bounds,
      backoff, and source failover.
- [x] Ensure invalid or mismatched backup bytes cannot advance authoritative
      journal/application state. Validate newly finalized payloads against the
      checkpoint base and suffix through the normal CDK finalization path.
- [x] Keep current-session backup work bounded by hard round/chunk/byte limits
      and session rotation; never load or transfer every historical session
      backup during startup.
- [x] Define crash-safe pruning for obsolete session backups after a newer
      session is durably active and retained checkpoint/finalized evidence covers
      the older session.
- [x] Fall back fail-closed to genesis/full recovery only for an empty federation
      or an explicitly supported database with no checkpoint; do not silently
      trust an invalid checkpoint.

Startup now loads and verifies the single trusted quorum plus the atomic scalar
frontier before issuing any history query. Exact agreement constructs an empty
retained suffix rooted at the checkpoint digests and signed session/member
cursors. When the durable frontier is later than the checkpoint, startup loads
only journal and finalized-item rows at or after the checkpoint cursors,
validates their order, finalized-item, and terminal-application digest chains
against the exact scalar frontier, and constructs the runtime from that bounded
suffix without reapplying already materialized rows. Inconsistent or missing
trusted metadata and invalid suffixes fail closed; only a database with no
trusted checkpoint uses full recovery. Pending-operation restoration uses the
indexed durable operation lookup so a checkpointed operation is not requeued.
Live finalization uses the same operation-ID lookup plus a normalized durable
conflict-key index. It hydrates only historical entries referenced by the next
bounded finalized unit into the existing journal/finalizer decision logic, so
old duplicates and semantic conflicts remain deterministic without rebuilding
an unbounded prefix set. Live-session recovery probes and loads only the active
session. Scalar backup metadata is checked before blobs are read, chunks are
loaded in bounded pages, peer recovery and local replay/writes share hard byte
and chunk limits, and the existing AlephBFT round/session rotation remains the
time bound. Backup bytes remain opaque non-authoritative input: every emitted
payload still passes through the checkpoint-based finalizer and ordinary atomic
journal/application path. After checkpoint and suffix validation succeeds,
advancing the active-session marker and pruning strictly older backups occur in
one transaction; no-checkpoint startup retains the legacy backups needed for
full recovery.

Verified with:

- `cargo test -p cdk-common --features federation
  journal_continues_from_a_trusted_base_without_retaining_the_prefix --lib`;
- `cargo test -p cdk-common --features federation
  checkpoint_journal_hydrates_only_indexed_historical_decisions --lib`;
- `cargo test -p cdk-common --features federation
  consensus_runtime_starts_at_a_trusted_checkpoint_without_prefix_rows --lib`;
- `cargo test -p cdk-axum --features federation
  federation_runtime_startup_at_trusted_checkpoint_loads_no_prefix_rows --lib`
  (exact frontier and one-row post-checkpoint suffix);
- `cargo test -p cdk-sqlite --features federation
  federation_aleph_bft_backup_persistence --lib`;
- `cargo test -p cdk-axum --features federation
  cdk_aleph_bft_db_backup_writer_enforces_session_bounds --lib`;
- `cargo test -p cdk-axum --features federation
  cdk_aleph_bft_session_io_preserves_identical_backup_chunks --lib`;
- `cargo test -p cdk-common --features federation --lib`;
- `cargo check -p cdk-axum --features federation`;
- `cargo clippy -p cdk-common --lib --features federation -- -D warnings`.

### Files and references

- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/consensus/tests.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/federation/journal.rs`
- `crates/cdk-sql-common/src/mint/migrations/{sqlite,postgres}/20260712000000_create_federation_schema.sql`
- `crates/cdk-axum/src/lib.rs` (`create_federation_runtime_state`)
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/checkpoint_manager.rs`
- `crates/cdk-axum/src/federation/checkpoint_catch_up.rs`
- `crates/cdk-axum/src/federation/operation_pipeline.rs`
- `crates/cdk-axum/src/federation/runtime_components.rs`
- `crates/cdk-axum/src/federation/dkg_persistence.rs`
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs`
- `crates/cdk-axum/src/federation/catch_up_routes.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-axum/src/custom_handlers.rs`
- `crates/cdk-axum/src/federation/tests.rs`

### Success criteria

- At an exact trusted checkpoint, startup loads no pre-checkpoint journal or
  finalized-item bodies, performs no full materialized-state audit, and verifies
  only one bounded checkpoint quorum.
- With a suffix of S items and active-session backup size B, startup work is
  O(S + B), independent of checkpoint height and federation lifetime.
- Old operation retries remain idempotent and old finalized-unit replays remain
  rejected.
- Partial writes or inconsistent journal/finalized/application/checkpoint
  frontier metadata cannot result in Healthy readiness. Arbitrary authoritative
  row corruption is detected by the explicit operator audit and durably latches
  the member unhealthy.
- Session rotation, local unit indexes, pending operations, and response replay
  continue from the correct nonzero cursors.

### Test conditions

- Synthetic equal-tail startup at checkpoint heights 1,000, 10,000, and 100,000
  with assertions on rows loaded and signatures verified, not only elapsed time.
- Startup with suffix lengths 0, 1, batch-size minus 1, and multiple batches.
- Startup with identical suffix and active-session backup sizes at historical
  heights 1,000, 10,000, and 100,000.
- Restart immediately before and after checkpoint promotion.
- Restart from both a scheduled checkpoint and a later recovery checkpoint
  anchored to it.
- Restart all members simultaneously from matching checkpoints.
- Restart with checkpoint signature corrupted, checkpoint missing, application
  digest altered, or a database cursor ahead/behind the checkpoint.
- Run the operator audit with one member's authoritative materialized row
  altered and require a durable fail-closed latch across restart.
- Restart from a checkpoint in the middle of an AlephBFT session with a valid
  local backup, a missing backup recovered from a peer, and an invalid first
  source followed by a valid second source.
- Restart after session rotation and assert no historical-session backup is
  loaded.
- Replay a pre-checkpoint operation ID and finalized unit from every configured
  member after restart.

## Milestone 6: Make readiness and observability checkpoint-native

### Objective

Remove certificate reconstruction from health/status paths and expose the
trusted checkpoint frontier as scalar runtime state.

### Checklist

- [x] Replace `VerifyingCatchUpCertificate` with `AwaitingQuorum`; keep the
      readiness state small and expose trusted/target checkpoint cursors through
      the scalar checkpoint status instead of duplicating them in the enum.
- [x] Replace certificate error variants with checkpoint mismatch, insufficient
      quorum, unavailable checkpoint, and checkpoint-tail catch-up errors.
- [x] Replace local status certificate fields with latest local scheduled
      checkpoint, latest scheduled quorum, latest recovery quorum, latest
      trusted frontier, checkpoint kind/lag, application digest, last audited
      frontier, durable audit-mismatch latch, and signer count/IDs.
- [x] Read status fields from in-memory/durable scalar frontier state without
      scanning journal or historical signature rows; loading the one bounded
      stored quorum is independent of federation history length.
- [x] Replace the certificate Prometheus gauge with a checkpoint-lag gauge.
- [x] Add scheduled/recovery checkpoint creation, stored-quorum transfer,
      promotion, catch-up batch, signature-verification, and startup-tail
      counters/timers needed for milestone-8 measurements.
- [x] Rename the mintd status guard to
      `--require-checkpoint-quorum` and update digest comparison output.
- [x] Add an explicit mintd operator materialized-state audit command. For the
      simplest safe version 1, require the current durable frontier to equal an
      already trusted checkpoint, verify that stored quorum, recompute the full
      authoritative-state digest, compare it with the signed digest, and durably
      latch the member fail-closed on mismatch. Do not form or fetch a new
      checkpoint as part of the audit command.
- [x] Do not schedule that operator audit automatically and do not invoke it
      from normal startup, status, readiness polling, or the peer-sync loop.
- [x] Update smoke scripts to wait for checkpoint coverage only at explicit
      recovery/checkpoint gates, not after every lightweight operation.
- [x] Keep logs transition-based and bounded; unchanged checkpoint polls must
      remain debug-silent.

### Progress and verification

Ordinary status now reads the scalar checkpoint frontier and bounded newest
checkpoint/quorum records. It performs no journal projection and no signature
verification. Peer convergence compares the rolling application digest and the
checkpoint ID that owns any reported full-state audit digest. The former
certificate coverage gauge and mintd guard have become checkpoint lag and
`--require-checkpoint-quorum`, and the local federation smoke scripts use those
checkpoint-native gates. The guard preserves the former coverage semantics: a
nonempty member must report enough distinct configured signers and the trusted
checkpoint cursor must cover its current operation frontier.

`cdk-mintd federation audit-checkpoint` is deliberately operator-only. The
private signed route accepts no caller-selected checkpoint: it verifies the
already trusted local quorum, requires the current scalar frontier to equal that
checkpoint exactly, then performs the expensive direct authoritative-state
projection. A mismatch commits the one-way durable latch and halts the current
AlephBFT session. No startup, polling, readiness, or background path calls this
audit.

Checkpoint-native observability now reuses the two bounded work/duration
collectors described in milestone 0. Quorum verification is wrapped once in
the Axum federation layer so startup, exchange, catch-up, audit, and promotion
report the actual number of signatures verified without changing the common
checkpoint protocol. Stored proof transfer is measured for both fetch and
publication, and the startup fast path separately identifies full-history and
post-checkpoint suffix rows.

Stable polling now treats locally stored proofs according to their trust
boundary: startup verifies the trusted proof, remote ingress verifies fetched
or published proofs, and catch-up/promotion verify before state changes; scalar
summary and fetch serving do not repeat that cryptographic work. Scheduled
candidate recovery runs once at startup and otherwise only from durable
finalized-unit application. The unconditional successful-round debug message
was removed. Twelve unchanged checkpoint summary/fetch/exchange rounds are
covered by a scoped log-capture and work-counter regression.

The post-implementation consolidation now makes these ownership boundaries
structural. `FederationNodeRuntime` fell from 2,834 to 562 lines and primarily
composes lifecycle handles. Checkpoint capture/proof/promotion/audit, bounded
catch-up verification/application, operation ordering/application/publication,
readiness/recovery state, DKG persistence, and wallet/FROST reconciliation live
behind focused modules and interfaces. Network page acquisition and peer
failover complete outside the application and consensus mutation locks; a
blocked-transport test proves both locks remain available while a peer request
is in flight. The acquisition order and forbidden responsibilities are recorded
in the module documentation, and the audit route delegates its fail-closed
decision instead of implementing business state in the handler.

Current cut-through verification completed:

- `cargo fmt --all -- --check`;
- shell syntax checks for both federation smoke templates;
- `git diff --check`;
- production and fresh-schema searches proving that no certificate type,
  field, table, metric, CLI flag, or configuration name remains.

Final post-cut-through type checks now pass for `cdk-common` and `cdk-axum`
with federation enabled and for `cdk-mintd` with `federation,sqlite` enabled.
The earlier stale certificate-era digest-retry fixture fields have been
removed; no compiler rerun remains pending for those changes.

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
- `crates/cdk/src/mint/federation/projection.rs`
- `crates/cdk-mintd/dev/federation/smoke-local-federation.sh.in`

### Success criteria

- A status request is O(1) with respect to history and performs zero signature
  verification.
- Idle status and sync polling produce no repetitive warning/info logs.
- Operators can distinguish local tip, latest scheduled checkpoint, recovery
  checkpoint, trusted/target frontier, application digest, suffix length,
  current-session backup state, last operator-audited frontier, durable audit
  mismatch, and whether live AlephBFT owns the remaining tail.
- An operator audit either verifies one exact signed checkpoint frontier or
  fails clearly because an exact stable frontier/quorum is unavailable; it
  never reports success from a moving or merely local tip.
- Existing alerting semantics remain fail-closed under checkpoint mismatch.

### Test conditions

- Status serialization and CLI formatting tests for empty, exact scheduled
  checkpoint, recovery checkpoint, suffix, catching-up, mismatch, and halted
  states.
- Metrics tests proving gauges are replaced rather than duplicated.
- Repeated status request test asserting no database history read or signature
  verification.
- Operator-audit tests for exact scheduled frontier, exact recovery frontier,
  moving tip, local row mismatch, insufficient quorum, durable mismatch latch,
  and restart after a successful audit.
- Log-capture test asserting stable idle rounds do not emit repeated recovery
  transitions.

## Milestone 7: Delete the catch-up-certificate implementation cut-through

### Objective

Finish with one recovery-evidence model and no dual wire, database, status, or
configuration behavior.

### Checklist

- [x] Delete `FederationCatchUpCertificate`,
      `FederationCatchUpCertificateQuorum`, transcript construction, range-key
      grouping, and range quorum verification.
- [x] Remove `catch_up_certificate` from
      `FederationJournalCatchUpResponse` and delete response signing.
- [x] Delete `catch_up_certificate_quorum_status`, contiguous-prefix rebuilding,
      its cache/invalidation plumbing, and certificate collection functions.
- [x] Delete certificate database methods, codecs, table/index names, and tests.
- [x] Confirm both scheduled and recovery policies use only
      `FederationCheckpointQuorum`; do not retain a renamed range-certificate
      compatibility path.
- [x] Delete certificate readiness/error/status/metric/CLI/config fields and all
      compatibility aliases.
- [x] Update current architecture, implementation, security-review, stress-test,
      and operator documentation to describe checkpoints.
- [x] Squash the checkpoint schema into the existing initial federation
      migrations; no existing federation database or upgrade bridge is
      supported after the cut-through change.
- [x] Regenerate federation development databases/configs before deployment.
- [x] Use `rg` to prove no production references to catch-up certificates remain;
      historical review text may retain clearly marked provenance only.

### Progress and verification

The range-certificate types, signature transcripts, quorum reconstruction,
runtime cache/collection path, database APIs and schema, response fields,
readiness state, status/CLI/config fields, and metrics have been deleted.
Journal serving is a bounded indexed range read for both targeted checkpoint
catch-up and the remaining non-target backup request. Scheduled and recovery
frontiers now share only `FederationCheckpointQuorum` and its persistence and
promotion path.

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

- [x] Run the full restart matrix: one member, threshold members, all members,
      clean shutdown, crash before checkpoint, crash during promotion, and long
      offline interval.
- [x] Run partition tests below and above consensus threshold and heal them.
- [x] Run scheduled-boundary agreement tests where the journal interval is
      crossed mid-unit and where only finalized-item progress triggers the
      checkpoint.
- [x] Run stable-tail recovery checkpoint tests with exact agreement, a moving
      tip, a stale recovery frontier, and a later scheduled checkpoint that
      supersedes it.
- [x] Run malicious source/checkpoint-signature tests from milestone 4 in the
      real multi-member harness.
- [x] Measure idle CPU and work counters for at least ten minutes with a long
      synthetic history.
- [x] Measure cold start at multiple history sizes with suffix zero and a fixed
      suffix and fixed active-session backup size; fit/report whether work
      scales with history, suffix, or current-session backup.
- [x] Repeat the federation receive stress matrix and compare confirmed TPS,
      journal TPS, latency, CPU, log rate, catch-up transitions, and checkpoint
      traffic with milestone 0.
- [x] Redeploy a fresh development federation and exercise member restart from
      scheduled and recovery checkpoints after creating several batches.
- [x] Recover a long-offline member using checkpoint quorums fetched from one
      peer and assert that historical signers perform zero new signature or
      journal-scan work.
- [x] Run the explicit operator materialized-state audit against matching and
      altered members; confirm that it is never invoked by normal startup,
      status, or peer sync.
- [x] Record aggregate findings without credentials, private URLs, machine
      identifiers, private keys, database copies, or bearer tokens.

### Local performance evidence

The Milestone 0 file-backed SQLite benchmark now reuses one synthetic history
to measure three restart shapes at 1,000, 10,000, and 100,000 finalized
operations:

1. an exact trusted checkpoint with no AlephBFT backup;
2. the same exact checkpoint with four active-session backup chunks totaling
   4,096 bytes;
3. a fixed 64-operation journal/finalized suffix with the same four chunks.

The timed backup cases reconstruct the federation runtime and then invoke the
same `CdkAlephBftSessionIo::from_db` path used to resume the live AlephBFT
session. The suffix uses the same consensus and durable transaction fixture as
the prefix; there is no benchmark-only row format or alternate startup loader.
One release run produced:

| Historical prefix | Exact, no backup | Exact + 4 KiB backup | 64-op suffix + 4 KiB backup |
| ---: | ---: | ---: | ---: |
| 1,000 | 8.428 ms | 10.268 ms | 11.479 ms |
| 10,000 | 8.422 ms | 10.460 ms | 11.663 ms |
| 100,000 | 8.434 ms | 10.148 ms | 11.437 ms |

At every history size, exact startup loaded zero journal/finalized rows; the
fixed-tail case loaded exactly 64 journal rows and 64 finalized items. Both
backup cases loaded exactly 4,096 bytes in four chunks. Every case verified the
same two checkpoint signatures, replayed zero pre-checkpoint entries, and
performed zero materialized-state audits. Exact-plus-backup timing varied by
0.312 ms across the 100x history range, while fixed-tail timing varied by
0.226 ms. The observed work therefore scales with suffix `S` plus current
session backup `B`, not retained history `H`.

The benchmark is reproducible with:

```bash
cargo test --release -p cdk-axum --features federation,prometheus \
  benchmark_checkpoint_startup_is_history_length_independent -- \
  --ignored --nocapture
```

Raw compiler and test output remains outside the repository; only these
payload-free aggregates are retained here.

### Local ten-minute idle evidence

The ignored release idle benchmark now reuses the same synthetic-history
builder as the cold-start benchmark. It creates and retains 100,000 genuine
finalized operations, promotes an exact two-signature recovery checkpoint,
restarts with zero journal or finalized prefix rows in memory, and performs 120
ordinary checkpoint-summary, checkpoint-fetch, and peer-exchange rounds at the
configured five-second cadence.

Two runs measured idle windows of 601.924 and 602.498 seconds. Both reported
zero checkpoint-work growth, zero signature verification, zero
materialized-state audits, zero catch-up requests, and zero emitted debug-or-
higher logs. The OS-timed run completed end to end in 628.19 seconds with 27.46
seconds of user CPU and 0.16 seconds of system CPU. These CPU totals include the
one-time 100,000-operation fixture construction and are therefore a
conservative upper bound for the idle window; the counter and log deltas isolate
the idle window itself. The benchmark is reproducible with:

```bash
cargo test --release -p cdk-axum --features federation,prometheus \
  benchmark_unchanged_checkpoint_idle_rounds -- --ignored --nocapture
```

### Local scheduled-boundary evidence

Two independent member runtimes now apply the same complete finalized unit and
prove that they derive byte-identical scheduled candidates, matching full-state
audit digests, distinct stored member signatures, and a valid assembled quorum.
The journal-triggered case uses a three-operation unit with a journal interval
of two: eligibility is crossed inside the unit, but both members checkpoint the
complete `journal=3, finalized=3` boundary. The finalized-triggered case uses
one operation twice with a journal interval of 100 and finalized-item interval
of two: the duplicate advances only finalized order, and both members derive
the same `journal=1, finalized=2` checkpoint. In both cases the next unit
finalization cursor is one, so no checkpoint can expose a partial unit.

The two agreement tests, the existing first-eligible-boundary retention test,
and the one-shot startup recovery test for an interrupted eligible capture pass
with federation and Prometheus enabled.

### Local ordered-tail recovery evidence

Two independent member runtimes apply the same complete finalized unit
containing normal traffic and a recovery barrier, derive a byte-identical
recovery checkpoint, and assemble distinct member signatures into a valid
quorum. The candidate commits to the terminal boundary after the entire unit,
not the barrier's position inside it. A separate traffic test places operations
on both sides of the barrier, proves both reach terminal state in order, and
then finalizes another operation successfully.

The barrier generation is domain-separated, federation-bound, and tamper
evident. A future requested cursor is durably rejected without rejecting the
rest of the unit. Concurrent local requests queue one effective barrier; a
30-second proposal lease and deterministic one-unit uncovered-tail bound
suppress repeated work. A one-unavailable-member test still forms a threshold
quorum, while a minority candidate never advances the trusted frontier. An
interrupted post-finality capture reconstructs consensus from durable state on
restart and derives the same checkpoint ID. Idle state proposes no barrier.

The previously verified monotonic-lineage test rejects a stale recovery
frontier and accepts the next scheduled checkpoint with or without an
intervening recovery. The shared database persistence test proves that
promoting a covering scheduled quorum removes its superseded recovery
checkpoint, quorum, and signatures while retaining scheduled history.

### Local restart and partition evidence

The existing five-member live AlephBFT harness now retains each member's test
database and seed so one shared restart helper can reopen the same durable
state. Three release tests restart one member, the four-member consensus
threshold, and all five members. Each case finalizes an operation, waits for
the current-session backup, cleanly stops the selected supervisors, rebuilds
the selected runtime states from their existing databases, and proves new
finality. Every member ends with the same journal projection and authoritative
materialized-state digest.

The remainder of the crash matrix stays in the narrower existing tests. The
one-shot startup recovery test simulates interruption after a boundary becomes
eligible but before its checkpoint candidate is captured. The shared SQLite
checkpoint persistence test rolls back candidate/quorum writes before
promotion and proves that neither the checkpoint nor retention side effects
escape the transaction. The single-source recovery test covers the long-offline
case. All of these focused release tests pass.

The router transport fault registry now also exposes one bidirectional
cross-group partition operation, reusing the same link filter already used by
the DKG and AlephBFT harnesses. In a 3-vs-2 split, the below-consensus side's
submission times out with no durable journal mutation; after healing, the same
operation finalizes. In a 4-vs-1 split, the consensus side finalizes while the
isolated member remains unchanged; after healing, that member catches up. Both
release tests finish with identical journal projections and materialized-state
digests on all five members.

### Local Byzantine catch-up evidence

The checkpoint catch-up harness uses independent source and target member
runtimes with separate databases and the production checkpoint application
path. Before accepting the source's bounded batch, the target now rejects a
quorum containing a corrupted member signature, a correctly signed but
below-threshold quorum, and a source response with a tampered operation
commitment. After all three failures, the target remains at its genesis
frontier with no materialized quote. The same target then accepts the original
quorum and honest response, promotes the exact checkpoint, and advances through
a second checkpoint suffix.

This keeps signature, threshold, response-binding, rolling-commitment,
application, and promotion checks in one end-to-end member-runtime test. The
focused test passes with federation enabled; lower-level mutation permutations
remain centralized in the checkpoint and transport modules.

### Local single-source offline-recovery evidence

A seven-member BFT harness now separates every recovery role. Members one
through five are the historical checkpoint signers, member six holds the
assembled quorum and all finalized bodies, and member seven starts from genesis
as the long-offline target. The target is configured with only member six's
real private federation router. Its first discovery pass fetches and persists
the stored five-signature quorum; its second pass downloads the three-operation
batch and bounded current-session backup from the same source, validates the
rolling commitments, and promotes the exact recovery checkpoint.

After promotion, all three quotes are materialized, the authenticated runtime
reports all three accepted operations, and no journal or finalized prefix rows
remain in memory. Aggregate checkpoint-work counters for each of the five
historical signers are snapshotted after quorum creation and remain unchanged
through recovery. The test therefore proves one-source body/proof transfer with
zero recovery-time signature or journal-scan work by the original signers. It
does not mark the member healthy from one source; the separate quorum-readiness
gate remains intact. The focused release test passes with federation and
Prometheus enabled.

### Local operator-audit evidence

The explicit operator path succeeds when the materialized state matches the
exact trusted checkpoint. An out-of-band quote mutation produces a different
digest, fails the audit, durably latches the mismatch, and makes the next audit
fail closed without rescanning. Both focused operator tests pass.

Normal paths remain audit-free. Every exact and fixed-tail startup shape in the
history-scaling benchmark records zero materialized-state audit samples. The
twelve-round unchanged-member test now also snapshots the audit-duration
counter around local status, checkpoint fetch, and peer checkpoint exchange;
all twelve rounds leave it unchanged alongside zero checkpoint work, signature
verification, catch-up requests, and emitted logs. The normal startup, status,
and peer-sync paths therefore validate bounded checkpoint metadata but never
invoke the full-state operator audit.

### Development stress instrumentation

The DKG roster command now accepts journal-operation and finalized-item
checkpoint intervals and constructs the shared `FederationCheckpointPolicy`
through the existing domain validator. Its production defaults remain 10,000.
The generated local harness uses 20 journal operations and 100 finalized items:
ordinary operation load crosses several scheduled boundaries, while duplicate
finalization alone does not make the small functional smoke checkpoint too
early. The Nocean development helper retains its independently configurable
20/20 defaults. Zero is rejected by the shared policy constructor.

The Nocean runtime exposes each member's Prometheus endpoint only on loopback.
One deployment-control command returns an aggregate-only CSV row per member
containing cumulative process CPU, RSS, daemon log lines, lagging/catch-up
transitions, and fixed checkpoint-work counters. It reads the existing rotating
daemon logs and Prometheus metrics rather than adding a second logging or
telemetry path.

The receive benchmark snapshots those rows immediately before load and after
post-load convergence. It rejects missing members, process replacement, and
counter regression, retains per-member deltas, and appends aggregate CPU, RSS,
log-rate, sync-transition, scheduled/recovery checkpoint, quorum-transfer,
promotion, catch-up-batch, and signature-verification fields to each throughput
result. Its local mode reuses generated public/private config, PID, log, and
loopback Prometheus files without SSH or SCP and keeps the same result schema as
the remote mode. A local fixture self-test proves the delta and rate
calculation. Shell syntax checks and a release `cdk-mintd` build with the exact
federation/SQLite/Prometheus feature set pass.

The fresh local development deployment used five SQLite members with a
three-member consensus threshold and the fake-wallet payment backend. The
release smoke passed mint, swap, successful and failed melt, restore,
check-state, crash restart, a four-batch long-offline interval, checkpoint
catch-up, and keyset rotation. Signed status then proved a member restart after
137 operations with all five members healthy, converged, checkpoint-covered,
and running the live AlephBFT session. The run exercised a scheduled checkpoint
as the restart base and the same canonical recovery checkpoint path at the
stable post-restart tip.

The live run exposed three bounded-path edge cases that are now regression
tested. Genesis readiness no longer requires an operator materialized-state
audit that does not yet exist. A healthy member with a live AlephBFT session no
longer re-enters startup quorum reconciliation because sequential peer status
snapshots observe a moving tip. Finally, checkpoint suffix reconstruction and
current-session backup replay recognize pre-checkpoint duplicates through
indexed operation-ID/conflict-key lookups for only the retained suffix or
finalized unit. Neither fix scans or replays the checkpointed prefix.

### Local receive-matrix evidence

The release benchmark repeated the milestone-0 `2:1`, `4:2`, `10:1`, `20:5`,
`40:10`, and `20:20` cases. All 96 receives succeeded and every timed journal
delta exactly matched successful wallet confirmations. The small smoke cases
produced 0.430 and 0.776 confirmed/journal TPS with p95 latency of 2.107 and
2.522 seconds, respectively. The measured stress cases were:

| N | Parallelism | Success | Confirmed/journal TPS | p95 | Aggregate member CPU | New warning/error logs | Sync transitions | Scheduled candidates |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 1 | 10/10 | 0.400 / 0.400 | 2.540 s | 45.74 s | 0 | 0 | 5 |
| 20 | 5 | 20/20 | 1.389 / 1.389 | 4.665 s | 63.36 s | 0 | 0 | 5 |
| 40 | 10 | 40/40 | 2.620 / 2.620 | 4.043 s | 78.64 s | 0 | 0 | 10 |
| 20 | 20 | 20/20 | 4.944 / 4.944 | 4.028 s | 15.04 s | 0 | 0 | 5 |

Across the 90 measured stress receives, the timed windows observed 25 member
candidate creations, representing five complete scheduled boundaries, with no
recovery checkpoint, catch-up batch, readiness transition, or new warning/error
log during load. Quorum transfer and promotion counters advanced during the
multi-wave cases, and signed post-run status confirmed the latest scheduled
quorum on every member. Before the ordered-barrier cut-through, a later
stable-tip recovery quorum also assembled without disturbing readiness; the
ordered replacement is covered by the focused sustained-traffic evidence
above and still needs a repeated full local smoke in final validation.

The exact milestone-0 matrix had 10:1, 20:5, 40:10, and 20:20 confirmed TPS of
0.119, 0.275, 0.270, and 0.566, with 20:5 and 40:10 client failure rates of 30%
and 47.5%. The local checkpoint run therefore shows no throughput regression,
removes the confirmed-versus-committed gap, and sharply reduces p95 latency for
the same workload shapes. Because milestone 0 ran on a remote deployment while
this repetition intentionally used faster localhost transport, the magnitude
of the improvement is not attributed to checkpoints alone. CPU/checkpoint
counters also have no like-for-like milestone-0 telemetry baseline; their
value here is that checkpoint work is explicit and no catch-up or log storm
appears during the measured windows. Raw wallet databases, tokens, logs, and
per-request artifacts remain outside the repository.

### Files and references

- `crates/cdk-axum/src/federation/tests.rs`
- `crates/cdk-axum/src/federation/tests/aleph_bft_routes.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/trusted_dealer.rs`
- `crates/cdk-mintd/dev/federation/smoke-local-federation.sh.in`
- `crates/cdk-mintd/src/cli.rs`
- `crates/cdk-mintd/src/federation_dkg_setup.rs`
- `crates/cdk-mintd/src/federation_setup/tests.rs`
- `justfile`
- `scripts/benchmark_nocean_federation.sh`
- `scripts/nocean_federation_control.sh`
- `scripts/reproduce_nocean_federation_stress_test.sh`
- `scripts/deploy_nocean_federation.sh`
- `docs/federated-cashu-nocean-stress-test.md`
- this implementation plan

### Success criteria

- Idle sync/status polling performs zero history scans and zero checkpoint
  signature verification.
- With zero suffix and fixed active-session backup size B, rows/backup bytes
  loaded and signatures verified at startup are constant as historical height
  increases, and startup performs zero full materialized-state audits.
- With fixed suffix S and active-session backup size B, startup work remains
  constant as historical height increases and grows only with S + B.
- A restarted member catches up from its trusted checkpoint and reaches the
  consensus tip without replaying the prefix before that checkpoint.
- A member can download every scheduled historical batch from one source and
  authenticate each with a previously assembled transferable quorum; the
  original signers do no recovery-time work.
- A post-scheduled tail becomes authoritative only through live AlephBFT or one
  recovery checkpoint quorum derived from an ordered complete-unit barrier.
- Current-session backup loading remains bounded independently of federation
  lifetime; startup never loads historical-session backups.
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
validate/catch up only the suffix, restore only the bounded current AlephBFT
session backup, and become healthy without loading or verifying the
pre-checkpoint history or recomputing the full materialized state. The member
trusts only transactionally consistent local frontier metadata on this fast
path. An explicit operator audit can recompute authoritative materialized state
at an exact signed frontier and durably halt the member on mismatch; no
background or startup audit is part of version 1.

In steady state, scheduled checkpoint creation is a sparse consequence of
finalized progress. An unchanged five-second peer-sync or status round performs
no checkpoint cryptography, no history scan, and no journal catch-up request.
The journal endpoint is used only when startup/recovery or a newer peer summary
shows that the local journal, finalized-item cursor, or trusted checkpoint is
behind. REST catch-up transfers each scheduled batch from one selected peer and
verifies a previously assembled bounded quorum separately. The post-scheduled
tail is handled by live AlephBFT or the same checkpoint proof created once at
the complete terminal boundary of an AlephBFT-ordered recovery barrier. All old
catch-up range-certificate types, tables,
routes, status fields, metrics, configuration, and compatibility behavior have
been removed.

Most importantly, the safety argument becomes simpler: AlephBFT orders the
history; cumulative digests commit to its finalized, accepted, and terminally
applied prefixes; checkpoint-time audits commit to authoritative materialized
state; a configured quorum signs sparse durable checkpoints; the bounded
current-session backup resumes upstream AlephBFT without authorizing
application state; and recovery advances from one verified checkpoint to the
next without repeatedly re-proving genesis.
