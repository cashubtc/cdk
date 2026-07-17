# Federated Cashu checkpoint and catch-up consolidation plan

Status: in progress (milestones 0-10 complete; milestone 11 in progress)
Scope: federation-only checkpoint, catch-up, startup recovery, and related
AlephBFT runtime consolidation
Primary goal: simplify the checkpoint and catch-up implementation without
weakening any existing safety property, while fixing the crash-recovery,
resource-bound, and sustained-load liveness holes identified in the current
implementation

## Mandatory context and use of this document

Before changing code under this plan, read
`docs/federated-cashu-implementation-plan.md` in full. It is the overarching
Federated Cashu implementation plan and defines the larger product goal,
architecture boundaries, safety invariants, cut-through compatibility policy,
and prohibited shortcuts. Also read
`docs/federated-cdk-checkpoint-implementation-plan.md` to understand why the
current signed-checkpoint protocol replaced range-oriented catch-up
certificates and which performance and recovery properties must be retained.

This file is a living implementation checklist and progress record. It must be
updated as work proceeds. For every milestone:

- set its status to `not started`, `in progress`, `complete`, or `blocked`;
- record design decisions, deviations, commands run, test results, benchmarks,
  and remaining risks in that milestone's progress notes;
- complete every required checklist item and exit test before starting the next
  milestone; and
- commit all coherent changes to Git before continuing to the next milestone.

Do not carry uncommitted implementation changes across milestone boundaries.
Use conventional commit messages, keep each milestone independently reviewable,
and include its code, database changes, tests, documentation, and recorded
verification evidence in the same commit or small milestone-local commit
series. Documentation-only preparation of this plan does not mark milestone 0
complete.

## Executive summary

The current checkpoint protocol has a strong safety model. AlephBFT remains the
only ordering mechanism; quorum-signed checkpoints attest to already-finalized,
fully applied state; catch-up bodies are validated against a transferable quorum
proof; and a checkpoint is promoted only after local durable state reaches its
exact frontier. The separation between finalized-item, journal-order,
terminal-application, and materialized-state commitments is intentional and
must remain.

The problem is not that these checks exist. The problem is that the same
frontier and suffix-validation concepts are represented and implemented many
times across `cdk-common`, `cdk-axum`, the database traits, status transport,
startup, peer sync, catch-up application, and checkpoint exchange. This has
created:

- several structs containing overlapping cursor and digest fields;
- several manually maintained definitions of equality, coverage, monotonicity,
  and suffix validity;
- multiple consensus-recovery constructors for slightly different combinations
  of genesis, checkpoint, journal suffix, and finalized-item suffix;
- separate startup and network catch-up validators for the same mathematical
  statement;
- an oversized node runtime that mixes operation submission, DKG, readiness,
  checkpoint management, catch-up, application, wallet reconciliation, and
  test controls;
- an overloaded catch-up response that transports journal history, finalized
  history, checkpoint metadata, and AlephBFT backup pages; and
- safety-motivated clone-and-buffer behavior whose resource costs are not
  adequately bounded.

The consolidation target is one canonical committed-frontier model, one shared
incremental suffix verifier, one recovery entry point, and focused runtime
components with explicit ownership. Startup and catch-up should both answer the
same question through the same code:

> Starting from a verified genesis or checkpoint base, does this ordered suffix
> deterministically produce the exact target frontier?

The implementation must continue to reject invalid or ambiguous history before
opening authoritative routes. Simplification means removing duplicate objects,
branches, and orchestration—not deleting independent commitments or weakening
verification.

## Confirmed defects and risks to fix

The following are not optional cleanup. They are correctness, liveness, or
resource-bound defects that this plan must close.

### 1. Trusted-checkpoint startup cannot recover a partially applied suffix

Live finalization and checkpoint catch-up durably persist finalized items and
journal entries before applying their business effects. That ordering correctly
prevents a trusted checkpoint from advancing ahead of durable history, but a
crash can leave:

```text
trusted checkpoint < application frontier < journal/finalized frontier
```

The no-checkpoint startup path replays accepted-but-unapplied entries. The
trusted-checkpoint-plus-tail path currently rejects any suffix for which
`application_next_index != journal_next_index`. A process can therefore fail
to restart after an ordinary crash between journal persistence and terminal
application.

The fixed startup path must validate the ordered suffix independently, restore
the consensus runtime, replay the contiguous non-terminal application suffix
idempotently, recompute the terminal-application chain, and remain unavailable
until the repaired frontier is exact and terminal. It must never invent,
truncate, or skip an ordered operation.

### 2. Paged catch-up accumulates an unbounded checkpoint batch in memory

Catch-up responses are paginated, but the requester currently concatenates all
pages for a checkpoint target into one in-memory response before applying the
batch. The default checkpoint interval is 10,000 journal operations or
finalized items, while an individual operation may be large. A valid historical
batch can therefore exhaust memory even though each network response is
bounded.

Catch-up must have explicit total item and byte bounds. It must verify and stage
or apply pages incrementally without requiring the complete checkpoint batch in
RAM. A peer must not be able to make memory use proportional to an unchecked
sum of otherwise valid page sizes.

### 3. Quiet-tip recovery can starve under sustained finalization

The current recovery-checkpoint path requires a quorum to report the exact same
current stable boundary and requires that boundary to be observed unchanged
twice. Under continuous traffic the tip may move between every poll. A member
that is behind cannot become healthy until it matches a quorum view, while it
may also be unable to start the live session that would naturally deliver the
moving tail.

The result can be indefinite recovery starvation. This plan replaces the
quiet-tip dependency with a deterministic AlephBFT-ordered recovery barrier, or
an equally strong mechanism demonstrated to make progress under sustained load.
The selected boundary must still be a completely finalized unit with terminal
application state, and checkpoints must remain attestations rather than an
alternate ordering path.

### 4. Whole-runtime cloning makes ordinary work history-proportional

Submission and finalization currently clone `FederationConsensusRuntime` to
obtain rollback behavior before durable persistence. That clone includes the
retained journal, finalized items, pending units, and duplicate/conflict
indexes. Even with checkpoint rebasing, this can create repeated O(retained
suffix) work and allocation on hot paths.

Replace whole-runtime cloning with prepared deltas, explicit validation and
commit phases, or immutable shared history plus a small mutable overlay. A
failed persistence operation must still leave the published in-memory runtime
unchanged.

### 5. Empty-database recovery remains genesis replay, not snapshot restore

A checkpoint is a commitment, not a physical state snapshot. A member with an
empty mint database must replay business operations from genesis and must
already possess its member identity and threshold secret material. This is an
intentional current limitation, not a reason to weaken checkpoint validation.

This plan must keep genesis catch-up correct and bounded per page, document its
retention assumptions, and define the checkpoint-bound database-backup contract
needed for practical disaster recovery. A peer-to-peer materialized-state
snapshot protocol is a separate design unless explicitly added to this plan by
reviewed amendment.

## Target architecture

### One canonical committed frontier

Introduce one canonical object, named during implementation according to
project conventions, with this conceptual shape:

```rust
struct FederationCommittedFrontier {
    journal: FederationJournalFrontier,
    finalized: FederationFinalizedFrontier,
    application: FederationApplicationFrontier,
    aleph_bft: FederationAlephBftRecoveryCursor,
}
```

The component types should group each cursor with its corresponding digest.
The AlephBFT cursor should contain the session, next finalized-unit index, and
per-member next-unit watermarks. Materialized-state audit metadata remains an
explicit checkpoint-level commitment because it has different computation and
trust rules.

`FederationCheckpoint`, durable frontier metadata, checkpoint boundaries,
checkpoint summaries, and peer status should embed this object or use lossless
projections from it. They must not each maintain independent handwritten lists
of the same fields.

The canonical object owns exact-match, coverage, monotonicity, complete-unit,
and fully-terminal checks. Protocol-specific callers may add stricter policy,
but may not reimplement partial comparisons.

### One verified checkpoint type

Raw deserialized checkpoint data and quorum-verified checkpoint data should be
distinguishable in the Rust type system. Introduce a private or public
`VerifiedFederationCheckpoint`/`VerifiedFederationCheckpointQuorum` wrapper
created only by full validation against the active `FederationConfig`.

Functions that promote a checkpoint, initialize recovery state, or authorize a
targeted catch-up batch should accept the verified wrapper instead of repeatedly
accepting raw payloads and relying on each caller to remember verification.
Database reads still return untrusted serialized values and must re-verify at
the trust boundary.

### One incremental suffix verifier

Create a shared verifier that starts from either genesis or a verified
checkpoint and incrementally consumes:

- finalized AlephBFT items in global finalized order;
- journal entries derived from accepted finalized operations; and
- terminal application results in accepted-operation order.

It should maintain rolling digests, expected cursors, complete-unit state,
operation/finalized correspondence, and target watermarks. It should support
bounded streaming and expose a final verified result only when all target
fields match exactly.

Startup-local suffix validation and remote checkpoint catch-up must use this
same verifier. Overlap handling may be a separate adapter, but it must feed the
same canonical verifier once the new suffix begins.

### One recovery entry point

Consolidate consensus recovery behind a conceptual API such as:

```rust
enum FederationRecoveryBase {
    Genesis,
    Checkpoint(VerifiedFederationCheckpoint),
}

FederationConsensusRuntime::recover(
    config,
    member_id,
    active_session,
    base,
    verified_suffix,
)
```

