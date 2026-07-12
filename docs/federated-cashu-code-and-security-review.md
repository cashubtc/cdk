# Federated Cashu Code and Security Review

Date: 2026-07-12

This document records actionable findings from a committed-code comparison of
`bls-federation-cleanup` against `bls-federation`. It is a work queue for
correctness, restart-safety, and operational-hardening fixes. Findings should
remain open until their implementation and focused regression tests have been
verified. Re-check nearby lifecycle paths while addressing each item so related
variants are not overlooked.

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
  immutability policy and existing `bls-federation` development databases; it
  is tracked as a critical database-integrity finding below.
- No source behavior was changed as part of this review.

## Critical Priority

The following findings require first attention because they affect whether a
member may safely become authoritative or whether persisted consensus/database
state can be interpreted correctly:

- **P12:** restart can clear a confirmed materialized-state mismatch without
  comparing the local state digest with certified quorum evidence.
- **P14:** incompatible operation and member REST payloads retain their old
  protocol versions and signature/hash domains.
- **P15:** deleted historical federation migrations remove the forward upgrade
  path for databases created from `bls-federation`.

## Open Findings

### P1: A Valid Single-Member Federation Starts but Cannot Operate

**Status:** Open

`ThresholdParams::validate` accepts a one-member federation with non-zero
thresholds, and mintd accepts and logs that configuration as an AlephBFT
federation. Live-session startup then returns `NoRemotePeers` without failing
router startup. Because federation readiness requires both a live session and
an inbound network queue, the resulting mint remains permanently unable to
serve authoritative federation reads or public federation submissions.

This became an unavoidable configuration trap when the cleanup removed the
in-memory consensus backend. A configuration that passes validation and mintd
startup must not produce a permanently halted mint.

Relevant code:

- `crates/cdk-common/src/federation.rs:1467`
- `crates/cdk-mintd/src/lib.rs:621`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:195`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:229`
- `crates/cdk-axum/src/federation/node_runtime.rs:574`

TODO:

- [ ] Decide explicitly whether production federation mode supports a
  single-member federation.
- [ ] If unsupported, reject `member_count < 2` in federation-specific config
  or mintd startup validation with a clear operator-facing error.
- [ ] If supported, allow a one-node AlephBFT session to start and finalize
  operations without a remote network peer.
- [ ] Ensure DKG/setup configuration follows the same member-count rule as
  runtime activation.
- [ ] Ensure the public federation config endpoint never advertises a config
  that mintd cannot operate.
- [ ] Add a full mintd/router startup test for a one-member config; do not limit
  coverage to `MintBuilder` construction.
- [ ] Add a test proving the chosen behavior is explicit: startup fails, or a
  mint quote and one state-changing operation reach finality.
- [ ] Add a regression test proving normal multi-member startup remains
  unchanged.

Acceptance criteria:

- Every federation config accepted by mintd can start its required consensus
  runtime and pass the readiness gate.
- Unsupported configurations fail before the HTTP service is advertised as an
  active federation member.

### P2: Session Failure Cleanup Can Race with Supervisor Publication

**Status:** Open

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

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:589`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:693`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:701`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:715`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:965`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1031`

TODO:

- [ ] Make supervisor publication and task activation an ordered, atomic
  lifecycle transition.
- [ ] Prefer constructing and publishing a generation-bound supervisor before
  the root task and completion watcher can finish, using a startup barrier if
  necessary.
- [ ] Ensure a completion watcher only clears the supervisor and inbound queue
  belonging to its own session generation; it must not clear a newer rotated
  session.
- [ ] Ensure startup returns an error instead of publishing resources when the
  root session fails before activation completes.
- [ ] Ensure all spawned tasks are aborted and joined on every partial-startup
  failure path.
- [ ] Add a deterministic test that blocks publication, forces the root task to
  panic or complete, then releases publication and proves no dead handle is
  installed.
