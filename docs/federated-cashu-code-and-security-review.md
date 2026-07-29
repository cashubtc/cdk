# Federated Cashu Code and Security Review

Date: 2026-07-12

> **Archived review snapshot — current disposition 2026-07-29:** This document
> preserves the original comparison and evidence; its `Open` labels and
> unchecked TODOs are not the current work queue. The authoritative
> reassessment is
> [`cleanup-2026-07-29.md`](cleanup-2026-07-29.md):
>
> | Original finding | Later disposition |
> |---|---|
> | P4 — Aleph retry starvation/restart recovery | Confirmed and deferred in cleanup Milestone 9. A safe change needs an explicit independently bounded retry/fairness design. |
> | P5 — staged apply replay proof | Addressed by the staged melt crash-boundary matrix retained and proven in Milestone 13. |
> | P11 — catch-up certificate soft-skip | Superseded by the cut-through checkpoint architecture; catch-up certificates are not a supported compatibility path. |
> | P13 — payment-observation readiness | Confirmed and deferred in Milestone 9 pending an explicit observation-origin/readiness contract. |
> | P18 — architecture enforcement | The signing-authority defect was closed by Milestone 5. Narrow cleanup was retained in Milestones 14–17; broad type/module rewrites without a reproduced bypass were rejected. |
> | Findings already marked resolved or complete below | Preserved as historical evidence and revalidated by cleanup Milestone 20's full federation and workspace suites. |
>
> The remaining deferred payment-evidence/recovery-identity work is owned by
> cleanup Milestone 11, and worker supervision by Milestone 12. Do not mark
> either production-ready on the strength of this historical review.

Checkpoint cut-through note: findings that mention catch-up certificates record
the superseded experimental recovery protocol. Current recovery and startup
requirements are tracked in
`docs/federated-cdk-checkpoint-implementation-plan.md`; no certificate
compatibility path remains.

This document records actionable findings from a committed-code comparison of
`bls-federation-cleanup` against `bls-federation`. It was the original work
queue for correctness, restart-safety, and operational-hardening fixes.
Historical findings remain open in the body until their implementation and
focused regression tests had been verified; the later disposition table above
now governs their current meaning.

## Implementation Principles

Address these findings with the smallest change that fully restores the
required safety or correctness invariant. Prefer narrow fixes over broad
refactors, and avoid changing adjacent behavior merely because it could be
cleaned up at the same time.

- Follow the federation's established lifecycle, journal, persistence,
  transaction, error-handling, and testing patterns. Reuse or extend an
  existing abstraction when it already owns the responsibility being fixed.
- Consolidate equivalent behavior into one authoritative path. Do not create a
  second helper, state machine, retry mechanism, status model, or protocol
  representation when the existing implementation can be corrected or
  generalized cleanly. Introduce a new abstraction only when the current one
  cannot preserve the required boundary without becoming less clear.
- Keep every change deterministic, replay-safe, idempotent, and explicit about
  transaction and crash boundaries. Exact retries and restart recovery must
  converge on the same durable outcome without duplicate side effects.
- Keep fixes scoped to federation behavior. Do not change or "fix" standalone
  or non-federated CDK behavior as part of this work. Shared code may be changed
  only when federation requires it and standalone behavior remains unchanged
  and covered.
- Add focused federation regression tests for the exact failure, its replay or
  restart variant where relevant, and the neighboring success path. Prefer
  deterministic synchronization and fault injection over timing-dependent
  tests.
- Run `cargo fmt --all` and the smallest relevant test/check set while
  developing, then run the broader affected federation suites before marking a
  finding complete.
- Update this security review and
  `docs/federated-cashu-implementation-plan.md` as fixes land so status,
  evidence, and remaining risk stay current.
- Commit coherent milestones separately using conventional commit messages.
  Each commit should leave the affected behavior internally consistent and
  include its focused regression coverage.

## Review Scope

- Compared committed cleanup-branch behavior against `bls-federation`.
- Concentrated on federation startup, mandatory AlephBFT operation, session
  supervision, catch-up reconstruction, persistence, materialized-state
  projection, payment-observation apply transactions, Aleph network retries,
  mintd federation config compatibility, and whether unfederated mint/swap/
  melt paths were accidentally changed.
- Existing uncommitted worktree changes were excluded from the comparison,
  except where they illuminated a pre-existing apply-path rollback gap that
  already exists on both branches.
- Re-evaluated the migration squash against the repository's migration-
  immutability policy. Pre-cleanup federation development databases are not a
  supported upgrade source; the remaining requirement is an explicit
  fresh-database/reset contract and an immutable boundary beginning with the
  first supported federation release.
- No source behavior was changed as part of this review.
- A 2026-07-13 reassessment then traced every finding against the current
  working tree and existing focused tests. That reassessment is recorded below
  separately from the original committed-branch comparison so later fixes can
  update confidence and disposition without rewriting the original evidence.

## Critical Priority

No Critical finding remains open. P12 was resolved on 2026-07-13 by retaining
the digest authenticated by catch-up certificate quorum, recomputing the local
materialized-state digest during fail-closed startup, and publishing Healthy
only after the two match.

P14 is a pre-release protocol-boundary cleanup rather than a Critical security
finding under the explicit cut-through policy that pre-cleanup federation
nodes, wire payloads, registries, configs, backups, and databases are
unsupported. P15, the related database-schema boundary, is complete under the
fresh-database policy recorded below.

## Severity Reassessment

Reassessed: 2026-07-13

Severity reflects the highest credible impact on federation safety,
correctness, availability, compatibility, or assurance. Confidence is the
reviewer's confidence that the assigned severity is appropriate; it is not an
exploit probability. "Release blocker" means the invariant should be restored
or explicitly ruled out with a deterministic regression before production use.

| Finding | Assessment | Severity | Confidence | Recommended disposition |
| --- | --- | --- | ---: | --- |
| P1 | One-member federation configs are rejected by the shared threshold invariant | Resolved (was Medium) | 98% | Complete: one-node deployments must use standalone mode. |
| P2 | Session tasks now activate only after generation-scoped supervisor publication | Resolved (was Medium) | 95% | Complete: construction, publication, activation, and teardown are ordered. |
| P3 | Projection reads are now keyed and batched by federation-tracked IDs | Resolved (was Medium) | 99% | Complete: query cost is independent of unrelated mint history. |
| P4 | Retry starvation confirmed; restart-loss subclaim not proven | Medium | 82% | Add bounded retry fairness and observability; do not restore durable gossip rows without a failed convergence reproduction. |
| P5 | Main explicit-rollback claim is resolved; staged melt lifecycle remains | Low residual | 90% | Close or narrow to replay/idempotence of the intentional multi-transaction melt saga. |
| P6 | Waiter reservation now precedes durable and runtime admission | Resolved (was High) | 99% | Complete: waiter, pending row, and runtime queue are published in one failure-safe order. |
| P7 | Active-session advance now validates and atomically rebinds pending rows | Resolved (was High) | 99% | Complete: startup and rotation cannot advance past discoverable durable pending work. |
| P8 | Duplicate recovery now republishes durable terminal outcomes idempotently | Resolved (was Medium) | 97% | Complete: normal finalization and replay share one bounded publication path. |
| P9 | Non-retryable apply and essential-task failures now halt the exact generation | Resolved (was High) | 99% | Complete: finalized payloads cannot be skipped while the member remains ready. |
| P10 | Every production Halted transition now uses generation-safe teardown | Resolved (was Medium) | 97% | Complete: Halted cannot retain a live supervisor or inbound queue. |
| P11 | Confirmed latent soft-skip of local certificate production | Medium | 88% | Fix with P12 so local digest/status failure cannot look like an empty successful round. |
| P12 | Restart revalidation now compares local state with certified quorum evidence | Resolved (was Critical) | 99% | Complete: startup remains fail-closed until certified order and materialized-state commitments match. |
| P13 | Readiness bypass confirmed; correct policy remains a design decision | Medium | 75% | Define and test a middle gate at minimum: live current session and not Halted. |
| P14 | Every federation-owned boundary now starts at strict initial v1 | Resolved (was Medium) | 98% | Complete: remaining v2 labels were rebased, obsolete shapes fail closed, and deterministic artifacts were regenerated. |
| P15 | Fresh-schema migration squash accepted; development databases unsupported | Complete | — | No further upgrade work required; freeze the consolidated migration at the first supported release. |
| P16 | Global lock removed; quote-scoped guard and durable saga claim added | Resolved (was High) | 99% | Complete: unrelated quotes proceed concurrently and only the durable transition winner may initiate payment. |
| P17 | Legacy registry migration conflicts with the accepted cut-through policy | Complete | 99% | Superseded by P14: reset the current registry shape to initial v1 and do not add a legacy loader or CLI alias. |
| P18 | Confirmed architecture-assurance gap, not a demonstrated current bypass | Medium | 90% | Add a sealed capability boundary or retain narrow interim enforcement. |
| P19 | Operation-ID replay policy is explicit and encoded in the shared tracker | Resolved (was Low) | 99% | Complete: operation retries use durable IDs and cannot acquire a transport replay window. |

There are no remaining Critical or High findings. P5 should not remain an open
broad transaction-refactor request in its current wording.

### Critical/High Resolution Design Audit

The resolved Critical/High findings were re-reviewed against their original
failure modes, current code, and focused regressions. "Best" below means the
smallest design that makes the safety authority durable and unambiguous within
the current CDK architecture, not the largest possible rewrite.

| Finding | Design verdict | Assessment and remaining boundary |
| --- | --- | --- |
| P6 | Best practical fix | Reserving an RAII waiter before persistence removes the demonstrated error path without a compensating delete. Publishing the already-built consensus clone immediately after commit leaves no further application-level cancellation point, and restart reconstruction handles process loss after durable commit. A dedicated durable admission actor/outbox could also reconcile ambiguous database-driver cancellation, but would be a much broader architecture for no demonstrated safety gain here. |
| P7 | Best available fix | The active marker is the serialization point for both insertion and advancement. One SQL transaction locks it, validates every pending envelope/ID, rejects future rows, and atomically rebinds older rows. This is stronger and simpler than an in-memory repair loop or a second migration table. Its linear scan occurs only on session advancement and is appropriate for the bounded pending set. |
| P9 | Correct safety design; future simplification is optional | Generation-scoped task events, publication barriers, and one teardown path prevent a stale or failed task from leaving a false-healthy member. A single structured-concurrency owner could eventually replace some custom watcher/event plumbing and the bounded root-exit grace, but that would primarily reduce code complexity; the current design is deterministic and fail-closed. |
| P12 | Best fix for the cut-through policy | Recomputing local materialized state and comparing it with exact, contiguous, quorum-signed journal coverage reconstructs the safety latch from authoritative evidence. Restoring the removed generic status snapshot would create a second source of truth and legacy persistence. The fix deliberately detects and blocks divergence; it does not attempt automatic state repair. |
| P16 | Best available fix | The quote-scoped RAII guard removes local head-of-line blocking and cleans itself on every exit, while the durable compare-and-set is the cross-process authority for exactly one payment initiator. Either mechanism alone would be insufficient. Recovery after the durable `PaymentAttempted` boundary remains the separate narrow P5 saga question, not a defect in quote isolation. |