Avoid separate public constructors for every combination of authentication,
checkpoint base, journal suffix, and finalized suffix. Tests may use a focused
test builder rather than expanding the production API.

### Focused runtime ownership

Split the current federation node runtime by responsibility. The target
ownership is conceptually:

```text
FederationNodeRuntime
├── FederationOperationPipeline
├── FederationCheckpointManager
├── FederationCatchUpManager
├── FederationReadinessCoordinator
├── FederationDkgRuntime
└── FederationOnchainRuntime
```

Names and exact module boundaries may change, but checkpoint creation,
signature exchange, catch-up validation/application, startup recovery, DKG,
and wallet reconciliation must no longer accumulate in one implementation
block. Shared locks and handles must have a documented owner and lock order.

### One AlephBFT-selected recovery boundary

Replace quiet-tip observation state with an on-demand recovery barrier ordered
as a normal AlephBFT consensus item. The barrier selects the first complete,
terminally applied unit boundary at or after its finalization. Members derive
and sign the same ordinary recovery checkpoint at that boundary.

The barrier:

- does not contain or authorize a caller-supplied checkpoint digest;
- does not bypass normal operation ordering or deterministic application;
- is deduplicated by a stable generation or request ID;
- is emitted only when recovery demand exists;
- is bounded and rate-limited so recovery requests cannot create periodic
  consensus spam; and
- cannot change scheduled-checkpoint lineage or sequence rules.

If implementation evidence supports a simpler alternative with the same
sustained-load liveness and safety properties, record and review that decision
in this document before changing the milestone.

## Security and behavior boundaries that must not change

- AlephBFT remains the only mechanism that orders federation operations and
  recovery barriers. Checkpoints never become an alternate consensus path.
- No signature share, spend-state mutation, payment authorization, or other
  safety-critical effect may occur before the authorizing operation is
  finalized and durably journaled.
- The journal-order, finalized-item, terminal-application, and
  materialized-state commitments remain independently verifiable. Consolidate
  their representation; do not collapse away their meanings.
- Checkpoints are valid only at complete finalized-unit boundaries with no
  non-terminal application gap.
- Checkpoint quorum verification requires distinct valid member identity
  signatures meeting the configured consensus threshold.
- A requester never trusts catch-up bodies merely because they came from an
  authenticated member. Bodies must reproduce the selected verified target.
- Catch-up may use one healthy body source and a separately obtained
  transferable quorum proof. It must retry a different source after invalid or
  unavailable data without weakening validation.
- Durable data and application state must reach a checkpoint before that
  checkpoint is promoted. A crash may leave data ahead of trust, never trust
  ahead of data.
- Deterministic business application and rejection must reuse the existing mint
  domain path. Remote rejection results must be reproduced or verified locally.
- Replay must not call payment backends or repeat unsafe external side effects.
  Payment observations and transaction/broadcast authorization remain
  consensus facts.
- Startup, catch-up, config, roster, keyset, session, digest, application, and
  materialized-state mismatches fail closed. There is no fallback to standalone
  mode.
- The durable materialized-state audit mismatch latch remains fail-closed and
  cannot be cleared by restart, catch-up, or peer agreement.
- Normal exact-checkpoint startup remains independent of total historical
  length and does not add a full materialized-state scan.
- Current-session AlephBFT backup recovery remains bounded and separately
  validated. Opaque backup bytes are not added to the checkpoint payload.
- Standalone CDK behavior and public Cashu wallet protocol behavior remain
  unchanged when federation mode is disabled.
- Federation functionality continues to follow the cut-through compatibility
  policy in `docs/federated-cashu-implementation-plan.md`. SQLite and PostgreSQL
  schema and behavior must change together where persistence changes are
  required.
- Do not introduce unbounded queues, arbitrary timeout increases, serialized
  tests, compatibility shims, dual recovery implementations, or bypass flags to
  hide liveness problems.

## Non-goals

- Replacing AlephBFT.
- Changing Cashu NUT wire behavior for ordinary wallets.
- Redesigning threshold BLS mint signing or FROST transaction signing.
- Designing federation membership changes or secret-share recovery.
- Automatically repairing arbitrary out-of-band database corruption.
- Building a peer-to-peer materialized-state snapshot protocol in this plan.
- Preserving compatibility with undeployed experimental federation checkpoint
  databases or wire formats when cut-through simplification requires changes.

## Milestone dependency and progress table

Update this table whenever a milestone changes state.

| Milestone | Description | Status | Commit(s) | Verification summary |
|---|---|---|---|---|
| 0 | Baseline, invariants, and fault reproduction | Complete | `test: freeze checkpoint consolidation baselines` | Crash windows, quiet-tip starvation, catch-up aggregation, runtime-clone growth, startup work, and focused safety suites are frozen with deterministic local tests. |
| 1 | Crash-safe checkpoint suffix replay | Complete | `fix: repair checkpoint startup application tails` | Trusted-checkpoint startup verifies ordered tails, repairs only the unapplied suffix through the existing idempotent application path, and revalidates the exact durable frontier before readiness. |
| 2 | Canonical frontier and verified checkpoint types | Complete | `refactor: unify federation checkpoint frontiers` | Canonical component/frontier types own equality and coverage rules; quorum verification returns an unforgeable authority wrapper required by promotion and trusted recovery boundaries. |
| 3 | Shared incremental suffix verifier | Complete | `refactor: unify federation suffix verification` | Startup, genesis reconstruction, and signed catch-up share one unit-buffered verifier, canonical overlap comparisons, live finalization semantics, rolling digests, and exact target proof. |
| 4 | Unified consensus recovery API | Complete | `refactor: unify federation consensus recovery` | Production startup, checkpoint rebasing, and signed catch-up recover only through an opaque verified suffix rooted at an explicit authenticated genesis/checkpoint base; raw reconstruction remains test-fixture-only. |
| 5 | Bounded streaming checkpoint catch-up | Complete | `feat: stream bounded checkpoint catch-up` | Catch-up verifies, stages, applies, and resumes one bounded complete-unit page at a time; exact-frontier promotion and native predecode HTTP limits keep memory and authority bounded. |
| 6 | Focused runtime component split | Complete | `refactor: split federation runtime components` | Checkpoint, catch-up, operation, readiness, DKG, wallet/FROST, and test-fault ownership is explicit; structured-concurrency and blocked-network regressions freeze the lock boundary. |
| 7 | AlephBFT-ordered recovery barrier | Complete | `feat: order recovery checkpoint barriers` | A versioned barrier is ordered and persisted as an ordinary AlephBFT item; its complete-unit terminal boundary deterministically creates the existing recovery-checkpoint candidate, while a local lease and fixed one-unit tail bound suppress duplicate work. |
| 8 | Transport and request-state consolidation | Complete | `refactor: separate federation recovery transport` | Checkpoint-history and AlephBFT-backup traffic use distinct signed message kinds, wire types, routes, clients, size limits, and mutation paths; recovery demand and its proposal lease are one explicit state machine. |
| 9 | Remove whole-runtime cloning | Complete | `refactor: replace federation runtime clones with deltas` | Submission and finalization prepare bounded non-mutating deltas, persist first, and commit under the existing serialized runtime lock; injected persistence failures leave published state unchanged and clone-path growth is removed from both production hot paths. |
| 10 | Bootstrap, backup, operator, and retention contract | Complete | `feat: define federation backup recovery contract` | A versioned checkpoint-bound backup manifest gates restore before runtime startup; v1 retains recovery bodies from genesis, protects the active backup, exposes bounded redacted status, and documents operator-only audit recovery and deferred peer snapshots. |
| 11 | Adversarial integration, performance, and final cleanup | In progress | — | Running final architecture cleanup, adversarial review, local fake-wallet campaigns, and the required validation matrix. |

## Milestone 0: Baseline, invariants, and fault reproduction

Status: complete

### Objective

Freeze the current behavior in focused tests and measurements before structural
changes. Reproduce each confirmed defect so later milestones prove improvement
rather than merely moving code.

### Checklist

- [x] Document the current type and call graph for startup, checkpoint capture,
      signature collection, quorum persistence, catch-up target selection,
      page serving, batch verification, application, promotion, backup recovery,
      and readiness.
- [x] Record which checkpoint/frontier fields are duplicated across durable
      metadata, boundary, checkpoint, summary, peer status, and transport types.
- [x] Add a deterministic fault-injection test that crashes or stops after
      finalized/journal persistence but before application with a trusted base
      checkpoint, then demonstrates the current restart failure.
- [x] Add a deterministic fault-injection test for a crash after some, but not
      all, operations in a checkpoint catch-up batch reach terminal application.
- [x] Add a bounded test fixture showing total catch-up allocation grows with
      the complete checkpoint batch despite page limits.
- [x] Add a sustained-finalization simulation in which a lagging member cannot
      observe the same quiet recovery tip twice.
- [x] Measure bytes and elapsed time spent cloning the consensus runtime at
      representative suffix sizes.
- [x] Record baseline exact-checkpoint startup, checkpoint-tail startup,
      genesis catch-up, one-checkpoint catch-up, and two-checkpoint catch-up
      counters.
- [x] Confirm existing digest mismatch, checkpoint fork, invalid signature,
      duplicate signer, partial-unit, invalid backup, and materialized-state
      latch tests remain active.

### Exit tests