- [ ] Add a concurrent rotation test proving an old completion watcher cannot
  remove the new session's supervisor or inbound queue.
- [ ] Add assertions that `Halted` status never coexists with a dead supervisor
  reported as active.
- [ ] Re-run the existing supervisor shutdown, root-panic, natural-rotation,
  restart, and pending-operation recovery tests.

Acceptance criteria:

- Session state follows a single monotonic lifecycle: constructing, running,
  stopping, stopped.
- A failed or completed task set cannot be published after cleanup.
- Cleanup is scoped to the exact session generation that triggered it.

### P3: Materialized-State Projection Full-Table Scans Undo Scaling for Melt and Completed Ops

**Status:** Open

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

Relevant code:

- `crates/cdk/src/mint/federation/projection.rs:21`
- `crates/cdk/src/mint/federation/projection.rs:144`
- `crates/cdk/src/mint/federation/projection.rs:181`
- `crates/cdk/src/mint/federation/projection.rs:205`
- `crates/cdk-common/src/database/mint/mod.rs:429`
- `crates/cdk-common/src/database/mint/mod.rs:664`
- `crates/cdk-sql-common/src/mint/quotes.rs:372`

TODO:

- [ ] Add batched federation melt-quote state lookup analogous to
  `get_federation_mint_quote_states`, keyed by the melt quote IDs collected
  from the journal projection set.
- [ ] Replace `get_completed_operations()` full-table load with batched or
  multi-get lookup by the completed federation operation UUIDs in the
  projection set.
- [ ] Keep payment-observation projection on an ordered, federation-scoped
  query path; do not regress to unordered full-table scans without an
  explicit ORDER BY contract used by digest construction.
- [ ] Bound memory and query cost by the federation-relevant key set, not by
  total mint history size.
- [ ] Add a benchmark or focused regression that seeds many unrelated melt
  quotes / completed operations and proves projection cost stays proportional
  to federation keys.
- [ ] Re-run digest compare, catch-up, restart, and status readiness tests
  after the lookup change.

Acceptance criteria:

- Materialized-state projection query cost scales with federation-tracked keys,
  not with total mint table size.
- Digest contents remain identical to the current projection for the same
  federation journal and durable state.

### P4: Aleph Network Retry Queue Is Restart-Fragile and Can Starve Under Load

**Status:** Open

Cleanup removed durable AlephBFT network-retry rows and kept only a bounded
in-memory FIFO (`CDK_ALEPH_BFT_NETWORK_RETRY_QUEUE_LIMIT = 4096`). Failed
deliveries are lost on process restart. Retries are attempted only when a pump
tick drains zero new outbound messages (`messages_drained == 0`), so sustained
fresh traffic can starve the retry queue until oldest entries are evicted.

The cleanup intentionally followed Aleph backup + protocol rebroadcast rather
than persisting every failed gossip send. That tradeoff is acceptable only if
restart and partition-heal paths are proven not to depend on the lost queue.
Until those proofs and regressions are explicit operator-facing guarantees,
this remains an open restart-safety and liveness hardening item.

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

### P5: Federation Apply Paths Can Exit Transactions Without Explicit Rollback

**Status:** Open

Several federation mint-apply paths begin a DB transaction, mutate durable
state (quotes, payment observations, signatures), and on error return with `?`
without an explicit `rollback()`. Success paths mark `Applied` and commit.
Some melt-payment flows also commit observation evidence in one transaction
before later quote/saga mutation, so a later failure can leave evidence
persisted while the journal entry remains non-`Applied`.

This pattern already existed on `bls-federation`; cleanup did not introduce it.
It is still a correctness bug: apply failure must leave durable federation
state unchanged except for deliberate Failed/Rejected status updates. Relying
on transaction Drop semantics is insufficient, and split-phase melt observation
commits weaken replay idempotence.

Uncommitted worktree helpers (`finish_federation_transaction`,
`finish_federation_apply_transaction`) and a payment-observation rollback test
appear aimed at this gap but are not part of the committed cleanup branch.