Final audit verification (2026-07-13):

- workspace-wide `cargo check --workspace --all-targets` passed;
- P6: seven submission-path tests plus the injected persistence-failure test
  passed;
- P7: the SQLite atomic rebind regression and Axum restart/finalization
  regression passed;
- P9: the deterministic non-retryable apply/generation halt regression passed;
- P12: all 18 startup/readiness regressions passed after tightening certificate
  coverage to exact equality;
- P16: the same-quote idempotency and unrelated-quote concurrency regressions,
  plus the generic saga compare-and-set regression, passed;
- P19: all seven shared sequence-tracker regressions and all seven private
  operation-route regressions passed.

## Findings

### P1: A Valid Single-Member Federation Starts but Cannot Operate

**Status:** Resolved (2026-07-13)

`ThresholdParams::validate` accepts a one-member federation with non-zero
thresholds, and mintd accepts and logs that configuration as an AlephBFT
federation. Live-session startup then returns `NoRemotePeers` without failing
router startup. Because federation readiness requires both a live session and
an inbound network queue, the resulting mint remains permanently unable to
serve authoritative federation reads or public federation submissions.

This became an unavoidable configuration trap when the cleanup removed the
in-memory consensus backend. A configuration that passes validation and mintd
startup must not produce a permanently halted mint.

**Resolution:** Federation mode now has one shared minimum-member invariant:
`ThresholdParams::validate` rejects `member_count < 2` with an explicit error
that directs a one-node operator to standalone mode. Because public config
construction, trusted-dealer generation, dealer-free DKG roster construction,
runtime config loading, wallet import, and mint-info advertisement all validate
the same threshold type, none of those paths can construct or publish a
one-member federation. Mintd therefore rejects the config before router
startup, instead of starting a permanently unavailable HTTP service.

Federation test fixtures that used one node only as a shortcut now use at least
two members. Melt-payment tests explicitly construct an operation for which the
local member is the deterministic elected payment actor; this preserves the
behavior under test without reintroducing the invalid production shape.

Relevant code:

- `crates/cdk-common/src/federation/config.rs:322`
- `crates/cdk-mintd/src/lib.rs:621`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:195`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:229`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:476`
- `crates/cdk-axum/src/federation/node_runtime.rs:544`

Verification:

- `threshold_params_validate_bft_defaults` proves a one-member threshold is
  rejected with the dedicated minimum-member error.
- `configure_federation_rejects_one_member_before_router_startup` loads public
  and private files through mintd's production configuration path and proves
  startup fails with the operator-facing error before router construction.
- `configure_federation_loads_aleph_bft_runtime_config` remains the positive
  multi-member startup regression.
- The federation-enabled `cdk-common` suite, mint federation suite, and focused
  DKG transport-policy regression exercise shared config, runtime, and setup
  paths with the new invariant.

Acceptance criteria:

- Every federation config accepted by mintd can start its required consensus
  runtime and pass the readiness gate.
- Unsupported configurations fail before the HTTP service is advertised as an
  active federation member.

### P2: Session Failure Cleanup Can Race with Supervisor Publication

**Status:** Resolved (2026-07-13)

AlephBFT session, network, apply, and completion-watcher tasks are spawned before
the new supervisor and inbound network handle are published in shared runtime
state. On a multithreaded runtime, a root-session panic or very fast completion
can run the completion watcher first. The watcher marks the member halted and
attempts to remove the supervisor, but sees no published supervisor. The startup
path can then continue and publish the already-dead supervisor and inbound
queue.

The member may therefore be halted while reporting live AlephBFT resources as
active. The dead resources can also prevent clean restart or rotation within
the process. The existing panic test does not force this publication ordering,
so it can pass without covering the race.

**Resolution:** Every newly spawned session task now waits on a shared
activation gate. The supervisor begins in `Constructing`; startup acquires the
live-session and inbound-queue publication locks, rechecks the halt epoch and
concurrent-publication invariant, transitions the supervisor to `Running`,
installs both handles, releases the locks, arms the completion watcher, and only
then opens the task gate. There is no await or cancellation point between
publication and activation. A root, network, apply, or activity task therefore
cannot finish, panic, mutate consensus, or emit traffic while its supervisor is
unpublished.

Construction-time halt or concurrent-publication failure shuts down a
`Constructing` supervisor, aborts and joins every owned task, and returns an
error without installing either handle. Completion cleanup remains scoped to a
process-unique generation, so an old watcher cannot clear a newer rotated
session. The lifecycle is monotonic: `Constructing -> Running -> Stopping ->
Stopped`.

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:338`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:516`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:716`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:907`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:917`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:938`

Verification:

- `aleph_bft_session_tasks_wait_for_supervisor_publication_before_activation`
  pauses startup after every task is spawned, proves an injected panicking root
  has not activated and no handle is visible, releases publication, and then
  proves the exact generation becomes Halted with both handles removed.
- `aleph_bft_session_halt_during_construction_aborts_without_publication`
  advances the halt epoch at the same deterministic boundary and proves startup
  returns an error, no task activates, and no resource is published.
- Existing supervisor shutdown, partial-startup failure, root/network panic,
  non-retryable apply failure, automatic-rotation failure, normal rotation,
  stale-generation, restart, and pending-recovery regressions continue to use
  the shared lifecycle path.

Acceptance criteria:

- Session state follows a single monotonic lifecycle: constructing, running,
  stopping, stopped.
- A failed or completed task set cannot be published after cleanup.
- Cleanup is scoped to the exact session generation that triggered it.

### P3: Materialized-State Projection Full-Table Scans Undo Scaling for Melt and Completed Ops

**Status:** Resolved (2026-07-13)

The cleanup moved materialized-state projection into
`crates/cdk/src/mint/federation/projection.rs` and batched mint-quote lookups
through `get_federation_mint_quote_states` in chunks of 500. Melt quotes and
completed operations still load the entire table (`get_melt_quotes()`,
`get_completed_operations()`) and filter in memory to the federation-relevant
IDs.

On `bls-federation`, those collections used per-ID lookups. On a mint with a
large non-federation history, status and catch-up digest computation can become
unbounded in time and memory even when the federation journal itself is small.
That is a cleanup-introduced operational regression opposite the stated scaling
goal for projection.

**Resolution:** The federation database abstraction now exposes projection-only
keyed reads for melt quotes and completed operations. Projection derives the
exact quote IDs and deterministic completed-operation UUIDs from the accepted
journal, queries both indexed primary keys in batches of 500, and rebuilds its
maps and missing-row markers from that bounded result. Its output order and row
contents are unchanged because the existing ordered federation key sets still
drive row construction; database return order is deliberately irrelevant.

The general quote and completed-operation database traits were not expanded,
so this remains a federation-only API change. Payment observations continue to
use the existing federation-only table query with its explicit consensus-order
contract. Their cost is proportional to federation history, not unrelated mint
history.

Relevant code:

- `crates/cdk/src/mint/federation/projection.rs:143`
- `crates/cdk/src/mint/federation/projection.rs:186`
- `crates/cdk/src/mint/federation/projection.rs:214`
- `crates/cdk-common/src/database/mint/mod.rs:958`
- `crates/cdk-sql-common/src/mint/completed_operations.rs:62`
- `crates/cdk-sql-common/src/mint/quotes.rs:306`

Verification:

- `federation_materialized_projection_keyed_reads` seeds 128 unrelated melt
  quotes and 128 unrelated completed operations, requests one tracked key plus
  one missing key from each collection, and proves only the tracked rows are
  returned. The shared regression runs against SQLite and is also registered
  for PostgreSQL.
- All 108 mint federation tests pass, as do the melt-quote catch-up,
  completed-operation catch-up, materialized status digest, and private
  completed-operation fingerprint regressions.
- Federation-enabled PostgreSQL and federation-disabled SQL compilation both
  pass, preserving backend and standalone feature boundaries.

Acceptance criteria:

- Materialized-state projection query cost scales with federation-tracked keys,
  not with total mint table size.
- Digest contents remain identical to the current projection for the same
  federation journal and durable state.

### P4: Aleph Network Retries Can Starve; Restart Recovery Needs an Explicit Contract

**Status:** Open

**Reassessment:** The starvation mechanism is confirmed because retry work is
attempted only when a pump tick drains no fresh outbound messages. The broader
claim that removing durable per-send retries necessarily breaks restart
recovery is not yet demonstrated: Aleph backup recovery, durable pending
operations, protocol rebroadcast, and existing restart/continuation tests are
meaningful counter-evidence. The narrow fairness and observability fix remains
worthwhile; restoring the deleted retry table does not follow without a failed
convergence reproduction.

Cleanup removed durable AlephBFT network-retry rows and kept only a bounded
in-memory FIFO (`CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT = 4096`). Failed
deliveries are lost on process restart. Retries are attempted only when a pump
tick drains zero new outbound messages (`messages_drained == 0`), so sustained
fresh traffic can starve the retry queue until oldest entries are evicted.

The cleanup intentionally followed Aleph backup + protocol rebroadcast rather
than persisting every failed gossip send. Existing restart, backup, pending-
operation, and continuation coverage is evidence that the tradeoff can be
sound; document that recovery contract and retain a focused outage/restart
regression. The confirmed bug in this item is retry starvation under sustained
fresh traffic, not the absence of a durable per-send retry table by itself.

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_network.rs:22`
- `crates/cdk-axum/src/federation/aleph_bft_network.rs:314`
- `crates/cdk-axum/src/federation/aleph_bft_network.rs:476`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:622`

TODO:

- [ ] Document the guaranteed recovery mechanism after restart during a peer
  outage: Aleph backup reload, pending-operation persistence, and protocol
  rebroadcast must be sufficient without the old retry table.
- [ ] Drain a bounded retry budget on every pump tick even when new outbound
  messages are present, or otherwise prevent sustained traffic from starving
  retries indefinitely.
- [ ] Keep hard queue bounds, but emit metrics/alerts when deliveries are
  dropped so operators can detect eviction under partition.
- [ ] Add a restart-during-peer-outage test that reaches finality after heal
  without persisted gossip retries.
- [ ] Add a sustained-outbound-traffic test proving failed deliveries are
  retried before the queue is force-evicted, or that eviction is accompanied
  by protocol-level recovery that still converges.
- [ ] Re-run partition/heal, one-offline window, and multi-member continuation
  Aleph tests after any pump-policy change.

Acceptance criteria:

- A member that restarts while a peer is unreachable still converges after the
  peer returns without requiring durable gossip-retry rows.
- Failed deliveries are not starved to eviction solely because fresh outbound
  traffic never idles the pump.

### P5: Staged Federation Apply Transactions Need Replay-Safety Verification

**Status:** Needs narrowing; explicit transaction rollback is implemented

**Reassessment:** Commit `c59be01ce` (`refactor: centralize federation apply
transactions`) introduced `finish_federation_transaction` and
`finish_federation_apply_transaction`. Current federation appliers route their
local database transaction outcomes through those helpers, and the runtime's
Failed/Rejected persistence path also rolls back a failed write explicitly.
The original claim that these paths generally rely on transaction Drop
semantics is therefore stale.

The remaining question is narrower: melt payment evidence, external payment
effects, saga transitions, and terminal journal application intentionally span
multiple crash-recovery boundaries. They cannot honestly be represented by
one database transaction. Retain this item only as a focused replay and
idempotence audit of that staged lifecycle; do not reopen the completed broad
transaction-ownership refactor.

The remaining staged melt path may intentionally commit immutable observation
evidence before a later quote or saga transition. That is not equivalent to an
uncommitted partial transaction: external payment side effects and crash-safe
saga phases require durable boundaries. The residual risk is that a crash or
late failure between those phases could replay to a different terminal result,
duplicate an outgoing payment, or leave members with different projected
evidence/application state. Those are the invariants the remaining work should
test.

Relevant code:

- `crates/cdk/src/mint/federation.rs:831` transaction finalizer
- `crates/cdk/src/mint/federation.rs` apply helpers for mint quote, mint quote
  payment, mint, swap, melt, melt quote payment, and related status updates
- `crates/cdk-axum/src/federation/node_runtime.rs:1559` operation apply-failure
  persistence paths
- `docs/federation-cleanup-todo.md`, item 25

TODO:

- [x] Introduce a shared apply-transaction finisher that commits on success and
  always rolls back on error before propagating the apply failure.
- [x] Ensure failed payment-observation apply inside one local transaction
  leaves no durable observation row
  and leaves journal status at `Accepted` until the runtime records
  Failed/Rejected deliberately.
- [ ] Document and test melt-payment phases that deliberately commit evidence
  before quote/saga finalization; do not imply one transaction spans external
  payment I/O.
- [x] On definitive validation failure, persist Rejected + `ErrorResponse`
  through the same transactional boundary used for status updates, then overlay
  consensus application state.
- [ ] Add or retain focused crash-point tests after evidence commit and before
  each later melt saga/quote transition.
- [ ] Prove a previously interrupted staged apply reaches the same deterministic
  Applied or Rejected outcome without duplicate payment or state publication.
- [ ] Re-run payment-observation, melt recovery, catch-up rejection equality,
  and journal replay suites before closing the narrowed item.

Acceptance criteria:

- Every local federation transaction explicitly commits or rolls back.
- Persisted evidence at an intentional saga boundary is replay-safe and cannot
  duplicate an outgoing payment or diverge terminal quote/application state.
- No failed local transaction leaves partial quote, proof, signature, or
  notification state.

## Additional Pre-existing Findings

These defects are present on both compared branches. They are not cleanup
regressions, but were found while tracing cleanup-sensitive lifecycle paths and
must remain visible in the production-readiness work queue.

### P6: Pending Operation Persistence Precedes Waiter Admission

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was High).

Before resolution, submission persisted a newly accepted pending operation
before registering its bounded response waiter and before publishing the cloned
consensus runtime that contained the operation. If waiter registration failed
because the configured limit was full, the method returned an error while
durable storage said the operation was pending and the live runtime did not
contain it. A later wallet retry could repair the mismatch, but recovery must
not depend on resubmission.

**Resolution:** Admission now operates on an unpublished consensus clone, then
reserves an RAII waiter, persists a newly accepted pending row, and publishes
the clone without another cancellation point. Waiter exhaustion therefore
happens before durable or runtime mutation. Persistence failure drops the
reservation automatically and leaves the original runtime unchanged. A crash
after durable commit remains safe because startup reconstructs the queue from
pending rows; no compensating delete creates another partial-failure path.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:723`
- `crates/cdk-axum/src/federation/node_runtime.rs:770`
- `crates/cdk-axum/src/federation/node_runtime.rs:803`
- `crates/cdk-axum/src/federation/node_runtime.rs:815`
- `crates/cdk-axum/src/federation/node_runtime.rs:1444`
- `crates/cdk-axum/src/lib.rs:610`