- [x] The new crash and moving-tip tests fail for the expected current reason,
      not because of timing or unrelated setup.
- [x] Resource tests report deterministic item and byte counts without relying
      on host-specific resident-memory sampling.
- [x] Existing checkpoint, catch-up, startup, SQLite, and PostgreSQL-focused
      tests pass unchanged.
- [x] `cargo fmt --all -- --check` passes.
- [x] Federation-disabled builds for affected crates still pass.

### Progress notes

The current control flow is:

- startup enters `create_federation_runtime_state_with_wallet`, verifies the
  configured federation and any durable trusted quorum, then selects exact
  checkpoint, checkpoint-plus-suffix, or genesis recovery. Exact recovery
  constructs the runtime at the checkpoint. Checkpoint-plus-suffix loads the
  journal and finalized suffix and calls `validate_checkpoint_startup_suffix`.
  That validator currently rejects a non-terminal application frontier before
  any idempotent application replay, which is the reproduced crash defect.
- live finalization enters `FederationNodeRuntime::apply_finalized_unit`, clones
  the consensus runtime, finalizes into the clone, persists finalized items and
  journal entries transactionally, publishes the clone, applies business
  effects, and then evaluates checkpoint capture. Submission has a second
  whole-runtime clone in `submit_envelope`.
- checkpoint capture derives a stable complete-unit boundary in the node
  runtime, creates or recovers a candidate, exchanges signature requests via
  the private REST routes, persists signatures, forms a verified quorum, and
  promotes it through the database transaction API.
- peer synchronization calls `sync_once_from_member`, selects the next locally
  stored quorum with `next_local_checkpoint_catch_up_target`, requests pages
  from the catch-up route, and currently appends every page to one
  `FederationJournalCatchUpResponse`. It then validates the complete batch,
  applies it, promotes the exact target, rebases the runtime, and separately
  recovers current-session AlephBFT backup chunks. Startup readiness is decided
  afterward by quorum reconciliation and live-session activation.

The cursor/digest inventory found overlapping handwritten representations in:

- `FederationCheckpointFrontier`: journal, finalized, and application cursors
  and digests plus trusted/audit metadata;
- `FederationCheckpointBoundary`: those three frontiers plus AlephBFT session,
  finalized-unit cursor, and per-member unit watermarks;
- `FederationCheckpoint`: the boundary fields plus config, lineage, policy,
  and materialized-state commitments;
- checkpoint summaries and status responses: partial checkpoint identity,
  kind, sequence, journal/finalized cursors, and several quorum projections;
- `FederationPeerStatus` and consensus runtime status: operation/journal,
  application, finalized-item, session, and audit projections; and
- catch-up request/response types: independently carried journal and finalized
  cursors plus target-checkpoint fields.

Manual comparison logic is correspondingly split across checkpoint promotion,
transport binding, node-runtime boundary matching/coverage, peer status
comparison, catch-up rolling-frontier validation, and startup exact/suffix
validation.

Fault and measurement hooks are deterministic and local:

- `trusted_checkpoint_restart_rejects_suffix_before_first_application` and
  `trusted_checkpoint_restart_rejects_partially_applied_suffix` persist valid
  finalized/journal suffixes after a trusted checkpoint, apply zero or one of
  two entries, and assert the current restart rejection reason is exactly
  `checkpoint startup suffix is not terminally applied`.
- `moving_recovery_tip_never_satisfies_quiet_tip_debounce` advances a valid
  boundary for 32 observations and confirms the existing identical-observation
  debounce never completes.
- `append_checkpoint_catch_up_page` is a behavior-preserving extraction of the
  production aggregation operation. Its fixture retained 12 operations from
  four pages: largest page 4,732 canonical bytes, final aggregate 17,305 bytes.
- runtime clone baselines retained 2,372, 151,871, and 608,771 canonical bytes
  for 1, 64, and 256 operations. Sample debug clone times were 33,041 ns,
  50,125 ns, and 151,375 ns. Bytes/items are the deterministic evidence;
  elapsed samples are diagnostic only.

Release-mode checkpoint startup remained independent of total history:

| History | Exact | Exact + 4 KiB backup | 64-row tail + backup |
|---:|---:|---:|---:|
| 1,000 | 8.287 ms | 10.154 ms | 11.358 ms |
| 10,000 | 8.211 ms | 10.105 ms | 11.469 ms |
| 100,000 | 8.114 ms | 10.124 ms | 11.392 ms |

Every exact run loaded zero journal/finalized rows, verified two signatures,
and performed zero application replay or materialized-state audits. Backup runs
loaded four chunks/4,096 bytes. Tail runs loaded exactly 64 journal and 64
finalized rows regardless of total history.

Genesis pagination used one request/page at 99 and 100 operations and two at
101 for a page limit of 100. The two-target catch-up baseline applied one
journal/one finalized item for the first checkpoint, one journal/two finalized
items for the second, and performed two promotions. The peer-sync regression
now exercises real in-memory private REST routes and a transferable recovery
quorum rather than a stale pre-checkpoint mock response.

Focused verification completed locally:

- `cargo test -p cdk-axum --features federation,prometheus checkpoint --lib`
  (15 passed, 2 benchmark tests ignored);
- the corresponding `catch_up` filter (22 passed, 1 process stress test
  ignored) and `federation_runtime_startup` filter (6 passed);
- focused moving-tip, clone, aggregation, pagination, peer-sync, backup,
  mismatch, and audit-latch regressions;
- `cargo test -p cdk-common --features federation checkpoint --lib`
  (15 passed, with one pre-existing dead-code warning);
- SQLite and PostgreSQL `federation_checkpoint_persistence` tests (one passed
  on each backend; PostgreSQL used the disposable local helper and was stopped
  afterward);
- `cargo fmt --all -- --check`; and
- `cargo check -p cdk-axum --no-default-features`.

The existing checkpoint quorum tests still cover invalid signatures, minority
proofs, duplicate signers, and unknown signers. Promotion tests cover lineage
fork rejection; startup/catch-up tests cover digest and incomplete-unit
mismatches; backup tests reject malformed pages without partial persistence;
and the durable materialized-state mismatch latch remains fail-closed.

### Commit gate

- [x] Commit milestone 0 tests, measurement hooks, documentation, and recorded
      baseline results before starting milestone 1.

## Milestone 1: Crash-safe checkpoint suffix replay

Status: complete

### Objective

Fix the release-blocking restart hole without weakening suffix validation or
checkpoint trust.

### Checklist

- [x] Separate ordered-suffix validation from terminal-application validation
      during trusted-checkpoint startup.
- [x] Restore the consensus runtime from the verified checkpoint plus the
      contiguous finalized/journal suffix before replaying business state.
- [x] Identify the exact contiguous terminal application prefix from durable
      application metadata.
- [x] Reject gaps, terminal-result conflicts, digest divergence, missing
      finalized evidence, changed envelopes, or changed consensus evidence.
- [x] Replay only accepted, applying, or retryable failed operations after the
      durable application cursor through the existing idempotent mint
      application path.
- [x] Reproduce deterministic business rejections locally and retain their
      standard Cashu error response.
- [x] Advance the terminal-application digest and materialized-state registry in
      the same atomic business transactions used by normal application.
- [x] Require the repaired application cursor and digest to reach the durable
      journal frontier before readiness can proceed.
- [x] Preserve data-before-trust ordering; do not promote a later checkpoint as
      part of repairing an untrusted local tail.
- [x] Add structured errors that distinguish a safely replayable application
      lag from irrecoverable history or digest corruption.

### Exit tests

- [x] Restart after journal persistence but before the first application
      resumes and reaches the same state as uninterrupted execution.
- [x] Restart after each operation in a multi-operation batch resumes from the
      durable application cursor without duplicating effects.
- [x] Applied mint, swap, melt, payment-observation, keyset, address, and
      transaction operations remain idempotent under checkpoint-tail replay.
- [x] Deterministic rejection replay produces the same response and application
      digest on every member.
- [x] An altered envelope, finalized item, consensus evidence field, or prior
      terminal result still fails closed before authoritative routes open.
- [x] A second restart after successful repair takes the exact-checkpoint or
      terminal-tail fast path and performs no duplicate business work.
- [x] SQLite and PostgreSQL transaction/fault tests pass.

### Progress notes

Startup now treats ordering trust and terminal application as two sequential
states. It first verifies the checkpoint-bound finalized/journal suffix and
recovers the consensus runtime without requiring the application cursor to be
at the tip. It then derives the one contiguous terminal prefix committed by the
durable application cursor and digest. Only the remaining accepted/applying or
retryable-failed entries are passed, in order, through the same
`Mint::apply_accepted_federation_operation` path used by live finalization.
After replay, startup reloads and revalidates the suffix, requires the exact
journal/application frontier, updates the recovered runtime's terminal states,
and only then permits readiness to continue. An already-terminal suffix returns
before a second database scan, preserving the fixed-tail fast path.

The extracted `apply_federation_journal_entry_for_replay` helper owns the
shared retry and failure state machine. A deterministic business error is
persisted as its standard Cashu `Rejected` response; an ambiguous error is
durably `Failed` and startup remains unavailable. Every operation-specific
business mutation, terminal journal state, application digest advance, and
materialized-state registration remains in the existing database transaction
inside the mint application path. No checkpoint is captured or promoted by
repair, so durable finalized/journal data still precedes any trust advance.
No schema changes were needed.

