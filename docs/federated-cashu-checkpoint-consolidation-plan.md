# Federated Cashu checkpoint and catch-up consolidation plan

Status: in progress
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
| 1 | Crash-safe checkpoint suffix replay | In progress | — | Separating ordered-tail verification from terminal application replay while retaining fail-closed startup. |
| 2 | Canonical frontier and verified checkpoint types | Not started | — | — |
| 3 | Shared incremental suffix verifier | Not started | — | — |
| 4 | Unified consensus recovery API | Not started | — | — |
| 5 | Bounded streaming checkpoint catch-up | Not started | — | — |
| 6 | Focused runtime component split | Not started | — | — |
| 7 | AlephBFT-ordered recovery barrier | Not started | — | — |
| 8 | Transport and request-state consolidation | Not started | — | — |
| 9 | Remove whole-runtime cloning | Not started | — | — |
| 10 | Bootstrap, backup, operator, and retention contract | Not started | — | — |
| 11 | Adversarial integration, performance, and final cleanup | Not started | — | — |

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

Status: not started

### Objective

Make one object own the definition of federation recovery progress and use the
Rust type system to separate raw checkpoint data from verified checkpoint
authority.

### Checklist

- [ ] Inventory every duplicated cursor/digest grouping and every handwritten
      `matches`, `covers`, or monotonicity comparison.
- [ ] Introduce the canonical committed-frontier object and focused journal,
      finalized, application, and AlephBFT component cursors.
- [ ] Embed or losslessly project that object in checkpoint, boundary, durable
      metadata, summary, status, and catch-up binding types.
- [ ] Move exact equality, coverage, non-regression, complete-unit, roster
      watermark, and fully-terminal validation onto the owning types.
- [ ] Introduce a verified checkpoint/quorum wrapper constructible only after
      version, federation, config digest, lineage, signature, distinct-signer,
      and threshold validation.
- [ ] Change promotion and recovery APIs to require the verified wrapper where
      practical.
- [ ] Remove manual partial comparisons and duplicated validation helpers after
      callers migrate.
- [ ] Keep serialization explicit and stable within the cut-through protocol;
      do not expose private wrapper invariants through unchecked constructors.
- [ ] Update SQLite and PostgreSQL mappings together if the canonical object
      changes stored columns or serialization.

### Exit tests

- [ ] Compile-time API tests or module privacy prevent constructing a verified
      checkpoint without quorum verification.
- [ ] Property tests cover reflexive exact matching, monotonic coverage,
      rollback rejection, changed-digest rejection at equal cursors, scheduled
      lineage, recovery anchoring, and roster watermark equality.
- [ ] Serialization round trips preserve checkpoint IDs and canonical bytes.
- [ ] Existing valid checkpoint fixtures retain their intended semantic fields;
      obsolete experimental fixtures are regenerated rather than adapted.
- [ ] No production caller manually compares a subset of canonical frontier
      fields to decide trust or readiness.

### Progress notes

Record final type names, field ownership, serialization decisions, and removed
helpers here.

### Commit gate

- [ ] Commit the canonical frontier/type migration and tests before starting
      milestone 3.

## Milestone 3: Shared incremental suffix verifier

Status: not started

### Objective

Replace parallel startup and network catch-up validators with one incremental
verification engine.

### Checklist

- [ ] Implement a verifier initialized from genesis or a verified checkpoint.
- [ ] Incrementally validate finalized-item contiguity, unit completeness,
      body hashes, provenance, session transitions, and per-member watermarks.
- [ ] Incrementally derive accepted journal operations from finalized items and
      validate journal indices, operation IDs, canonical envelopes, conflict
      outcomes, consensus evidence, and order digests.
- [ ] Incrementally validate terminal application order and application digest.
- [ ] Make overlap validation an explicit pre-suffix adapter with one canonical
      comparison implementation.
- [ ] Return an opaque verified suffix/frontier result only after exact target
      equality.
- [ ] Use the verifier for trusted-checkpoint local startup suffix validation.
- [ ] Use the same verifier for remote checkpoint-bound catch-up.
- [ ] Use the same verifier for genesis reconstruction where doing so removes
      duplicate logic without slowing the exact-checkpoint path.
- [ ] Delete superseded startup, catch-up rolling-frontier, and overlap helper
      implementations.

### Exit tests

- [ ] Identical suffixes presented through startup and network catch-up produce
      identical verified results.
- [ ] A mutation corpus changing every committed field is rejected by both
      paths at the same logical validation stage.
- [ ] Chunk boundaries, page sizes, and item grouping do not change the final
      verified frontier.
- [ ] Partial AlephBFT units are rejected without advancing verified state.
- [ ] Duplicate and invalidly authenticated finalized operations reproduce the
      same journal behavior as live finalization.