TODO:

- [x] Make waiter admission, durable pending persistence, and runtime queue
  publication one failure-safe transition.
- [x] Prefer reserving the waiter first with an RAII registration that is
  automatically removed if persistence fails.
- [x] Avoid a compensating delete by ensuring waiter failure occurs before the
  pending insert and runtime publication.
- [x] Preserve duplicate-submission joining without consuming another mempool
  slot.
- [x] Add a waiter-limit test proving a rejected distinct operation exists in
  neither the database nor the live consensus queue.
- [x] Add a database-failure test proving no waiter or pending row leaks.
- [x] Add a restart invariant test proving every persisted pending row is
  represented in the recovered consensus queue.

Acceptance criteria:

- Every submission return path leaves a new operation represented in both
  durable pending state and the live queue, or in neither.

Verification:

- all seven focused AlephBFT submission tests pass across success, timeout,
  duplicate, mempool-full, and waiter-full paths;
- the waiter-capacity regression proves a rejected distinct operation is
  absent from both the current consensus queue and durable pending rows;
- deterministic one-shot pending-persistence failure proves the RAII waiter is
  released, no state is published, and an exact retry is admitted once;
- the existing pre-backup restart regression proves a durable pending
  operation is recovered, finalized, and removed without wallet resubmission;
- all 28 standalone Axum tests pass with federation disabled.

### P7: Session Rotation Does Not Rebind Durable Pending Operations

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was High).

Automatic session rotation requeues unfinalized local operations into the next
AlephBFT session in memory, but their durable pending records retain the old
session ID. Rotation subsequently persists the new active-session marker. A
crash before finalization causes startup to recover the new session while
loading only pending rows already tagged with that session, so old-session
operations can remain stranded until a wallet resubmits them.

Resolution:

- `set_federation_aleph_bft_active_session` now makes the marker and pending
  rows one database invariant. In the same transaction it writes the monotonic
  marker, reads back the effective active session, locks and fully decodes every
  pending record, verifies each stored operation ID against its envelope, fails
  closed on future-session rows, and rebinds every older row to the active
  session.
- Pending insertion locks that same marker row, verifies the operation ID before
  writing, and rejects a row older than the active session. A concurrent insert
  therefore either commits before advancement and is rebound, or observes the
  advanced marker and fails; it cannot appear late under the old session.
- Startup recovery performs that atomic advance/rebind before selecting pending
  rows for the recovered consensus queue. A state left with an advanced marker
  and older pending rows is repaired and loaded during the same boot, not one
  restart later.
- The same transaction method is used by natural startup, automatic rotation,
  and catch-up session advancement, so no separate marker-only path can strand
  pending work.
- In-memory rotation retains both already queued items and items dispatched into
  unfinalized units. The durable rows for both classes are session-rebound by
  operation ID before the next live session is published.

Relevant code:

- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-sql-common/src/mint/federation/journal.rs`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs`
- `crates/cdk-axum/src/lib.rs`

TODO:

- [x] Persist the new active session and rebind every requeued pending operation
  in one transaction.
- [x] Validate each pending operation ID and envelope before rebinding it.
- [x] Do not start or publish the new session until the durable rotation
  transaction commits.
- [x] Define safe startup handling for pending rows belonging to older sessions.
- [x] Add crash-point tests before rotation persistence, after the active marker
  commits, and before the requeued operation finalizes.
- [x] Add a restart test proving multiple requeued operations finalize and are
  removed without wallet resubmission.
- [x] Cover both queued operations and dispatched-but-unfinalized units.

Verification:

- The generic SQL database regression proves active marker and multi-row rebind
  commit together, roll back together, reject stale insertion and a
  future-session inconsistency, and advance cleanly once that future session
  becomes active.
- The consensus regression rotates with one queued operation and one operation
  already dispatched into an unfinalized unit, then finalizes both in session 1.
- The Axum restart regression seeds two session-0 pending rows, commits the
  atomic advance/rebind to session 1, reconstructs the runtime, finalizes both,
  and proves their durable pending rows are removed without another submission.

Acceptance criteria:

- Advancing the durable active-session marker cannot make a durable pending
  operation undiscoverable by startup recovery.
- Rotation remains idempotent across every tested crash boundary.

### P8: Duplicate Finalized-Unit Recovery Does Not Notify Waiters

**Status:** Resolved (2026-07-13)

The duplicate-finalized-unit path can retry journal application successfully
and return current responses, but unlike normal finalization it does not call
`notify_operation`. If processing was interrupted after durable finalization
but before application or notification, an active request waiter can time out
even after duplicate recovery makes the operation terminal.

**Resolution:** Normal and duplicate finalized-unit handling now converge on a
single terminal-response publication helper. Before that helper runs, journal
application reloads each operation's committed application state and overlays
the exact `Applied` or `Rejected` state into the in-memory consensus journal.
This closes both recovery gaps: an operation already applied durably but still
shown as accepted in memory is repaired, and a durable business rejection
retains its persisted Cashu `ErrorResponse`.

