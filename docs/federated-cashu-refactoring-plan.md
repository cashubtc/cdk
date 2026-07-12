# Federated Cashu refactoring plan

## Purpose

This document reviews the current CDK federation implementation from a code
quality and architecture perspective and defines a staged refactoring plan. The
goal is to make the implementation smaller, easier to navigate, harder to misuse,
and less expensive to maintain without weakening any consensus, journal,
threshold-signing, catch-up, or wallet-recovery invariant.

This is a refactoring plan, not a protocol redesign. Public Cashu behavior,
federation operation ordering, BLS threshold signing, durable operation replay,
and standalone mint behavior must remain unchanged unless a section explicitly
states otherwise.

Two project decisions are fixed for this plan:

1. AlephBFT is the only federation consensus backend. The in-memory consensus
   backend was a development harness and must be removed rather than maintained
   as a second production abstraction.
2. Mint federation implementation should move from the monolithic
   `crates/cdk/src/mint/federation.rs` into focused modules under
   `crates/cdk/src/mint/federation/`, after dead protocol paths have been
   removed. The final module boundaries should follow the surviving
   responsibilities rather than a preselected one-file-per-operation layout.

## Executive assessment

The implementation has a strong protocol foundation:

- Federation operations have canonical, versioned envelopes.
- Operation IDs, canonical envelopes, complete conflict-key sets, and balance
  audits are explicit and deterministic. The redundant request-transcript
  layer and singular journal conflict key have been removed.
- Accepted order, application status, consensus evidence, and materialized-state
  digests are modeled and persisted.
- State-mutating mint, swap, melt, quote payment-observation, and keyset-rotation
  paths pass through explicit federation operations. Restore and check-state are
  threshold reads and should not be ordered merely for symmetry.
- The wallet validates member advertisements and aggregates threshold BLS shares.
- Private member REST requests are authenticated and sequence checked.
- Catch-up verifies accepted ranges and quorum catch-up certificates.
- The test suite covers many real multi-member, partition, restart, and
  convergence scenarios.

The largest structural weakness is not a lack of abstraction. It is that several
different abstractions and orchestration responsibilities overlap:

- `MintState` has become the owner of much of the federation node runtime.
- `cdk-common` contains two consensus runtime/adapter families covering the same
  backend choice.
- Wallet fanout is generic at the transport level but duplicated at the quorum
  decision level.
- Mint operation application repeats transaction and journal lifecycle work.
- Materialized-state projection re-describes operation effects outside the mint
  application layer.
- Configuration validity is sometimes represented by method naming rather than
  by the type system.
- Large files, particularly the Axum federation test file, make otherwise good
  boundaries difficult to see.

The best refactors therefore establish one owner for each invariant and remove
parallel ways of expressing the same lifecycle.

## Review guidance

The checkpoint designs below are implementation guidance, not contracts. Each
checkpoint includes a current verdict based on the code after AlephBFT,
federated wallet error recovery, DKG performance work, and the broader cleanup
audit. A verdict may narrow, defer, combine, or reject a sketched abstraction
while retaining the checkpoint's useful outcome.

The dominant sequencing rule is deletion before reorganization. Do not move,
wrap, or generalize the in-memory backend, external-finalization driver,
consensus restore machinery, redundant transcript/conflict fields, or obsolete
tests before deciding whether they should simply be removed.

## Safety invariants

Every refactoring checkpoint must preserve the following invariants.

### Consensus and ordering

- AlephBFT is the sole source of federation operation order.
- No state-mutating mint, swap, melt, payment-observation, or keyset-rotation
  operation may bypass consensus in federation mode. Restore and check-state
  remain authenticated, readiness-gated threshold reads and do not require a
  no-op consensus operation.
- A finalized operation has one stable operation ID and one stable order index.
- Conflict detection remains deterministic and is evaluated against canonical
  operation contents.
- Catch-up cannot append entries without validating the exact accepted range and
  required quorum evidence.
- Gossip sequence watermarks remain liveness/authentication metadata and do not
  become a substitute for consensus order.

### Application and storage

- Domain mutations and terminal application state are persisted atomically.
- Rejected operations do not appear in `completed_operations`.
- A deterministic rejection may carry its standard Cashu `ErrorResponse`, but
  infrastructure and ambiguous failures remain retryable or unresolved.
- Replay is idempotent. Applying the same accepted operation twice cannot issue
  additional value, spend an input twice, or create mismatched quote state.
- A member that was offline can recover accepted order and deterministically
  converge its materialized database state.
- Materialized-state digests cover every consensus-relevant durable effect,
  including persisted rejection metadata, while accepted-order hashes remain
  independent of post-consensus local application metadata.

### Signing and wallet behavior

- Federation mode accepts and signs only v3/BLS aggregate keysets.
- A threshold of valid shares is required before returning an aggregate result.
- Invalid shares are excluded and cannot contribute to threshold success.
- One member cannot determine a consensus-dependent business rejection.
- Threshold agreement on the same standard Cashu error follows the existing
  `ErrorResponse -> Error -> wallet saga -> CLI` path.
- Ambiguous outcomes preserve recoverable wallet saga state.
- Standalone mint and wallet behavior remain unchanged.

### Networking and identity

- Private REST messages remain authenticated against configured federation
  member identity keys.
- Message kind, federation ID, sender ID, session, sequence, and payload bindings
  remain validated where applicable.
- Backpressure, retries, and catch-up must not weaken request authentication or
  duplicate-operation checks.

## Current structural baseline

The following sizes illustrate where responsibilities have accumulated. They are
not quality metrics by themselves, but they identify costly review surfaces.

| File | Approximate lines | Main responsibilities currently combined |
|---|---:|---|
| `crates/cdk-axum/src/federation/tests.rs` | 22,289 | Routes, consensus, faults, partitions, catch-up, restart, digest, DKG, fixtures |
| `crates/cdk-common/src/federation.rs` | 5,963 | Errors, identities, thresholds, payment policy, keysets, DKG, config, invites |
| `crates/cdk-axum/src/lib.rs` | 4,411 | General server state plus federation submission, finalization, catch-up, replay, status |
| `crates/cdk-common/src/federation/consensus.rs` | 3,855 | Consensus items, Aleph adapter, two runtime abstractions, in-memory backend and simulator |
| `crates/cdk-common/src/federation/operation.rs` | 3,381 | Operation payloads, payment evidence/reporting, conflict keys, canonicalization, audits |
| `crates/cdk/src/mint/federation.rs` | 2,660 | Admission and application for every federation operation |
| `crates/cdk/src/wallet/federation.rs` | 2,578 | Connector construction, validation, fanout, quorum decisions, aggregation, recovery |
| `crates/cdk-mintd/src/federation_setup.rs` | 2,411 | Setup orchestration, DKG phases, persistence, validation, output |
| `crates/cdk-sql-common/src/mint/federation.rs` | 2,224 | Every federation SQL record and journal row codec |
| `crates/cdk-axum/src/federation/aleph_bft_session.rs` | 1,399 | Session startup, tasks, activity, rotation, completion, apply loop |