Relevant code:

- `crates/cdk/src/mint/federation.rs` apply helpers for mint quote, mint quote
  payment, mint, swap, melt, melt quote payment, and related status updates
- `crates/cdk-axum/src/federation/node_runtime.rs` operation apply-failure
  persistence paths

TODO:

- [ ] Introduce a shared apply-transaction finisher that commits on success and
  always rolls back on error before propagating the apply failure.
- [ ] Ensure failed payment-observation apply leaves no durable observation row
  and leaves journal status at `Accepted` until the runtime records
  Failed/Rejected deliberately.
- [ ] Revisit melt-payment observation flows that commit evidence before quote
  finalization; keep evidence and status transitions in one atomic apply
  transaction where possible.
- [ ] On definitive validation failure, persist Rejected + `ErrorResponse`
  through the same transactional boundary used for status updates, then overlay
  consensus application state.
- [ ] Add focused tests for mint-payment method mismatch, swap validation
  failure, and melt-payment apply failure proving observation/status rollback.
- [ ] Add a replay test proving a previously failed apply can be retried to the
  same deterministic Applied or Rejected outcome without duplicate side
  effects.
- [ ] Re-run payment-observation, melt recovery, catch-up rejection equality,
  and journal replay suites after the finisher lands.

Acceptance criteria:

- Every federation apply error path either fully rolls back its transaction or
  explicitly persists a Failed/Rejected outcome in one committed transaction.
- No apply failure leaves orphan payment observations or partial quote
  mutations while the journal entry remains `Accepted`.

## Additional Pre-existing Findings

These defects are present on both compared branches. They are not cleanup
regressions, but were found while tracing cleanup-sensitive lifecycle paths and
must remain visible in the production-readiness work queue.

### P6: Pending Operation Persistence Precedes Waiter Admission

**Status:** Open

Submission persists a newly accepted pending operation before registering its
bounded response waiter and before publishing the cloned consensus runtime that
contains the operation. If waiter registration fails because the configured
limit is full, the method returns an error while durable storage says the
operation is pending and the live runtime does not contain it. A later wallet
retry can repair the mismatch, but recovery must not depend on resubmission.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:715`
- `crates/cdk-axum/src/federation/node_runtime.rs:738`
- `crates/cdk-axum/src/federation/node_runtime.rs:756`
- `crates/cdk-axum/src/federation/node_runtime.rs:759`
- `crates/cdk-axum/src/federation/node_runtime.rs:1390`

TODO:

- [ ] Make waiter admission, durable pending persistence, and runtime queue
  publication one failure-safe transition.
- [ ] Prefer reserving the waiter first with an RAII registration that is
  automatically removed if persistence fails.
- [ ] Otherwise, deterministically remove the pending row if waiter admission
  fails before runtime publication.
- [ ] Preserve duplicate-submission joining without consuming another mempool
  slot.
- [ ] Add a waiter-limit test proving a rejected distinct operation exists in
  neither the database nor the live consensus queue.
- [ ] Add a database-failure test proving no waiter or pending row leaks.
- [ ] Add a restart invariant test proving every persisted pending row is
  represented in the recovered consensus queue.

Acceptance criteria:

- Every submission return path leaves a new operation represented in both
  durable pending state and the live queue, or in neither.

### P7: Session Rotation Does Not Rebind Durable Pending Operations

**Status:** Open

Automatic session rotation requeues unfinalized local operations into the next
AlephBFT session in memory, but their durable pending records retain the old
session ID. Rotation subsequently persists the new active-session marker. A
crash before finalization causes startup to recover the new session while
loading only pending rows already tagged with that session, so old-session
operations can remain stranded until a wallet resubmits them.

Relevant code:

- `crates/cdk-common/src/federation/consensus.rs:1626`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1187`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1213`
- `crates/cdk-axum/src/lib.rs:2527`
- `crates/cdk-axum/src/lib.rs:2555`
- `crates/cdk-sql-common/src/mint/federation.rs:1164`

TODO:

- [ ] Persist the new active session and rebind every requeued pending operation
  in one transaction.
- [ ] Validate each pending operation ID and envelope before rebinding it.
- [ ] Do not start or publish the new session until the durable rotation
  transaction commits.
- [ ] Define safe startup handling for pending rows belonging to older sessions.
- [ ] Add crash-point tests before rotation persistence, after the active marker
  commits, and before the requeued operation finalizes.
- [ ] Add a restart test proving multiple requeued operations finalize and are
  removed without wallet resubmission.
- [ ] Cover both queued operations and dispatched-but-unfinalized units.

Acceptance criteria:

- Advancing the durable active-session marker cannot make a durable pending
  operation undiscoverable by startup recovery.
- Rotation remains idempotent across every tested crash boundary.

### P8: Duplicate Finalized-Unit Recovery Does Not Notify Waiters

**Status:** Open

The duplicate-finalized-unit path can retry journal application successfully
and return current responses, but unlike normal finalization it does not call
`notify_operation`. If processing was interrupted after durable finalization
but before application or notification, an active request waiter can time out
even after duplicate recovery makes the operation terminal.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:1470`
- `crates/cdk-axum/src/federation/node_runtime.rs:1481`
- `crates/cdk-axum/src/federation/node_runtime.rs:1515`
- `crates/cdk-axum/src/federation/node_runtime.rs:1524`