Publication rejects any non-terminal response, records apply completion, then
notifies active waiters and inserts the response into the existing bounded
cache. Notification remains idempotent: an existing operation ID is replaced
in place without another order entry, and the normal cache limit is enforced
after every publication. No waiter or cache mutation occurs before durable
application succeeds.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:255`
- `crates/cdk-axum/src/federation/node_runtime.rs:291`
- `crates/cdk-axum/src/federation/node_runtime.rs:1527`
- `crates/cdk-axum/src/federation/node_runtime.rs:1568`
- `crates/cdk-axum/src/federation/node_runtime.rs:1703`

Verification:

- `duplicate_finalized_unit_recovers_lost_applied_response_publication`
  suppresses the first post-commit publication, proves the original waiter and
  cache remain untouched while materialized state is durable, resets the
  in-memory marker to model an interrupted overlay, replays the finalized unit,
  and proves the waiter receives `Applied`.
- `duplicate_finalized_unit_recovers_lost_rejected_response_publication` repeats
  the same interruption for a deterministic conflicting swap, verifies the
  exact persisted Cashu rejection survives an interrupted in-memory overlay and
  reaches the waiter, and replays the duplicate repeatedly.
- Both tests prove waiter count returns to zero and repeated duplicate units
  leave the response cache at one entry per operation. Existing normal
  finalization, unmaterialized duplicate recovery, cache-bound, and all 108
  mint federation regressions pass.

Acceptance criteria:

- Every path that makes an operation terminal publishes the same terminal
  response to active waiters and the bounded response cache.

### P9: A Non-Retryable Apply Failure Can Leave a False-Healthy Session

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was High).

**Reassessment:** The original mechanism is slightly inaccurate. A
non-retryable per-payload apply error returns from
`apply_federation_aleph_bft_finalized_unit_with_retry`, but the outer receiver
loop continues rather than the apply task exiting. The safety outcome remains:
the finalized payload was not applied, no Halted/quarantine transition occurs,
and the live session can continue reporting ready. Rewrite tests and the fix
around a silently skipped finalized payload, while still supervising actual
task termination as a neighboring essential-task invariant.

The finalized-unit receiver calls a per-payload retry helper. A non-retryable
error makes that helper log and return without propagating failure or marking
the member Halted; the receiver then waits for the next payload. Readiness is
derived from the installed supervisor and inbound queue, so the member may
remain submission-ready after skipping application of finalized state. The
supervisor also monitors completion of only the root AlephBFT session, leaving
actual unexpected apply/network task termination as a related unsupervised
failure mode.

Resolution:

- The finalized-unit retry helper now returns a non-retryable error instead of
  logging and returning success to the outer receive loop. The apply task sends
  that terminal outcome to the live-session watcher, which marks the exact
  generation Halted and tears down its admissions and sibling tasks.
- Root completion/panic, apply failure/panic/exit, and network-pump panic/exit
  are represented as explicit essential-task events. Root-coupled channel
  closure gets a short bounded opportunity to deliver the root completion
  event; an isolated task exit is terminal.
- Each installed supervisor carries a process-unique generation. Task failures
  can halt only that installed generation. A regression rotates to a new
  supervisor and proves a delayed old-generation failure changes neither the
  halt epoch nor the new live resources.
- Session publication is barriered until the complete supervisor is installed.
  A monotonic halt epoch prevents setup work that raced a terminal halt from
  publishing afterward.
- Persisted/operator-visible halt reasons use fixed contextual text; raw apply
  or task errors are logged locally but are not copied into the halt reason.

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:258`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:689`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:693`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1239`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1254`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1289`

TODO:

- [x] Add deterministic fault injection for a non-retryable finalized-payload
  apply error and prove the member halts instead of remaining falsely ready.
- [x] Make the non-retryable payload failure halt/quarantine the exact live
  session generation and tear down its consensus admissions.
- [x] Supervise the apply loop, network pump, and root session as
  essential tasks whose unexpected exit halts the member.
- [x] Distinguish expected task cancellation during shutdown or rotation from
  unexpected failure.
- [x] On failure, publish a fixed redacted halt reason, remove the inbound queue,
  reject new public submissions, and join sibling tasks.
- [x] Add tests for apply-task failure, network-pump termination, normal
  shutdown, natural rotation, and restart recovery of finalized work.

Verification:

- Deterministic two-member finality reaches the injected non-retryable apply
  boundary, then both members become Halted, remove live/inbound resources,
  retain no falsely Applied journal entry, and reject a new public submission.
- Root-task and network-pump panic regressions both stop and join the owned
  supervisor generation.
- Normal automatic rotation still reaches session 2; stale generation failure
  leaves the rotated supervisor and inbound queue installed.
- Existing pending-before-backup and persisted-backup restart recovery tests
  remain the durable replay coverage for finalized work interrupted with the
  live generation down.

Acceptance criteria:

- No finalized payload can be skipped while the member remains healthy or
  submission-ready.
- No essential AlephBFT task can terminate unexpectedly while the member still
  reports a healthy, submission-ready session.

### P10: Halted Transitions Do Not Consistently Tear Down Live Session Resources

**Status:** Resolved (2026-07-13)

Cleanup added fail-closed handling when automatic AlephBFT session rotation
fails after natural completion: the completion watcher now calls
`mark_federation_halted(...)`. Panic and rotation-disabled completion paths
correctly use `halt_federation_aleph_bft_session`, which marks Halted **and**
removes the live supervisor plus inbound network queue. The rotation-failure
path only flips sync status to Halted. The ordinary rotation flow has already:

1. advanced the in-memory Aleph session and requeued unfinalized local items
   (`begin_aleph_bft_session_requeue_pending_finalization`);
2. shut down and cleared the previous supervisor / inbound queue;
3. failed while restarting the next live session.

Because the previous resources are normally already absent, rotation failure
alone does not prove that stale live resources remain. Its remaining concerns
are partial publication through P2 and the P7 durable pending-rebind gap.

The stronger concrete bug is in peer-sync halt sites that call only
`mark_federation_halted` for equal-height digest mismatch or no catch-up
progress while the current live session can still be installed. Those sites
predate or parallel the cleanup. Fix all terminal transitions with the same
generation-scoped teardown helper so Halted always means “no live Aleph
resources.” Public submissions are rejected while Halted, so this is primarily
a restart-safety/resource-lifecycle issue; P13 makes residual observer
admission the important neighboring policy risk.

Resolution:

- One teardown helper now owns terminal transition ordering: take the matching
  live supervisor while holding the lifecycle write guard, clear the inbound
  queue, advance the halt epoch, publish a generic Halted reason, and then stop
  and join owned tasks.
- Completion-watcher failures target their exact generation; peer-sync fatal
  errors target the current generation. A stale watcher cannot remove a newer
  supervisor.
- Automatic-rotation startup failure accepts either the old matching generation
  or the expected no-supervisor state after rotation detached it, while refusing
  to tear down a concurrently installed different generation.
- Equal-height digest mismatch and no-progress peer catch-up now call this
  helper instead of changing sync status alone.
- P7 supplies the durable side: active-session advance and pending rebind commit
  together before a failed rotation is allowed to settle in Halted.

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1000`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1031`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1187`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1201`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1213`
- `crates/cdk-axum/src/federation/sync_loop.rs:719`
- `crates/cdk-axum/src/federation/sync_loop.rs:739`

TODO:

- [x] Route every terminal federation halt through one helper that marks
  Halted, clears the inbound queue, and shuts down / joins the live-session
  supervisor for the current generation.
- [x] Use that helper on automatic-rotation failure instead of
  `mark_federation_halted` alone.
- [x] Use the same helper for peer-sync digest-mismatch and no-progress halts.
- [x] Ensure a failed rotation after in-memory session advance cannot leave a
  published supervisor or inbound queue that outlives the Halted status (P2).
- [x] Coordinate with P7 so failed rotation either rolls back the in-memory
  session advance or persists pending rebinds before Halted is durable.
- [x] Add a deterministic test that forces completed-session rotation to fail
  after the previous supervisor is cleared and proves: status is Halted, live
  session handle is absent, inbound queue is absent, and public submit remains
  rejected.
- [x] Add a peer-sync halt test proving digest-mismatch Halted also clears
  Aleph live resources.
- [x] Re-run natural rotation, root-panic halt, restart recovery, and
  pending-operation recovery suites.

Verification:

- Injected automatic-rotation startup failure halts both members only after the
  old supervisors are detached, and proves no live or inbound resources remain.
- The equal-height peer digest-mismatch regression now starts a live supervisor
  first and proves the fatal sync decision stops it and clears inbound state.
- Supervisor shutdown, root panic, natural rotation, stale-generation, and P7
  pending recovery regressions pass through the shared lifecycle path.

Acceptance criteria:

- Every Halted transition removes live Aleph session and inbound resources for
  that generation.
- Automatic rotation failure cannot leave Halted coexisting with an active
  supervisor or inbound queue.
- Failed rotation has an explicit durable story for requeued pending
  operations (shared with P7).

### P11: Local Catch-Up Certificate Persistence Soft-Skips When Digest Is Absent

**Status:** Open

After renaming session outcomes to catch-up certificates, local certificate
signing in the peer-sync loop requires
`local_peer_status(...).materialized_state_digest`. When that digest is `None`,
the loop logs a warning and reports zero certificates collected instead of
failing closed. Catch-up certificate quorum (and the healthy-via-certificate
transition) can therefore stall without a hard error.

`local_peer_status` currently fills the digest on the success path, so this is
latent rather than continuously hit. It is still a cleanup-introduced
fail-open seam: any future status path that omits the digest, or a partial
status helper reused here, silently disables local certificate production.

Relevant code:

- `crates/cdk-axum/src/federation/sync_loop.rs:288`
- `crates/cdk-axum/src/federation/sync_loop.rs:301`
- `crates/cdk-axum/src/federation/sync_loop.rs:406`
- `crates/cdk-axum/src/federation.rs:639`

TODO:

- [ ] Fail closed when local catch-up certificate signing cannot obtain a
  materialized-state digest (return a typed error or compute the digest
  inline from the journal projection set).
- [ ] Do not treat a missing local digest as a successful empty collection
  round.
- [ ] Fail closed on local peer-status read errors the same way (today a status
  Err only warns and reports zero certificates collected).
- [ ] Keep peer collection independent: a local signing failure must not be
  confused with peer backoff.
- [ ] Add a unit/integration test that forces local status without a digest
  (or injects digest computation failure) and proves the member surfaces the
  failure instead of silently skipping certificate persistence.
- [ ] Re-run catch-up certificate quorum, restart, and readiness tests after
  the fail-closed change.

Acceptance criteria:

- Local catch-up certificate production either persists signed chunks or
  reports a clear failure; it never no-ops on a missing digest.