## Target architecture

The target has one directional flow:

```mermaid
flowchart TD
    Wallet["Federated wallet connector"] --> PublicApi["Public Cashu HTTP handlers"]
    PublicApi --> NodeRuntime["FederationNodeRuntime"]
    PeerApi["Authenticated private REST handlers"] --> NodeRuntime
    NodeRuntime --> Aleph["AlephBFT consensus service"]
    Aleph --> Finalization["Ordered finalization pipeline"]
    Finalization --> MintApply["Mint federation application service"]
    MintApply --> Database["Mint database transaction"]
    Database --> Projection["Materialized-state projection"]
    NodeRuntime --> CatchUp["Catch-up and recovery service"]
    CatchUp --> Aleph
    CatchUp --> MintApply
```

The intended ownership boundaries are:

| Concern | Sole owner |
|---|---|
| HTTP parsing and response conversion | Axum handlers |
| Member authentication and common private request admission | Typed authenticated request boundary, with message-class-specific sequence handling |
| Node task lifecycle and request coordination | `FederationNodeRuntime` |
| Ordering and finality | AlephBFT consensus service |
| Operation admission rules | Mint federation admission module |
| Domain state transitions | Operation-specific mint federation appliers |
| Application transaction lifecycle | Federation apply transaction/context |
| Durable row encoding | Database backend federation codecs |
| Materialized-state effects and digest inputs | Mint federation projection module |
| Wallet member fanout and threshold decisions | Typed member outcomes plus explicit endpoint quorum policies |
| Wallet recovery policy | Existing wallet sagas and `Error` classification |

## Ranked opportunity matrix

| Rank | Refactor | Verdict | Risk | Main result |
|---:|---|---|---|---|
| 1 | Remove in-memory consensus, external finalization, and backend dispatch | Do first | High | One AlephBFT runtime model and no dead backend branches |
| 2 | Extract `FederationNodeRuntime` from `MintState` | Strongly worthwhile | Medium | Axum becomes a thin transport boundary |
| 3 | Add validated setup/active federation config types | Strongly worthwhile | Medium | Validation, aggregate keys, and digest become construction invariants |
| 4 | Add an AlephBFT session supervisor under the node runtime | Strongly worthwhile | Medium-high | Explicit task, rotation, recovery, and shutdown lifecycle |
| 5 | Centralize federation apply transactions | Worthwhile, narrow scope | Medium | Atomic status/domain lifecycle cannot be forgotten |
| 6 | Model journal application state explicitly | Worthwhile after hash review | Medium | Clear order/application boundary in types and persistence |
| 7 | Move materialized-state projection into the mint domain | Worthwhile, benchmark first | Medium-high | Axum stops duplicating mint database ownership |
| 8 | Finish typed wallet quorum collection | Finish existing partial work | Medium | Shared scheduling and explicit endpoint decisions |
| 9 | Move mint implementation into `mint/federation/` | Worthwhile after deletion | Low | Focused modules matching surviving responsibilities |
| 10 | Split the giant test file | Worthwhile around R4 | Low | Faster diagnosis without preserving obsolete tests |
| 11 | Add a typed private request boundary | Conditional on message classification | Medium | Shared authentication without hidden sequence side effects |
| 12 | Centralize SQL journal row encoding | Worthwhile after schema cleanup | Low-medium | Less schema and column-order drift |
| 13 | Split federation common types by domain | Defer | Low | Smaller modules after protocol shape stabilizes |
| 14 | Structure runtime errors | Apply selectively | Medium | Typed retry and readiness decisions without error-type proliferation |

## Refactoring checkpoints

Each checkpoint below must be independently reviewable, testable, and committed.
Mechanical moves should not be combined with semantic changes.

## R0. Freeze a behavioral baseline

### Objective

Create a reproducible baseline before moving runtime or consensus code.

### Review verdict: Keep, but keep it lightweight

This is necessary because the following changes affect ordering, restart, and
wallet recovery invariants. Reuse the implementation plan, existing benchmark
documents, and established test commands instead of creating another large
baseline artifact. A separate commit is useful only when R0 adds durable test
fixtures or harness behavior; documentation-only repetition is not a valuable
checkpoint by itself.

### Work

- Record the exact focused test commands for:
  - canonical operation and conflict-key behavior;
  - journal append, replay, application state, and digest verification;
  - AlephBFT unit production and finalization;
  - private REST authentication and sequence rejection;
  - two-, five-, and 11-member convergence;
  - partition, restart, catch-up, and session rotation;
  - federated mint, swap, melt, restore, and check-state;
  - wallet threshold share aggregation and ambiguous recovery;
  - standalone mint and wallet regression coverage.
- Record one fresh five-member fakewallet flow containing mint, swap, and melt.
- Record journal tips, state digests, completed operations, quote states, and wallet
  balances after the flow.
- Ensure tests do not select the in-memory consensus backend as the source of
  expected production behavior. Mark those tests for conversion or deletion in
  R4.

### Tests

- Run formatting checks without changing production behavior.
- Run the focused federation suites with AlephBFT selected.
- Run representative standalone mint and wallet tests.

### Acceptance criteria

- Baseline commands and expected outcomes are documented.
- All behavior-preservation refactors can be compared against the same scenarios.
- Any pre-existing failures are listed separately and are not silently folded
  into refactoring work.

### Suggested commit

`test: establish federation refactoring baseline`

## R1. Reorganize the Axum federation tests

### Objective

Turn the 22,000-line test module into navigable suites and establish a reusable
multi-member harness before production code moves.

### Review verdict: Worthwhile, but split around deletion work

The file size is a real maintenance problem, but moving every current test
before R4 would carefully preserve a large body of in-memory-backend and
external-finalization coverage that should be deleted. First extract only the
minimal AlephBFT cluster and deterministic wait/shutdown fixtures needed for R4.
After obsolete production paths and tests are removed, split the surviving
suites physically. Avoid a single feature-rich cluster harness that becomes a
new test framework with hidden operation ordering.

### Target layout

```text
crates/cdk-axum/src/federation/tests/
  mod.rs
  fixtures/
    mod.rs
    cluster.rs
    members.rs
    operations.rs
    payments.rs
    transport.rs
    wait.rs
  admission.rs
  private_routes.rs
  aleph_runtime.rs
  aleph_network.rs
  aleph_sessions.rs
  finalization.rs
  catch_up.rs
  state_digest.rs
  restart.rs
  partitions.rs
  payment_observation.rs
  dkg.rs
  metrics.rs
```

### Work

- Move surviving tests without changing assertions or timing during each pure
  relocation commit. Delete tests for removed behavior in the corresponding
  production-deletion commit instead of relocating them first.