The local fault matrix persists a two-operation suffix after a trusted
checkpoint and crashes before operation zero, after operation zero, and after
a retryable `Failed` marker. Each restart reaches the uninterrupted
materialized state; the second restart is a no-op. A conflicting swap test
reproduces the same rejection response and application digest and continues to
a later valid operation. Mutation tests fail closed for changed envelopes,
consensus evidence, and prior terminal results. Existing finalized-item
mismatch coverage retains the missing/changed finalized evidence cases. The
shared mint replay path's complete federation suite covers mint, swap, melt,
payment-observation, keyset, address, and transaction idempotence. The obsolete
melt test that attempted an illegal terminal `Applied -> Failed` transition was
corrected to assert terminal-result immutability.

Verification completed locally:

- `cargo test -p cdk-axum --features federation,prometheus trusted_checkpoint
  --lib` (8 passed);
- the full `cdk` federation library suite (768 passed);
- focused `checkpoint` (20 passed, 2 ignored), `catch_up` (22 passed, 1
  ignored), and `federation_runtime_startup` (6 passed) filters;
- SQLite and PostgreSQL `federation_checkpoint_persistence` tests (one passed
  on each backend; the disposable local PostgreSQL helper was stopped);
- `cargo test -p cdk-common --features federation checkpoint --lib` (15
  passed, with one pre-existing dead-code warning);
- federation-enabled checks with and without Prometheus, the
  federation-disabled `cdk-axum` check, `cargo fmt --all -- --check`, and
  `git diff --check`.

The strict `cdk-axum` Clippy command remains blocked by the crate's existing
lint backlog (25 library and 103 test-target findings); it reported no new
finding in the startup repair or replay code. The release benchmark retained
history-independent startup: exact-checkpoint times were 8.332, 8.197, and
8.126 ms at 1k/10k/100k history; exact plus a 4 KiB backup was 10.163, 10.078,
and 10.116 ms; and a fixed 64-row tail plus backup was 11.415, 11.310, and
11.362 ms. Exact startup loaded zero history rows; tail startup loaded exactly
64 journal and 64 finalized rows at every history size, with zero replay for
the already-terminal benchmark suffix.

### Commit gate

- [x] Commit the complete crash-safe replay fix and all exit tests before
      starting milestone 2.

## Milestone 2: Canonical frontier and verified checkpoint types

Status: complete

### Objective

Make one object own the definition of federation recovery progress and use the
Rust type system to separate raw checkpoint data from verified checkpoint
authority.

### Checklist

- [x] Inventory every duplicated cursor/digest grouping and every handwritten
      `matches`, `covers`, or monotonicity comparison.
- [x] Introduce the canonical committed-frontier object and focused journal,
      finalized, application, and AlephBFT component cursors.
- [x] Embed or losslessly project that object in checkpoint, boundary, durable
      metadata, summary, status, and catch-up binding types.
- [x] Move exact equality, coverage, non-regression, complete-unit, roster
      watermark, and fully-terminal validation onto the owning types.
- [x] Introduce a verified checkpoint/quorum wrapper constructible only after
      version, federation, config digest, lineage, signature, distinct-signer,
      and threshold validation.
- [x] Change promotion and recovery APIs to require the verified wrapper where
      practical.
- [x] Remove manual partial comparisons and duplicated validation helpers after
      callers migrate.
- [x] Keep serialization explicit and stable within the cut-through protocol;
      do not expose private wrapper invariants through unchecked constructors.
- [x] Update SQLite and PostgreSQL mappings together if the canonical object
      changes stored columns or serialization.

### Exit tests

- [x] Compile-time API tests or module privacy prevent constructing a verified
      checkpoint without quorum verification.
- [x] Property tests cover reflexive exact matching, monotonic coverage,
      rollback rejection, changed-digest rejection at equal cursors, scheduled
      lineage, recovery anchoring, and roster watermark equality.
- [x] Serialization round trips preserve checkpoint IDs and canonical bytes.
- [x] Existing valid checkpoint fixtures retain their intended semantic fields;
      obsolete experimental fixtures are regenerated rather than adapted.
- [x] No production caller manually compares a subset of canonical frontier
      fields to decide trust or readiness.

### Progress notes

`FederationCommittedFrontier` now owns the journal, finalized-item,
terminal-application, and AlephBFT recovery dimensions through
`FederationJournalFrontier`, `FederationFinalizedFrontier`,
`FederationApplicationFrontier`, and `FederationAlephBftRecoveryCursor`.
It owns shape, full terminality, complete-unit, exact-roster, exact-match, and
monotonic-coverage rules. `FederationCheckpoint`,
`FederationCheckpointBoundary`, and the scalar durable
`FederationCheckpointFrontier` expose lossless projections rather than
duplicating those decisions. Durable metadata also owns the narrower
checkpoint-position operation required before a database loads historical
digests. Peer committed-state comparison moved onto `FederationPeerStatus`.

Checkpoint summaries now carry the exact canonical committed frontier instead
of two unbound cursor scalars. This is a cut-through private REST shape change;
there is no compatibility adapter. Signed checkpoint JSON itself and the SQL
checkpoint/frontier columns are unchanged, so canonical checkpoint bytes,
fixed checkpoint IDs, and existing database records retain their semantics.
SQLite and PostgreSQL needed only the shared promotion API update, not a schema
or mapping migration.

`FederationCheckpointQuorum` remains the raw bounded transport/storage proof.
Its `verify` method now returns `VerifiedFederationCheckpointQuorum`, whose
fields are private and which deliberately has no deserializer or unchecked
constructor. Proof verification establishes payload version and shape,
federation/config binding, exact roster, signature validity, signer
distinctness, and threshold. The database promotion API accepts only this
wrapper and validates promotion lineage against the transactionally locked
current checkpoint, avoiding a check/use race. Startup and catch-up recovery
take their checkpoint reference from the verified wrapper before making trust
decisions. The old `checkpoint_covers`, `checkpoint_summary_covers`, and
handwritten checkpoint/boundary and durable/checkpoint equality logic were
removed or reduced to canonical projections.

Verification completed locally:

- `cargo check --workspace --all-targets` passed;
- `cargo test -p cdk-common --features federation checkpoint --lib` passed 17
  tests, including committed-frontier property checks, fixed checkpoint vectors,
  lineage, roster, quorum, and serialization tests;
- `cargo test -p cdk-common --features federation --doc` passed the external
  compile-fail test proving the verified wrapper cannot be constructed;
- `cdk-axum` checkpoint (20 passed, 2 ignored), catch-up (22 passed, 1
  ignored), startup (6 passed), and quorum/session (7 passed) filters passed;
- targeted `cdk-mintd` checkpoint-status guards passed 6 tests;
- SQLite and disposable-local PostgreSQL checkpoint persistence passed, and
  PostgreSQL was stopped afterward;
- federation-disabled `cdk-axum` check, formatting, and diff checks passed;
- strict library Clippy passed for `cdk-common` and `cdk-sql-common`.

The strict `cdk-common` test-target Clippy command remains blocked by two
pre-existing test lints, and strict `cdk-axum` library Clippy remains blocked by
the same 25 pre-existing findings recorded in milestone 1; neither reported a
new canonical-frontier finding. The full `cdk-mintd` library run reached 207
passes and the same six unrelated existing failures: two status tests whose
assertions already disagree with their unchanged single-member/error-message
logic, and four trusted-dealer setup tests rejected by the current production
cut-through policy. The checkpoint-status subset changed here is green.

### Commit gate

- [x] Commit the canonical frontier/type migration and tests before starting
      milestone 3.

## Milestone 3: Shared incremental suffix verifier

Status: complete

### Objective

Replace parallel startup and network catch-up validators with one incremental
verification engine.

### Checklist

- [x] Implement a verifier initialized from genesis or a verified checkpoint.
- [x] Incrementally validate finalized-item contiguity, unit completeness,
      body hashes, provenance, session transitions, and per-member watermarks.
- [x] Incrementally derive accepted journal operations from finalized items and
      validate journal indices, operation IDs, canonical envelopes, conflict
      outcomes, consensus evidence, and order digests.
- [x] Incrementally validate terminal application order and application digest.
- [x] Make overlap validation an explicit pre-suffix adapter with one canonical
      comparison implementation.
- [x] Return an opaque verified suffix/frontier result only after exact target
      equality.
- [x] Use the verifier for trusted-checkpoint local startup suffix validation.
- [x] Use the same verifier for remote checkpoint-bound catch-up.
- [x] Use the same verifier for genesis reconstruction where doing so removes
      duplicate logic without slowing the exact-checkpoint path.
- [x] Delete superseded startup, catch-up rolling-frontier, and overlap helper
      implementations.

### Exit tests

- [x] Identical suffixes presented through startup and network catch-up produce
      identical verified results.
- [x] A mutation corpus changing every committed field is rejected by both
      paths at the same logical validation stage.
- [x] Chunk boundaries, page sizes, and item grouping do not change the final
      verified frontier.
- [x] Partial AlephBFT units are rejected without advancing verified state.
- [x] Duplicate and invalidly authenticated finalized operations reproduce the
      same journal behavior as live finalization.
- [x] Exact-checkpoint startup consumes no historical prefix rows.

### Progress notes