- Local status-read failures follow the same fail-closed rule (do not treat an
  error as an empty collection round).

### P12 [CRITICAL]: Restart Can Clear a Materialized-State Mismatch Without Revalidation

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was Critical) — before the fix, a member previously
proven to have divergent materialized state could become authoritative after
restart without proving that the divergence was repaired.

**Resolution:** `FederationCatchUpCertificateQuorum` and the contiguous quorum
status now retain the materialized-state digest authenticated by the verified
certificate signatures. Startup no longer promotes its initial fail-closed
status before reading durable evidence. For non-empty history it requires a
fully contiguous certificate quorum covering exactly the local journal,
projects the current local materialized state, compares the two digests, and
makes one generation-safe final status transition. A matching digest opens
Healthy; incomplete or excess coverage remains in certificate verification; a
mismatch remains `MaterializedStateMismatch`; and projection, digest, or
storage errors remain Halted. The final transition is
discarded if a newer readiness state was published while the asynchronous
checks ran, so startup cannot overwrite a concurrent safety decision.

No general status snapshot or compatibility table was restored. Durable
journal rows and signed catch-up certificates remain the authoritative evidence.

Original defect and provenance:

The readiness cleanup had removed durable `FederationMemberStatusSnapshot` state,
including `MaterializedStateMismatch` and safety-relevant `Halted` reasons.
Startup then reconstructed readiness from the local journal length and stored
catch-up certificate coverage. `catch_up_certificate_quorum_status` groups and
verifies certificates using their materialized-state digest, but its returned
`CatchUpCertificateQuorumStatus` retained only signers and index/count fields.
`restore_startup_readiness` therefore marked the member Healthy when certified
operation-count coverage reached the local journal length without recomputing
the local materialized-state digest or comparing it with the certified digest.

On `bls-federation`, a detected materialized-state mismatch was persisted and
restored, so startup remained fail-closed. The cleanup correctly identified
that ordinary Lagging/CatchingUp labels are transient, but it removed the
durable safety latch together with those operational labels.

The pre-fix restart-with-certificate test signed the actual local digest. It
did not cover the critical case where stored certificates bind digest A while
the restarted local database produces digest B.

Cleanup provenance and useful references:

- commit `567d9c821` (`refactor: separate federation state commitments`)
  established the distinct materialized-state digest carried by catch-up
  evidence. This is the evidence startup must preserve and compare, rather than
  reducing the quorum result to operation coverage;
- commit `f92990b0d` (`refactor: move federation readiness into runtime`)
  introduced the startup path that marks Healthy from
  `quorum.operation_count` coverage. At that point the older persisted status
  snapshot still provided a separate mismatch latch;
- commit `28407b92f` (`refactor: derive federation readiness at runtime`) and
  cleanup item 31 removed `FederationMemberStatusSnapshot`, its database APIs,
  the `federation_member_status` table, and startup status restoration. This is
  the cleanup change that turned the existing count-only reconstruction into a
  fail-open restart path for a previously detected mismatch;
- the useful part of cleanup item 31 should remain: ordinary Lagging and
  CatchingUp labels are transient and should be derived. Reopen only its claim
  that durable certificate evidence is sufficient as currently consumed.
  `restore_startup_readiness` must compare the recomputed local materialized
  digest with the digest bound by the fully contiguous certified range, or a
  minimal durable quarantine must provide an equivalent latch.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:420`
- `crates/cdk-axum/src/federation/node_runtime.rs:504`
- `crates/cdk-axum/src/federation/node_runtime.rs:516`
- `crates/cdk-axum/src/federation/node_runtime.rs:545`
- `crates/cdk-axum/src/federation/node_runtime.rs:557`
- `crates/cdk-axum/src/federation.rs:379`
- `crates/cdk-axum/src/federation.rs:439`
- `crates/cdk-axum/src/federation.rs:509`
- `crates/cdk-axum/src/lib.rs:2298`
- `docs/federation-cleanup-todo.md`, item 31

TODO:

- [x] Preserve the quorum-certified materialized-state digest in
  `CatchUpCertificateQuorumStatus`; do not reduce safety evidence to operation
  count alone.
- [x] Recompute the local materialized-state digest during startup and compare
  it with the certified digest before marking the member Healthy.
- [x] Persist a minimal durable quarantine/safety latch for confirmed
  materialized-state mismatch, or prove that startup revalidation always
  reconstructs an equivalent fail-closed state before authoritative routes
  open.
- [x] Keep ordinary Lagging/CatchingUp status transient; do not restore the
  entire old status-snapshot mechanism solely to fix the safety latch.
- [x] Add a restart regression with valid quorum certificates for digest A and
  a deliberately altered local projection producing digest B. Assert that
  authoritative reads and public submission remain blocked.
- [x] Add the matching control case proving digest A == local digest returns
  Healthy.
- [x] Test empty history, partially certified history, multiple contiguous
  certificate ranges, and a mismatch at the latest fully certified range.
- [x] Coordinate with P10 so a digest-mismatch halt also tears down live Aleph
  resources and cannot be reopened by the count-only startup path.

Acceptance criteria:

- Certificate count/coverage is never sufficient by itself to reopen a member
  with non-empty history.
- A restarted member becomes authoritative only after its local order and
  materialized-state commitments match verified quorum evidence.
- A confirmed materialized-state mismatch remains fail-closed across restart
  until the underlying divergence is repaired and revalidated.

Verification:

- all seven focused `cdk-common` catch-up-certificate tests pass, including
  preservation of the authenticated materialized-state digest;
- all 18 Axum startup tests pass, including matching-digest Healthy,
  insufficient-quorum verification, empty history, and a valid quorum for a
  deliberately different digest that blocks submission, restore, and
  check-state;
- five focused Axum catch-up quorum tests pass, including a two-range prefix
  proving the latest certified digest governs the completed prefix;
- all 28 standalone Axum tests pass and `cdk-common` checks without default
  features, confirming the federation-only repair does not alter standalone
  behavior.

### P13: Payment-Observation Submit Bypasses Public Federation Readiness

**Status:** Open

**Reassessment:** The bypass is confirmed, but it is not automatically a bug
to let a Lagging or CatchingUp member contribute locally observed payment
evidence. The minimum defensible contract is narrower than the public Healthy
gate: require a live current-generation session and reject Halted/tearing-down
members. Whether other non-Healthy states may submit should be an explicit,
rate-bounded policy tested against observation quorum and quarantine behavior.

Wallet-facing federation submits go through
`ensure_federation_ready_for_public_submit` (authoritative-read readiness:
Healthy + live session + inbound). Payment-observation envelopes submitted by
local backend observers use
`submit_federation_payment_observation_envelope`, which skips that gate and
calls `submit_federation_envelope_with_sequence` directly.

On `bls-federation` this path also skipped readiness and optionally forwarded
to a primary. Cleanup removed primary-forwarding, so every observer injects
directly into the local Aleph mempool whenever a live session will accept the
envelope. A member that is `Lagging`, `CatchingUp`, or
`VerifyingCatchUpCertificate` can therefore still order mint/melt payment
observations into global consensus while wallet mint/melt remains blocked.

That can be desirable (observations should keep flowing toward quorum) but it
is currently an implicit exception without a written policy:

1. A lagging member may observe backend state that is not yet consistent with
   federation tip and propose evidence others already rejected or have not
   applied.
2. Observation traffic still consumes pending-mempool and waiter capacity
   while the member is not ready for public work, interacting with P6 under
   load.
3. After incomplete halt teardown (P10), any residual live session keeps
   observer injectability while public routes reject, widening the
   half-live window.

Relevant code:

- `crates/cdk-axum/src/lib.rs:1510`
- `crates/cdk-axum/src/lib.rs:1543`
- `crates/cdk-axum/src/custom_handlers.rs` mint/melt payment observation
  submit helpers
- `crates/cdk-axum/src/federation/node_runtime.rs:602` envelope submit / pending
  mempool admission

TODO:

- [ ] Decide the readiness contract for local payment observations explicitly:
  (a) allow submit whenever a live Aleph session can accept them (document
  and test), or (b) require the same Healthy authoritative-read gate as
  public/wallet submits, or (c) a middle gate (live session + not Halted,
  but allow Lagging).
- [ ] If lagging submit remains allowed, bound observer inject rate and ensure
  conflicting observations still reach quarantine / fail closed under policy
  rather than finalizing quotes from a minority backlog tip.
- [ ] Ensure Halted always rejects observation submit once live session and
  inbound are torn down (P10); never leave observer-only half-open.
- [ ] Keep observation handlers free of consensus-backend branching after
  Aleph-only cleanup.
- [ ] Add tests for observation submit under Healthy, Lagging,
  VerifyingCatchUpCertificate, and Halted for both mint and melt
  observations.
- [ ] Add a multi-member test where a lagging observer proposes payment
  evidence and prove finalization matches the chosen policy (accept toward
  quorum vs reject until caught up).
- [ ] Re-run payment-observation quorum, quarantine, melt recovery, and
  mempool-limit suites.

Acceptance criteria:

- The readiness rule for payment observations is intentional, documented, and
  covered by tests for every `FederationSyncStatus` variant that can appear
  with a live session.
- Observer inject cannot keep a Halted or resource-torn-down member
  effectively live for consensus admissions.

## Additional Cleanup Regressions

### P14: Cut Through Every Federation-Owned Format to a Single Initial v1

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was Medium) — this was a mandatory pre-release
consistency and assurance cleanup, not a demonstrated cryptographic or
consensus vulnerability among nodes running the same build. Confidence remains
98% because both the original v2 markers and their removal are directly
observable.

The implementation plan chose a cut-through approach: federation has no
supported deployment history, and nodes, payloads, configs, registries,
journals, backups, DKG state, and databases produced before this cleanup are
not compatibility inputs. The implementor must therefore treat the complete
current federation design as its first release and cleanly set every
federation-owned protocol, wire, persisted-artifact, hash-domain, and local
registry version to v1. Discarded development iterations must not appear as
v2, and must not survive through aliases, defaults used as upgrade shims,
legacy DTOs, alternate decoders, deprecated flags, version ranges, or database
migrations.

This is deliberately stronger than renumbering five constants. It establishes
one coherent initial v1 surface and removes any implication that artifacts from
`bls-federation`, earlier cleanup commits, or development deployments are
loadable. Operators and developers recreate them. The only compatibility
boundary begins after this reset is released.

Completed reset scope:

| Component | Pre-reset | Initial release |
| --- | --- | --- |
| Operation envelope | `FEDERATION_OPERATION_VERSION = 2` | `1` |
| Operation ID hash domain | `cdk-federation-operation-v2` | `cdk-federation-operation-v1` |
| DKG commitment hash domain | `cdk-federation-dkg-commitment-v2` | `cdk-federation-dkg-commitment-v1` |
| Persisted DKG participant state | `FEDERATION_DKG_PARTICIPANT_STATE_VERSION = 2` | `1` |
| DKG transcript hash domain | `cdk-federation-dkg-transcript-v2` | `cdk-federation-dkg-transcript-v1` |
| CLI federation registry | `FEDERATION_REGISTRY_VERSION = 2` | `1` for the current `quote_coordinator_member_id` shape |

All other federation-owned boundaries found by the audit are already v1:
wallet protocol, invite, private REST, catch-up certificate, consensus item,
Aleph unit payload/body domain, public DKG ceremony, transcript signatures,
activation signatures, roster/config digests, journal/materialized-state
domains, keyset-rotation proof domain, and federation HTTP routes. Keep their
current complete shapes and exact-version checks, but audit their decoders for
hidden acceptance of earlier development shapes.

Do not reset unrelated version spaces:

- `20260712000000_create_federation_schema.sql` is a migration identifier, not
  a protocol version;
- Cashu keyset v3/BLS is the cryptographic keyset format and remains unchanged.

The completed audit found no federation multi-version decoder, legacy DTO,
Serde alias, deprecated CLI alias, or experimental-schema migration adapter.
P17's former proposal to load registry v1, map `primary_member_id`, and write
registry v2 remains withdrawn. The current `quote_coordinator_member_id`
registry shape is v1, and the removed `primary_member_id` shape is rejected.

Several current structs use `#[serde(default)]` on semantically optional v1
fields, including setup/DKG lifecycle metadata, optional consensus evidence,
payment proofs, and catch-up request/response options. Do not delete these
mechanically. For each default or optional field, prove it is required by the
current v1 state machine or API semantics. Remove it only if its sole purpose
is accepting an obsolete development payload. Missing required v1 fields,
unknown fields on strict config/policy types, altered versions, and old enum or
field names must fail closed.