- Extract a `FederationTestCluster` that owns member states, transport handles,
  configured thresholds, fake payment backends, and shutdown behavior.
- Give the harness explicit operations:
  - `start` and `shutdown`;
  - `submit` and `submit_to_member`;
  - `partition`, `isolate`, `heal`, and `set_member_online`;
  - `restart_member` and `restart_all`;
  - `await_recorded`, `await_applied`, `await_terminal`, and `await_converged`;
  - `journal_tips`, `state_digests`, and `assert_converged`;
  - fakewallet payment observation control.
- Replace repeated polling loops with bounded helper methods that include useful
  timeout context.
- Keep fault behavior explicit. Avoid a large scenario macro that hides operation
  order or member state.

### Tests

- Pure relocation commits must preserve their test count. The completed R1/R4
  sequence should preserve behavioral coverage, not the raw count of tests for
  deliberately removed behavior.
- Run each new test module independently.
- Run the complete federation-enabled Axum suite once after all moves.
- Confirm harness shutdown leaves no live AlephBFT or HTTP tasks.

### Acceptance criteria

- No production code changes.
- Test names and assertions remain traceable through Git history.
- New fault tests can be written without copying cluster startup code.

### Suggested commits

- `test: extract federation cluster fixtures`
- `test: split axum federation suites`

## R2. Move mint federation code into a directory

### Objective

Make operation ownership visible while preserving every existing public method
and behavior.

### Review verdict: Worthwhile after protocol cleanup, with fewer prescribed modules

The production file remains large and should be split, while its test suite is
already partly organized under `mint/federation/tests/`. Perform this after
removing consensus restore and other dead fields so code is not moved and then
deleted. The target layout below is illustrative: create a module only when a
surviving responsibility has enough behavior to justify it, and do not force a
one-file-per-operation structure for small appliers.

### Target layout

```text
crates/cdk/src/mint/federation/
  mod.rs
  admission.rs
  runtime.rs
  quote.rs
  mint.rs
  swap.rs
  melt.rs
  restore.rs
  payment_observation.rs
  keyset.rs
  projection.rs
  tests/
    mod.rs
    admission.rs
    runtime.rs
    quote.rs
    mint.rs
    swap.rs
    melt.rs
    payment_observation.rs
    keyset.rs
```

When the production move is complete, the existing
`crates/cdk/src/mint/federation.rs` should become
`crates/cdk/src/mint/federation/mod.rs`; do not retain both production module
forms. The existing directory used by split tests is only a partial completion
of this checkpoint.

### Responsibility map

| Module | Responsibility |
|---|---|
| `mod.rs` | Public facade, narrow re-exports, shared private types |
| `admission.rs` | Pre-consensus deterministic admission and bounds checks |
| `runtime.rs` | Journal entry dispatch and application entry points |
| `quote.rs` | Mint/melt quote creation and quote-state validation |
| `mint.rs` | Paid mint quote consumption and partial blind signing |
| `swap.rs` | Input spending, output balance, and swap signing |
| `melt.rs` | Melt execution, proof state, payment result, and change signing |
| `restore.rs` | Restore request validation and stored share retrieval |
| `payment_observation.rs` | Observation validation, quorum, reports, and quarantine |
| `keyset.rs` | v3/BLS keyset checks and rotation application |
| `projection.rs` | Materialized-state effect collection and digest projection |

### Work

- Perform file movement before introducing new helpers.
- Preserve current method visibility unless a method is proven module-private.
- Keep shared constants in the narrowest module that uses them.
- Avoid a `utils.rs`; name helpers after their domain responsibility.
- Keep test modules alongside their corresponding production modules.
- Run `cargo fmt` after moves, but avoid unrelated formatting churn.

### Tests

- All existing `cdk` mint federation tests.
- Federation-enabled `cdk-axum` compilation.
- Representative standalone mint, swap, and melt tests.

### Acceptance criteria

- This checkpoint changes paths and visibility only.
- No operation serialization, operation ID, conflict key, state transition, or
  database behavior changes.
- `mod.rs` remains a facade rather than becoming the renamed monolith.

### Suggested commit

`refactor: split mint federation operations into modules`

## R3. Extract `FederationNodeRuntime` from Axum `MintState`

### Objective

Make Axum a transport adapter and give federation node orchestration one explicit
owner.

### Review verdict: Strongly worthwhile

This remains one of the highest-value boundaries. `MintState` directly owns a
large set of consensus, synchronization, admission, notification, finalization,
and session handles, so lifecycle correctness is difficult to inspect. Start
with a concrete `FederationNodeRuntime`; add a trait only if a real test boundary
cannot be expressed with narrow transport, storage, clock, or finalization
dependencies. Coordinate this checkpoint with R10 so node and session lifecycle
ownership are not modeled twice.

### Target layout

```text
crates/cdk-axum/src/federation/runtime/
  mod.rs
  node.rs
  submission.rs
  finalization.rs
  catch_up.rs
  recovery.rs
  notifications.rs
  status.rs
```

### Runtime ownership

`FederationNodeRuntime` should own or encapsulate:

- the AlephBFT consensus/session service;
- synchronization and readiness state;
- pending response waiters and bounded response cache;
- serialized finalized-unit application;
- catch-up coordination and catch-up certificate verification;
- pending gossip sequence metadata;
- operation notifications and federation metrics;
- startup replay and recovery;
- task handles and cancellation.

It should depend on a narrow mint application interface and database interfaces,
not on Axum request or response types.

### Work

- Move methods in mechanical groups from `MintState` to the runtime.
- Replace direct `MintState` field access with runtime methods.
- Keep response conversion in HTTP handlers.
- Do not introduce an event bus merely to avoid direct method calls. A narrow
  runtime API is simpler and easier to test.
- Make readiness a runtime-owned state machine rather than a collection of
  handler-level checks.
- Preserve existing metrics names and labels during the move.

### Runtime API sketch

```rust
pub(crate) trait FederationRuntime {
    async fn submit(
        &self,
        envelope: FederationOperationEnvelope,
        source_sequence: Option<FederationSourceSequence>,
    ) -> Result<FederationSubmitOperationResponse, FederationRuntimeError>;

    async fn apply_finalized_unit(
        &self,
        unit: FederationAlephBftFinalizedUnit,
    ) -> Result<FederationFinalizationResult, FederationRuntimeError>;

    async fn apply_catch_up(
        &self,
        response: FederationJournalCatchUpResponse,
        evidence: FederationCatchUpCertificateQuorum,
    ) -> Result<FederationCatchUpResult, FederationRuntimeError>;

    fn readiness(&self) -> FederationReadiness;
}
```

The concrete type may be used directly if a trait does not improve testability.
Do not add dynamic dispatch by default.

### Tests