`FederationSuffixVerifier` starts from either genesis digests or a
`VerifiedFederationCheckpointQuorum`. It accepts finalized-item and journal
chunks independently, retains only the currently unclosed AlephBFT unit, and
does not advance its committed finalized frontier until the unit body is
complete. Closing a unit reconstructs the canonical payload and sends it
through the same authenticated live finalization path, which reproduces roster,
authorization, duplicate-operation, conflict, session, unit-finalization, and
per-member watermark behavior instead of maintaining a second implementation.

Each derived journal row is paired once with either a full durable row or the
compact catch-up projection. The shared comparison checks index, operation ID,
order-digest links, canonical envelope, conflict keys, consensus evidence, and
consensus-conflict outcome. Full local rows overlay only terminal application
state; compact peer terminal state is used solely to verify the signed
application frontier until local deterministic replay reproduces it. The
application digest advances only over the contiguous terminal prefix, so a
terminal row beyond a durable non-terminal gap is retained but cannot move the
committed cursor. `VerifiedFederationSuffix` remains field-private and is
created only after journal, finalized, application, and—when checkpoint
targeted—AlephBFT frontiers equal the target exactly.

Trusted-checkpoint startup, the no-checkpoint genesis reconstruction path, and
checkpoint-bound peer catch-up now use this engine. Exact-checkpoint startup
passes empty chunks and therefore performs no journal, finalized-item, or
historical-guard reads. A non-empty checkpoint tail loads only targeted
pre-checkpoint duplicate/conflict guards referenced by that tail. The old
startup suffix validator, catch-up rolling-digest validator, genesis journal
comparison loop, and handwritten journal/finalized overlap comparators were
removed. Overlap uses `FederationCatchUpOperation::ensure_matches_journal_entry`
and `FederationAlephBftFinalizedItem::ensure_matches` everywhere.

Verification completed locally:

- `cargo test -p cdk-common --features federation suffix_verifier --lib` (8
  passed), including full-versus-compact equivalence, finalized and journal
  mutation corpora, one-item page boundaries through multi-item units, partial
  units, application gaps, duplicates, and invalid authorization;
- `cargo test -p cdk-axum --features federation checkpoint --lib` (19 passed),
  including exact-checkpoint zero-prefix startup and two successive signed
  checkpoint promotions;
- focused `catch_up` (22 passed, 1 ignored), `trusted_checkpoint` (8 passed),
  and `federation_runtime_startup` (6 passed) filters;
- the full `cdk-common` federation library suite (405 passed, 1 ignored, after
  correcting the stale local-status field fixture) and full `cdk` federation library suite
  (768 passed; 765 passed in the sandbox and the three loopback-listener tests
  passed separately with local bind permission);
- `cargo check --workspace --all-targets`, strict `cdk-common` Clippy,
  `cargo fmt --all -- --check`, and `git diff --check`.

Strict `cdk-axum` Clippy remains blocked by the pre-existing library lint
backlog; after grouping the new startup verifier parameters it reports no new
Milestone 3 finding.

### Commit gate

- [x] Commit the shared verifier migration and tests before starting milestone
      4.

## Milestone 4: Unified consensus recovery API

Status: complete

### Objective

Collapse the combinatorial consensus recovery constructors into one reviewed
path based on a recovery base and a verified suffix.

### Checklist

- [x] Introduce the genesis/checkpoint recovery-base enum or equivalent.
- [x] Make the unified recovery function consume only a verified suffix result,
      not unrelated raw journal and finalized vectors.
- [x] Reconstruct journal base, finalized cursors, duplicate guards, conflict
      guards, session, finalization index, and per-member watermarks in one
      implementation.
- [x] Retain explicit roster/authentication validation for all production
      recovery calls.
- [x] Provide a narrow test builder for unit tests that do not need production
      authentication plumbing.
- [x] Migrate startup, checkpoint promotion rebasing, catch-up, and test callers.
- [x] Remove obsolete `from_checkpoint`, `from_checkpoint_suffix`,
      `from_checkpoint_and_finalized_items`, and parallel authenticated variants
      once no longer needed.
- [x] Ensure pending local operation requeue remains separate from authoritative
      recovered history.

### Exit tests

- [x] Genesis, exact checkpoint, checkpoint plus local tail, checkpoint plus
      remote tail, later session, duplicate finalized unit, and pending-operation
      restart scenarios use the same recovery entry point.
- [x] Invalid roster, authorization, session rollback, replayed pre-checkpoint
      unit, and watermark mismatch tests remain fail-closed.
- [x] Recovered runtime status and journal are byte-for-byte or semantically
      identical to uninterrupted live finalization.
- [x] Public production recovery API surface is smaller than before this
      milestone.

### Progress notes

`FederationConsensusRecoveryBase` now makes the authenticated root explicit:
`Genesis`, or `Checkpoint(&VerifiedFederationCheckpointQuorum)`. The shared
suffix verifier has one constructor taking that base, and the only production
history-recovery boundary is
`FederationConsensusRuntime::recover_verified_suffix(VerifiedFederationSuffix)`.
The runtime cannot be recovered from independently supplied journal/finalized
vectors. Empty live construction remains the separate
`authenticated_aleph_bft` API and therefore cannot be mistaken for history
recovery.

The base initializer validates the complete configuration and local roster,
and a checkpoint base additionally requires the unforgeable verified-quorum
wrapper. The verifier then reconstructs the journal base, duplicate/conflict
guards, session transitions, finalization cursor, finalized-item cursor, and
per-member unit watermarks by driving the same live finalization path. Exact
checkpoint rebasing also creates an empty verified suffix at the target before
discarding the retained tail. Pending local operations are loaded and requeued
only after authoritative recovery completes, so they never become recovered
history.

Removed public production constructors include
`authenticated_aleph_bft_from_checkpoint`,
`authenticated_aleph_bft_from_checkpoint_suffix`,
`authenticated_aleph_bft_from_checkpoint_and_finalized_items`,
`authenticated_aleph_bft_from_finalized_items`, and
`aleph_bft_from_finalized_items`, together with their combinatorial internal
variants. Explicitly named `*_for_test` helpers remain only for raw malformed
history fault injection and cross-crate fixture utilities that are compiled as
library support; none is called by startup, checkpoint promotion, or signed
catch-up.

Verification completed locally:

- the full `cdk-common` federation library suite (405 passed, 1 ignored) and
  strict `cdk-common` Clippy;
- the Axum checkpoint suite (19 passed), catch-up suite (22 passed, 1 ignored),
  and pending-operation restart recovery test;
- focused shared-suffix (8 passed), raw recovery (6 passed), and exact
  checkpoint-base tests;
- `cargo check --workspace --all-targets` and `git diff --check`.

### Commit gate

- [x] Commit the unified recovery API and caller migration before starting
      milestone 5.

## Milestone 5: Bounded streaming checkpoint catch-up

Status: complete

### Objective

Make recovery memory and network work explicitly bounded while retaining
complete target validation and data-before-trust promotion.

### Checklist

- [x] Add a consensus-configured or protocol-fixed maximum checkpoint batch
      byte budget in addition to item-count intervals.
- [x] Define whether scheduled eligibility triggers on journal count, finalized
      count, cumulative canonical bytes, or any of those thresholds.
- [x] Bound request, response, staged, decoded, and aggregate in-flight bytes.
- [x] Replace whole-batch `Vec` aggregation with page-by-page incremental
      verification through the shared suffix verifier.
- [x] Select and document one crash-safe staging model: durable staging tables,
      durable untrusted suffix rows, or incrementally applied verified pages
      while retaining the old trusted checkpoint.
- [x] Ensure a failed page or source cannot contaminate the trusted frontier and
      can be discarded or safely overlapped on retry.
- [x] Persist enough rolling verifier progress to resume after a crash without
      downloading or replaying already durable pages unnecessarily.
- [x] Promote the target checkpoint only after every target cursor, digest, and
      watermark matches exactly and business application is terminal.
- [x] Preserve one-source body fetching with source failover and transferable
      quorum-proof fetching.
- [x] Add bounded metrics for pages, canonical bytes, staged bytes, applied
      bytes, retries, and peak in-flight batch bytes.

### Exit tests

- [x] Catch-up memory remains within a deterministic configured bound for a
      checkpoint containing the maximum supported item count and large
      operations.
- [x] One-page and many-page delivery produce identical state and checkpoint
      promotion.
- [x] Crash after every page, after staging, during application, and before
      promotion resumes safely from durable progress.
- [x] Invalid data on the first source is rejected and a second source can
      complete recovery from the unchanged trusted base.
- [x] No-progress, oversized-page, oversized-batch, integer-overflow, and
      decompression/decoding amplification cases fail with structured errors.
- [x] Genesis catch-up and multiple sequential scheduled checkpoints remain
      supported without whole-history memory accumulation.

### Progress notes

Selected the simplest crash-safe model: each complete-unit page is authenticated
against the already verified target quorum, suffix-verified, durably persisted,
and deterministically applied while the previous checkpoint remains the only
trusted checkpoint. No staging tables or second recovery protocol were added.
Only an exact final page may promote the target. A valid prefix from a source
may remain as an untrusted durable suffix; an invalid page changes nothing, and
another source resumes at the durable scalar cursor.

The protocol now uses a signed checkpoint policy and fixed cut-through limits:

- scheduled eligibility fires when any of accepted-operation count, finalized
  item count, or cumulative canonical finalized bytes crosses its interval;
- the defaults are 10,000 accepted operations, 10,000 finalized items, or 32
  MiB of canonical finalized history;