TODO:

- [ ] Route duplicate recovery through the same terminal response-publication
  helper used by normal finalization.
- [ ] Notify only after journal application and durable application-state update
  complete successfully.
- [ ] Preserve idempotency when the response is already cached.
- [ ] Add an interruption test after durable finalization and before
  notification, then replay the unit and prove the original waiter completes.
- [ ] Cover both Applied and Rejected operations, including persisted Cashu
  `ErrorResponse` recovery.
- [ ] Prove repeated duplicate units do not grow waiter or response-cache state
  without bound.

Acceptance criteria:

- Every path that makes an operation terminal publishes the same terminal
  response to active waiters and the bounded response cache.

### P9 Candidate: Apply-Task Failure Can Leave a False-Healthy Session

**Status:** Needs focused reproduction

The finalized-unit apply loop exits on a non-retryable error. The supervisor
owns that task but monitors completion of only the root AlephBFT session.
Readiness is derived from the installed supervisor and inbound queue rather
than the health of every essential task, so the member may remain
submission-ready after it has stopped applying finalized state.

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:258`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:689`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:693`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1279`
- `crates/cdk-axum/src/federation/node_runtime.rs:597`

TODO:

- [ ] Add deterministic fault injection for a non-retryable finalized-unit
  apply error and verify the current supervisor/readiness behavior.
- [ ] If confirmed, supervise the apply loop, network pump, and root session as
  essential tasks whose unexpected exit halts the member.
- [ ] Distinguish expected task cancellation during shutdown or rotation from
  unexpected failure.
- [ ] On failure, persist a redacted halt reason, remove the inbound queue,
  reject new public submissions, and join sibling tasks.
- [ ] Add tests for apply-task failure, network-pump termination, normal
  shutdown, natural rotation, and restart recovery of finalized work.

Acceptance criteria:

- No essential AlephBFT task can terminate unexpectedly while the member still
  reports a healthy, submission-ready session.

### P10: Failed Automatic Session Rotation Marks Halted Without Full Session Teardown

**Status:** Open

Cleanup added fail-closed handling when automatic AlephBFT session rotation
fails after natural completion: the completion watcher now calls
`mark_federation_halted(...)`. Panic and rotation-disabled completion paths
correctly use `halt_federation_aleph_bft_session`, which marks Halted **and**
removes the live supervisor plus inbound network queue. The rotation-failure
path only flips sync status to Halted.

That is incomplete. Completed-session rotation can already have:

1. advanced the in-memory Aleph session and requeued unfinalized local items
   (`begin_aleph_bft_session_requeue_pending_finalization`);
2. shut down and cleared the previous supervisor / inbound queue;
3. failed while restarting the next live session.

Leaving Halted without a single teardown helper means leftover or partially
published resources can remain installed (see also P2), in-process recovery
cannot cleanly restart the live session, and the durable pending rebind gap in
P7 becomes harder to reason about after a failed rotation. Public submissions
are rejected while Halted (`is_ready()` is Healthy-only), so this is primarily
a restart-safety / resource-lifecycle bug rather than a silent accept path.

The same incomplete pattern also exists on peer-sync halt sites that only call
`mark_federation_halted` (equal-height digest mismatch and no catch-up
progress). Those predate or parallel the cleanup; fix them with the same
shared teardown helper so Halted always means “no live Aleph resources.”

Relevant code:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1000`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1031`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1187`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1201`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1213`
- `crates/cdk-axum/src/federation/sync_loop.rs:719`
- `crates/cdk-axum/src/federation/sync_loop.rs:739`

TODO:

- [ ] Route every terminal federation halt through one helper that marks
  Halted, clears the inbound queue, and shuts down / joins the live-session
  supervisor for the current generation.
- [ ] Use that helper on automatic-rotation failure instead of
  `mark_federation_halted` alone.
- [ ] Use the same helper for peer-sync digest-mismatch and no-progress halts.
- [ ] Ensure a failed rotation after in-memory session advance cannot leave a
  published supervisor or inbound queue that outlives the Halted status (P2).
- [ ] Coordinate with P7 so failed rotation either rolls back the in-memory
  session advance or persists pending rebinds before Halted is durable.
- [ ] Add a deterministic test that forces completed-session rotation to fail
  after the previous supervisor is cleared and proves: status is Halted, live
  session handle is absent, inbound queue is absent, and public submit remains
  rejected.
- [ ] Add a peer-sync halt test proving digest-mismatch Halted also clears
  Aleph live resources.
- [ ] Re-run natural rotation, root-panic halt, restart recovery, and
  pending-operation recovery suites.

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
- `crates/cdk-axum/src/federation.rs:615`

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

**Status:** Open

**Severity:** Critical — a member previously proven to have divergent
materialized state can become authoritative after restart without proving that
the divergence was repaired.

The readiness cleanup removed durable `FederationMemberStatusSnapshot` state,
including `MaterializedStateMismatch` and safety-relevant `Halted` reasons.
Startup now reconstructs readiness from the local journal length and stored
catch-up certificate coverage. `catch_up_certificate_quorum_status` groups and
verifies certificates using their materialized-state digest, but its returned
`CatchUpCertificateQuorumStatus` retains only signers and index/count fields.
`restore_startup_readiness` therefore marks the member Healthy when certified
operation-count coverage reaches the local journal length without recomputing
the local materialized-state digest or comparing it with the certified digest.

On `bls-federation`, a detected materialized-state mismatch was persisted and
restored, so startup remained fail-closed. The cleanup correctly identified
that ordinary Lagging/CatchingUp labels are transient, but it removed the
durable safety latch together with those operational labels.

The existing restart-with-certificate test signs the actual local digest. It
does not cover the critical case where stored certificates bind digest A while
the restarted local database produces digest B.

Relevant code:

- `crates/cdk-axum/src/federation/node_runtime.rs:309`
- `crates/cdk-axum/src/federation/node_runtime.rs:350`
- `crates/cdk-axum/src/federation/node_runtime.rs:361`
- `crates/cdk-axum/src/federation.rs:375`
- `crates/cdk-axum/src/federation.rs:406`
- `crates/cdk-axum/src/federation.rs:433`
- `crates/cdk-axum/src/federation/sync_loop.rs:846`

TODO:

- [ ] Preserve the quorum-certified materialized-state digest in
  `CatchUpCertificateQuorumStatus`; do not reduce safety evidence to operation
  count alone.
- [ ] Recompute the local materialized-state digest during startup and compare
  it with the certified digest before marking the member Healthy.
- [ ] Persist a minimal durable quarantine/safety latch for confirmed
  materialized-state mismatch, or prove that startup revalidation always
  reconstructs an equivalent fail-closed state before authoritative routes
  open.
- [ ] Keep ordinary Lagging/CatchingUp status transient; do not restore the
  entire old status-snapshot mechanism solely to fix the safety latch.
- [ ] Add a restart regression with valid quorum certificates for digest A and
  a deliberately altered local projection producing digest B. Assert that
  authoritative reads and public submission remain blocked.
- [ ] Add the matching control case proving digest A == local digest returns
  Healthy.
- [ ] Test empty history, partially certified history, multiple contiguous
  certificate ranges, and a mismatch at the latest fully certified range.
- [ ] Coordinate with P10 so a digest-mismatch halt also tears down live Aleph
  resources and cannot be reopened by the count-only startup path.

Acceptance criteria:

- Certificate count/coverage is never sufficient by itself to reopen a member
  with non-empty history.
- A restarted member becomes authoritative only after its local order and
  materialized-state commitments match verified quorum evidence.
- A confirmed materialized-state mismatch remains fail-closed across restart
  until the underlying divergence is repaired and revalidated.

### P13: Payment-Observation Submit Bypasses Public Federation Readiness

**Status:** Open

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

- `crates/cdk-axum/src/lib.rs:1647`
- `crates/cdk-axum/src/lib.rs:1679`
- `crates/cdk-axum/src/lib.rs:1693`
- `crates/cdk-axum/src/lib.rs:1999`
- `crates/cdk-axum/src/custom_handlers.rs` mint/melt payment observation
  submit helpers
- `crates/cdk-axum/src/federation/node_runtime.rs` envelope submit / pending
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

### P14 [CRITICAL]: Incompatible Operation and REST Shapes Retain Their Old Protocol Versions

**Status:** Open

**Severity:** Critical — consensus and member-protocol data that are no longer
mutually decodable still advertise the same versions, weakening upgrade and
restart safety at the protocol boundary.

The cleanup made incompatible changes to consensus operations and private REST
payloads without advancing their protocol versions:

- operation version 2 removed the `Restore` variant and removed or tightened
  observation fields while retaining `FEDERATION_OPERATION_VERSION = 2` and
  the `cdk-federation-operation-v2` hash domain;
- REST version 1 changed catch-up payloads, required current fields, and renamed
  or removed status/evidence shapes while retaining
  `FEDERATION_REST_API_VERSION = 1` and the v1 signing domains.

An old persisted operation-v2 restore envelope cannot be decoded by the new
operation-v2 enum. Likewise, old and new members can reject one another's
payloads as malformed even though both advertise REST v1. Config-digest checks
reduce the chance of an intentionally mixed live cluster, but they do not make
the version claims accurate and do not provide a clear upgrade/reset boundary
for existing journal rows, backups, fixtures, or operator diagnostics.

This overlaps cleanup TODO item 43. The protocol reset must land together with
the incompatible cut-through changes rather than remain deferred after them.

Relevant code:

- `crates/cdk-common/src/federation/operation.rs:28`
- `crates/cdk-common/src/federation/operation.rs:31`
- `crates/cdk-common/src/federation/operation.rs:121`
- `crates/cdk-common/src/federation/operation.rs:1155`
- `crates/cdk-common/src/federation/transport.rs:30`
- `crates/cdk-common/src/federation/transport.rs:34`
- `crates/cdk-common/src/federation.rs:1493`
- `docs/federation-cleanup-todo.md` item 43

TODO:

- [ ] Advance the operation protocol version and its hash-domain tag for the
  current operation enum/canonical shape.
- [ ] Advance the member-facing REST protocol version and every signature/body
  domain whose accepted payload shape changed.
- [ ] Keep wallet-facing public protocol versions separate from member REST
  versions as planned by cleanup item 33; do not force unrelated wallet API
  changes to share one version.
- [ ] Regenerate public-config digests, DKG transcripts, invites, canonical
  vectors, signed catch-up fixtures, and persisted test records.
- [ ] Define the startup behavior for old operation-v2 journals and Aleph
  backups: supported migration or explicit destructive reset. Do not attempt
  to read them as the new operation-v2 format.
- [ ] Add mixed-version startup and private-route tests proving failure occurs
  at an explicit version/config boundary before malformed payload processing.
- [ ] Add a persisted old `Restore` envelope regression proving it is reported
  as unsupported old protocol data rather than corrupt current data.

Acceptance criteria:

- Every incompatible canonical or signed payload shape has a distinct version
  and domain from `bls-federation`.
- Mixed-version nodes and persisted old state fail with an explicit upgrade or
  reset error, never an ambiguous current-format decode failure.

### P15 [CRITICAL]: Squashed Federation Migrations Remove the Existing Upgrade Path

**Status:** Open

**Severity:** Critical — deleting migration versions that may already be
recorded in a database violates the repository's migration-immutability rule
and can prevent the database from opening or migrating.

The cleanup deletes the experimental federation migration chain in both SQLite
and PostgreSQL and replaces it with one
`20260712000000_create_federation_schema.sql`. Any database created while
developing or testing `bls-federation` can contain applied migration versions
that are now absent from the embedded migrator. Such databases do not gain a
forward schema transition and can fail migration validation rather than
starting with the new schema.

The claim that federation is undeployed limits production exposure but does not
make historical migration files mutable. Existing contributor, CI artifact,
staging, or operator databases still need either a valid forward migration or
an explicit destructive reset contract. The squashed migration is also still
being edited as later cleanups rename tables, demonstrating that it is not yet
an immutable release boundary.

Relevant code:

- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260712000000_create_federation_schema.sql:1`
- `crates/cdk-sql-common/src/mint/migrations/postgres/20260712000000_create_federation_schema.sql:1`
- deleted `20260703*` through `20260711*` federation migrations in both
  migration directories