- Unit tests for readiness transitions.
- Submission waiter deduplication and cleanup.
- Duplicate finalized-unit handling.
- Catch-up verification and rejection.
- Replay after restart.
- Existing private and public route tests using the extracted runtime.
- Five-member mint/swap/melt convergence.

### Acceptance criteria

- General `cdk-axum/src/lib.rs` no longer implements federation consensus or
  catch-up algorithms.
- Handlers parse, authenticate, call the runtime, and convert responses.
- Runtime tests do not require constructing an Axum router unless HTTP behavior
  is the subject under test.

### Suggested commits

- `refactor: extract federation submission runtime`
- `refactor: move federation finalization and catch-up runtime`
- `refactor: isolate federation startup recovery`

## R4. Remove in-memory consensus and unify the consensus boundary

### Objective

Delete the development-only in-memory consensus backend and leave one coherent
AlephBFT production architecture.

### Review verdict: Highest-priority refactor

This produces the largest immediate reduction in branches, test scaffolding,
configuration, and misleading abstractions. Expand the checkpoint to remove the
legacy external-finalization driver, private finalized-unit route, finality-mode
dispatch, and mint-to-mint primary forwarding that AlephBFT already bypasses.
Do not preserve these paths as test features. Keep only narrow fakes at actual
proposal, finalized-stream, transport, clock, or storage boundaries.

### Important distinction

`InMemoryFederationJournal` is an in-process representation of a journal used by
runtime logic. It is not necessarily the obsolete in-memory consensus backend.
Do not delete it blindly. After backend removal, rename it to `FederationJournal`
or `FederationJournalView` if the old name is misleading.

### Production types to remove or collapse

- `InMemoryFederationConsensus`;
- `InMemoryFederationConsensusNetwork`;
- `InMemoryFederationConsensusAdapter`;
- production-selectable `InMemory` consensus backend variants;
- `FederationConsensusBackendAdapterRuntime` if it duplicates the surviving
  runtime;
- backend branches whose only purpose is dispatching between in-memory and
  AlephBFT;
- configuration, CLI, and environment values that select `in_memory`;
- compatibility aliases for the undeployed development backend.
- the external-finalization driver and private finalized-unit delivery protocol;
- mint-to-mint primary forwarding used only by the non-Aleph backend.

### Target consensus boundary

Use one production service with three explicit responsibilities:

1. `AlephBftProposalQueue`
   - accepts validated federation consensus items;
   - bounds queue size and operation size;
   - feeds the active AlephBFT data provider.
2. `AlephBftFinalizationStream`
   - receives finalized units from upstream AlephBFT;
   - validates session and item metadata;
   - yields deterministic ordered decisions.
3. `FederationOrderedJournal`
   - appends finalized operations;
   - verifies order and evidence;
   - exposes catch-up ranges and application state.

Avoid a trait whose only implementation is AlephBFT. Keep narrow traits only at
boundaries that tests genuinely replace, such as transport or durable storage.

### Test replacement strategy

Tests that currently rely on the in-memory backend must move to one of:

- the real upstream AlephBFT runtime with deterministic test timing;
- direct journal tests when the subject is append/replay rather than consensus;
- a small `#[cfg(test)]` fake of a narrow finalization stream when fault injection
  requires exact decisions;
- a transport mock when the subject is network delivery.

Do not preserve a full fake consensus implementation in production modules under
a different name.

### Work sequence

1. Inventory every construction and match on `FederationConsensusBackendKind`.
2. Convert tests that assert backend-neutral behavior to AlephBFT or journal-level
   tests.
3. Remove runtime selection from mint startup and configuration.
4. Collapse `FederationConsensusRuntime` and
   `FederationConsensusBackendAdapterRuntime` into the surviving AlephBFT service.
5. Delete in-memory network simulation types and their production exports.
6. Remove dead errors, routes, transport messages, driver loops, and status such
   as unsupported-backend and external-finalization branches that can no longer
   occur.
7. Rename remaining types that still imply multiple backends.
8. Run unused-dependency and dead-code checks.

### Tests

- Proposal admission, batching, and bounds using AlephBFT.
- Finalized operation order and duplicate-unit behavior.
- Below-threshold non-finality and restored-quorum finality.
- Conflicting operation order under the real backend.
- Restart from pre-backup and post-backup states.
- Partition, heal, catch-up, and convergence.
- No configuration accepts `in_memory`, `memory`, or equivalent aliases.
- No production crate exports an in-memory consensus engine.

### Acceptance criteria

- AlephBFT is the only constructible federation consensus implementation.
- There is one runtime abstraction rather than two enums dispatching over the
  same backend set.
- Production code contains no alternate local-finality shortcut.
- Tests retain deterministic fault injection without shipping a second consensus
  implementation.

### Suggested commits

- `test: move federation consensus coverage to aleph bft`
- `refactor: collapse federation consensus runtime`
- `refactor: remove in-memory federation consensus`

## R5. Introduce a typed wallet quorum collector

### Objective

Centralize member scheduling, typed outcomes, threshold decisions, and ambiguity
while keeping endpoint-specific validation explicit.

### Review verdict: Finish the existing partial implementation; do not rebuild it

Milestone 26 already retains typed member failures and implements threshold
agreement on standard Cashu rejections. The remaining value is consolidating
duplicated endpoint scheduling, cancellation, matching-view, and ambiguity
logic. The `QuorumPolicy` trait sketch is optional and probably unnecessary:
prefer concrete policy functions or enums until multiple implementations prove
a trait useful. Preserve the existing wallet error and saga authority exactly.

### Current duplication

The wallet has one low-level concurrent member collector followed by separate
collectors for mint, melt, restore, mint quote status, melt quote status, batch
quote status, and check-state. Each repeats some combination of:

- threshold calculation;
- timeout and retry policy;
- response collection;
- early stopping;
- matching-view detection;
- inactive-keyset special handling;
- failure formatting;
- insufficient-response errors.

The low-level collector now retains typed errors and threshold rejection
metadata, but endpoint-specific collectors still duplicate terminal-decision and
response-policy logic and eventually collapse ambiguous diagnostics to strings.

### Target types

```rust
struct MemberOutcome<T> {
    member_id: MemberId,
    attempts: u16,
    result: Result<T, Error>,
}

enum ThresholdDecision<T> {
    Success(T),
    Rejected(Error),
    Ambiguous(QuorumFailure),
}

trait QuorumPolicy<T> {
    type Output;

    fn observe(&mut self, outcome: MemberOutcome<T>);
    fn decision(&self) -> Option<ThresholdDecision<Self::Output>>;
}
```

Exact naming may differ. The important boundary is that scheduling returns typed
member outcomes and operation policies decide what constitutes agreement.

### Policies to support

- Valid BLS share threshold with cryptographic aggregation validation.
- Matching standard `ErrorResponse` threshold.
- Matching mint quote status threshold.
- Matching melt quote status threshold.
- Restore response threshold and aggregate reconstruction.
- Check-state threshold view.
- Optional primary fallback only for the existing quote-creation/status behavior
  where protocol semantics permit it.