- [ ] Exact-checkpoint startup consumes no historical prefix rows.

### Progress notes

Record the verifier state machine, invariants owned by each method, and deleted
duplicate functions here.

### Commit gate

- [ ] Commit the shared verifier migration and tests before starting milestone
      4.

## Milestone 4: Unified consensus recovery API

Status: not started

### Objective

Collapse the combinatorial consensus recovery constructors into one reviewed
path based on a recovery base and a verified suffix.

### Checklist

- [ ] Introduce the genesis/checkpoint recovery-base enum or equivalent.
- [ ] Make the unified recovery function consume only a verified suffix result,
      not unrelated raw journal and finalized vectors.
- [ ] Reconstruct journal base, finalized cursors, duplicate guards, conflict
      guards, session, finalization index, and per-member watermarks in one
      implementation.
- [ ] Retain explicit roster/authentication validation for all production
      recovery calls.
- [ ] Provide a narrow test builder for unit tests that do not need production
      authentication plumbing.
- [ ] Migrate startup, checkpoint promotion rebasing, catch-up, and test callers.
- [ ] Remove obsolete `from_checkpoint`, `from_checkpoint_suffix`,
      `from_checkpoint_and_finalized_items`, and parallel authenticated variants
      once no longer needed.
- [ ] Ensure pending local operation requeue remains separate from authoritative
      recovered history.

### Exit tests

- [ ] Genesis, exact checkpoint, checkpoint plus local tail, checkpoint plus
      remote tail, later session, duplicate finalized unit, and pending-operation
      restart scenarios use the same recovery entry point.
- [ ] Invalid roster, authorization, session rollback, replayed pre-checkpoint
      unit, and watermark mismatch tests remain fail-closed.
- [ ] Recovered runtime status and journal are byte-for-byte or semantically
      identical to uninterrupted live finalization.
- [ ] Public production recovery API surface is smaller than before this
      milestone.

### Progress notes

Record the removed constructors, remaining test-only entry points, and recovery
API rationale here.

### Commit gate

- [ ] Commit the unified recovery API and caller migration before starting
      milestone 5.

## Milestone 5: Bounded streaming checkpoint catch-up

Status: not started

### Objective

Make recovery memory and network work explicitly bounded while retaining
complete target validation and data-before-trust promotion.

### Checklist

- [ ] Add a consensus-configured or protocol-fixed maximum checkpoint batch
      byte budget in addition to item-count intervals.
- [ ] Define whether scheduled eligibility triggers on journal count, finalized
      count, cumulative canonical bytes, or any of those thresholds.
- [ ] Bound request, response, staged, decoded, and aggregate in-flight bytes.
- [ ] Replace whole-batch `Vec` aggregation with page-by-page incremental
      verification through the shared suffix verifier.
- [ ] Select and document one crash-safe staging model: durable staging tables,
      durable untrusted suffix rows, or incrementally applied verified pages
      while retaining the old trusted checkpoint.
- [ ] Ensure a failed page or source cannot contaminate the trusted frontier and
      can be discarded or safely overlapped on retry.
- [ ] Persist enough rolling verifier progress to resume after a crash without
      downloading or replaying already durable pages unnecessarily.
- [ ] Promote the target checkpoint only after every target cursor, digest, and
      watermark matches exactly and business application is terminal.
- [ ] Preserve one-source body fetching with source failover and transferable
      quorum-proof fetching.
- [ ] Add bounded metrics for pages, canonical bytes, staged bytes, applied
      bytes, retries, and peak in-flight batch bytes.

### Exit tests

- [ ] Catch-up memory remains within a deterministic configured bound for a
      checkpoint containing the maximum supported item count and large
      operations.
- [ ] One-page and many-page delivery produce identical state and checkpoint
      promotion.
- [ ] Crash after every page, after staging, during application, and before
      promotion resumes safely from durable progress.
- [ ] Invalid data on the first source is rejected and a second source can
      complete recovery from the unchanged trusted base.
- [ ] No-progress, oversized-page, oversized-batch, integer-overflow, and
      decompression/decoding amplification cases fail with structured errors.
- [ ] Genesis catch-up and multiple sequential scheduled checkpoints remain
      supported without whole-history memory accumulation.

### Progress notes

Record selected staging design, bounds, worst-case calculations, and measured
peak work here.

### Commit gate

- [ ] Commit bounded streaming catch-up and all fault/resource tests before
      starting milestone 6.

## Milestone 6: Focused runtime component split

Status: not started

### Objective

Reduce the federation node god object and make checkpoint/catch-up ownership and
lock boundaries reviewable.

### Checklist

- [ ] Extract checkpoint capture, signing, quorum assembly, fetch, persistence,
      promotion, and audit into a checkpoint manager.