- `.github/agents/review.md` database migration guidance

TODO:

- [ ] Restore the deleted migration files once their versions have existed in
  repository history, and add new forward migrations for removed columns,
  renamed tables, constraints, and the final schema.
- [ ] Add upgrade tests that start from the last `bls-federation` SQLite and
  PostgreSQL schemas and migrate to the cleanup schema without losing
  standalone or federation state.
- [ ] Keep fresh-schema catalog parity tests, but do not use them as a
  substitute for upgrade-path tests.
- [ ] If the project deliberately chooses destructive reset instead, add an
  explicit startup error and documented reset command; do not silently present
  the branch as upgrade-compatible.
- [ ] Stop editing any migration once it is accepted as historical; add a new
  migration for subsequent item-32 table renames or constraints.

Acceptance criteria:

- Every database that applied a migration present on `bls-federation` either
  migrates forward deterministically or receives a clear, intentional reset
  error before normal startup.
- Released migration files remain immutable after this cleanup.

### P16: One Global Melt-Observation Lock Serializes Unrelated Payments

**Status:** Open

**Severity:** High — a slow or unavailable backend for one quote can block
payment initiation, settlement observation, repair scans, and status responses
for every other melt quote on that member.

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

Relevant code:

- `crates/cdk-axum/src/federation/payment_observation.rs:20`
- `crates/cdk-axum/src/federation/payment_observation.rs:49`
- `crates/cdk-axum/src/federation/payment_observation.rs:91`
- `crates/cdk-axum/src/federation/payment_observation.rs:169`
- `crates/cdk/src/mint/federation.rs:990`