### Work

- Separate `MemberFanout` scheduling from `QuorumPolicy` decisions.
- Retain typed errors until the final decision.
- Centralize timeout and retry accounting.
- Make early termination policy-driven: success threshold, matching rejection
  threshold, or impossibility of either outcome.
- Preserve member identity on every response and failure.
- Produce structured ambiguous diagnostics without making strings part of
  behavior.
- Keep BLS share validation in an aggregation policy, not in generic fanout.
- Keep wallet saga behavior unchanged. The connector returns existing `Error`
  values and sagas remain authoritative.

### Tests

- Threshold success with exactly the threshold and with extra members.
- Threshold matching standard rejection.
- Mixed different errors without rejection threshold.
- Mixed successes and errors where success still reaches threshold.
- Insufficient agreement after all responses.
- Timeout and retry exhaustion.
- Invalid share exclusion.
- Inactive or mismatched keyset responses.
- Slow members cancelled after a terminal decision.
- Interrupted receive recovery through restore/check-state.
- Definitive and ambiguous mint/melt wallet saga outcomes.
- Standalone connector behavior unchanged.

### Acceptance criteria

- Endpoint collectors no longer implement their own scheduling loops.
- Typed errors are available to the threshold decision.
- One member cannot decide a consensus-dependent rejection.
- There is no federation-specific business-error classifier.

### Suggested commits

- `refactor: add typed federation member fanout`
- `refactor: unify wallet quorum collection`

## R6. Centralize mint application transactions

### Objective

Make atomic domain mutation and journal application state a structural guarantee.

### Review verdict: Worthwhile as a narrow lifecycle consolidation

Several appliers already update `Applied` status inside the same transaction as
domain mutations, and deterministic rejection persistence already has a focused
atomic path. R6 should consolidate repeated begin/status/commit sequences and
make rollback deterministic rather than redesigning mint business logic. The
self-consuming wrapper below is illustrative, not required; a scoped helper or
transaction callback that follows existing database conventions may be simpler.

### Target abstraction

Introduce a narrow transaction context around the existing mint database
transaction:

```rust
struct FederationApplyTransaction<'a> {
    operation_id: FederationOperationId,
    tx: &'a mut dyn MintTransaction,
}

impl FederationApplyTransaction<'_> {
    async fn mark_applied(self) -> Result<(), Error>;
    async fn mark_rejected(self, rejection: ErrorResponse) -> Result<(), Error>;
    async fn rollback(self) -> Result<(), Error>;
}
```

The actual database trait shape should follow existing CDK conventions. The
helper must not hide quote, proof, signature, or payment business logic.

### Work

- Inventory every federation apply method and its transaction boundaries.
- Require every terminal application path to finish through one of:
  `mark_applied`, `mark_rejected`, or explicit rollback/retry.
- Persist deterministic rejection metadata in the same transaction as rejected
  status.
- Keep retryable storage failures non-terminal.
- Ensure drop is not the primary rollback mechanism for a failed write
  transaction.
- Add operation ID and operation kind to transaction tracing.
- Remove duplicated begin/status/commit sequences after conversion.

### Tests

- Failure injection before domain mutation.
- Failure injection after domain mutation but before status update.
- Failure injection after status update but before commit.
- Deterministic rejection commits status and `ErrorResponse` atomically.
- Retryable failure leaves no terminal status or partial domain mutation.
- Replay after process restart completes exactly once.
- Concurrent duplicate application remains idempotent.
- Mint, swap, and melt each have at least one atomicity test.

### Acceptance criteria

- No operation applier manually repeats the standard begin/status/commit
  lifecycle.
- Domain and application state cannot commit independently.
- Explicit rollback occurs on handled transaction failures.

### Suggested commit

`refactor: centralize federation apply transactions`

## R7. Model journal application state explicitly

### Objective

Make the difference between immutable consensus order and mutable post-consensus
application metadata obvious in the type system.

### Review verdict: Worthwhile, but use legal states rather than grouped options

The current `status` plus optional rejection can express invalid combinations.
Use an enum or an equivalently constrained representation for pending, applied,
and rejected-with-standard-response states. Do not implement the struct sketch
literally because it still permits `Applied` with a rejection. The digest audit
is complete: journal `state_digest` and `operation_outcome_hash` were redundant
and have been removed. Do not reintroduce them into the application-state type.

### Target model

```rust
struct FederationJournalEntry {
    // Immutable ordered fields.
    index: u64,
    operation_id: FederationOperationId,
    envelope: FederationOperationEnvelope,
    conflict_keys: Vec<FederationOperationConflictKey>,
    previous_order_digest: FederationJournalDigest,
    order_digest: FederationJournalDigest,
    consensus_evidence: Option<FederationJournalConsensusEvidence>,

    // Post-consensus application metadata, persisted in the existing row.
    application: FederationApplicationState,
}

enum FederationApplicationState {
    Pending,
    Applied,
    Rejected {
        response: ErrorResponse,
    },
}
```

The materialized-state digest is a snapshot-level convergence commitment, not
per-entry application metadata. The minimum required grouping remains `status`
and `rejection`.

### Work

- Document which entry fields are covered by accepted-order hashing.
- Document which fields may change after finalization.
- Introduce constructors for ordered entries and legal application transitions.
- Prevent impossible combinations, including:
  - `Applied` with a rejection;
  - `Rejected` without a rejection when the rejection is deterministic and
    representable;
  - non-terminal state with terminal outcome metadata.
- Keep persistence in the existing journal table unless a separate storage
  design is justified independently. This refactor does not add consensus work.
- Update catch-up overlay and verification to compare application state
  explicitly.

### Tests

- Serialization round trips for every legal state.
- Illegal transition rejection.
- Accepted-order digest unchanged when application metadata changes.
- Materialized-state digest changes when persisted rejection metadata changes.
- Catch-up rejects application-state mismatch.
- Old undeployed development formats are removed rather than supported with
  compatibility branches.

### Acceptance criteria

- Order and application metadata are distinguishable from field names and APIs.
- Hashing code cannot accidentally include or exclude mutable fields through
  whole-struct serialization.
- No new table, journal, consensus item, or AlephBFT round is introduced.

### Suggested commit

`refactor: model federation journal application state`

## R8. Move materialized-state projection into the mint domain

### Objective

Eliminate the second description of operation effects currently maintained by
the Axum layer.

### Review verdict: Correct ownership, but benchmark and avoid another effect model

Axum should not understand mint rows, so moving snapshot ownership is valuable.
However, a large exhaustive `FederationMaterializedEffects` mapping can remain a
second description of application semantics even when placed nearby. Begin with
a mint-owned, batched snapshot API and benchmark growing journals to remove the
current N+1 query pattern. Add an incremental fingerprint/projection model only
if measurements require it and full recomputation can continuously verify it.