Cleanup provenance and useful references:

- commit `1685eb760` and cleanup item 9 removed the `Restore` operation kind,
  payload, enum variant, canonical encoding branches, and fixtures while
  leaving operation version 2 and the v2 operation-ID domain unchanged;
- commits `e6136066e` and `f525c33b8` removed melt failure text and wall-clock
  observation fields from operation payloads without advancing the operation
  boundary;
- commit `7ab85c1d1` and cleanup item 15 made previously defaulted operation,
  status, and catch-up fields mandatory while retaining the surrounding
  operation-v2 and private-REST-v1 claims;
- commits `dddc8e82b` and `d6125e7df` compacted and renamed catch-up payloads.
  The catch-up certificate has its own renamed signature domain, but the signed
  REST envelope/body domains and REST API version remained v1;
- commit `606792bb1` and cleanup item 33 correctly separated wallet-facing
  compatibility from operation, REST, Aleph, and catch-up artifact versions.
  Reuse that boundary: an operation or REST bump must not force an unrelated
  wallet protocol/config change;
- commit `3b595c4e4` skipped cleanup item 43's cosmetic renumbering of the
  undeployed protocol to v1 and declared the current v2/v1 labels authoritative
  for development. Reopen that decision now to remove discarded development
  history before the first supported release;
- cleanup item 30 renamed `primary_member_id` to
  `quote_coordinator_member_id`, intentionally omitted a compatibility alias,
  and bumped the local registry to v2. Preserve the no-alias decision but reset
  the complete current registry shape to v1; do not add a v1-to-v2 migrator;
- the final melt-quote operation shape commits the canonical public response
  plus internal backend metadata. That complete shape is now covered by the
  initial-v1 operation vectors; the superseded `FederationMeltQuoteTerms`
  proposal is not a compatibility input.

Under cleanup item 33, wallet public configs, invites, and their digests remain
independent from member-only protocol versions. Preserve that ownership
boundary while making every boundary's current format v1. Regenerate the
current operation vectors/IDs, signed member payload fixtures, DKG
commitment/state/transcript vectors, registry fixtures, generated federation
configs, and every artifact whose canonical bytes or hashes change. Old
development registries, journals, Aleph backups, DKG state, configs, and
databases are recreated rather than migrated.

Resolution evidence in code:

- `crates/cdk-common/src/federation/operation.rs:30` and `:33` define the
  v1 operation domain/version; the canonical v1 vector and strict
  altered-version/removed-shape tests start at `:2049`.
- `crates/cdk-common/src/federation.rs:115`, `:116`, and `:118` define the v1
  DKG commitment, participant-state, and transcript boundaries; DKG v1 state
  rejection coverage starts at `:2341`.
- `crates/cdk-cli/src/sub_commands/federation.rs:22` resets the complete
  current registry to v1; exact-v1 and removed-`primary_member_id` tests start
  at `:826` and `:846`.
- `crates/cdk-common/src/federation/config.rs`, `consensus.rs`, `dkg.rs`,
  `operation.rs`, and `transport.rs` reject unknown fields on federation-owned
  v1 wire and persisted shapes instead of silently accepting removed fields.
- `crates/cdk-common/src/federation/consensus/tests.rs:780` proves removed
  Aleph version-advertisement fields are rejected at their governing boundary;
  `crates/cdk-common/src/federation/transport.rs:2823` covers the removed
  catch-up `session_outcome` field.
- `docs/federation-cleanup-todo.md`, items 9, 15, 23, 33, and 43

TODO:

- [x] Decide that pre-cut-through nodes, payloads, registries, journals,
  backups, DKG state, configs, and databases are unsupported.
- [x] Finish the current operation shape before rebasing its vectors.
- [x] Change the operation envelope version and operation-ID domain from v2 to
  v1.
- [x] Change the DKG commitment domain, participant-state version, and
  transcript-hash domain from v2 to v1.
- [x] Reset `FEDERATION_REGISTRY_VERSION` from 2 to 1 without accepting or
  translating the earlier `primary_member_id` registry shape. Replace the
  stale-v1 test with exact current-v1 acceptance plus version 0, 2, and future
  rejection tests.
- [x] Keep wallet, REST, consensus item, Aleph unit, catch-up certificate, and
  every already-v1 domain unchanged.
- [x] Search the complete workspace for federation-owned version constants,
  `-vN` domains, version dispatch/ranges, Serde aliases/defaults, untagged or
  alternate decoders, deprecated flags, legacy field/enum names, compatibility
  DTOs, migration adapters, and documentation promising old-format support.
- [x] Remove every path whose purpose is compatibility with a pre-cut-through
  federation artifact. Retain optional/defaulted fields only with a documented
  current-v1 semantic reason and a current-shape test.
- [x] Regenerate operation IDs/golden vectors, DKG vectors and participant
  state, transcripts/signatures, generated public/private configs, config
  digests, invites, registry files, and affected fixtures through the existing
  generators.
- [x] Recreate experimental federation databases, Aleph backups, DKG state,
  configs, registries, and local artifacts; add no compatibility decoder,
  alias, translator, or migration.
- [x] Update cleanup item 43 and protocol documentation to name the resulting
  format as the initial v1 release boundary.
- [x] Add exact v1 golden-vector/current-shape tests and altered-version,
  removed-field-name, removed-enum, and unknown-field rejection tests at each
  governing boundary.
- [x] Run shared-core, mint, Axum, wallet/CLI, mintd setup/DKG, SQLite,
  PostgreSQL, and federation smoke suites before closing P14.

Resolution result:

- No production `cdk-federation-*-v2` domain, federation-owned version constant
  set to `2`, registry `primary_member_id`, Serde alias, untagged decoder, or
  federation compatibility migration remains. The consolidated SQL migration
  identifier and Cashu v3/BLS keysets were intentionally left unchanged.
- Operation v1 canonical bytes now produce operation ID
  `f3c0714fa42d9af1c4ac53555c9d854ee66fc4bc2d28710bbdb26acde2278bcd`.
  DKG commitment and transcript goldens are respectively
  `0cbfd40e1de0dc973b5e9bc8fc2ac50529181b4c814c1db3f14491be4222a6ae`
  and `5ed12527c0fd385dc5d3fb5d275429fdeeaeff416f0ea8dc32a3c1f2bd49cb88`.
- Versions 0, 2, and `u16::MAX`, removed operation fields and the removed
  `restore` variant, removed config/Aleph version fields, removed registry and
  catch-up fields, and unknown fields on strict v1 shapes all fail closed.
- Verification passed for `cdk-common` federation tests (337 tests), CLI
  registry tests (12), SQLite federation tests (23), PostgreSQL federation
  compilation, and the wallet connector's nine aggregate-response tests. The
  full Axum federation run was also executed: all version/strict-shape,
  consensus, DKG, and P14 paths passed; its remaining failures are the existing
  single-member melt fixtures that directly persist quotes and assume the
  local member is always the operation-ID-elected payer, plus one timing test.
  Those fixtures do not indicate acceptance of a pre-cut-through format and
  should be repaired as a separate test-harness task rather than by weakening
  deterministic payer election.

Acceptance criteria:

- Every federation-owned protocol, artifact, registry version, and hash domain
  is v1 at its governing boundary; only migration identifiers and Cashu keyset
  versions remain in their unrelated version spaces.
- Canonical v1 operation IDs, DKG commitments/state/transcripts, configs, and
  signatures are deterministic and covered by golden vectors; the current
  registry shape round-trips as v1.
- No decoder, alias, deprecated flag, version branch/range, default, or
  migration exists solely to accept a pre-cut-through federation artifact.
- Pre-cut-through artifacts are explicitly unsupported, rejected fail-closed,
  and excluded from deployment/upgrade guarantees.
- Future incompatible changes advance the relevant local version/domain
  without changing unrelated wallet compatibility metadata.

### P15: Squashed Federation Migrations Use the Accepted Fresh-Database Contract

**Status:** Complete (2026-07-13)

**Severity:** Accepted pre-release reset — not an open security finding.
Pre-cleanup federation databases are explicitly unsupported and disposable, so
the absent forward migration is intentional rather than a data-preservation
violation.