- one checkpoint batch is capped at 20,000 accepted operations, 20,000
  finalized items, and 64 MiB of canonical finalized history;
- one signed request is capped at 64 KiB, one response page at 8 MiB, and one
  checkpoint transfer at 256 MiB including JSON and compact-claim overhead.

Native bitreq, reqwest, and Tor transports enforce the page bound while reading
the response, before JSON decoding. Item caps additionally bound decoded object
overhead. The route trims only at a complete AlephBFT-unit boundary and keeps
compact journal claims aligned with accepted finalized items. A single unit
that cannot fit is rejected rather than split.

There is no whole-batch response aggregate. At most one fetched page is held by
the sync loop. Fail-closed page verification clones the bounded retained suffix
runtime, so worst-case recovery memory is a constant function of the 64 MiB /
20,000-item suffix caps plus one 8 MiB page and fixed structural overhead, never
of federation lifetime. After a successful page, the in-memory runtime carries
the rolling verifier state. The same journal, finalized, and application
digests and cursors are durable; after a process crash startup reconstructs that
bounded suffix once and requests only pages after the durable cursor.

Checkpoint metrics now record pages fetched/applied, encoded transport bytes,
staged/applied bytes, retry count, and the largest encoded page in the latest
batch. The peak gauge measures encoded in-flight page work rather than
allocator-specific heap overhead.

Local verification completed with:

- `cargo check --workspace --all-targets`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- the full `cdk-common` federation suite (405 passed, 1 ignored), including the
  shared suffix suite (8 passed);
- the full `cdk` federation library suite (768 passed);
- the Axum checkpoint suite (26 passed, 2 ignored) and catch-up suite (26
  passed, 1 ignored), including maximum-page trimming, no-progress/overflow,
  one-page versus many-page equivalence, source failover, crash after every
  durable page, partial/staged application restart, and pre-promotion failure;
- the Prometheus library suite (11 passed) and mint-daemon fake-wallet library
  suite (55 passed);
- bounded HTTP response tests with both native bitreq and reqwest backends.

### Commit gate

- [x] Commit bounded streaming catch-up and all fault/resource tests before
      starting milestone 6.

## Milestone 6: Focused runtime component split

Status: complete

### Objective

Reduce the federation node god object and make checkpoint/catch-up ownership and
lock boundaries reviewable.

### Checklist

- [x] Extract checkpoint capture, signing, quorum assembly, fetch, persistence,
      promotion, and audit into a checkpoint manager.
- [x] Extract target selection, page acquisition, verification, staging,
      application, and source failover into a catch-up manager.
- [x] Extract submission, pending persistence, finalization persistence,
      deterministic application, and response publication into an operation
      pipeline.
- [x] Extract readiness transitions and quorum-compatible peer-view decisions
      into a readiness coordinator.
- [x] Move DKG persistence and ceremony state out of checkpoint/catch-up runtime
      code without redesigning DKG behavior.
- [x] Keep on-chain wallet/FROST reconciliation behind a focused interface
      invoked after deterministic application.
- [x] Define the owner and acquisition order of consensus, operation-application,
      checkpoint, recovery, and database transaction locks.
- [x] Remove test-only flags from production component layouts where a test
      harness or injected trait can provide the same fault control.
- [x] Keep router handlers thin and free of business-state decisions.

### Exit tests

- [x] Existing behavior tests pass without reaching through component-private
      state.
- [x] Lock-order tests or structured concurrency tests cover simultaneous live
      finality, checkpoint capture, catch-up, signature exchange, status reads,
      and shutdown.
- [x] No component holds a network request open while holding the application
      or consensus mutation lock unless explicitly justified and tested.
- [x] `FederationNodeRuntime` becomes primarily composition and lifecycle
      ownership rather than the implementation of every workflow.
- [x] Module documentation identifies each component's authority and forbidden
      responsibilities.

### Progress notes

`FederationNodeRuntime` is now a 562-line composition/lifecycle owner, down from
2,834 lines before the split. Workflow implementations live in focused modules:

- `checkpoint_manager.rs` owns local capture, signatures, quorum assembly,
  durable fetch/persistence, promotion, and operator audit;
- `sync_loop.rs` owns peer/target selection, bounded page acquisition, retry,
  backoff, and source failover, while `checkpoint_catch_up.rs` owns authenticated
  page verification, durable staging, deterministic application, and exact
  target promotion;
- `operation_pipeline.rs` owns admission, pending/finalization persistence,
  serialized application, wallet reconciliation, and terminal publication;
- `runtime_components.rs` owns the application lock, recovery observation,
  readiness state, DKG handles, wallet/FROST handles, and the test-only fault
  harness;
- `dkg_persistence.rs` owns ceremony/FROST persistence without checkpoint,
  catch-up, or readiness authority; and
- router audit handling delegates its fail-closed business decision to
  `MintState::audit_trusted_federation_checkpoint`.

The documented mutation order is: take short readiness or recovery-observation
locks independently; complete peer network work; acquire the operation lock;
acquire consensus only when required; then open the database transaction. No
path may acquire an earlier lock while holding a later one. Checkpoint page
requests complete before either mutation lock is acquired. A deterministic
blocked-transport regression waits until the request is in flight and then
proves both locks remain immediately available.

The test surface no longer reaches component-private fields. Four checkpoint
and persistence fault switches moved into the `cfg(test)` fault harness, and
construction/access uses narrow methods. On-chain wallet reconciliation is a
single post-application interface, and FROST access is similarly focused.

Local verification completed with:

- production `cargo check -p cdk-axum --features federation,prometheus
  --all-targets`;
- the checkpoint suite (28 passed, 2 manual benchmarks ignored) and catch-up
  suite (26 passed, 1 process stress test ignored);
- the six-way structured-concurrency test and the blocked-peer-request lock
  test;