**Implementation verdict: Complete.** The projector is mint-owned, operation
coverage remains exhaustive, and database reads are bounded and batched. A
dedicated quote-state projection avoids loading history outside the commitment.
The 1,000-operation release benchmark improved from 5,917us to 4,231us, with a
4,091us repeat. Incremental durable fingerprints were not added because the
measured full audit is linear and about 4ms at that size. See
`docs/federated-cashu-materialized-state-benchmark.md` for measurements and
rejected candidates.

### Target design

Add `mint/federation/projection.rs` with a domain API such as:

```rust
impl Mint {
    pub async fn federation_materialized_state_snapshot(
        &self,
        entries: &[FederationJournalEntry],
    ) -> Result<FederationMaterializedStateSnapshot, Error>;
}
```

Internally, an operation can expose deterministic references to affected rows:

```rust
struct FederationMaterializedEffects {
    mint_quotes: BTreeSet<QuoteId>,
    melt_quotes: BTreeSet<QuoteId>,
    proof_ys: BTreeSet<PublicKey>,
    blinded_messages: BTreeSet<PublicKey>,
    completed_operations: BTreeSet<FederationOperationId>,
    payment_observations: BTreeSet<PaymentObservationKey>,
    keyset_rotations: Vec<KeysetRotationKey>,
    rejections: Vec<FederationRejectionKey>,
}
```

This descriptor identifies rows; it does not duplicate the database mutation
logic.

### Work

- Move projection code out of `cdk-axum` without changing row names or fields.
- Add one exhaustive operation-to-effects mapping near operation application.
- Require adding a projection case whenever a new `FederationOperation` variant
  is added.
- Keep canonical row construction deterministic and sorted.
- Consider deriving projection references during application only if they can be
  reconstructed and verified deterministically after restart.
- Keep Axum limited to requesting and comparing the digest.

### Tests

- Golden snapshot rows for every operation kind.
- Rejected operation projection includes rejection but no completed-operation
  row.
- Missing expected row produces a different digest and readiness failure.
- Extra durable effect is detected where the digest scope requires it.
- Reordered database query results produce the same digest.
- Independent members produce identical snapshots after mint, swap, and melt.
- Catch-up member converges to the same snapshot.

### Acceptance criteria

- Axum does not match on `FederationOperation` to infer mint database effects.
- Operation application and projection changes are reviewed in the same module.
- Adding an operation variant causes an exhaustive compiler failure in projection
  code until handled.

### Suggested commit

`refactor: move federation state projection into mint`

## R9. Introduce `ValidatedFederationConfig`

### Objective

Represent full federation configuration validation and its computed digest as a
construction invariant rather than a caller promise.

### Review verdict: Strongly worthwhile, with phase-specific validated types

This addresses a demonstrated hot path and removes unsafe method-name promises
such as accepting a raw config in an `already_validated` path. Prefer a raw
deserialized type followed by `ValidatedSetupFederationConfig` and
`ValidatedActiveFederationConfig`, because DKG setup and active runtime have
different legal fields. Cache the config digest, aggregate keys, member indexes,
and thresholds in the active representation. Keep member advertisement and
active-keyset checks cheap and dynamic.

### Target type

```rust
#[derive(Debug, Clone)]
pub struct ValidatedActiveFederationConfig {
    config: Arc<FederationConfig>,
    digest: FederationConfigDigest,
}

impl ValidatedActiveFederationConfig {
    pub fn try_new(config: FederationConfig) -> Result<Self>;
    pub fn config(&self) -> &FederationConfig;
    pub fn digest(&self) -> FederationConfigDigest;
}
```

### Work

- Perform full v3/BLS keyset, roster, threshold, transcript, payment policy, and
  identity validation once at external import or mint startup.
- Compute and store the config digest during construction.
- Pass the validated type to mint runtime, wallet connector, member verifier, and
  DKG completion paths.
- Remove methods whose safety relies on names such as
  `from_validated_*` while still accepting a raw `FederationConfig`.
- Keep cheap runtime validation for member advertisements and active keysets.
- Decide whether deserialization can construct only raw config, requiring an
  explicit validation boundary before runtime use.

### Tests

- Invalid threshold, roster, identity, transcript, and payment-policy configs.
- Non-v3/non-BLS aggregate keysets rejected.
- Aggregate key/share mismatch rejected.
- Digest computed once and stable.
- Wallet connector cannot be constructed from raw unvalidated config.
- Startup cannot install a raw config in runtime state.
- Standalone startup remains unaffected.

### Acceptance criteria

- Runtime code receives validated configuration by type.
- No hot path performs full BLS aggregate-key interpolation.
- Config digest does not require repeated full validation.

### Suggested commit

`refactor: add validated federation config`

## R10. Add an AlephBFT session supervisor

### Objective

Give AlephBFT task ownership, activity, rotation, recovery, and shutdown one
explicit lifecycle state machine.

### Review verdict: Strongly worthwhile as a child of R3

The live session currently owns several manually managed tasks, so explicit
cancellation, joining, rotation, and failure state are justified. Build the
supervisor as a concrete component owned by `FederationNodeRuntime`, not as a
second peer runtime or general task framework. R3 should define who owns it and
R10 should define the owned session lifecycle. Preserve event-driven idle
behavior and Aleph backup recovery.

### Target states

```text
Stopped
Starting(session)
Running(session)
Rotating(from, to)
Recovering(session)
Stopping(session)
Halted(reason)
```

### Work

- Introduce `AlephBftSessionSupervisor` under the Axum federation runtime.
- Move ownership of session, activity broadcast, network pump, finalized apply,
  and completion-watcher tasks into the supervisor.
- Use one cancellation mechanism and deterministic task joining.
- Make session rotation an explicit transition with preconditions.
- Prevent two active session task sets for the same member.
- Restore durable pending activity and backup state through the supervisor.
- Expose a small status snapshot rather than individual task handles.
- Keep idle behavior event-driven; do not restore periodic empty units.

### Tests

- Start and clean stop.
- Duplicate start rejected or idempotent.
- Rotation only after valid completion conditions.
- Restart with pending operation.
- Restart from persisted backup.
- Task failure transitions to halted/not-ready state.
- Shutdown joins every owned task.
- No activity traffic or CPU loop while fully idle.
- Activity wake and refresh continue while durable pending work exists.

### Acceptance criteria

- Session tasks cannot outlive their supervisor silently.
- Rotation and restart logic do not manipulate task handles from unrelated
  modules.
- Runtime readiness reflects supervisor state.

### Suggested commits

- `refactor: add aleph bft session supervisor`
- `refactor: centralize aleph session rotation and recovery`

## R11. Split common federation types

### Objective

Reduce the 5,900-line common facade and make dependency direction visible.