The cleanup deletes the experimental federation migration chain in both SQLite
and PostgreSQL and replaces it with one
`20260712000000_create_federation_schema.sql`. Any database created while
developing or testing `bls-federation` can contain applied migration versions
that are now absent from the embedded migrator. CDK's custom migrator does not
validate that every recorded historical migration still exists. Instead, it
sees the new consolidated migration name as unapplied and executes it over the
old federation schema. The consolidated migration begins with a plain
`ALTER TABLE ... ADD COLUMN` and then plain `CREATE TABLE` statements, so an
old database fails on duplicate columns/tables before normal startup. The
concrete failure is reapplying a fresh-schema migration over an existing
unsupported schema rather than sqlx-style missing-version validation.

Because federation is unreleased and pre-cleanup databases are disposable, the
squash itself is accepted. Developers and operators must create a fresh
federation database when moving to this branch; no compatibility migration,
preflight detector, or reset adapter is required for experimental databases.
The consolidated migration may continue changing until the first supported
federation release. From that release onward it becomes immutable and all
schema changes use appended migrations.

Cleanup provenance and useful references:

- commit `b7c231487` (`refactor: squash federation schema migrations`) and
  cleanup item 16 deleted the 20-file SQLite and PostgreSQL federation chains
  and introduced the single `20260712000000_create_federation_schema.sql`
  migration for each backend;
- item 16's fresh-database catalog and CRUD tests prove the consolidated schema
  is valid for a new database. They do not exercise a database whose
  `migrations` table already records any deleted `20260703*` through
  `20260711*` names;
- `crates/cdk-sql-common/src/common.rs::migrate` checks only whether each
  currently embedded migration name is absent, then runs it. It neither
  validates missing historical names nor supplies a transition from the old
  federation schema;
- later cleanup commits, including `d6125e7df` and `28407b92f`, edited the same
  consolidated migration as tables and fields were renamed or removed. Those
  edits are acceptable while this remains an unreleased fresh-schema boundary;
- after the first supported release, preserve the consolidated migration
  exactly and append new migrations in accordance with the repository's normal
  migration-immutability policy.

Relevant code:

- `crates/cdk-sql-common/src/common.rs:74`
- `crates/cdk-sql-common/src/common.rs:93`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260712000000_create_federation_schema.sql:1`
- `crates/cdk-sql-common/src/mint/migrations/postgres/20260712000000_create_federation_schema.sql:1`
- deleted `20260703*` through `20260711*` federation migrations in both
  migration directories
- `docs/federation-cleanup-todo.md`, item 16
- `.github/agents/review.md` database migration guidance

Completion record:

- [x] Pre-cleanup experimental federation databases are unsupported and must
  be recreated.
- [x] SQLite and PostgreSQL use one consolidated fresh-schema federation
  migration.
- [x] Fresh-schema catalog, CRUD, concurrency, restart, and catch-up coverage
  validate the consolidated schema.
- [x] Standalone migration history remains unchanged.
- [x] The first supported federation release will freeze the consolidated
  migration; every later schema change will append a new migration.

Acceptance criteria:

- A fresh database reaches the intended federation schema deterministically.
- Pre-cleanup federation databases and their migration history are outside the
  supported upgrade contract.
- The consolidated migration is immutable from the first supported federation
  release onward.

### P16: One Global Melt-Observation Lock Serializes Unrelated Payments

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was High) — before the fix, a slow or unavailable
backend for one quote could block payment initiation, settlement observation,
repair scans, and status responses for every other melt quote on that member.

`FederationPaymentObserver` owns one `Arc<tokio::sync::Mutex<()>>` for all melt
quotes. `observe_melt_quote` holds that lock while it loads saga state, may
initiate or check an external payment, reads existing observations, and waits
for consensus submission of a new observation. These are network and consensus
latency boundaries, so the critical section can last seconds or until an
upstream timeout.

The lock prevents duplicate concurrent attempts for the same quote, but making
it federation-wide introduces head-of-line blocking that did not exist in the
separate pre-cleanup paths. The repair loop also walks pending melt quotes
sequentially through the same lock, so one stuck quote can prevent later quotes
from being repaired indefinitely.

Some synchronization became necessary because the cleanup moved payment
initiation out of serialized consensus application and into several concurrent
observer triggers. Removing the lock without replacement is unsafe because the
saga's `SetupComplete` to `PaymentAttempted` update is unconditional, allowing
two callers to reach `make_payment`. The correct replacement is a quote-scoped
single-flight lock or, preferably, an atomic durable claim that permits exactly
one caller to initiate payment without blocking unrelated quotes.

Resolution:

- `FederationPaymentObserver` no longer owns or holds a federation-wide melt
  mutex across backend and consensus work.
- `Mint` keeps a quote-scoped active-payment set. Its RAII guard is removed on
  success, error, or cancellation, so it suppresses only overlapping local
  probes for the same quote and cannot accumulate terminal quote IDs.
- The authoritative payment-initiation boundary is durable, not in-memory:
  `SagaTransaction::compare_and_set_saga_state` atomically claims
  `SetupComplete -> PaymentAttempted`. Only the transaction that changes the
  row calls `make_payment`; another process or mint instance that loses the
  claim returns without initiating a second payment.
- Standalone melt keeps its existing unconditional saga transition and payment
  path. The conditional claim is used only for federation observation.

This combined design is stronger than either replacement alone. A keyed local
guard removes head-of-line blocking and prevents premature same-process status
checks while a backend call is in flight, but cannot coordinate processes or
survive restart. The database compare-and-set supplies that authority across
processes and restarts without serializing unrelated quote IDs.

Relevant code:

- `crates/cdk-axum/src/federation/payment_observation.rs`
- `crates/cdk/src/mint/federation.rs`
- `crates/cdk/src/mint/melt/melt_saga/mod.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/saga.rs`

TODO:

- [x] Replace the global lock with a per-quote keyed lock or another
  quote-scoped single-flight mechanism.
- [x] Bound and clean up the keyed-lock map when a quote becomes terminal so
  arbitrary quote IDs cannot grow it indefinitely.
- [x] Keep exactly-one elected payment attempt for concurrent requests against
  the same quote.
- [x] Do not hold a global observer lock while waiting on backend I/O or
  consensus finality.
- [x] Add a deterministic test with two different pending quotes: block the
  first backend probe and prove the second quote can still initiate/observe and
  submit its result.
- [x] Retain the existing concurrent-identical-melt regression proving one
  outgoing payment attempt for one quote.

Verification:

- The generic saga database regression proves the first conditional transition
  succeeds and a stale second claim does not change the row.
- A deterministic same-quote race blocks the winning backend call and proves a
  concurrent duplicate returns without a second `make_payment`.
- A deterministic two-quote race blocks the first backend call and proves the
  second quote reaches its backend concurrently.

Acceptance criteria:

- Unrelated melt quotes make progress independently.
- Same-quote concurrency remains idempotent and cannot trigger duplicate
  outgoing payment attempts.

### P17: Legacy Wallet Registry Migration Is Superseded by Cut-Through

**Status:** Complete — closed as not applicable and folded into P14
(2026-07-13)

**Severity:** Not an open finding. Confidence is 99%.

The earlier assessment treated the rename from `primary_member_id` to
`quote_coordinator_member_id` as a deployed local-state compatibility problem
and proposed a v1-to-v2 loader plus a deprecated CLI alias. That premise
conflicts with the project's explicit cut-through policy: no federation
deployment or development artifact predating the cleanup is supported.

Do not implement the proposed compatibility layer. P14 now requires the
complete current registry shape to become its initial v1 by resetting
`FEDERATION_REGISTRY_VERSION` from 2 to 1. The old `primary_member_id` field,
old CLI flag, omitted policy fields, and any earlier registry shape remain
unsupported and must not be translated, defaulted, or silently accepted.
Developers recreate or re-import the registry.

This preserves the useful part of cleanup item 30—the narrower coordinator
semantics and absence of a compatibility alias—while removing the development-
history-only v2 label. Exact current-v1 validation and rejection of version 0,
2, and future versions belong to P14's completion tests.

Relevant code:

- `crates/cdk-cli/src/sub_commands/federation.rs:22`
- `crates/cdk-cli/src/sub_commands/federation.rs:72`
- `crates/cdk-cli/src/sub_commands/federation.rs:218`
- `crates/cdk-cli/src/sub_commands/federation.rs:305`
- `crates/cdk-cli/src/sub_commands/federation.rs:823`

No independent TODO remains. Registry renumbering, legacy-shape rejection, and
current-v1 round-trip coverage are part of P14.

### P18: Security-Critical Architecture Tests Were Removed Before Enforceable Boundaries Existed

**Status:** Open

**Severity:** Medium assurance gap — no current signing bypass or
pre-commit publication bug was demonstrated, but future changes can violate
security-sensitive architecture rules without a focused regression failing.

Cleanup item 41 removed source scanners that guarded several important
invariants:

- Public handlers must not call raw blind-signing APIs directly.
- Federation mint, swap, melt, restore, and quote paths must pass through the
  intended readiness and consensus boundaries.
- Pub/sub notifications must occur only after the surrounding database
  transaction commits.
- Production federation builds must not expose fake-wallet or development
  smoke-test surfaces.

The removed tests were formatting-sensitive and were not proofs of runtime
behavior, so retaining them permanently would not be a strong architecture.
However, their replacements primarily exercise existing route behavior plus a
production CLI parser and migrated-schema fixture. Those tests verify the
reviewed paths today, but they do not prevent a new handler from acquiring a
raw signatory, a new publication site from moving before commit, or a new
production feature edge from exposing development-only code.

Existing behavioral coverage reduces the immediate risk: federation public
routes have fail-closed tests, standalone mint APIs reject federation signing
paths, and quote lifecycle tests exercise some commit-before-publish behavior.
The remaining issue is the lack of an enforceable boundary covering future
call sites, not evidence that current production behavior already violates the
invariants.

**Reassessment:** The gap is concrete at the capability level even though no
current bypass was found. `FederationRuntimeConfig::private_config` is public,
and `MemberConfigPrivate::sign_blinded_messages` is also public, so an Axum
handler can acquire raw federation signing capability without going through
`Mint::blind_sign` and its member-mode guard. A useful fix is to expose narrow
identity/network signing and persisted federation-share response capabilities
to outer crates while keeping raw blinded-message share construction inside
the mint federation application boundary.

Relevant changes:

- commit `133412af8` removed the source architecture checks
- `crates/cdk-axum/src/custom_handlers.rs`
- `crates/cdk/src/mint/federation.rs:149`
- `crates/cdk-common/src/federation/signing.rs:147`
- `crates/cdk/src/mint/mod.rs`
- `crates/cdk/src/mint/federation/tests/runtime.rs`
- `crates/cdk-mintd/src/lib.rs`
- `docs/federation-cleanup-todo.md`, item 41

TODO:

- [ ] Restrict raw signing behind module-private or sealed typed capabilities
  that public HTTP handlers cannot access.
- [ ] Expose federation handlers only to consensus submission and persisted
  signature-share response APIs.
- [ ] Encapsulate commit and notification publication, or add transaction
  failure tests proving no authoritative event is emitted before commit.
- [ ] Keep production feature-matrix checks proving fake-wallet and smoke
  commands are absent from the compiled surface.
- [ ] Until type-level boundaries exist, retain narrowly scoped enforcement for
  the direct-signing allowlist and commit-before-publish rule rather than
  removing all coverage at once.

Acceptance criteria:

- Public federation handlers cannot compile against raw signing capabilities.
- A failed transaction emits no mint, melt, swap, proof-state, or quote
  notification.
- Every public signing path has a behavioral test proving no signature or
  authoritative mutation occurs before consensus finality.
- Production federation feature builds exclude development-only payment and
  smoke-test surfaces.

### P19: SubmitOperation Uses Operation-ID Replay Semantics

**Status:** Resolved (2026-07-13)

**Severity:** Resolved (was Low) — exact operation replay remains idempotent by
operation ID and does not duplicate consensus mutation. The signed wire
contract now explicitly gives `SubmitOperation` different replay semantics from
message classes with sequence windows.

Before resolution, `FederationSignedRequest` documented every `sequence` as a
monotonic sender-local replay watermark, and the common sequence tracker could
allocate a volatile per-kind window for `SubmitOperation`. The private operation
route did not use that tracker: it verified the signature and operation
admission, then submitted only the operation envelope. The behavior was safe,
but the shared abstraction and wire documentation contradicted it.

The route tests deliberately establish the resulting behavior:

- Replaying the exact signed operation returns its durable duplicate outcome.
- Sending a different operation with the same sender and sequence is accepted
  as a second operation with a different operation ID.

Operation-ID idempotency is the correct primary defense for an effectful
operation because it survives restart and permits safe retry after a lost
response. Enforcing a volatile sequence before checking the operation ID would
reject a legitimate exact retry. Binding a sequence after duplicate resolution
would add only same-process diagnostics: it disappears on restart, while a
malicious member can always choose a fresh sequence. The selected policy makes
the durable invariant explicit instead of adding a second, weaker identity.

**Resolution:** `SubmitOperation.sequence` is authenticated transport provenance,
not a replay watermark. Canonical operation IDs and durable pending/finalized
records provide restart-safe idempotency. The route continues to bypass
sequence ordering, and the shared sequence tracker now encodes the exception:
recording a `SubmitOperation` sequence is a no-op and cannot allocate a replay
window. This is preferable to a volatile `(sender, sequence)` binding, which
would disappear on restart without providing Byzantine containment, or a
highest-sequence check, which would break safe exact retry after response loss.
Effectful DKG replay state remains durable; status, catch-up, readiness, and
Aleph traffic retain isolated volatile windows.

Relevant code:

- `crates/cdk-common/src/federation/transport.rs:61`
- `crates/cdk-common/src/federation/transport.rs:381`
- `crates/cdk-common/src/federation/transport.rs:543`
- `crates/cdk-common/src/federation/transport.rs:616`
- `crates/cdk-axum/src/federation.rs:301`
- `crates/cdk-axum/src/federation.rs:317`
- `crates/cdk-axum/src/federation/tests/private_operation_route.rs:148`
- `crates/cdk-axum/src/federation/tests/private_operation_route.rs:177`

TODO:

- [x] Decide and document whether `SubmitOperation.sequence` is intentionally
  informational because operation IDs own idempotency, or whether it must
  detect distinct operations reusing one sender sequence.
- [x] Do not add a volatile sequence-to-operation binding: it would not survive
  restart, and operation-ID duplicates already resolve to durable outcomes.
- [x] Keep effectful DKG sequences durable and keep status, catch-up, readiness,
  and Aleph transport windows isolated by message kind.
- [x] Add a focused test for the selected same-sequence/different-operation
  policy while retaining exact-retry behavior across response loss and restart.
- [x] Update the `FederationSignedRequest.sequence` documentation if replay
  semantics intentionally differ by message kind.

Acceptance criteria:

- Exact operation retries remain idempotent and return the durable outcome.
- Distinct operations reusing one signed sender sequence are either rejected
  consistently or explicitly documented and tested as allowed.
- Sequence handling cannot cause duplicate state mutation or block a valid
  exact retry after response loss.

Verification:

- `cargo test -p cdk-common --features federation sequence_tracker_` — 7 passed.
- `cargo test -p cdk-axum --features federation private_operation_route_` — 7 passed.
- The common regression proves `SubmitOperation` cannot allocate replay state;
  the route regressions retain exact-operation retry and explicitly allow a
  distinct canonical operation ID to reuse the signed sequence.

## Accepted / Not Tracked as Bugs

These cleanup deltas were reviewed and are not open findings for this cut-
through branch:

- **Safer payment-observation default.** `FederationConfig::new()` now uses
  consensus-threshold quorum instead of development single-observation. This
  is intentionally stricter. Local/regtest single-observation setups must set
  the development policy explicitly and enable
  `allow_development_payment_observation_policy`. DKG/final-config assembly in
  mintd and mint builder already re-copies the setup payment policy after
  `FederationConfig::new()`.
- **Removed unenforced payment-policy operator limits.** Fields such as
  `max_pending_quotes`, `pending_quote_ttl_secs`, `stale_observation_secs`, and
  `trust_model` were removed with `deny_unknown_fields`. Operator payment-
  observation *reports* now use a fixed 15-minute stale age
  (`FEDERATION_STALE_PAYMENT_OBSERVATION_SECS`), matching the former default.
  Reintroducing a tunable report threshold is optional product work, not a
  correctness regression.
- **Wall clock removed from payment observations.** `observed_time` /
  `failure_reason` were dropped from observation matching. Federated mint
  quote `payment.time` records consensus logical time
  (`operation_index + 1`). Axum handlers already sent `observed_time: None`,
  so runtime quorum behavior is unchanged.
- **Mintd federation config fail-closed renames.** `deny_unknown_fields` plus
  removal/rename of `consensus_backend`,
  `session_outcome_evidence_chunk_size`, and
  `state_digest_checkpoint_retention` are accepted cut-through breaks. Carry
  forward configs are unsupported and must be regenerated.
- **Compact catch-up wire format.** Catch-up responses carry compact operation
  commitments and reconstruct full journal entries from Aleph finalized items.
  No accidental payload-loss bug was found in the committed apply path.
- **Restore outside consensus.** Restore remains a readiness-gated read of
  signature shares; it is no longer a journal operation.
- **AlephBFT-only runtime.** Removal of the in-memory consensus backend and
  backend selection is intentional. Related single-member readiness traps are
  tracked separately as P1.
- **Rotation-disabled completion fail-closed.** When automatic rotation is
  disabled (`automatic_rotation_limit == Some(0)`), natural session completion
  now halts via the full teardown helper instead of skipping the watcher.
  Production peer-sync validation already rejects `rotation_limit == 0`, so
  this path is test/internal only; the fail-closed behavior is accepted.
- **Unfederated mint/swap/melt paths.** No behavioral changes were found in
  standalone (non-federation) quote, swap, melt, or mint handlers. Shared SQL
  quote helpers gained an additive federation-gated projection lookup only.
- **Optional observation `payment_id` serde form.** Mint
  `FederationMintQuotePaymentObservation.payment_id` uses
  `skip_serializing_if = "Option::is_none"` without an explicit
  `#[serde(default)]`. Current `serde` treats missing `Option` fields as
  `None`, so encode/decode round-trips used by journal, Aleph unit bodies, and
  observer enroll remain valid. Melt observations still annotate `default`
  explicitly for consistency only.