- [ ] Extract target selection, page acquisition, verification, staging,
      application, and source failover into a catch-up manager.
- [ ] Extract submission, pending persistence, finalization persistence,
      deterministic application, and response publication into an operation
      pipeline.
- [ ] Extract readiness transitions and quorum-compatible peer-view decisions
      into a readiness coordinator.
- [ ] Move DKG persistence and ceremony state out of checkpoint/catch-up runtime
      code without redesigning DKG behavior.
- [ ] Keep on-chain wallet/FROST reconciliation behind a focused interface
      invoked after deterministic application.
- [ ] Define the owner and acquisition order of consensus, operation-application,
      checkpoint, recovery, and database transaction locks.
- [ ] Remove test-only flags from production component layouts where a test
      harness or injected trait can provide the same fault control.
- [ ] Keep router handlers thin and free of business-state decisions.

### Exit tests

- [ ] Existing behavior tests pass without reaching through component-private
      state.
- [ ] Lock-order tests or structured concurrency tests cover simultaneous live
      finality, checkpoint capture, catch-up, signature exchange, status reads,
      and shutdown.
- [ ] No component holds a network request open while holding the application
      or consensus mutation lock unless explicitly justified and tested.
- [ ] `FederationNodeRuntime` becomes primarily composition and lifecycle
      ownership rather than the implementation of every workflow.
- [ ] Module documentation identifies each component's authority and forbidden
      responsibilities.

### Progress notes

Record final module boundaries, lock order, dependency direction, and line/API
reductions here.

### Commit gate

- [ ] Commit the component split and concurrency tests before starting
      milestone 7.

## Milestone 7: AlephBFT-ordered recovery barrier

Status: not started

### Objective

Remove dependence on a naturally quiet tip and guarantee that recovery can
select a quorum-signable boundary during sustained valid traffic.

### Checklist

- [ ] Define a versioned, bounded recovery-barrier consensus item with a stable
      generation/request ID.
- [ ] Define who may request or propose a barrier and how duplicate concurrent
      requests converge.
- [ ] Rate-limit or lease barrier generation so a lagging or malicious member
      cannot create unbounded consensus traffic.
- [ ] Finalize the barrier through ordinary AlephBFT ordering and durable
      finalized-item persistence.
- [ ] Define the exact complete-unit, terminal-application boundary selected by
      the finalized barrier.
- [ ] Derive the existing recovery checkpoint payload at that boundary without
      accepting any caller-provided digest or materialized-state claim.
- [ ] Preserve the latest scheduled checkpoint as the recovery checkpoint's
      anchor and do not increment the scheduled sequence.
- [ ] Reuse ordinary checkpoint candidate, signature, quorum, dissemination,
      catch-up, and promotion logic after boundary selection.
- [ ] Remove quiet-tip two-observation request state and peer-summary coupling
      once the barrier path is proven.
- [ ] Ensure a barrier does not block normal mint operations beyond the bounded
      serialization needed to capture its terminal boundary.

### Exit tests

- [ ] A member starting far behind catches up and becomes healthy while other
      members continuously finalize operations.
- [ ] Concurrent barrier requests from multiple members produce one effective
      checkpoint generation or harmless deterministic duplicates.
- [ ] Minority-only requests cannot forge or promote a checkpoint.
- [ ] Traffic continues before and after the barrier without reordered or lost
      operations.
- [ ] Barrier replay after restart derives the same checkpoint ID.
- [ ] Recovery checkpoint formation remains possible with one unavailable
      member as long as consensus and signature thresholds are satisfied.
- [ ] Idle steady state emits no barrier traffic, signatures, catch-up requests,
      or repeated checkpoint work.

### Progress notes

Record the barrier protocol, rate limit, sustained-load results, and removal of
quiet-tip state here.

### Commit gate

- [ ] Commit the ordered recovery barrier, removal of quiet-tip logic, and
      sustained-load tests before starting milestone 8.

## Milestone 8: Transport and request-state consolidation

Status: not started

### Objective

Give checkpoint history pages and opaque AlephBFT backup pages separate,
minimal transport contracts and eliminate impossible recovery-request states.

### Checklist

- [ ] Introduce a focused checkpoint catch-up page containing only fields needed
      to verify and advance journal/finalized/application history to a target.
- [ ] Introduce a separate AlephBFT backup page request/response with its own
      session, chunk, byte, authentication, timeout, and completion rules.
- [ ] Remove dummy or empty journal response construction used solely to carry
      backup data.
- [ ] Keep signed private REST authentication, receiver binding, message-kind
      isolation, replay windows, and body hashes intact.
- [ ] Replace boolean-plus-optional recovery request state with one explicit
      state machine, or delete it entirely if the ordered barrier makes it
      unnecessary.