- all eight focused FROST tests; and
- 55 DKG-named driver, persistence, route, transcript, restart, and admission
  cases. The one remaining broad-name test selected by the `dkg_` substring
  exits before its DKG assertions on the pre-existing fake fixture's `Unknown
  quote` value-operation error and is not caused by the component move.

`cargo fmt --all -- --check` and `git diff --check` pass. Strict workspace
Clippy remains blocked by existing warnings outside this milestone, including
warnings in `cdk` and pre-existing Axum AlephBFT/status/test code; the moved
modules themselves compile without warnings.

### Commit gate

- [x] Commit the component split and concurrency tests before starting
      milestone 7.

## Milestone 7: AlephBFT-ordered recovery barrier

Status: complete

### Objective

Remove dependence on a naturally quiet tip and guarantee that recovery can
select a quorum-signable boundary during sustained valid traffic.

### Checklist

- [x] Define a versioned, bounded recovery-barrier consensus item with a stable
      generation/request ID.
- [x] Define who may request or propose a barrier and how duplicate concurrent
      requests converge.
- [x] Rate-limit or lease barrier generation so a lagging or malicious member
      cannot create unbounded consensus traffic.
- [x] Finalize the barrier through ordinary AlephBFT ordering and durable
      finalized-item persistence.
- [x] Define the exact complete-unit, terminal-application boundary selected by
      the finalized barrier.
- [x] Derive the existing recovery checkpoint payload at that boundary without
      accepting any caller-provided digest or materialized-state claim.
- [x] Preserve the latest scheduled checkpoint as the recovery checkpoint's
      anchor and do not increment the scheduled sequence.
- [x] Reuse ordinary checkpoint candidate, signature, quorum, dissemination,
      catch-up, and promotion logic after boundary selection.
- [x] Remove quiet-tip two-observation request state and peer-summary coupling
      once the barrier path is proven.
- [x] Ensure a barrier does not block normal mint operations beyond the bounded
      serialization needed to capture its terminal boundary.

### Exit tests

- [x] A member starting far behind catches up and becomes healthy while other
      members continuously finalize operations.
- [x] Concurrent barrier requests from multiple members produce one effective
      checkpoint generation or harmless deterministic duplicates.
- [x] Minority-only requests cannot forge or promote a checkpoint.
- [x] Traffic continues before and after the barrier without reordered or lost
      operations.
- [x] Barrier replay after restart derives the same checkpoint ID.
- [x] Recovery checkpoint formation remains possible with one unavailable
      member as long as consensus and signature thresholds are satisfied.
- [x] Idle steady state emits no barrier traffic, signatures, catch-up requests,
      or repeated checkpoint work.

### Progress notes

`FederationRecoveryBarrier` is a versioned consensus item bound to the
federation ID, latest scheduled sequence/anchor, and the proposer's finalized
cursor. Its stable generation ID is a domain-separated hash of only those
fields. The item contains no checkpoint boundary, rolling digest, application
outcome, or materialized-state claim. Any authenticated member may request
recovery through the existing private summary request, but only a locally
healthy member with a live AlephBFT session may propose the item.

Each member admits at most one queued or awaiting-finality barrier and reserves
a 30-second local proposal lease before constructing it. A checkpoint is reused
only when it already covers the requested finalized frontier; otherwise a new
ordered barrier is required because catch-up may not apply an uncertified tail.
The lease and one-pending admission prevent repeated requests from turning into
unbounded audit/signature work without making restart cost depend on federation
age. Concurrent proposers may still order harmless barriers, but one complete
finalized unit produces at most one effective candidate boundary.

Finalization durably records the barrier through the ordinary item pipeline.
A cursor beyond the barrier's own ordered position is terminally rejected
without rejecting the rest of its unit. After every operation in the complete
unit has reached terminal application, checkpoint capture derives the boundary
locally, retains the latest verified scheduled anchor and sequence, and enters
the existing candidate/signature/quorum/exchange/promotion path. A stale-anchor
barrier requests a replacement instead of producing a forked lineage. Startup
replays the last complete finalized unit's accepted barrier so a crash between
durable finality and candidate persistence derives the same checkpoint ID.

The former two-identical-observation latch and quorum matching of peer
`stable_tip` summaries have been removed. Traffic-before/barrier/traffic-after
tests prove the entire unit is captured without reordering or loss; concurrent
proposal, replay, minority non-promotion, one-unavailable-member quorum, local
lease, covered-frontier suppression, and idle-no-work tests cover the bounded
protocol. The pre-existing long-offline checkpoint catch-up and live-tail tests
continue to cover promotion and readiness from the ordered recovery proof.

Local verification completed with:

- `cargo test -p cdk-common --features federation recovery_barrier --
  --nocapture` plus the invalid-future-cursor test;
- focused Axum concurrent-request, complete-unit traffic, restart replay,
  one-unavailable-member, proposal-lease, and idle tests;
- `cargo test -p cdk-axum --features federation checkpoint -- --nocapture`
  (28 passed);
- `cargo check -p cdk-axum --all-targets --features federation`; and
- `cargo fmt --all -- --check` and `git diff --check`.

### Commit gate

- [x] Commit the ordered recovery barrier, removal of quiet-tip logic, and
      sustained-load tests before starting milestone 8.

## Milestone 8: Transport and request-state consolidation

Status: complete

### Objective

Give checkpoint history pages and opaque AlephBFT backup pages separate,
minimal transport contracts and eliminate impossible recovery-request states.

### Checklist

- [x] Introduce a focused checkpoint catch-up page containing only fields needed
      to verify and advance journal/finalized/application history to a target.
- [x] Introduce a separate AlephBFT backup page request/response with its own
      session, chunk, byte, authentication, timeout, and completion rules.
- [x] Remove dummy or empty journal response construction used solely to carry
      backup data.
- [x] Keep signed private REST authentication, receiver binding, message-kind
      isolation, replay windows, and body hashes intact.
- [x] Replace boolean-plus-optional recovery request state with one explicit
      state machine, or delete it entirely if the ordered barrier makes it
      unnecessary.
- [x] Consolidate checkpoint summary transport around the canonical frontier
      projection and verified checkpoint IDs.
- [x] Split the oversized transport module into domain files without creating
      circular ownership or duplicate serialization definitions.
- [x] Remove obsolete experimental aliases and constructors under the
      cut-through policy.

### Exit tests

- [x] Journal catch-up cannot accidentally carry or interpret AlephBFT backup
      bytes.
- [x] Backup recovery cannot mutate journal, application, checkpoint, or trusted
      frontier state.
- [x] Wrong kind, sender, receiver, sequence, session, checkpoint ID, or body
      hash is rejected before mutation.
- [x] Pagination and size bounds are independently enforced for history and
      backup endpoints.
- [x] Wire round-trip tests and authenticated REST integration tests pass for
      every new request/response type.

### Progress notes

`FederationJournalCatchUpRequest` and
`FederationJournalCatchUpResponse` now carry only checkpoint-bounded journal,
finalized-item, and terminal-application evidence. Their serde contracts reject
the removed `aleph_bft_backup` field. The generic non-target catch-up client and
the `with_aleph_bft_backup` constructors were deleted, so backup recovery can no
longer fabricate an empty journal page merely to move opaque bytes.

Opaque live-session backup transfer now owns the `AlephBftBackup` signed message
kind, `FederationAlephBftBackupRequest/Response` types, and
`/federation/v1/aleph-bft/backup` route. The request binds an exact session and
next chunk. The signed response additionally binds federation, serving member,
request sequence, kind, body hash, session, and starting chunk. Raw bytes,
encoded bytes, chunks per page, request body, completion, and no-progress rules
are validated independently of checkpoint-history pagination. The peer client
uses its own bounded request call and timeout; session-wide chunk/byte bounds
still apply before atomic backup replacement.

The backup wire types moved into the focused
`federation/transport/aleph_bft_backup.rs` domain module, and Axum serving moved
into `backup_routes.rs`. Checkpoint summaries no longer expose the obsolete
quiet-tip `stable_tip`; candidates and quorums are represented only by canonical
checkpoint IDs, kinds, sequences, and committed frontiers.

Recovery demand and the proposal deadline are now one mutex-protected
`Idle | Requested { next_proposal_at }` state machine. Request, lease, and clear
transitions cannot create the prior boolean/optional combinations.

Wire tests reject backup fields in history messages, altered signed bodies,
wrong receivers, wrong members/cursors, no-progress pages, and independent
chunk/raw/encoded limits. Axum tests prove wrong-kind rejection before replay
state mutation, bounded authenticated paging, malformed-page atomicity,
predecode oversize rejection, and that successful backup replacement leaves
journal, materialized application, checkpoint, and trusted frontier state
unchanged.

Local verification completed with:

- all 44 `cdk-common` federation transport tests;
- the Axum backup suite (22 passed);
- the Axum catch-up suite (26 passed, 1 manual process stress test ignored);
- the Axum checkpoint suite (28 passed);
- `cargo check -p cdk-axum --all-targets --features federation`; and
- `cargo fmt --all -- --check` and `git diff --check`.

### Commit gate

- [x] Commit transport/request-state consolidation and tests before starting
      milestone 9.

## Milestone 9: Remove whole-runtime cloning

Status: complete

### Objective

Make submission and finalization work proportional to the new delta rather than
the retained checkpoint suffix while preserving persistence-before-publication
atomicity.

### Checklist

- [x] Measure the remaining clone sites after prior consolidation.
- [x] Introduce prepared submission and finalized-unit deltas, or an equivalent
      validate/persist/commit API.
- [x] Ensure preparation does not mutate the published runtime.
- [x] Persist pending/finalized/journal data before committing the in-memory
      delta where required by the existing durability contract.
- [x] Make failed persistence discard the prepared delta without manual
      rollback of unrelated state.
- [x] Store immutable retained history behind shared ownership where it must
      remain available in memory.
- [x] Keep duplicate/conflict detection deterministic across concurrent
      submissions and finalized units.
- [x] Remove `FederationConsensusRuntime::clone()` from production hot paths.

### Exit tests

- [x] Injected pending and finalization persistence failures leave the published
      runtime unchanged.
- [x] Concurrent duplicate/conflicting submissions retain existing behavior.
- [x] Submission and finalization allocation/time do not grow linearly with the
      retained suffix merely because of rollback protection.
- [x] Checkpoint rebasing, catch-up replacement, session rotation, and duplicate
      finalized-unit response recovery still work.
- [x] Benchmarks show the expected reduction relative to milestone 0.

### Progress notes

The two remaining live clone sites were the submission and finalized-unit paths
in `FederationNodeRuntime`. Both now use opaque prepared deltas. Preparation
performs all size, duplicate, conflict, authorization, unit, finalized-index,
journal-index, digest, and response derivation without mutating the runtime.
The existing consensus mutex remains held across the database transaction, so
after persistence succeeds commit is a short, non-fallible application of the
already validated queue, finalization-handler, journal, and pending-unit
changes. A failed transaction simply drops the prepared value; no rollback or
replacement runtime exists.

The simplest ownership model needed no second retained-history owner. The
published runtime keeps its journal, finalized items, and indexes single-owned;
preparation borrows them and allocates only per-unit sets/maps, duplicate rows
actually referenced by the delta, and new finalized/journal/response objects.
The bounded recovery suffix verifier still uses its fail-closed clone outside
the ordinary submission/finalization hot paths, where milestone 5 already caps
the retained suffix and one in-flight page.

The milestone-0 clone baseline retained 2,372, 151,871, and 608,771 canonical
bytes at 1, 64, and 256 operations. The replacement benchmark prepared one
submission plus one finalized operation at those same sizes. Its encoded delta
was 3,768, 3,779, and 3,790 bytes, while diagnostic debug preparation times
were 0.376, 0.393, and 0.384 ms. A test-only simulation of the former two-clone
path took 0.510, 0.549, and 0.918 ms. Timing is diagnostic; the deterministic
result is that retained bytes grow by more than 250x while prepared bytes stay
within 22 bytes and ordinary production work copies none of the retained
suffix.

Focused local verification completed with:

- all 48 `cdk-common` federation consensus tests and strict `cdk-common`
  Clippy;
- all 28 Axum checkpoint tests and 26 catch-up tests, with the existing
  process-level stress test ignored;
- prepared-delta sizing/timing, pending and finalization persistence-failure,
  duplicate pending submission, finalized conflict, duplicate-response
  recovery, and session-rotation tests;
- `cargo check -p cdk-axum --all-targets --features federation`; and
- `cargo fmt --all -- --check` and `git diff --check`.

Workspace/Axum `-D warnings` Clippy is not recorded as passing under the current
stable toolchain because unrelated existing lints outside this milestone are
reported; no milestone-9 diagnostic identified a new warning in the changed
paths.

### Commit gate

- [x] Commit runtime clone removal and performance/failure tests before starting
      milestone 10.

## Milestone 10: Bootstrap, backup, operator, and retention contract

Status: complete

### Objective

Make the operational limits of checkpoints explicit and ensure genesis replay,
database restore, secret material, history retention, and audit procedures form
a coherent recovery story.

### Checklist

- [x] Document that empty-database bootstrap requires genesis replay and valid
      pre-existing member identity, BLS, and FROST secret material.
- [x] Document the minimum journal, finalized-item, scheduled-checkpoint, and
      active-session backup retention needed to support the oldest advertised
      recovery point.
- [x] Define a database-backup manifest bound to federation ID, config digest,
      trusted checkpoint ID, durable frontier, schema version, and required
      secret-storage generation.
- [x] Add operator validation that a restored database and trusted checkpoint
      match before startup.
- [x] Specify how operators recover from the durable materialized-state audit
      mismatch latch; do not add an automatic clear path.
- [x] Specify pruning rules that never delete bodies required by the oldest
      supported bootstrap or backup recovery point.
- [x] Ensure status and inspection commands expose the trusted base, retained
      recovery floor, current suffix size/bytes, active session backup status,
      and audit state without exposing secrets or local paths.
- [x] Decide in a reviewed follow-up document whether peer-to-peer state
      snapshots are needed; keep them outside this implementation unless scope
      is deliberately amended.

### Exit tests

- [x] Empty-database genesis catch-up succeeds through multiple scheduled
      checkpoints and a recovery checkpoint with bounded memory.
- [x] Database backup plus matching secrets and checkpoint restarts at the exact
      fast path without prefix reads.
- [x] Database backup paired with the wrong federation, config, checkpoint,
      schema, or secret generation fails before routes open.
- [x] Retention tests prove required bodies and the active backup cannot be
      pruned below the advertised recovery floor.
- [x] Operator audit success and durable mismatch-latch behavior are covered by
      CLI or route integration tests.

### Progress notes

Version 1 advertises one simple recovery floor: genesis. Journal bodies,
finalized-item bodies, and scheduled checkpoint quorums remain available from
that floor; redundant recovery proofs may be pruned, and active-session backup
pruning is transactionally protected. This avoids a second retention watermark
or peer snapshot protocol while preserving deterministic empty-database replay.

`FederationDatabaseBackupManifest` binds a quiesced database snapshot to the
federation and public-config digests, member ID, trusted checkpoint, exact
durable frontier, schema contract, active AlephBFT session/backup size, and a
domain-separated generation of the external member identity/BLS secret file.
`cdk-mintd federation backup-manifest` creates the private manifest. Configured
restore validation reconstructs it from the opened database and supplied
member configuration, compares every field, and runs before DKG reconciliation,
mint construction, or route startup. FROST state remains part of the database
snapshot and must be restored with it.

The operator guide records normal restart, checkpoint-bound backup creation,
restore, genesis bootstrap, and the one-way materialized-state audit mismatch
latch. Version 1 has neither a background audit nor an automatic latch-clear
path. The separate snapshot decision records peer-to-peer materialized-state
snapshots as deliberately deferred until retained history or measured replay
cost justifies their protocol and operational complexity.

Local verification used SQLite and the fake wallet. Focused common manifest,
consensus, transport, mintd restore/config/status, Axum checkpoint/status, and
shared SQLite checkpoint/backup persistence tests pass. The Axum checkpoint
suite includes empty-database replay through two scheduled checkpoints and a
recovery checkpoint, plus exact-checkpoint restart with zero prefix reads.
PostgreSQL federation code compiles; no live PostgreSQL service was used.
Changed common and SQL crates pass strict Clippy. Broader strict Clippy remains
blocked by unrelated existing warnings in core `cdk` and mintd smoke/setup code;
no warning reported a milestone-10 changed path.

### Commit gate

- [x] Commit bootstrap/backup/retention behavior, tests, and operator
      documentation before starting milestone 11.

## Milestone 11: Adversarial integration, performance, and final cleanup

Status: in progress

### Objective

Prove that the consolidated implementation is simpler, bounded, crash-safe, and
at least as secure as the implementation it replaces, then remove superseded
code and update the architectural record.

### Checklist

- [ ] Delete superseded frontier structs, comparison helpers, recovery
      constructors, batch aggregators, quiet-tip state, overloaded transport
      fields, compatibility scaffolding, and dead tests.
- [ ] Update `docs/federated-cashu-implementation-plan.md` and
      `docs/federated-cdk-checkpoint-implementation-plan.md` to point to the
      consolidated design and accurately describe current behavior.
- [ ] Add an architecture guide showing the final object model, ownership,
      startup sequence, catch-up sequence, checkpoint lifecycle, lock order,
      and failure recovery.
- [ ] Compare production lines, public/internal API count, duplicate validation
      helpers, runtime allocations, startup work, and catch-up memory against
      milestone 0.
- [ ] Run multi-member crash, restart, partition, delayed peer, invalid source,
      duplicate delivery, sustained load, and session rotation campaigns.
- [ ] Verify logs, metrics, and errors are bounded and contain no private
      material, credentials, sensitive payloads, machine-specific paths, or PII.
- [ ] Obtain an independent security/protocol review of the final frontier,
      verifier, barrier, promotion, and replay state machines.
- [ ] Record every command run and every test intentionally not run.

### Required final test matrix

- [ ] Exact trusted-checkpoint restart performs no historical prefix reads and
      no full materialized-state scan.
- [ ] Trusted checkpoint plus terminal local tail validates only the suffix.
- [ ] Trusted checkpoint plus partially applied tail replays safely after every
      injected crash boundary.
- [ ] Genesis bootstrap remains deterministic and bounded per configured page
      and staged-byte limits.
- [ ] One and multiple scheduled-checkpoint catch-ups promote only exact verified
      targets.
- [ ] Sustained-load recovery barrier lets a lagging member reach healthy
      without stopping valid traffic.
- [ ] Invalid checkpoint signatures, duplicate signers, wrong federation/config,
      rollback, lineage fork, equal-cursor digest mismatch, partial units,
      altered envelopes, altered application outcomes, and altered materialized
      digest all fail closed.
- [ ] Invalid or unavailable body and backup sources fail over without advancing
      trust.
- [ ] A crash never leaves a trusted checkpoint ahead of journal, finalized, or
      terminal application state.
- [ ] Pending operations survive restart without double finalization or effects.
- [ ] AlephBFT backup recovery remains separately bounded and cannot authorize
      mint state.
- [ ] SQLite and PostgreSQL federation test suites pass.
- [ ] `cargo test -p cdk-common --features federation` passes.
- [ ] `cargo test -p cdk --features federation` passes.
- [ ] `cargo test -p cdk-axum --features federation --lib` passes, using network
      permission for localhost-listener tests from the first attempt.
- [ ] Relevant integration tests pass with deterministic test-thread settings
      only where the existing suite requires them.
- [ ] `cargo check --workspace --all-targets` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo test --lib --workspace --exclude cdk-postgres` passes.
- [ ] Federation-disabled standalone and WASM checks remain successful where
      affected by shared-code changes.