### Review verdict: Useful but defer until protocol deletion stabilizes

Mechanical organization has low behavioral risk, but doing it before removing
obsolete consensus, restore, transcript, conflict, and transport types creates
needless movement and merge noise. Start with a few coarse ownership modules,
such as config/DKG, ordered operations, journal/application state, and private
transport. Do not force every small identity, threshold, keyset, or error type
into its own file unless that establishes a real dependency boundary.

### Target layout

```text
crates/cdk-common/src/federation/
  mod.rs
  error.rs
  identity.rs
  threshold.rs
  config.rs
  keyset.rs
  payment_policy.rs
  dkg/
    mod.rs
    commitment.rs
    transcript.rs
    state.rs
  operation/
    mod.rs
    mint.rs
    melt.rs
    swap.rs
    restore.rs
    payment_observation.rs
  journal.rs
  consensus.rs
  signing.rs
  transport.rs
  trusted_dealer.rs
```

### Work

- Move types by domain with no wire-format changes.
- Keep the top-level module as a curated public facade.
- Reduce broad re-exports where direct module paths make ownership clearer.
- Separate DKG phase records from finalized public config.
- Consider phase-specific DKG state types only where they eliminate invalid
  transitions without making persistence cumbersome.
- Split payment observation reports from core operation envelope definitions.
- Keep canonical serialization helpers near the types whose hashes they define.

### Tests

- JSON and canonical-byte golden tests before and after moves.
- Operation ID and config digest golden tests.
- Public API compilation for dependent crates.
- DKG transcript and signature validation.
- Trusted-dealer generation unchanged.

### Acceptance criteria

- Pure moves do not change serialization or hashes.
- `mod.rs` is a facade, not another multi-thousand-line implementation file.
- Module imports communicate whether code handles config, operation, journal,
  signing, or transport concerns.

### Suggested commits

- `refactor: split federation config and dkg types`
- `refactor: organize federation operation types`

## R12. Centralize SQL federation row codecs

### Objective

Prevent query column lists, row decoding, and migrations from drifting apart.

### Review verdict: Worthwhile after schema cleanup

A single typed journal row codec and grouped storage families will reduce real
column-order and nullable-field risk. The singular conflict key is already
removed; perform this after deciding the remaining digest-checkpoint, durable
gossip-retry, and experimental columns, then squash undeployed federation
migrations. Do not build a dynamic SQL abstraction; retain static parameterized
statements and shared explicit decoding.

### Work

- Introduce a `FederationJournalRow` storage representation with explicit
  conversion to/from `FederationJournalEntry`.
- Centralize the canonical journal select column order if supported safely by the
  existing query API.
- Keep SQL statements static and parameterized. Do not replace clear SQL with an
  ad hoc string builder.
- Group SQL implementation by record family:
  - config and DKG;
  - journal and application state;
  - AlephBFT backup/session data;
  - payment observations and quarantine;
  - member status and sequences.
- Add migration tests only for deployed formats. Federation is undeployed, so
  squash experimental federation migrations after the retained schema is
  settled rather than testing every intermediate development schema.
- Keep SQLite and PostgreSQL behavior equivalent.

### Tests

- Journal row encode/decode round trip.
- Nullable rejection and consensus-evidence fields.
- Every journal retrieval method uses the same codec.
- SQLite and PostgreSQL query parity where test infrastructure permits.
- Fresh-schema parity for the final undeployed federation schema; do not require
  migration from discarded experimental federation schemas.
- Corrupt enum, digest, JSON, and operation ID fields fail with structured
  storage errors.

### Acceptance criteria

- Adding a journal field requires changing one row codec and clearly identified
  SQL/migration sites.
- Repeated select lists cannot silently decode into the wrong field order.
- SQL modules are divided by storage concept rather than one giant implementation.

### Suggested commit

`refactor: centralize federation sql row codecs`

## R13. Add a typed private federation request extractor

### Objective

Make authentication, admission control, message-kind validation, and sequence
handling uniform across private REST routes.

### Review verdict: Conditionally worthwhile after classifying message semantics

Shared signature, federation, sender, and message-kind verification are useful.
Sequence durability and consumption are not uniform: DKG or effectful control
requests differ from idempotent reads and high-frequency Aleph gossip. First
classify each private message and remove the external finalized-unit route.
Then share the common authenticated envelope boundary while keeping class-
specific sequence persistence explicit. Avoid generic middleware that commits a
watermark before the handler's intended acceptance point.

### Target boundary

```rust
struct AuthenticatedFederationRequest<T> {
    sender: MemberId,
    sequence: u64,
    payload: T,
    admission_permit: OwnedSemaphorePermit,
}
```

The exact type should also retain any verified session or federation metadata
needed by the handler.

### Work

- Move admission permit acquisition and signed-request verification into a
  shared extractor or route wrapper.
- Parameterize the expected `FederationRestMessageKind` through a trait or
  route-local constant.
- Make sequence consumption semantics explicit:
  - verification before persistence;
  - persistence only at the intended acceptance point;
  - rejected session/message requests do not consume inappropriate watermarks.
- Keep operation admission separate where it validates canonical federation
  operation contents beyond transport authentication.
- Standardize error conversion and metrics.

### Tests

- Unknown member and bad signature.
- Wrong federation and wrong message kind.
- Replay, skipped sequence, and concurrent duplicate sequence.
- Closed admission semaphore and backpressure.
- Invalid session does not consume sequence when that is the intended rule.
- DKG, status, catch-up, activity, network-message, and finalized-unit routes all
  use the shared boundary.

### Acceptance criteria

- Private handlers begin with an already authenticated typed request.
- No route independently reimplements signature or sequence verification.
- Security behavior remains visible and testable rather than hidden in generic
  middleware with implicit side effects.

### Suggested commit

`refactor: unify private federation request admission`

## R14. Introduce structured runtime and infrastructure errors

### Objective

Reduce broad `Custom(String)` and `anyhow` plumbing in federation runtime code
without creating a federation-specific Cashu business-error policy.

### Review verdict: Apply selectively, not as a broad taxonomy project

Typed errors are valuable where retryability, HTTP status, readiness, or test
control flow currently depends on display text. They are not valuable merely to
replace every diagnostic string with another wrapper enum. Add structured
variants at proven decision boundaries while doing R3, R4, R6, and R13, and
leave leaf diagnostics simple when nobody classifies them. Never duplicate or
reinterpret standard Cashu business errors.

### Error boundaries

Use structured errors for infrastructure layers:

- `FederationRuntimeError` for readiness, lifecycle, and orchestration failures;
- `FederationConsensusError` for proposal/finalization/session failures;
- `FederationTransportError` for authenticated private REST delivery;
- existing storage error types for database failures;
- existing standard Cashu `Error` and `ErrorResponse` for mint business errors.

### Work