TODO:

- [ ] Replace the global lock with a per-quote keyed lock or another
  quote-scoped single-flight mechanism.
- [ ] Bound and clean up the keyed-lock map when a quote becomes terminal so
  arbitrary quote IDs cannot grow it indefinitely.
- [ ] Keep exactly-one elected payment attempt for concurrent requests against
  the same quote.
- [ ] Do not hold a global observer lock while waiting on backend I/O or
  consensus finality.
- [ ] Add a deterministic test with two different pending quotes: block the
  first backend probe and prove the second quote can still initiate/observe and
  submit its result.
- [ ] Retain the existing concurrent-identical-melt regression proving one
  outgoing payment attempt for one quote.

Acceptance criteria:

- Unrelated melt quotes make progress independently.
- Same-quote concurrency remains idempotent and cannot trigger duplicate
  outgoing payment attempts.

### P17: Wallet Registry v1 Is Rejected for a Losslessly Migratable Rename

**Status:** Open

**Severity:** Medium — existing federation-aware CLI wallets become unusable
until their registry is manually recreated, without a security or data-model
reason requiring destructive rejection.

The wallet cleanup renames `primary_member_id` to
`quote_coordinator_member_id`, changes the CLI flag, removes Serde defaults,
bumps `FEDERATION_REGISTRY_VERSION` from 1 to 2, and adds a test that explicitly
rejects registry v1. The old member preference can be mapped losslessly to the
new quote-coordinator preference because the cleanup narrowed that setting's
runtime role rather than changing its type or trust boundary.