### Final acceptance criteria

- [ ] The implementation has one canonical frontier model.
- [ ] Startup and catch-up use one suffix verifier.
- [ ] Consensus recovery has one production entry point.
- [ ] Checkpoint and catch-up workflows have focused owners.
- [ ] Catch-up memory and bytes are explicitly bounded.
- [ ] Partially applied checkpoint suffixes recover after crashes.
- [ ] Recovery makes progress under sustained finalization.
- [ ] Production hot paths do not clone the complete consensus runtime.
- [ ] Transport separates history catch-up from AlephBFT backup transfer.
- [ ] No existing security invariant has been removed or weakened.
- [ ] Documentation and operator procedures match the code.
- [ ] An independent reviewer can follow checkpoint authority from quorum proof
      through suffix verification, deterministic application, promotion, and
      readiness without consulting parallel implementations.

### Progress notes

Record final results, residual risks, independent review conclusions, and links
to follow-up work here.

### Commit gate

- [ ] Commit final cleanup, documentation, and verification evidence. Mark this
      plan complete only after the final commit is present and all acceptance
      criteria are checked.

## Implementation discipline

- Prefer deleting parallel paths over wrapping both in a new abstraction.
- Introduce a shared type only when it owns a real invariant or removes repeated
  state; do not replace concrete duplication with generic indirection that is
  harder to review.
- Keep cryptographic verification close to the type transition from raw to
  verified data.
- Keep database writes and trust promotion explicit. Do not hide transactional
  boundaries behind constructors or serialization.
- Use structured federation errors with enough cursor/checkpoint context for
  operators, while keeping messages bounded and redacted.
- Use full-path `tracing` macros and retain low-overhead metrics for checkpoint
  creation, verification, staged bytes, replayed entries, source failures,
  promotion, and readiness.
- Do not change timeout values, checkpoint intervals, queue bounds, or test
  serialization merely to make a failing milestone pass. Measure and address
  the underlying state-machine or resource problem.
- If a milestone reveals that a proposed consolidation would weaken a safety
  property, stop, mark the milestone blocked, record the conflict here, and
  obtain review before continuing.

## Completion definition

This plan is complete only when every milestone is marked complete, every
milestone boundary has a corresponding Git commit, all final acceptance tests
pass, the old duplicate implementations are removed, and the overarching
Federated Cashu documentation describes the consolidated implementation rather
than the system that preceded it.