- Replace strings that are later parsed, classified, or asserted by text.
- Include structured fields such as member, session, operation ID, expected,
  found, and retryability where deterministic.
- Convert to public HTTP responses only at the route boundary.
- Preserve standard Cashu errors from mint logic unchanged.
- Do not add a federation business-error whitelist or parallel wallet classifier.
- Keep operator diagnostics free of private keys, auth tokens, raw proofs, and
  other secrets.

### Tests

- Structured conversion at each crate boundary.
- Retry decisions do not depend on error display text.
- Standard Cashu business errors round trip unchanged.
- Infrastructure failures remain ambiguous to wallet sagas.
- Logs contain useful identifiers without secret material.

### Acceptance criteria

- Display strings are diagnostics, not control flow.
- Wallet recovery remains governed by existing `Error` behavior.
- Public route behavior and standalone errors remain unchanged.

### Suggested commit

`refactor: structure federation runtime errors`

## Cross-cutting cleanup rules

### Prefer deletion over compatibility

Federated Cashu has not been deployed. Remove old development-only consensus
backend selection, obsolete config variants, stale fixtures, and dead route
semantics. Do not add migrations or compatibility wrappers solely to preserve
experimental in-repo formats unless current tests require migration of a format
the project has explicitly chosen to retain.

### Avoid over-abstraction

- Do not genericize mint, swap, and melt business rules into one opaque processor.
- Do not introduce dynamic dispatch where a concrete runtime has one production
  implementation.
- Do not add a general event bus for direct runtime coordination.
- Do not create `utils`, `helpers`, or `misc` dumping-ground modules.
- Do not move federation logic into macros merely to reduce line count.
- Do not retain the in-memory consensus backend as a test feature. Prefer narrow
  test doubles at actual boundaries.

### Keep standalone code isolated

- Federation mode checks should remain at explicit adapter or dispatch points.
- Standalone mint business logic should be reused where it is already the source
  of truth.
- Standalone storage schemas and wallet connector behavior should not acquire
  federation-only requirements.
- Feature combinations and WASM compatibility must continue to compile where
  currently supported.

### Documentation and commits

- Commit after every coherent checkpoint.
- Use conventional commit messages.
- Keep moves separate from semantic changes.
- Update architecture and progress documents after verified behavior changes.
- Do not commit local paths, generated private member configs, key shares,
  credentials, or machine-specific test artifacts.

## Verification matrix

The following matrix should be run at the checkpoints indicated above and in full
before declaring the refactoring complete.

| Area | Required verification |
|---|---|
| Formatting | `cargo fmt --all -- --check` |
| Common federation types | `cargo test -p cdk-common federation` plus serialization/hash goldens |
| Mint application | Focused `cdk` federation mint, quote, swap, melt, restore, payment, and keyset tests |
| Wallet connector | Threshold success/rejection/ambiguity, invalid share, status, restore, and check-state tests |
| SQL | `cdk-sql-common`, SQLite federation DB tests, and PostgreSQL tests when available |
| Axum routes | Private authentication, public operation routes, readiness, and error conversion |
| AlephBFT | Data provider, network pump, activity wake, finalization, session rotation, backup, and recovery |
| Multi-member | Fresh 3-of-5 and 8-of-11 federation scenarios |
| Fault tolerance | Below-threshold partition, above-threshold partition, heal, restart, delayed member, catch-up |
| End-to-end | Fakewallet mint, swap/send/receive, melt, restore/check-state, and final balance convergence |
| Standalone | Representative non-federated mint, swap, melt, wallet saga, and CLI tests |
| Performance | Idle CPU, request latency, DB contention, network retry volume, and no duplicate quote churn |

## Completion criteria

The refactoring program is complete when all of the following are true:

- AlephBFT is the only federation consensus backend in production code and
  configuration.
- The in-memory consensus engine, network simulator, adapter, and selection
  branches are deleted.
- There is one clear consensus runtime boundary.
- `MintState` no longer implements federation consensus, finalization, catch-up,
  or recovery algorithms directly.
- Mint federation production code lives under
  `crates/cdk/src/mint/federation/` and is organized by surviving domain
  responsibility.
- Wallet fanout preserves typed member outcomes and shares scheduling without
  hiding endpoint-specific threshold decisions.
- Mint application paths share an atomic application transaction lifecycle.
- Journal order and post-consensus application state are explicitly separated in
  types and hash documentation.
- Materialized-state projection is owned by the mint federation domain, not Axum.
- Runtime components accept phase-appropriate validated setup or active
  federation configuration types.
- AlephBFT session tasks have one supervisor and deterministic shutdown.
- Private REST handlers share authenticated envelope verification while keeping
  message-class-specific sequence semantics explicit.
- Federation test suites are split and use focused reusable fixtures rather
  than one mandatory all-purpose cluster framework.
- SQL row codecs are centralized enough that schema changes cannot silently
  reorder journal decoding.
- Full fakewallet mint, swap, melt, restore, partition, restart, and catch-up
  flows pass with converged journals and materialized-state digests.
- Standalone CDK behavior and tests remain unchanged.

## Recommended execution order

The safest sequence is:

1. R0: record a lightweight behavioral baseline using existing commands and
   benchmark documents.
2. R1 phase A: extract only the minimal real-Aleph cluster, deterministic wait,
   and shutdown fixtures needed to perform deletion safely.
3. R4: remove the in-memory backend, legacy primary forwarding, external-
   finalization driver and route, duplicate consensus abstractions, and tests
   whose behavior was deliberately removed.
4. Complete the remaining cleanup decisions for hash semantics before
   reorganizing their types. Consensus restore, request transcripts, singular
   conflict keys, and observation wall-clock fields are already removed.
5. R1 phase B: split the surviving Axum federation suites without preserving a
   framework for deleted behavior.
6. R9: introduce validated setup and active configuration types early so later
   runtime moves use the final invariant-bearing boundary.
7. R2: move the surviving mint federation production code into focused modules.
8. R3 and R10: extract the concrete node runtime and its owned Aleph session
   supervisor together.
9. R6 and R7: consolidate application transactions and introduce legal typed
   application states after digest/outcome semantics are settled.
10. R8: move batched materialized-state projection into the mint domain, using
    benchmarks before adding incremental projection machinery.
11. R5: finish consolidation of the existing typed wallet member outcomes and
    endpoint quorum policies.
12. R13 and R12: classify private message sequence semantics, consolidate the
    authenticated request boundary, settle the schema, squash undeployed
    federation migrations, and centralize retained row codecs.
13. R11 and R14: split common modules and add structured errors opportunistically
    where the preceding work establishes a clear boundary.
14. Run the complete verification matrix and update federation architecture and
    progress documentation.

This order deletes obsolete behavior before moving it, introduces validated
configuration before runtime extraction, and combines lifecycle refactors that
must share one owner. Each coherent checkpoint remains independently testable
and reconstructable in Git.