- **Melt `failure_reason` removal.** Production live observation handlers always
  set the field to `None` before it was deleted, so matches_payment_evidence
  behavior is unchanged for real traffic.
- **Compact catch-up requires complete finalized-item evidence.** Non-empty
  catch-up suffixes without finalized-item evidence are rejected. That is
  fail-closed, not silent loss; recovery depends on retaining
  `aleph_bft_finalized_items` as the authoritative history store.
- **Materialized-state empty-`fields` digest domain.** Rows always serialize
  `fields` (including `{}`). Digests and digests-vector tests bind the new
  domain; mixed pre-cleanup peers are unsupported for this cut-through. P14
  records the one-time reset of every remaining federation-owned v2 label,
  including the local registry, and regeneration of the first supported v1
  artifacts.
- **Failed application status intentionally not overlaid on Aleph recovery.**
  Only `Applied` / `Rejected` overlay after rebuild from finalized items so
  durable `Failed` operations re-enter accepted apply for retry. This is
  deliberate retry semantics, not lost Failed-status corruption.

## Review Verdict

The cleanup branch is not production ready. P12, the sole Critical finding, and
P1/P2/P3/P6/P7/P8/P9/P10/P14/P16/P19 are resolved. The reassessment retains no
Critical or High findings, four Medium findings (P4, P11, P13, P18), and one Low
residual finding (P5). P15 is complete under the accepted fresh-database policy;
P17 is closed as superseded by P14's completed cut-through registry reset.

P14 completed the one-time reset of every federation-owned format to its clean
initial v1, including the current CLI registry shape, removed legacy acceptance
paths, regenerated artifacts, and froze the first-supported boundary.
P15 accepts the consolidated fresh-schema migration and is closed. Neither item
permits support for pre-cut-through nodes, payloads, registries, configs,
backups, or databases.

No accidental regression was demonstrated in compact catch-up reconstruction,
commitment separation, or unfederated payment handlers. P4 should be narrowed
to retry fairness/observability unless a restart convergence failure is
reproduced. P5's broad explicit-rollback work is complete and should remain
open only for focused staged-melt replay/idempotence verification. P10 is the
general Halted teardown invariant rather than a proven stale-resource outcome
of ordinary automatic rotation alone. P13 still requires an explicit policy
decision; P19 now codifies operation-ID replay semantics without adding
restart-volatile sequence binding. P17's former compatibility proposal is
withdrawn and folded into P14; P18 remains an assurance boundary, not evidence
of a current signing bypass.