- [ ] Consolidate checkpoint summary transport around the canonical frontier
      projection and verified checkpoint IDs.
- [ ] Split the oversized transport module into domain files without creating
      circular ownership or duplicate serialization definitions.
- [ ] Remove obsolete experimental aliases and constructors under the
      cut-through policy.

### Exit tests

- [ ] Journal catch-up cannot accidentally carry or interpret AlephBFT backup
      bytes.
- [ ] Backup recovery cannot mutate journal, application, checkpoint, or trusted
      frontier state.
- [ ] Wrong kind, sender, receiver, sequence, session, checkpoint ID, or body
      hash is rejected before mutation.
- [ ] Pagination and size bounds are independently enforced for history and
      backup endpoints.
- [ ] Wire round-trip tests and authenticated REST integration tests pass for
      every new request/response type.

### Progress notes

Record final endpoint/type layout and removed transport fields here.

### Commit gate

- [ ] Commit transport/request-state consolidation and tests before starting
      milestone 9.

## Milestone 9: Remove whole-runtime cloning

Status: not started

### Objective

Make submission and finalization work proportional to the new delta rather than
the retained checkpoint suffix while preserving persistence-before-publication
atomicity.

### Checklist

- [ ] Measure the remaining clone sites after prior consolidation.
- [ ] Introduce prepared submission and finalized-unit deltas, or an equivalent
      validate/persist/commit API.
- [ ] Ensure preparation does not mutate the published runtime.
- [ ] Persist pending/finalized/journal data before committing the in-memory
      delta where required by the existing durability contract.
- [ ] Make failed persistence discard the prepared delta without manual
      rollback of unrelated state.
- [ ] Store immutable retained history behind shared ownership where it must
      remain available in memory.
- [ ] Keep duplicate/conflict detection deterministic across concurrent
      submissions and finalized units.
- [ ] Remove `FederationConsensusRuntime::clone()` from production hot paths.

### Exit tests

- [ ] Injected pending and finalization persistence failures leave the published
      runtime unchanged.
- [ ] Concurrent duplicate/conflicting submissions retain existing behavior.
- [ ] Submission and finalization allocation/time do not grow linearly with the
      retained suffix merely because of rollback protection.
- [ ] Checkpoint rebasing, catch-up replacement, session rotation, and duplicate
      finalized-unit response recovery still work.
- [ ] Benchmarks show the expected reduction relative to milestone 0.

### Progress notes

Record selected delta/ownership design and before/after measurements here.

### Commit gate

- [ ] Commit runtime clone removal and performance/failure tests before starting
      milestone 10.

## Milestone 10: Bootstrap, backup, operator, and retention contract

Status: not started

### Objective

Make the operational limits of checkpoints explicit and ensure genesis replay,
database restore, secret material, history retention, and audit procedures form
a coherent recovery story.

### Checklist

- [ ] Document that empty-database bootstrap requires genesis replay and valid
      pre-existing member identity, BLS, and FROST secret material.
- [ ] Document the minimum journal, finalized-item, scheduled-checkpoint, and
      active-session backup retention needed to support the oldest advertised
      recovery point.
- [ ] Define a database-backup manifest bound to federation ID, config digest,
      trusted checkpoint ID, durable frontier, schema version, and required
      secret-storage generation.
- [ ] Add operator validation that a restored database and trusted checkpoint
      match before startup.
- [ ] Specify how operators recover from the durable materialized-state audit
      mismatch latch; do not add an automatic clear path.
- [ ] Specify pruning rules that never delete bodies required by the oldest
      supported bootstrap or backup recovery point.
- [ ] Ensure status and inspection commands expose the trusted base, retained
      recovery floor, current suffix size/bytes, active session backup status,
      and audit state without exposing secrets or local paths.
- [ ] Decide in a reviewed follow-up document whether peer-to-peer state
      snapshots are needed; keep them outside this implementation unless scope
      is deliberately amended.

### Exit tests

- [ ] Empty-database genesis catch-up succeeds through multiple scheduled
      checkpoints and a recovery checkpoint with bounded memory.
- [ ] Database backup plus matching secrets and checkpoint restarts at the exact
      fast path without prefix reads.
- [ ] Database backup paired with the wrong federation, config, checkpoint,
      schema, or secret generation fails before routes open.
- [ ] Retention tests prove required bodies and the active backup cannot be
      pruned below the advertised recovery floor.
- [ ] Operator audit success and durable mismatch-latch behavior are covered by
      CLI or route integration tests.

### Progress notes

Record the backup manifest, retention policy, operator commands, and deferred
snapshot decision here.

### Commit gate

- [ ] Commit bootstrap/backup/retention behavior, tests, and operator
      documentation before starting milestone 11.

## Milestone 11: Adversarial integration, performance, and final cleanup

Status: not started

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