Rejecting the old sidecar therefore converts a terminology/ownership cleanup
into avoidable local-state breakage. It is stricter than needed even for an
undeployed federation protocol because wallet registry files are local tooling
state, not consensus-visible configuration.

Relevant code:

- `crates/cdk-cli/src/sub_commands/federation.rs:22`
- `crates/cdk-cli/src/sub_commands/federation.rs:72`
- `crates/cdk-cli/src/sub_commands/federation.rs:218`
- `crates/cdk-cli/src/sub_commands/federation.rs:305`
- `crates/cdk-cli/src/sub_commands/federation.rs:823`

TODO:

- [ ] Load registry v1 and migrate `primary_member_id` to
  `quote_coordinator_member_id` in memory, then persist v2 atomically.
- [ ] Alternatively add a Serde alias for the old field while retaining strict
  rejection of genuinely unknown fields.
- [ ] Keep `--primary-member-id` as a deprecated Clap alias for at least one
  transition period.
- [ ] Preserve explicit validation that the migrated member exists in the
  public roster.
- [ ] Add v1-to-v2 migration tests for `Some(member)`, `None`, missing optional
  policy fields previously supplied by defaults, and an unknown member.

Acceptance criteria:

- A valid registry produced by `bls-federation` loads under the cleanup branch
  without manual deletion or re-import.
- Invalid or unknown-member policies remain fail-closed after migration.

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
  forward configs from `bls-federation` must be rewritten.
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
  domain; mixed pre-cleanup peers are unsupported for this cut-through.
- **Failed application status intentionally not overlaid on Aleph recovery.**
  Only `Applied` / `Rejected` overlay after rebuild from finalized items so
  durable `Failed` operations re-enter accepted apply for retry. This is
  deliberate retry semantics, not lost Failed-status corruption.

## Review Verdict

The cleanup branch requires the open findings above (P1–P17) to be resolved or
explicitly ruled out with deterministic tests before it should be treated as
production ready. P12, P14, and P15 are explicitly **Critical** and should be
prioritized: they cover fail-open materialized-state readiness after restart,
incompatible consensus/member payloads retaining old protocol versions, and
destructive migration-history replacement. No accidental regression was
demonstrated in compact catch-up reconstruction, commitment separation, or
unfederated payment handlers. Projection scaling, retry durability,
apply-transaction rollback, incomplete Halted teardown after failed rotation,
soft-skip catch-up certificate signing, payment-observation readiness policy,
global melt-observer serialization, and registry migration remain open
alongside pending-operation atomicity, rotation recovery, waiter notification,
and essential-task supervision.
