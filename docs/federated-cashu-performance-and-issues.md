# Federated Cashu Performance and Issue Report

Date: 2026-07-14

## Relationship to the federation implementation plan

This document is the investigation, remediation, and verification plan for the
performance, reliability, and robustness work that remains after the core
federation implementation. It is governed by
[federated-cashu-implementation-plan.md](federated-cashu-implementation-plan.md).
The implementation plan is the design and safety contract; this document must
not weaken or work around its invariants to make a benchmark pass.

The most important concepts carried forward from the implementation plan are:

- Federation mode preserves the public Cashu wallet-to-mint protocol while
  requiring consensus ordering and BLS threshold authorization. Standalone CDK
  behavior remains unchanged unless federation mode is explicitly enabled.
- Consensus must precede signature-share creation and externally visible
  safety-critical state changes. No member may independently authorize mint,
  swap, or melt results.
- The federation journal is the authoritative operation history. Operations
  require canonical identities, durable ordering, deterministic and idempotent
  application, exact replay, and matching materialized state.
- Local time, randomness, unordered data, network I/O, and member-local backend
  results must not influence deterministic application unless consensus commits
  the relevant value.
- Payment side effects use the designated replay-safe, quorum-authorized flow.
  Member-local payment or database state is not federation evidence.
- Wallet quorum behavior belongs in the centralized federation connector:
  member fanout, view agreement, error classification, fallback, share
  validation, aggregation, and ambiguous-result handling must not be
  reimplemented independently in each wallet flow.
- The mint domain continues to own Cashu validation and business state changes.
  Federation orchestration owns ordering, quorum authorization, replay, and the
  point at which shared domain behavior runs. Public handlers remain thin.
- Startup, catch-up, DKG, rotation, roster, keyset, transcript, configuration,
  and live-session mismatches fail closed. Local consistency alone is not proof
  that a member is current or able to participate in consensus.
- Federation keysets are v3/BLS-only and must reject aggregate-key, public-share,
  local-share, or keyset-version mismatch without falling back to standalone
  behavior.
- Prefer simplification and existing CDK patterns before adding new types,
  queues, retries, background systems, or parallel implementations. Do not hide
  liveness failures with larger timeouts, unbounded queues, retry storms, or
  serialized tests.
- Federation development remains cut-through because no production federation
  format has been deployed. Change experimental federation members, clients,
  schemas, tests, and fixtures together instead of adding compatibility layers.
- Temporary instrumentation is allowed for investigation only when it cannot
  influence protocol behavior. Remove it before production unless it becomes
  documented, opt-in or low-overhead, bounded observability that exposes no
  secrets or sensitive payloads.

## Milestone working agreement

Work proceeds one milestone at a time in the order below unless a documented
dependency or newly confirmed critical issue requires reprioritization.

Every milestone follows the same evidence-first gate:

1. Reproduce or invalidate the reported behavior under a bounded, documented
   workload.
2. Add temporary measurement or tracing only where existing evidence is
   insufficient.
3. Identify the causal mechanism with hard process, journal, database, network,
   client, or code-path evidence.
4. Do not implement a fix while the milestone remains unconfirmed or the root
   cause is only speculative.
5. Once the cause is demonstrated, choose the smallest fix that fits the
   existing owning layer and reuses established CDK patterns.
6. Verify the fix against the minimal regression, relevant adverse cases,
   restart/replay behavior, and unchanged standalone behavior.
7. Remove temporary instrumentation unless it satisfies the production
   observability rules above.

After every milestone, all code, tests, verification results, and this document
must be updated together and committed to git with a conventional commit. The
document update must record the final status, root-cause evidence, chosen fix,
commands run, meaningful results, instrumentation removed or retained, and any
tests not run. A milestone is not complete until that commit exists.

If investigation or stress work exposes a previously unidentified networking,
consensus, mint, wallet, storage, or operational issue, do not fold an
unconfirmed fix into the active milestone. Append a new `U` milestone after all
existing numbered and `U` milestones at the end of the milestone execution
plan. Record which milestone exposed it, the raw evidence, why it might matter,
alternative explanations, and the stricter validation required. Its status
remains **Unconfirmed** and no fix may begin until independent reproduction
establishes that the issue is real.

### Status values

| Status | Meaning |
| --- | --- |
| **Pending** | Defined work has not started because earlier ordered milestones still own the active investigation. |
| **Unconfirmed** | Observation or hypothesis exists, but a bounded independent reproduction has not established a real defect. |
| **Confirmed historically; revalidation pending** | A prior revision reproduced the failure, but current code must reproduce it again before any fix is attempted. |
| **Confirmed; investigation pending** | The failure is reproducible, but the causal mechanism is not yet proven. |
| **Root cause confirmed; fix not started** | Evidence identifies the mechanism and owning layer; implementation may begin. |
| **Fix in progress** | A narrow fix is being implemented with its regression. |
| **Verification in progress** | The fix exists and the required adverse, restart, and compatibility checks are running. |
| **Complete** | Success criteria pass, temporary instrumentation is resolved, the document is current, and a milestone commit exists. |
| **Blocked** | A named prerequisite prevents useful progress; the blocking evidence and required external change are documented. |

Checked boxes record evidence or work already completed. Unchecked boxes are
requirements for milestone completion; they are not assumptions that the
proposed fix is correct.

### Milestone roadmap

| Order | Milestone | Current status | Detailed issues |
| ---: | --- | --- | --- |
| 1 | Fresh-member recovery, readiness, and live-session convergence | Fix in progress | 1, 2 |
| 2 | Cancellation-safe server application and bounded wallet completion | Confirmed; investigation pending | 3, 4, 5 |
| 3 | Exactly-once melt completion under repetition and member faults | Confirmed; investigation pending | 6 |
| 4 | AlephBFT cadence, network backpressure, and HTTP liveness | Confirmed; investigation pending | 8 |
| 5 | Signed private-status liveness and bounded operator reporting | Confirmed; investigation pending | 7, 12 |
| 6 | Federation discovery and import completion | Confirmed historically; revalidation pending | 9 |
| 7 | Wallet token-preparation and fragmentation scaling | Unconfirmed | 10 |
| 8 | Controlled local and remote capacity characterization | Blocked by Milestones 1-5 | 11 and performance baselines |
| 9 | Production release validation and operator readiness | Pending | Remaining release work |
| U1 | Validate payment-report quote-age semantics | Unconfirmed | 13 |
| U2 | Validate network-pump retry duplication | Unconfirmed | Secondary Issue 8 hypothesis |

## Executive summary

The current federation implementation performs well for normal local operation
and restart, but it still has a critical recovery defect. A five-member local
federation reached healthy state in 1.303 seconds, processed 100 parallel
receives at 25.09 committed swaps per second, restarted all five members at a
316-operation height in 2.958 seconds, and restarted one member with an intact
database in 1.673 seconds.

A member whose database was erased did not recover. After approximately 155
seconds it held only 1 of 316 accepted operations while reporting
`sync=healthy` and `ready=true`. Its logs showed an AlephBFT backup/collection
round mismatch followed by a new session that the other four members rejected
as the wrong session. This is both a catch-up failure and an incorrect readiness
claim. It is the highest-priority open issue in this report.

A focused local confirmation run reproduced both defects again from a smaller
35-operation history. The erased member recovered only through operation 10,
remained at 11 of 35 operations for more than 15 minutes, entered session 1
while the other four members remained in session 0, and continued to report
`sync=healthy ready=true`. A second load run exposed the complementary case:
four members rotated to session 1 while one stalled member remained in session
0. After restart the stalled member recovered the exact 144-entry journal, but
the strict ready/converged/certificate status command still returned success
with the live-session split present. Readiness therefore fails to prove both
federation currency and live-session compatibility.

The same local run also found two load-sensitive failures: nine of ten repeated
standard-operation workers returned `Payment failed` during their second melt,
while the tenth stalled at the same step; and one heavily used member stopped
answering signed private status requests even though its public information
route remained fast. A restart cleared the status failure.

The focused confirmation run reproduced the melt-confirmation stall and private
status failure independently. It also sharpened the wallet result: 43 parallel
one-second-deadline receives all committed and persisted one sat in their
wallets, but none of the 43 CLI processes exited within the two-minute bound
after printing `Received: 1`. Four members applied all 144 journal entries while
the unresponsive fifth member remained at 100 entries, with one accepted and
unapplied. This did not reproduce the earlier exact client-reported-failure
shape, but it confirmed an adjacent post-success completion hang and another
minority application divergence.

A fresh Nocean deployment exposed a separate consensus liveness and
backpressure failure. The first receive case, configured for only one concurrent
wallet, caused all five public HTTPS endpoints to become temporarily
unresponsive. Only 10 accepted journal operations produced approximately 744
AlephBFT DAG units. `UnitBroadcast` queues grew to roughly 80-160 messages per
member while receive requests timed out, then the public endpoints recovered
without a restart after the queues drained. Mint CPU remained far below host
saturation. Code tracing confirmed that the fixed 100 ms active schedule
overrides AlephBFT's normal 500 ms steady delay and that the data provider
permits empty units while payload activity remains pending. This amplifies a
small workload into enough consensus traffic to starve operation and HTTP work.

Earlier Nocean testing found wallet fanout, retry, and ambiguous-result problems
under member catch-up pressure. Those findings remain relevant even though the
new local receive-only result is much faster. The local and remote throughput
numbers are not directly comparable because the network, host capacity,
software revisions, and workload shapes differ.

## Milestone execution plan

### Milestone 1: Fresh-member recovery, readiness, and live-session convergence

**Status:** Fix in progress

**Covers:** Issues 1 and 2

**Objective:** Make recovery from an empty or stale database deterministic, and
make readiness prove that a member is current and compatible with the live
federation session rather than merely locally consistent.

#### Existing evidence

- [x] An erased member recovered only 1 of 316 accepted operations and still
  reported `sync=healthy` and `ready=true` after approximately 155 seconds.
- [x] A smaller independent run recovered only 11 of 35 operations, then
  remained stalled for more than 15 minutes.
- [x] The erased member entered session 1 while the other four members remained
  in session 0 and rejected its traffic as the wrong session.
- [x] The complementary split was observed under load: four members entered
  session 1 while one remained in session 0, yet strict ready, convergence, and
  certificate checks succeeded because heights and digests matched.
- [x] An intact single-member restart and a full-federation restart recovered
  quickly, separating the empty/stale recovery problem from ordinary replay.

#### Investigation and evidence gate

- [x] Build a minimal, deterministic process test that separately covers an
  erased database at startup and session rotation under load.
- [x] Record each startup state transition, local journal boundary, checkpoint
  trust classification, finalized and backup boundaries, active session, peer
  session evidence, and readiness reason.
- [x] Trace range requests and responses, peer selection, retry classification,
  and the decision made by `should_defer_catch_up_to_live_aleph_bft`.
- [x] Prove where and why catch-up stops: missing range availability, incorrect
  trusted-checkpoint classification, premature deferral to live consensus,
  backup/finalized boundary handling, session selection, or another measured
  cause.
- [x] Prove the exact inputs that allow readiness and strict status to pass
  during a session split.
- [x] Confirm that the cause holds across at least two journal heights and is
  not dependent on a single timing window.
- [x] Record the root cause and owning layer here before changing production
  behavior.

#### Root-cause evidence

A bounded current-revision process reproduction started five members, committed
one standard batch, and converged all members at seven operations in AlephBFT
session 0. Member 5 was stopped, its work directory was preserved outside the
active path, and it was restarted with an empty work directory. Within ten
seconds it reported zero operations, no finalized items, no catch-up certificate
evidence, `session=1`, `sync=healthy`, and `ready=true`. Those values remained
unchanged in every five-second sample for 90 seconds while the other four
members remained at seven operations in session 0. Both sides repeatedly
rejected AlephBFT traffic with `expected=0, found=1` or `expected=1, found=0`.
The same causal shape was previously reproduced at histories of 35 and 316
operations, so it is not a large-journal or single-timing-window effect.

The code path explains every observed transition:

1. Router construction creates an empty consensus runtime at session 0 and
   starts the live AlephBFT task before startup readiness revalidation or the
   peer-sync loop.
2. `restore_startup_readiness` treats an empty journal as trusted and changes
   the startup halt directly to `Healthy` without any peer comparison,
   certificate evidence, finalized-item history, or session evidence.
3. The fresh session lacks the persisted backup and finalized prefix needed to
   rejoin peers already advanced in session 0. It terminates and rotates the
   empty local runtime to session 1.
4. Each peer-status response proves the local journal is behind, but
   `should_defer_catch_up_to_live_aleph_bft` returns true solely because local
   sync is `Healthy` and the local live-session and inbound handles exist. This
   return occurs before the code marks the member lagging or sends a journal
   range request, so recovery is intentionally skipped on every round.
5. `sync_ready` is derived only from the local sync enum. Operator
   `--require-ready` checks only that boolean, convergence checks only height and
   state digests, and zero-operation certificate checks require no evidence.
   No guard compares the active AlephBFT session, so an equal-height session
   split can pass every existing strict guard.

The owning layers are the AlephBFT/router startup ordering, peer-sync readiness
transition and deferral policy, peer status evidence, and the operator status
guards. The evidence supports one coherent correction: an untrusted startup
checkpoint must remain non-ready and must not start live consensus until peer
sync establishes and recovers a quorum-compatible journal/session view; strict
status must reject session disagreement. The temporary reproduction wrapper is
git-ignored and must be removed after the permanent regression passes.

The first implementation pass removed premature startup and readiness and was
then re-run as a five-member process test. Member 5 stayed non-ready at zero
operations, recovered the exact seven-operation journal, finalized cursor,
order and materialized-state digests, and quorum catch-up certificate, and
briefly became healthy in session 0. It then moved alone to session 1 and
correctly returned to `awaiting_quorum`; strict convergence failed for the
remaining 75-second observation window. The member log supplies the missing
causal boundary: upstream AlephBFT inferred collection round 62 from the peers
but backup round 0 locally, reported `Backup state behind unit collection
state`, stopped its creator, and returned from `run_session`. CDK classified
that indistinguishably returned task as normal session completion and rotated
unilaterally. Therefore journal recovery and fail-closed readiness are working,
but an erased member cannot resume a progressed session until its signed DAG
backup is recovered or the quorum moves to a new session. Any accepted fix must
address that continuity boundary; suppressing the readiness error or merely
delaying unilateral rotation is insufficient.

#### Implementation requirements

- [x] Do not start implementation until the investigation gate identifies the
  causal mechanism with hard evidence.
- [ ] Apply the smallest fix in the existing startup, catch-up, status, or
  consensus-session path; do not add a parallel recovery system.
- [ ] Ensure an untrusted or empty local checkpoint recovers authoritative
  history before joining or advancing the live session.
- [ ] Derive readiness from recent signed quorum evidence, including compatible
  journal height and digests, certificates, roster/configuration identity,
  session identity, and bounded evidence age.
- [ ] Make strict status fail when a member is locally consistent but cannot
  participate in the current quorum session.
- [ ] Preserve fail-closed behavior for roster, transcript, keyset,
  configuration, and journal disagreement.
- [ ] Keep standalone mint startup and readiness behavior unchanged.

#### Success criteria

- [ ] A member starting with an empty database converges exactly to the quorum
  journal and materialized state without manual intervention.
- [ ] A stale member catches up across the checkpoint and live-consensus
  boundary and joins the same session as the quorum.
- [ ] A lagging, stale, or session-incompatible member never reports ready or
  passes strict status.
- [ ] Intact single-member and full-federation restarts retain their fast replay
  path and do not redownload authoritative history unnecessarily.
- [ ] Recovery never creates conflicting accepted operations, digests,
  certificates, or live sessions.

#### Required tests

- [ ] Unit tests for startup state transitions, checkpoint trust decisions,
  catch-up deferral, readiness evidence age, and session compatibility.
- [ ] Five-member process regression with an erased member after approximately
  35 committed operations.
- [ ] Height regressions at approximately 1, 100, 1,000, and 10,000 operations,
  with bounds scaled and recorded rather than hidden by a single long timeout.
- [ ] Equal-height/equal-digest but mismatched-session regression proving strict
  status fails.
- [ ] Intact single-member restart, full-federation restart, interrupted
  catch-up/restart, and two-member-behind cases.
- [ ] Focused federation tests plus the relevant standalone startup and mint
  tests.

#### Closeout checklist

- [ ] Remove all temporary recovery and readiness instrumentation, or document
  why retained metrics are bounded and safe for production.
- [ ] Run formatting, focused unit/integration tests, process regressions, and
  standalone compatibility tests; record commands and results here.
- [ ] Update the status, root cause, chosen fix, results, and any tests not run.
- [ ] Verify that logs, fixtures, and committed artifacts contain no secrets,
  private federation material, or machine-specific information.
- [ ] Commit the milestone with a conventional commit.

### Milestone 2: Cancellation-safe server application and bounded wallet completion

**Status:** Confirmed; investigation pending

**Covers:** Issues 3, 4, and 5

**Depends on:** Milestone 1 for final catch-up fault classification

**Objective:** Once an operation is accepted, make its server-side completion
independent of request cancellation, and make wallet quorum operations return a
bounded, semantically correct definitive or ambiguous result.

#### Existing evidence

- [x] Earlier Nocean testing recorded 25 client failures whose operations still
  committed and whose valid shares existed on all members.
- [x] In the focused local run, 43 of 43 wallets printed `Received: 1` and stored
  one sat, but none of their CLI processes exited within two minutes.
- [x] Four members applied all 144 operations while the fifth stopped at 100,
  with operation 100 accepted but not applied; restart replay completed it.
- [x] Direct receive overlap against an intact member completed 20 of 20 cases,
  showing that failure depends on load, peer state, or post-success cleanup.
- [x] The exact earlier failure-after-commit client result was not reproduced in
  the focused local run; the adjacent post-success completion hang was.

#### Investigation and evidence gate

- [ ] Create separate minimal regressions for failure-after-commit,
  success-before-process-exit, and accepted-but-unapplied minority state.
- [ ] Instrument wallet lifecycle phases: queried operation identity, member
  fanout, view agreement, threshold decision, saga/database commit, returned
  output, surplus future drain/drop, runtime shutdown, and process exit.
- [ ] Instrument the server lifecycle: accepted, applying, applied, result
  cached, waiter attached/dropped, handler cancelled, and replayed.
- [ ] Determine whether the client hang is caused by surplus requests, task or
  connection shutdown, database ownership, retry loops, or another measured
  resource.
- [ ] Determine whether accepted-but-unapplied state is caused by request
  cancellation, an application worker failure, session divergence, lock
  starvation, or another measured mechanism.
- [ ] Separate retryable catch-up/session conditions from definitive transport,
  validation, and threshold rejection using typed evidence.
- [ ] Confirm deterministic operation identity and replay behavior when the
  original client disappears at every lifecycle boundary.
- [ ] Record the root causes and owning client/server layers here before fixing
  either path.

#### Implementation requirements

- [ ] Do not implement a fix until the corresponding client or server causal
  mechanism has hard evidence.
- [ ] Keep accepted operation completion durable and independent of the HTTP
  request lifetime, using existing journal/application worker patterns.
- [ ] Make application idempotent and ensure restart resumes any accepted but
  non-terminal operation without repeating externally visible side effects.
- [ ] Let any valid roster member contribute while distinguishing retryable
  lag/session conditions from member rejection.
- [ ] Return as soon as threshold-valid agreement is sufficient, while safely
  cancelling, detaching, or boundedly draining surplus work.
- [ ] Preserve the wallet saga until the outcome is definitive; return a typed
  pending or ambiguous result when a timeout cannot prove failure.
- [ ] Keep deterministic operation IDs stable across retries and avoid new
  per-flow fanout implementations.
- [ ] Keep standalone wallet and mint behavior unchanged.

#### Success criteria

- [ ] A successful wallet operation returns and its process exits within the
  documented bound once a valid threshold result exists.
- [ ] Client cancellation at any point after acceptance cannot strand an
  operation indefinitely in `accepted` or corrupt materialized state.
- [ ] Every accepted operation reaches one durable terminal outcome on every
  recovering member through live application or exact replay.
- [ ] A client never reports ordinary definitive failure when quorum evidence
  only supports a pending or ambiguous outcome.
- [ ] Retry with the same operation identity yields the original result and no
  duplicate issuance, spending, or payment side effect.
- [ ] One or two slow, catching-up, or unresponsive members do not block a valid
  three-of-five result.

#### Required tests

- [ ] Wallet connector tests with one and two slow, catching-up, wrong-session,
  malformed, rejecting, and unresponsive members.
- [ ] Threshold-valid success test that proves surplus member work cannot delay
  return or process shutdown.
- [ ] Forty-three-or-greater parallel receive regression with wallet balances,
  process exits, journal convergence, and application state all asserted.
- [ ] Client cancellation at accepted, applying, applied-before-response, and
  response-before-cleanup boundaries.
- [ ] Server restart with accepted-but-unapplied work and repeated operation-ID
  recovery.
- [ ] Definitive threshold rejection, timeout-before-threshold, and typed
  ambiguous-result tests.
- [ ] Focused federation wallet/mint tests plus equivalent standalone wallet
  and mint tests.

#### Closeout checklist

- [ ] Remove temporary lifecycle instrumentation or document retained bounded
  metrics and their privacy properties.
- [ ] Run formatting, focused tests, process regressions, restart/replay tests,
  and standalone compatibility tests; record commands and results here.
- [ ] Update the status, separate root causes, chosen fixes, evidence, and any
  tests not run.
- [ ] Verify committed logs and fixtures contain no tokens, proofs, private
  federation material, or machine-specific information.
- [ ] Commit the milestone with a conventional commit.

### Milestone 3: Exactly-once melt completion under repetition and member faults

**Status:** Confirmed; investigation pending

**Covers:** Issue 6

**Depends on:** Milestone 2 wallet completion semantics

**Objective:** Make repeated melt operations reach one threshold-consistent,
exactly-once terminal result without hangs when members are slow or offline.

#### Existing evidence

- [x] Nine of ten concurrent standard-operation workers returned
  `Payment failed` during their second melt while the tenth stalled there.
- [x] A focused sequential run with one member offline completed the first melt
  and stalled during the second melt-confirmation path for more than 120
  seconds.
- [x] The repeated second-operation shape was reproduced independently of the
  larger mixed workload.

#### Investigation and evidence gate

- [ ] Reproduce with one wallet and two sequential melts before adding
  concurrency, then add one offline member and concurrent wallets separately.
- [ ] Trace canonical operation ID, quote ID, elected payer, backend payment ID,
  quote state, member observations, consensus entries, saga state, polling, and
  lock acquisition/hold times.
- [ ] Establish whether the stall is in wallet result handling, payer election,
  payment observation, mint quote transition, fake-wallet behavior, saga
  polling, or another measured path.
- [ ] Prove whether all members agree on the same backend payment observation
  and whether any repeated poll or replay attempts the external side effect.
- [ ] Revalidate the causal mechanism with a real Lightning backend before
  generalizing a fake-wallet-only observation.
- [ ] Record the root cause and owning layer here before changing melt logic.

#### Implementation requirements

- [ ] Do not implement a fix until the investigation gate proves the causal
  mechanism.
- [ ] Reuse the existing payment observation, payer-election, saga, journal, and
  quote-state patterns; do not add a second melt coordinator.
- [ ] Preserve exactly-once external payment semantics and quorum authorization.
- [ ] Keep replay deterministic: replay consumes recorded payment evidence and
  never calls the Lightning backend again.
- [ ] Return a typed pending or ambiguous outcome when payment finality cannot
  be established, rather than a false definitive failure.
- [ ] Keep standalone melt behavior unchanged.

#### Success criteria

- [ ] Two and many consecutive melts complete within a documented bound with
  zero, one, or two unavailable members when a quorum remains.
- [ ] Every melt has one payer decision, at most one external payment, one
  threshold-consistent terminal quote state, and exact replay.
- [ ] Failure, timeout, and already-paid backend responses produce the correct
  retryable, ambiguous, or definitive wallet result.
- [ ] Restart during observation or after payment cannot duplicate payment or
  lose the terminal result.

#### Required tests

- [ ] Minimal two-sequential-melt process regression with one member offline.
- [ ] Ten-or-greater concurrent wallets performing repeated mint/swap/melt
  sequences.
- [ ] Payer unavailable before payment, payer loss after payment, backend
  pending/failure/already-paid, client cancellation, and restart/replay tests.
- [ ] Fake-wallet regression plus at least one real Lightning backend fixture.
- [ ] Focused federation payment tests and unchanged standalone melt tests.

#### Closeout checklist

- [ ] Remove temporary payment instrumentation or document retained bounded,
  non-sensitive metrics.
- [ ] Run formatting, focused tests, process regressions, restart/replay tests,
  real-backend validation, and standalone tests; record results here.
- [ ] Update the status, root cause, chosen fix, evidence, and any tests not run.
- [ ] Verify artifacts contain no invoices, preimages, credentials, private
  federation material, or machine-specific information.
- [ ] Commit the milestone with a conventional commit.

### Milestone 4: AlephBFT cadence, network backpressure, and HTTP liveness

**Status:** Confirmed; investigation pending

**Covers:** Issue 8

**Objective:** Prevent a small application workload from creating excessive
empty consensus traffic, queue growth, and public/private HTTP starvation while
retaining prompt consensus for real work.

#### Existing evidence

- [x] A fresh Nocean deployment became temporarily unresponsive during the
  first one-wallet receive workload and recovered after queues drained without
  a restart.
- [x] Ten accepted journal operations produced approximately 744 DAG units.
- [x] `UnitBroadcast` queues grew to roughly 80-160 messages per member while
  requests timed out.
- [x] Mint CPU remained far below host saturation, arguing against simple host
  CPU exhaustion.
- [x] Code tracing confirmed the fixed 100 ms active schedule replaces the
  upstream 500 ms steady delay and that empty units remain possible while
  payload activity is pending.
- [x] A separate possible network-pump retry amplifier remains unconfirmed and
  is tracked independently as Milestone U2.

#### Investigation and evidence gate

- [ ] Reproduce with a minimal sequential workload and capture accepted
  operations, payload-bearing units, empty units, active/idle cadence, queue
  depths, event-loop delay, and HTTP latency on the same timeline.
- [ ] Define and measure the lifetime of the activity signal that selects the
  faster schedule, including stale or perpetually pending activity.
- [ ] Trace data-provider readiness, unit proposal, network send/receive, and
  application worker scheduling without logging payloads or private material.
- [ ] Independently compare the current 100 ms override, the upstream schedule,
  and any bounded activity-sensitive schedule under identical workloads.
- [ ] Prove which measured mechanism causes HTTP starvation: proposal cadence,
  broadcast volume, an unbounded queue, runtime scheduling, lock contention, or
  another cause.
- [ ] Keep the U2 retry hypothesis isolated unless its own independent evidence
  gate is satisfied.
- [ ] Record the complete causal chain and owning layer here before fixing it.

#### Implementation requirements

- [ ] Do not implement a cadence or queue change until the measured causal chain
  is complete.
- [ ] Use the smallest existing AlephBFT configuration, activity-hint, bounded
  channel, or backpressure pattern that addresses the proven cause.
- [ ] Make activity expiry explicit and bounded if activity-sensitive cadence is
  retained.
- [ ] Bound network queues and preserve protocol-required messages without
  creating retry storms or silently dropping correctness-critical traffic.
- [ ] Do not treat larger HTTP/client timeouts as the fix.
- [ ] Preserve standalone behavior and avoid forking upstream consensus logic
  unless no supported configuration solves the demonstrated issue.

#### Success criteria

- [ ] Empty-unit production and network queue depth remain within documented
  bounds at idle and for a ten-operation burst.
- [ ] Public information, wallet operation, and signed private-status latency
  remain bounded while consensus traffic is active.
- [ ] The federation returns to steady idle cadence after a bounded grace period.
- [ ] Payload commit latency and throughput do not regress beyond an explicitly
  accepted, measured tradeoff.
- [ ] One slow or temporarily disconnected member cannot create unbounded
  broadcast growth on healthy members.

#### Required tests

- [ ] Unit tests for cadence selection, activity expiry, data-provider hints,
  queue bounds, and backpressure behavior.
- [ ] Five-member process test with one sequential wallet and a ten-operation
  receive workload.
- [ ] Idle, burst, sustained, one-slow-peer, disconnect/reconnect, and queue-full
  regressions with HTTP latency sampled concurrently.
- [ ] Controlled comparison against the upstream schedule using identical
  software, topology, and workload.
- [ ] Focused consensus/network tests and relevant standalone HTTP tests.

#### Closeout checklist

- [ ] Remove high-volume tracing and experimental cadence switches, or document
  retained bounded metrics and stable configuration.
- [ ] Run formatting, focused tests, local process regressions, and the Nocean
  validation workload; record commands, distributions, and any tests not run.
- [ ] Update the status, root cause, chosen fix, performance tradeoff, and
  before/after measurements.
- [ ] Verify artifacts contain no network addresses, credentials, private
  federation material, payloads, or machine-specific information.
- [ ] Commit the milestone with a conventional commit.

### Milestone 5: Signed private-status liveness and bounded operator reporting

**Status:** Confirmed; investigation pending

**Covers:** Issues 7 and 12

**Depends on:** Milestone 1 readiness semantics; coordinate measurements with
Milestone 4

**Objective:** Keep authenticated diagnostic status responsive and accurate
under load, catch-up, restart, and session divergence without weakening access
control or masking application liveness failures.

#### Existing evidence

- [x] Signed private-status requests hung on heavily used members while the same
  members' public `/v1/info` routes returned HTTP 200 quickly.
- [x] Peer logs recorded private-status timeouts and exponential backoff.
- [x] Restart cleared the status failure.
- [x] One affected member was also behind at 100 of 144 entries and in a
  different live session, so correlation with application/consensus lag remains
  possible but unproven.

#### Investigation and evidence gate

- [ ] Reproduce status failure independently during steady load, catch-up,
  session rotation, and after client-side timeout/cancellation.
- [ ] Measure admission permit acquisition/release, task lifetime, lock wait and
  hold times, report construction, serialization, response size, cache access,
  event-loop delay, and cancellation cleanup.
- [ ] Identify whether permits, locks, report fanout, large detail generation,
  runtime starvation, or another measured mechanism blocks the route.
- [ ] Determine whether status failure causes application lag, results from it,
  or shares a third cause.
- [ ] Audit status fields against Milestone 1 readiness evidence so operator
  output distinguishes local consistency, quorum currency, and live-session
  participation.
- [ ] Record the root cause and owning handler/reporting layer here before a fix.

#### Implementation requirements

- [ ] Do not implement a fix until the blocking mechanism is demonstrated.
- [ ] Apply the smallest change to existing admission, locking, caching,
  snapshot, or report-detail patterns.
- [ ] Avoid holding application or consensus locks during expensive report
  construction, signing, serialization, or network I/O.
- [ ] Keep permits cancellation-safe and bounded; do not replace the problem
  with an unbounded task or queue.
- [ ] Prefer a small summary by default and explicit bounded detail retrieval if
  report size is a proven contributor.
- [ ] Preserve signed access control and ensure no secrets or sensitive payloads
  enter status output, logs, or metrics.
- [ ] Do not hide the failure with a larger request timeout.

#### Success criteria

- [ ] Signed status returns within a documented bound during wallet load,
  catch-up, restart, session rotation, and repeated client cancellation.
- [ ] Timed-out clients cannot leak permits, locks, tasks, or cached builders.
- [ ] Status accurately reports lag and session incompatibility and agrees with
  strict readiness semantics.
- [ ] Public wallet routes and consensus/application work remain responsive
  while operators poll status at the supported rate.
- [ ] Authentication and information disclosure behavior remain unchanged.

#### Required tests

- [ ] Continuous signed-status polling during receive/swap/melt load,
  fresh-member recovery, intact restart, and session rotation.
- [ ] Repeated timeout/cancellation and maximum-concurrency admission tests that
  assert every permit and task is released.
- [ ] Summary/detail size and serialization-bound tests with a large journal.
- [ ] Cache invalidation and snapshot consistency tests across applied entries,
  catch-up, and session change.
- [ ] Focused authenticated route tests and unchanged public/standalone route
  tests.

#### Closeout checklist

- [ ] Remove temporary lock/permit/task tracing or document retained bounded
  observability.
- [ ] Run formatting, focused tests, process stress regressions, and standalone
  route tests; record commands, latency distributions, and tests not run.
- [ ] Update the status, root cause, chosen fix, results, and operator-visible
  semantics.
- [ ] Audit committed examples and logs for credentials, request signatures,
  network identity, private federation material, or machine-specific data.
- [ ] Commit the milestone with a conventional commit.

### Milestone 6: Federation discovery and import completion

**Status:** Confirmed historically; revalidation pending

**Covers:** Issue 9

**Depends on:** Milestones 1 and 2 for current readiness and connector semantics

**Objective:** Make URL discovery and federation import complete within a bound
and behave consistently with importing the same validated configuration from a
file.

#### Existing evidence

- [x] Earlier testing fetched the discovery configuration successfully with
  HTTP 200.
- [x] Adding the federation by discovery URL hung afterward.
- [x] Importing the equivalent generated configuration file succeeded.
- [ ] The failure has not yet been revalidated against the current code and
  local process harness.

#### Investigation and evidence gate

- [ ] Re-run URL and file import against the same fresh local federation and
  record whether the historical defect still exists.
- [ ] Timestamp discovery fetch, decoding, roster/config validation, metadata
  and keyset fetches, connector fanout, wallet registry persistence, task
  cleanup, and CLI process exit.
- [ ] Compare the normalized URL-derived and file-derived configurations before
  any network follow-up.
- [ ] Test one slow, unreachable, wrong-session, and inconsistent member to
  determine whether fanout cancellation or agreement is the blocking phase.
- [ ] If current code does not reproduce the issue after repeated bounded runs,
  close this milestone as invalidated with evidence and do not make a fix.
- [ ] If reproduced, record the root cause and owning discovery/import or
  connector layer here before implementation.

#### Implementation requirements

- [ ] Do not implement a fix unless current bounded reproduction confirms the
  defect and the causal mechanism.
- [ ] Reuse the centralized federation connector for follow-up agreement and
  member error classification.
- [ ] Make cancellation and cleanup bounded after enough validated federation
  evidence exists.
- [ ] Preserve strict roster, keyset, transcript, configuration, and endpoint
  validation; faster import must not accept partial or conflicting identity.
- [ ] Keep equivalent validated URL and file imports semantically identical.

#### Success criteria

- [ ] Repeated URL discovery/import completes and the CLI exits within the
  documented bound for a healthy local federation.
- [ ] URL and file import produce the same normalized federation identity and
  usable wallet configuration.
- [ ] A minority slow or unreachable member does not block valid threshold
  import, while conflicting federation identity fails closed.
- [ ] Repeated import is idempotent and cannot leave a partial wallet registry
  entry.

#### Required tests

- [ ] Healthy local URL-versus-file equivalence process test.
- [ ] Slow, unreachable, malformed, identity-mismatched, and insufficient-quorum
  member cases.
- [ ] Cancellation during each import phase, retry with the same identity, and
  wallet restart after interrupted persistence.
- [ ] Focused discovery/connector tests and standalone single-mint import tests.

#### Closeout checklist

- [ ] Remove temporary phase instrumentation or document retained bounded
  import telemetry.
- [ ] Run formatting, focused tests, local process regressions, and standalone
  import tests; record results and tests not run.
- [ ] Update the status with either invalidation evidence or the confirmed root
  cause, fix, and verification.
- [ ] Verify committed discovery examples contain no private endpoints,
  credentials, federation secrets, or machine-specific information.
- [ ] Commit the milestone with a conventional commit, including when the
  evidence closes the issue without a production code change.

### Milestone 7: Wallet token-preparation and fragmentation scaling

**Status:** Unconfirmed

**Covers:** Issue 10

**Objective:** Determine whether large wallet proof sets cause a real local
token-preparation or storage bottleneck, distinct from federation consensus and
network latency, and fix only the measured owning path.

#### Existing evidence

- [x] Token preparation slowed in a 240-payment workflow while only 84
  operations entered federation consensus.
- [x] Proof fragmentation, CLI startup, SQLite access, and coin selection are
  plausible contributors, but none is yet established as the cause.
- [ ] A controlled proof-count scaling curve has not yet confirmed a defect.

#### Investigation and evidence gate

- [ ] Generate controlled wallets with increasing proof counts and equivalent
  total value, denomination distribution, and spent/pending state.
- [ ] Measure CLI/runtime startup, wallet database reads and writes, proof-state
  reconciliation, coin selection, token serialization, and online member calls
  separately.
- [ ] Compare offline send/token preparation with online federation operations
  to exclude consensus and networking from the local path.
- [ ] Measure query plans and row counts for any suspected SQLite operation and
  compare another supported wallet backend where practical.
- [ ] Repeat across cold/warm caches and independent wallets; report median and
  tail latency rather than a single run.
- [ ] If latency remains bounded or belongs to the benchmark harness, close the
  milestone without a production fix.
- [ ] If confirmed, record the causal algorithm/query and owning wallet/storage
  layer here before implementation.

#### Implementation requirements

- [ ] Do not implement speculative proof compaction, caching, indexing, or coin
  selection changes before hard evidence identifies the bottleneck.
- [ ] Use the smallest existing wallet database, proof-selection, batching, or
  query pattern that fixes the demonstrated cause.
- [ ] Preserve proof-state correctness, deterministic selection constraints,
  fee handling, privacy behavior, and restart safety.
- [ ] Avoid federation-specific wallet logic if the measured issue is a general
  standalone wallet concern.

#### Success criteria

- [ ] Token-preparation latency follows a documented acceptable curve across the
  tested proof-count range and improves materially at the confirmed bottleneck.
- [ ] Online and offline behavior show no correctness, privacy, fee, or proof
  state regression.
- [ ] Database growth and memory use remain bounded and interrupted preparation
  is restart-safe.

#### Required tests

- [ ] Benchmarks for small, medium, and large fragmented proof sets with fixed
  value and repeated independent samples.
- [ ] Offline send, online send, receive, swap, and restart cases.
- [ ] SQLite and at least one other supported wallet backend when the fix is in
  shared storage/query logic.
- [ ] Focused wallet tests and equivalent standalone behavior tests.

#### Closeout checklist

- [ ] Remove query/profiling instrumentation and uncommitted benchmark wallets,
  or document retained bounded metrics.
- [ ] Record benchmark construction, distributions, root cause or invalidation,
  chosen fix if any, commands, and tests not run.
- [ ] Verify fixtures contain no real proofs, tokens, seeds, or machine-specific
  information.
- [ ] Commit the milestone with a conventional commit, including an
  evidence-only close if no defect is confirmed.

### Milestone 8: Controlled local and remote capacity characterization

**Status:** Blocked by Milestones 1-5

**Covers:** Issue 11 and the local/Nocean performance baselines

**Objective:** Establish repeatable throughput, latency, convergence, resource,
and saturation limits only after known correctness and liveness failures can no
longer invalidate benchmark results.

#### Prerequisite gate

- [ ] Milestone 1 recovery/readiness success criteria are complete.
- [ ] Milestone 2 operation completion/application success criteria are
  complete.
- [ ] Milestone 3 melt success criteria are complete for mixed workloads.
- [ ] Milestone 4 consensus/network/HTTP liveness success criteria are complete.
- [ ] Milestone 5 status observability is reliable enough to measure the run.
- [ ] Benchmark revisions, topology, member resources, network conditions,
  database state, and workload composition are pinned and documented without
  committing private deployment information.

#### Investigation and measurement checklist

- [ ] Define separate receive-only, swap-only, melt-only, mixed sequential, and
  mixed concurrent workloads with deterministic setup and teardown.
- [ ] Measure client completion, consensus acceptance, application completion,
  member convergence, and CLI process exit as separate latency boundaries.
- [ ] Collect CPU, resident memory, disk I/O, database latency/size, event-loop
  delay, consensus units, payload density, queue depth, session changes, and
  status latency.
- [ ] Run at least five clean repetitions per concurrency point and report
  median, tail, spread, error/ambiguous counts, and journal skew.
- [ ] Increase concurrency until a stable saturation boundary is identified;
  stop a run when correctness or safety criteria fail rather than treating bad
  operations as throughput.
- [ ] Compare local and remote results only when software, configuration,
  workload, and measurement boundaries are equivalent; otherwise report them as
  separate environments.
- [ ] Attribute the first constrained resource or queue with evidence.
- [ ] Append any newly observed defect as an unconfirmed `U` milestone with its
  provenance instead of fixing it inside the benchmark milestone.

#### Implementation requirements

- [ ] Treat this primarily as characterization, not permission for speculative
  optimization.
- [ ] For every confirmed new bottleneck that requires production changes,
  create a separate evidence-first milestone and complete it before publishing
  a new capacity claim.
- [ ] Keep benchmark-only controls isolated from production behavior and remove
  temporary profiling instrumentation.

#### Success criteria

- [ ] Local and remote capacity tables are repeatable, include uncertainty, and
  use clearly defined latency and throughput boundaries.
- [ ] Every successful run ends with exact journal, digest, certificate,
  materialized-state, and live-session convergence.
- [ ] Saturation is associated with a measured resource or bounded queue rather
  than unexplained failures, hangs, or readiness claims.
- [ ] Results distinguish protocol cost, client cost, payment-backend cost, and
  deployment/network cost.

#### Required tests

- [ ] Concurrency ladder for every workload shape, including `N=10:1` and the
  existing Nocean shapes where comparable.
- [ ] One and two member slow/offline cases below the quorum fault limit.
- [ ] Restart and recovery immediately after a saturated run.
- [ ] Sustained-duration run long enough to expose queue, storage, and memory
  growth.
- [ ] Baseline standalone run for costs shared with non-federated CDK.

#### Closeout checklist

- [ ] Remove temporary profilers, high-volume metrics, generated wallets, and
  private deployment artifacts.
- [ ] Record exact sanitized methodology, distributions, saturation point,
  limitations, commands, and tests not run.
- [ ] Update every affected claim in this document and link any new `U`
  milestones.
- [ ] Commit the benchmark harness/test/document update with a conventional
  commit.

### Milestone 9: Production release validation and operator readiness

**Status:** Pending

**Covers:** Remaining release work from the implementation plan

**Depends on:** Milestones 1-8 and every confirmed `U` milestone

**Objective:** Demonstrate that the federation is safe, interoperable,
recoverable, operable, and understandable in production-like conditions before
any production-readiness claim.

#### Readiness investigation

- [ ] Inventory every safety, liveness, recovery, payment, performance, and
  operational claim in the implementation plan and map it to current evidence.
- [ ] Identify missing independent review, real-backend, multi-host, upgrade,
  partition, sustained-load, backup, and incident-response evidence.
- [ ] Confirm all experimental federation formats, clients, fixtures, and nodes
  move together under the cut-through compatibility policy.
- [ ] Treat every newly observed defect as a new unconfirmed `U` milestone with
  provenance and independent reproduction; do not patch it ad hoc during the
  release exercise.

#### Completion requirements

- [ ] Complete independent protocol and security review of operation identity,
  replay, BLS aggregation, payment authorization, recovery, status trust, DKG,
  rotation, and secret handling.
- [ ] Validate mint, swap, and melt flows against supported real payment
  backends, including pending, failed, already-paid, and restart cases.
- [ ] Run multi-host deployment, rolling restart, full restart, interrupted
  upgrade, fresh-member recovery, keyset rotation, and supported configuration
  change drills.
- [ ] Run sustained load, network delay/loss, minority partition, member loss,
  disk pressure, and database interruption tests within the documented fault
  model.
- [ ] Verify backup and restore procedures for member-local durable state and
  authoritative federation recovery.
- [ ] Complete operator documentation for DKG, federation bootstrap, secure
  configuration distribution, readiness/status interpretation, recovery,
  rotation, backup, monitoring, alerting, and incident response.
- [ ] Document remaining limits, unsupported operations, experimental format
  policy, and safe rollback/abort points.

#### Success criteria

- [ ] Independent review has no unresolved critical or high-severity findings.
- [ ] All required real-backend, multi-host, recovery, upgrade, fault, and
  sustained-load tests pass with exact federation convergence.
- [ ] Operators can perform bootstrap, recovery, rotation, backup/restore, and
  incident drills from the documentation without developer-only knowledge.
- [ ] Monitoring distinguishes local health, quorum currency, live-session
  compatibility, application backlog, payment ambiguity, and resource pressure.
- [ ] No known confirmed correctness, liveness, security, or data-loss issue is
  deferred behind a production-readiness claim.

#### Required tests and release evidence

- [ ] Sanitized test matrix with versions, topology classes, workloads, fault
  injections, commands, results, and explicitly omitted tests.
- [ ] Independent review record and resolution mapping.
- [ ] Real-backend exactly-once payment evidence.
- [ ] Multi-host recovery/upgrade/partition evidence.
- [ ] Capacity and sustained-resource report from Milestone 8.
- [ ] Final standalone compatibility suite and public Cashu protocol tests.

#### Closeout checklist

- [ ] Remove all temporary release instrumentation, credentials, generated
  private configurations, test seeds, invoices, proofs, and deployment records.
- [ ] Update this document and the implementation plan with final status,
  limitations, operator references, evidence, and tests not run.
- [ ] Run formatting and the agreed complete release test suite; record the
  sanitized results.
- [ ] Commit the release-validation milestone with a conventional commit.

### Milestone U1: Validate payment-report quote-age semantics

**Status:** Unconfirmed

**Found during:** Focused local confirmation work associated with Milestones 1,
2, and 5

**Related issue:** Issue 13

**Observation:** Fake-wallet payment reports showed
`quote_age_secs` near 1.784 billion while `stale=false`.

#### Investigation and evidence gate

- [ ] Reproduce with a minimal fake-wallet quote and record the stored timestamp,
  report calculation inputs, units, and missing-value behavior.
- [ ] Repeat with at least one real payment backend and both fresh and
  intentionally stale quotes.
- [ ] Determine whether this is a display-only calculation, missing test
  metadata, clock/epoch mismatch, persistence defect, or incorrect staleness
  decision.
- [ ] Verify that no correctness or payer-election decision consumes the
  misleading age before classifying severity.
- [ ] Require an independent reproduction outside the original stress harness.
- [ ] Do not change production behavior until the cause and impact are proven.

#### Implementation requirements

- [ ] If confirmed, apply the smallest fix in the existing timestamp/reporting
  path and define explicit unknown-age semantics rather than inventing an age.
- [ ] If invalidated as fake-wallet fixture metadata, fix only the fixture/test
  expectation or close with evidence; do not alter production semantics.

#### Success criteria

- [ ] Fresh, stale, and unknown quotes report consistent age/staleness across
  fake and real backends and across restart.
- [ ] No correctness or operator decision interprets an unknown creation time as
  a precise epoch-sized quote age.

#### Required tests

- [ ] Unit tests for fresh, stale, terminal, missing, future, and boundary
  timestamps.
- [ ] Fake-wallet and real-backend process tests across persistence and restart.

#### Closeout checklist

- [ ] Remove timestamp instrumentation and generated payment artifacts.
- [ ] Update status, cause or invalidation, impact, commands, results, and tests
  not run.
- [ ] Commit the evidence and any justified fix with a conventional commit.

### Milestone U2: Validate network-pump retry duplication

**Status:** Unconfirmed

**Found during:** Nocean Issue 8 investigation, now Milestone 4

**Observation:** The network pump emits one outbound message per iteration; after
three remote sends satisfy threshold, only 25 ms remains for the fourth send,
and unfinished work may be retried. This could amplify traffic, but the observed
AlephBFT `UnitBroadcast` queue is not itself proof that CDK retried requests.

#### Investigation and evidence gate

- [ ] Expose bounded diagnostic counts for CDK outbound queue depth, attempt
  identity, per-peer attempt count, completion, cancellation, and retry cause.
- [ ] Use a controlled delayed but healthy fourth peer and compare zero-delay,
  sub-25 ms, and greater-than-25 ms cases under identical consensus payload.
- [ ] Distinguish CDK-level duplicate send attempts from expected AlephBFT
  broadcast/retransmission and transport-level behavior.
- [ ] Prove whether retries are necessary for delivery, whether unfinished work
  is lost or duplicated, and whether retry volume materially contributes to
  queue growth or HTTP latency.
- [ ] Reproduce independently of the original Nocean workload before linking it
  causally to Milestone 4.
- [ ] Do not change retry behavior while the hypothesis remains unconfirmed.

#### Implementation requirements

- [ ] If confirmed, use the smallest existing queue/attempt ownership pattern so
  a send has one bounded completion lifecycle and only classified retryable
  failures are retried.
- [ ] Do not add unbounded concurrency, per-peer queues, or arbitrary timeout
  increases.

#### Success criteria

- [ ] A delayed healthy peer receives required messages without duplicate
  application work, retry storms, or quorum-progress regression.
- [ ] Queue depth, attempts per message, delivery latency, and HTTP latency stay
  within documented bounds during delay and reconnect tests.

#### Required tests

- [ ] Deterministic unit tests for completion, cancellation, classified retry,
  queue bounds, and delayed peers.
- [ ] Five-member slow-peer, disconnect/reconnect, and sustained-load process
  tests with attempt and delivery assertions.

#### Closeout checklist

- [ ] Remove per-attempt diagnostics or retain only bounded non-sensitive
  counters.
- [ ] Update status, causal evidence or invalidation, commands, results, and
  relationship to Milestone 4.
- [ ] Commit the evidence and any justified fix with a conventional commit.

## Scope and evidence labels

This report consolidates:

- the 2026-07-13 five-member Nocean receive stress test documented in
  [federated-cashu-nocean-stress-test.md](federated-cashu-nocean-stress-test.md);
- the 2026-07-14 local five-member restart, recovery, and transaction benchmark;
- the 2026-07-14 focused local confirmation of Issues 1, 2, 3, and 5, including
  the incidental revalidation of Issues 6 and 7;
- the local CPU regression investigation already recorded in that stress-test
  document.

The labels used below are:

- **Confirmed:** directly reproduced with process, journal, database, or client
  evidence.
- **Most likely cause:** consistent with the trace and code path, but not yet
  proven by a minimal failing regression.
- **Resolved and verified:** reproduced before the fix and measured or tested
  after the fix.
- **Needs revalidation:** previously observed, but affected code has changed and
  the exact process workload has not yet been repeated.

## Test configurations

### Local benchmark

- Five members on one local machine.
- BLS signature threshold: 3 of 5.
- Consensus threshold: 4 of 5.
- AlephBFT consensus, SQLite databases, and fakewallet payment processors.
- Current release-mode `cdk-mintd` and federation-enabled `cdk-cli` built from
  the same working tree before the run.
- Strict restart health required ready members, one converged height and digest,
  catch-up-certificate coverage, and no digest or invalid-operation alerts.
- Temporary processes were stopped after the run.

### Focused local issue-confirmation run

- Two disposable five-member local federations using AlephBFT, SQLite, and
  fakewallet payment processors.
- BLS signature threshold 3 of 5 and consensus threshold 4 of 5.
- Release-mode CDK 0.17.0 binaries. The `cdk-mintd` SHA-256 was
  `ce7ecb1e78f28add3020f23d3d15652e40efdebe7986ce29f2a1a90d1fe8c779` and
  the `cdk-cli` SHA-256 was
  `ff1df5e97aaea6837168d188c6adcb90b3da852f943c24f6bf31883899fa6e29`.
- The first federation created a converged 35-operation history, stopped member
  5, deleted only that member's SQLite database, WAL, and shared-memory files,
  and restarted it while the other four members remained live.
- The second federation stopped member 5 with its database intact, advanced the
  other four members, restarted member 5, and overlapped its startup with 20
  isolated wallet receives. A later 43-wallet receive wave used a deliberately
  adverse one-second per-member deadline and a two-minute wall-clock bound.
- Signed status, journal row counts and states, AlephBFT active sessions, wallet
  balances, public-route availability, client output, and process completion
  were recorded. Bearer-token artifacts remained private.
- All temporary mint processes were stopped and all test listeners were closed
  after evidence collection.

### Nocean benchmark

- Five co-located members on a four-logical-CPU development server.
- Remote load generation over HTTPS.
- The same 3-of-5 signature and 4-of-5 consensus thresholds.
- Full details, binary identities, method, and raw result tables are in the
  dedicated Nocean stress-test document.

### Fresh Nocean backpressure reproduction

- The current local working tree was compiled for Linux on the development
  machine and copied to the server with `scripts/deploy_nocean_federation.sh`.
- The remote federation was stopped, all member databases, AlephBFT backup
  state, and DKG state were erased, and a new five-member federation was set up
  and started. This was an intentionally destructive reset of a development
  deployment.
- Before load, all five members were healthy and converged at zero operations,
  with empty mempools and finalized cursors at zero.
- Idle interval CPU was approximately 1.1-1.2% per mint and each new SQLite
  database occupied approximately 4 KiB.
- The default `scripts/benchmark_nocean_federation.sh` matrix was started. The
  failure appeared during its first timed case, `N=10, parallelism=1`, after
  token and receiver-wallet preparation had completed.
- CPU, public and loopback endpoint latency, signed federation status, journal
  height, DAG statistics, and AlephBFT repeat counters were observed while the
  case ran and after it was interrupted.

## Performance results

### Current local results

| Measurement | Result | Qualification |
| --- | ---: | --- |
| Initial five-member startup | 1.303 s | Zero-operation federation to five healthy live sessions |
| Sequential standard batch | 7 entries in 4.797 s | Mint, swap, melt, restore; not a raw swap benchmark |
| Parallel receive workload | 100/100 successful | One accepted swap per receive |
| Parallelism | 20 | Isolated receiver wallet per token |
| Client-completion journal rate | 25.09 entries/s | 100 entries in 3.986 s |
| Post-client convergence delay | 1.710 s | All five members converged after clients completed |
| Commit-to-convergence rate | 17.56 entries/s | 100 entries over the full 5.696 s interval |
| Final converged height | 316 entries | Matching order and materialized-state digests |
| Full five-member restart | 2.958 s | Exact timed rerun at height 316 |
| One-member intact restart | 1.673 s | Height 316 with certificate coverage |
| One-member erased recovery | Failed | Only height 1 after approximately 155 s |

The 100-receive result is the cleanest current local throughput measurement. It
includes CLI process startup, HTTP fanout, consensus, threshold-share
collection, wallet verification, and wallet persistence. Wallet creation and
federation import occurred before the timed interval.

### Focused local issue-confirmation results

| Measurement | Result | Qualification |
| --- | ---: | --- |
| Erased-member source height | 35 operations | Five-member signed convergence before erasure |
| Erased-member recovered height | 11 operations | Stalled through index 10 for more than 15 minutes |
| Erased-member status | `healthy`, `ready=true` | Local certificate covered only operations 0-10 |
| Erased-member session | 1 versus peer session 0 | Repeated bidirectional session-mismatch rejection |
| Intact-member overlap receives | 20/20 successful | 0.56-0.81 s per receive while member 5 restarted |
| Post-overlap convergence | 57 operations on all five | Matching journal and materialized-state digests |
| Adverse-deadline receive wave | 43/43 printed success | Every wallet persisted a one-sat balance |
| Client process completion | 0/43 within two minutes | Every process hung after printing `Received: 1` |
| Healthy-majority journal | 144 applied entries | Members 1, 2, 4, and 5 |
| Stalled minority journal | 100 entries | 99 applied and one accepted on member 3 |
| Intact member 3 restart | Journal recovered to 144 applied | Private status responsiveness also returned |
| Post-restart sessions | Four in session 1, member 3 in session 0 | Strict ready/converged status still exited successfully |

The one-second deadline case is an intentionally adverse correctness probe, not
a production throughput result. Its important result is the separation between
threshold success, wallet persistence, process completion, minority
application, and live-session convergence.

### Earlier Nocean results

| Workload | Observed result |
| --- | ---: |
| Sustained confirmed receive plateau | Approximately 0.27 swaps/s |
| Highest confirmed one-wave burst | 0.566 swaps/s |
| Highest journal-rate estimate | Approximately 0.566 entries/s |
| Concurrency-5 failure rate | 30.0% |
| Concurrency-10 failure rate | 47.5% |

These numbers must not be compared as if they were two revisions of one
controlled benchmark. The local run removed public-network latency and used a
different machine, newer code, a bounded one-wave receive test, and a corrected
commit-through-convergence timer. The improvement is encouraging, but it does
not by itself prove a 40-90x production throughput increase.

### Fresh Nocean backpressure incident

The fresh-state run did not complete a benchmark row because it was stopped
when the public endpoints became unavailable. Its partial measurements are
still diagnostically important:

| Observation | Result |
| --- | ---: |
| Timed case | `N=10, parallelism=1` |
| Operations before timed case | 6 |
| Operations after interruption and convergence | 10 |
| First successful receive | 6.587 s |
| Failed receive | 108.233 s |
| Failure detail | 0 of 3 required shares; all five member requests timed out after 90 s |
| Later successful receive | 35.082 s |
| Finalized items after recovery | 40 per member |
| AlephBFT DAG units after 10 accepted operations | Approximately 744 |
| Peak observed `UnitBroadcast` backlog | Approximately 80-160 per member |
| Mint CPU during observed bursts | Generally 4-12%, versus approximately 1-2% idle |
| Recovery | HTTPS recovered after backlog drainage; no restart required |

The interrupted case must not be presented as a completed TPS result. It does
show that the federation can enter a severe latency and availability failure at
parallelism 1, so this incident is not explained by excessive client
concurrency.

### Nocean cadence-amplification rerun

A subsequent run started from a healthy federation and increased concurrency
one bounded step at a time. The default matrix did not reproduce a complete
HTTPS outage, but it reproduced the unit amplification, periodic CPU
saturation, latency increase, and prolonged wallet preparation stalls:

| Case | Result | Confirmed TPS | Successful receive latency |
| --- | ---: | ---: | ---: |
| `N=10, parallelism=1` | 10/10 | 0.151 | p50 6.562 s, max 6.704 s |
| `N=20, parallelism=5` | 20/20 | 0.739 | p50 6.671 s, max 6.878 s |
| `N=20, parallelism=10` | 20/20 | 1.388 | p50 6.901 s, max 7.474 s |
| `N=20, parallelism=20` | 20/20 | 2.456 | p50 6.959 s, max 8.118 s |
| `N=40, parallelism=40` | Stopped during preparation at 20/40 tokens | Not timed | Preparation repeatedly paused for minutes |

During the final preparation run, aggregate mint CPU peaked at 369% on the
four-logical-CPU host, the reconstructed DAG reached 6,489 units at only 144
accepted operations, and `UnitBroadcast` reached approximately 81-83 per
member. Public endpoint latency increased, but lightweight server-local
`/v1/info` remained responsive. After stopping the benchmark, queues drained to
the normal five repeatable broadcasts and CPU returned to approximately 1% per
mint without a restart.

## Detailed correctness and liveness evidence

The issue records below preserve the raw observations, reproduction notes, and
earlier analysis. The milestone status and evidence gates above govern whether
implementation may begin; a `Most likely cause` in this section is a hypothesis,
not authorization to fix it.

### 1. Fresh-database member does not catch up

**Severity:** Critical  
**Status:** Confirmed in two independent local process runs

Member 5 was stopped at converged height 316 and its SQLite database, WAL, and
shared-memory files were deleted. The other four members remained live. The
fresh member was then given approximately 155 seconds to reach the existing
height. It reached only height 1.

The member did not merely recover slowly. Its logs showed:

- AlephBFT backup state behind the unit-collection state;
- the AlephBFT creator and consensus service terminating early;
- the member starting or advertising session 1 while the existing federation
  remained in session 0;
- repeated non-retryable session-mismatch responses in both directions;
- the private AlephBFT network receiver closing.

The focused confirmation reproduced the same failure from a converged
35-operation history. The erased member initially recovered through index 10,
then remained at 11 of 35 operations for more than 15 minutes. It entered
session 1 while all four peers remained in session 0 and repeatedly received
non-retryable `expected=0, found=1` responses; it also rejected peer traffic as
`expected=1, found=0`. The smaller history proves that the defect does not
require hundreds of operations or a long-running database.

**Most likely cause:** startup ordering allows an empty member to install a live
AlephBFT session and mark itself healthy before peer-height discovery and REST
journal recovery. Once it is `Healthy` with a live session and inbound network,
`should_defer_catch_up_to_live_aleph_bft` defers REST catch-up. The empty member
then tries to participate without the persisted AlephBFT backup and finalized
history required to join the peers' current session. Session rotation makes the
split explicit rather than repairing it.

**Required fix:** a member with no trustworthy local checkpoint must begin
fail-closed in startup recovery, compare signed peer status, recover journal,
finalized-item, application, certificate, and AlephBFT backup state, and only
then install the live session and become healthy. Deferral to live AlephBFT is
valid only when the local live session is known to be continuous with the peer
session and history.

**Required regression:** start four members with several hundred operations,
erase the fifth database, and restart it. The fifth member must initially report
startup/catch-up rather than healthy, must not start a conflicting session, and
must reach the exact peer height, order digest, materialized-state digest,
finalized cursor, and catch-up-certificate coverage.

Add a small-history variant with approximately 35 operations so the critical
failure runs cheaply in ordinary CI. The test must remain active beyond the
first partial REST recovery and fail if progress stops before the peer tip or if
the member starts any session not supported by quorum signed peer status.

### 2. A behind member can falsely report healthy and ready

**Severity:** Critical  
**Status:** Confirmed for height lag and live-session divergence

At local height 1 versus peer height 316, the erased member reported:

```text
sync=healthy ready=true operations=1
```

Its catch-up certificate covered its one local operation, so
`--require-ready --require-catch-up-certificate-quorum` was also insufficient to
detect the 315-operation deficit. The benchmark had to impose the expected
height explicitly.

The focused confirmation reproduced the same claim at 11 local operations
versus 35 peer operations:

```text
sync=healthy ready=true operations=11 session=1
```

The member's certificate covered only indices 0-10 while the peers' certificates
covered indices 0-34. More importantly, the second federation demonstrated
that equal height and digest are also insufficient. After member 3 restarted,
all five members held the same 144 applied journal entries and matching order
and materialized-state digests, but member 3 remained in AlephBFT session 0
while the other four were in session 1. The strict command using
`--require-ready`, `--require-converged`, catch-up-certificate quorum, and both
alert checks exited successfully. All five members reported healthy and ready.

This is a distinct missing invariant: the operator can prove historical state
convergence while one member is unable to participate in the quorum's current
live consensus session.

**Most likely cause:** readiness proves internal consistency only against the
member's local tip. A fresh runtime defaults to healthy before it has established
a recent signed peer-height view, and the live-session catch-up deferral returns
without first marking the node lagging.

**Required fix:** distinguish local consistency from federation currency. A
member must not advertise authoritative readiness until it has compared itself
with quorum peer evidence during startup. The status response should expose the
last successful peer comparison and its age. The operator status command should
continue to compare members, but a single-member health check must also fail
closed when startup peer validation has never completed.

Readiness and strict convergence must additionally require a quorum-compatible
AlephBFT session view. Status should expose the locally active session, the
quorum-observed peer session, the age of that observation, and whether the
member has exchanged valid traffic in that session. A session mismatch must
make the member non-ready even when height, digests, application state, and
certificate coverage match.

### 3. Wallet operations can commit while the client reports failure

**Severity:** High  
**Status:** Confirmed on Nocean; exact failure not reproduced locally after
fanout changes, but post-success client completion still failed

In the Nocean concurrency-5 and concurrency-10 cases, every attempted swap
advanced the converged journal even though 25 wallet processes exited without
enough shares. Database inspection later showed that all five mints applied all
25 operations and persisted operation-bound signature shares.

This creates an ambiguous client result: blind retry can encounter spent inputs
even though the original command reported failure.

The focused local revalidation used 43 isolated receiver wallets, full
five-member fanout, zero additional member retries, and a deliberately adverse
one-second per-member timeout. All 43 operations advanced the healthy majority
from height 101 to 144. Every CLI printed `Received: 1`, and every wallet
database persisted a one-sat balance, so the exact earlier shape of a normal
failure after commit did not recur. However, none of the 43 CLI processes exited
within the two-minute wall-clock bound. They were stopped only after their
success output and wallet persistence had been verified.

At the same checkpoint, four members had 144 applied entries while member 3
held only 100 entries, comprising 99 applied and one accepted. Thus threshold
success, wallet persistence, minority application, and client-process
completion can still diverge. The local result must not be described as 43
clean client successes merely because success text was printed.

**Most likely cause:** the wallet previously cached only the first
threshold-sized compatible member subset. One temporary catch-up/readiness error
therefore reduced a nominal 3-of-5 operation to 2-of-3. Even after scheduling all
members, returning after threshold success and dropping unfinished request
futures can leave minority handlers accepted-but-unapplied. Catch-up errors were
also treated too much like terminal request failures instead of retryable member
readiness states.

**Required behavior:** all validated roster members remain eligible; retryable
member states do not count as rejection votes; the identical deterministic
request remains recoverable until threshold success, threshold definitive
rejection, or an overall deadline; and a deadline returns a typed
pending/ambiguous result rather than an ordinary failure.

Success must also have a bounded completion contract. After threshold success
and durable wallet persistence, surplus member work should be detached or
drained under a separate bound that cannot prevent the API or CLI process from
returning. Detachment must not cancel accepted server operations. Record
separate timestamps for threshold decision, wallet commit, user-visible output,
surplus-fanout completion, and process exit.

### 4. Threshold completion can cancel minority member application

**Severity:** High  
**Status:** Confirmed state divergence; cancellation is the strongest hypothesis

A local full-fanout follow-up found four members with an applied operation while
one member retained the same operation as accepted but unapplied. The client had
already obtained a threshold result.

The focused confirmation produced a larger variant. Four members reached 144
applied entries while member 3 remained at 100 entries with one row still
accepted and unapplied. The 43 affected clients had already printed success and
persisted their received balances. Restarting member 3 replayed the journal to
144 applied entries, proving that the durable history was recoverable but live
application had stopped.

**Most likely cause:** pending HTTP request futures are dropped once the wallet
has enough shares. Cancellation can occur after a minority handler has accepted
the journal item but before local application and response publication finish.

The wallet may stop waiting for surplus responses, but it must not be the owner
of whether an accepted mint operation finishes durably. The server submission
path needs cancellation-safe completion after acceptance, and the wallet should
drain or detach surplus requests in a bounded way that does not abandon server
work.

### 5. Wallet catch-up handling remains too brittle

**Severity:** High  
**Status:** Confirmed on Nocean; direct intact-member case passed locally, but
load-induced member divergence still blocked client completion

Wallet errors named members that were lagging, catching up, or verifying
catch-up certificates. These are retryable availability states, not definitive
transaction rejection votes. A catching-up member caused the command to fail
even when enough unused federation members could eventually provide shares.

The wallet must preserve operation IDs and blinded outputs, continue fanout to
all eligible members, retry with bounded backoff, and retain its saga on an
overall deadline. The mint should use a typed federation catch-up error and may
be able to return a strictly operation-bound persisted share for an exact replay
before applying the general new-request readiness gate.

The focused local process run produced two materially different results:

- Member 5 was stopped with an intact database while the other four advanced.
  After it restarted, 20 isolated receives overlapped its startup and all 20
  completed successfully in 0.56-0.81 seconds. All five members then converged
  at 57 operations. This is positive revalidation of the ordinary one-behind-
  member case.
- Under the later 43-wallet wave, member 3 stopped applying at height 100 while
  four members reached 144. Peers timed out its signed private status, all 43
  clients printed success but did not exit within two minutes, and the cluster
  split across AlephBFT sessions. Restart restored member 3's journal but not
  its live session. This is not the original typed `catching_up` error, but it
  confirms that one non-completing member can still prevent bounded client
  completion despite a healthy signature and consensus quorum.

The current evidence therefore narrows Issue 5: ordinary intact restart with
four healthy spares works, while load-induced accepted/unapplied, status-
unresponsive, or session-incompatible members remain unsafe for wallet
completion.

### 6. Repeated concurrent melts fail or stall

**Severity:** High for federation melt availability  
**Status:** Confirmed in two local process runs; root cause not yet isolated

Ten concurrent standard-operation workers each completed one full mint, swap,
melt, and restore batch. During the second batch:

- nine workers returned `Payment failed` from melt confirmation;
- one worker stalled at `standard.melt-confirm.start` until the benchmark was
  stopped;
- mint, swap, check-state, and melt-quote stages had already succeeded.

This is not evidence of insufficient consensus throughput: the receive-only
test later committed 100 swaps successfully at parallelism 20.

The focused confirmation incidentally reproduced the stall without ten
concurrent workers. With member 5 offline but a valid four-member consensus
quorum, the first standard mint/swap/melt/restore batch completed. The second
batch progressed through mint, swap, check-state, melt-quote creation, and quote
visibility, then remained at `standard.melt-confirm.start` until the explicit
120-second scenario bound expired. Receive/swap-only work was used afterward so
this independent melt failure would not contaminate the wallet catch-up test.

This smaller reproduction suggests that high wallet concurrency is not required;
consecutive melt state or payment-observation history may be sufficient. It
also strengthens the need to test quorum operation with one member offline.

**Most likely cause:** a race in concurrent melt payment ownership,
payment-observation finalization, or quote/saga state transitions. The current
evidence does not distinguish wallet polling from mint-side observer election or
fakewallet payment-state handling. It should not be dismissed as fakewallet-only
until the operation IDs, elected payer, payment identifiers, quote transitions,
and observations from one failed melt are traced across all members.

**Required regression:** run at least ten wallets through two consecutive melts
in parallel and require exactly-once payment, a threshold-consistent terminal
state, no stalled confirmations, and identical quote/payment observations on all
members.

Add a minimal sequential process regression: one wallet, one member offline,
and two consecutive standard batches. Bound each melt-confirmation phase and
record the quote, elected payer, payment identifier, observer reports, saga
state, and terminal response on every member.

### 7. Signed private status can become unresponsive under load

**Severity:** High operationally  
**Status:** Confirmed independently on two members in two local runs

After the concurrent standard-operation workload, member 1 continued to answer
the public information endpoint in approximately 30 ms but did not answer signed
private status within 20 seconds. Other members logged repeated 10-second status
timeouts against member 1 and increased reconnect backoff to 60 seconds. A
normal restart restored member 1 status immediately and it was healthy at the
existing height in 0.172 seconds.

The focused confirmation reproduced the same route-specific failure on member
3. During token preparation, all five public `/v1/info` routes returned HTTP
200, but peers repeatedly logged ten-second signed-status timeouts against
member 3 with reconnect backoff increasing through one, two, four, and eight
seconds. A direct signed status request produced no response within a 15-second
bound. Database inspection showed member 3 stalled at 100 journal entries while
the other four reached 144. Restart restored private status immediately and
replayed all 144 entries as applied.

The failure coincided with member 3 remaining in AlephBFT session 0 while the
other four moved to session 1. The trace does not prove whether status
unresponsiveness caused application/session lag, resulted from it, or shared a
lock/permit cause. These events must be correlated in the next instrumented
run.

**Most likely cause:** a private-status handler, admission permit, or state lock
was held or starved, rather than the HTTP runtime being generally unavailable.
Member 1 was also the preferred quote coordinator during the failing workload,
which concentrated quote and payment-observation work there. Status generation
serializes increasingly large payment reports and inspects several federation
caches, so lock duration and response size are plausible amplifiers. No stack
sample captured the exact holder.

**Required regression:** continuously issue bounded signed status requests while
running repeated parallel mint/swap/melt batches. Every member must retain a
bounded status latency, and a timed-out client must not leak a private REST
admission permit or retain a state lock.

The regression must continue through AlephBFT session rotation and assert that
status responsiveness, journal application, and live-session membership remain
coupled. Capture handler admission wait, lock wait/hold time, serialization
time, response size, event-loop delay, and the operation/session state at every
timeout.

### 8. Small receive load can amplify empty consensus units and starve HTTPS

**Severity:** Critical for remote availability  
**Status:** Confirmed liveness failure and confirmed cadence-amplification
source; retry amplification remains a secondary hypothesis

On a freshly reset federation, the first sequential receive case temporarily
made all five public mint endpoints unavailable. Loopback and public HTTP
recovered without a process restart after the AlephBFT broadcast backlog
drained. This rules out a persistent reverse-proxy configuration failure and
makes protocol/runtime backpressure much more likely than raw CPU exhaustion.

The following evidence was captured:

- only 10 accepted journal operations corresponded to approximately 744 total
  DAG units and 40 finalized items per member;
- reconstructed-DAG reports showed approximately 666 units during the first
  affected period and approximately 744 after a second burst;
- `UnitBroadcast` repeat counters reached approximately 161, fell through
  roughly 56-68 to approximately 5, then rose again to approximately 80-83 and
  drained again;
- creator logs printed `Delay passed.` at approximately 100 ms intervals during
  the bursts;
- all five wallet requests in one receive timed out after 90 seconds and the
  wallet returned `insufficient federation signature shares: required 3,
  received 0`;
- interval CPU generally remained around 4-12% per mint, with no host CPU
  saturation; and
- once the repeat queues drained, all five public `/v1/info` routes returned
  HTTP 200 again without restarting the federation.

The controlled rerun added stronger evidence:

- the default concurrency matrix completed successfully, proving that a full
  outage is intermittent rather than a deterministic threshold at concurrency
  20;
- each successful receive still took approximately 6.6-8.1 seconds;
- aggregate mint CPU repeatedly reached 300-369% on the four-logical-CPU host;
- the above-default run became operationally stalled during token preparation,
  before its timed parallel receive began;
- the federation remained journal-converged while the wallet waited, showing
  that consensus acceptance and client completion can separate; and
- stopping load allowed CPU and broadcast queues to return to idle without a
  process restart.

**Confirmed source:** production overrides AlephBFT's normal unit-creation
schedule with a fixed 100 ms delay whenever the shared activity hint is active.
Upstream AlephBFT normally waits 5 seconds for round zero and 500 ms for
subsequent rounds through the range relevant here. The active override therefore
creates units five times faster than the normal steady schedule and fifty times
faster for round zero.

At the same time, the CDK data provider explicitly returns `None` when it has no
transaction payload but the activity hint remains active. AlephBFT interprets
that as permission to create an empty unit. While a real payload is awaiting
finalization, all five members can therefore create roughly ten units per
second, or approximately fifty federation units per second, most without Cashu
payloads. Every unit creates dissemination, signature-verification, backup, and
private-REST work. The measured 100 ms creator logs, DAG growth, CPU bursts, and
`UnitBroadcast` queues match this mechanism.

This behavior came from the change named `fix: bound active AlephBFT cadence`:
the previous code retained AlephBFT's default schedule while active and used the
two-second slowdown only for idle tail rounds. The change instead applies 100
ms to every active round. The intent was to bound proposal latency, but the
result is a fivefold increase in normal active unit creation.

The principal code paths to instrument are:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs`, especially
  `CDK_ALEPH_BFT_ACTIVE_UNIT_CREATION_DELAY`,
  `CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY`, the activity-broadcast loop, and
  delay configuration;
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`, especially
  `CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE` and
  `CdkAlephBftActivityHint`, `CdkAlephBftDataProvider::get_data`, and
  `mark_network_activity_finalized`; and
- `crates/cdk-axum/src/federation/aleph_bft_network.rs`, where inbound
  consensus messages extract activity identifiers and call
  `mark_network_activity`.

**Correction to the original hypothesis:** empty units do not directly renew
the activity lease. The inbound activity marker extracts IDs only from included
payload data, so a payload-free unit contributes no new activity ID.
Finalization also shortens matching body-hash and operation-ID leases to one
second. The defect is sustained fast empty-unit creation while legitimate
payload activity is pending, not a self-renewing empty-unit loop.

**Secondary likely amplifier:** the CDK network pump drains one outbound message
per iteration. It submits that message to all remote peers, but after reaching
the three-remote-member threshold it waits only 25 ms for unfinished peers.
Unfinished deliveries are classified as retryable and queued again. Under load,
a healthy but slightly slower fourth peer can therefore create duplicate
private-REST delivery work. This follows from the code, but the Nocean trace did
not expose the CDK retry-queue length, so it is not yet proven necessary for the
failure. AlephBFT's reported `UnitBroadcast` task count must not be treated as
the same queue.

**Reproduction:** on the development deployment only, start from a fresh
federation and then run the smallest benchmark case while monitoring every
member:

```bash
# Build locally, upload with SCP, install, and verify the current binaries.
scripts/deploy_nocean_federation.sh

# Destructive: erase only the disposable remote development federation.
ssh nocean 'cd ~/cdk-federation && \
  bash scripts/stop-federation.sh && \
  bash scripts/reset-federation-data.sh --yes && \
  bash scripts/setup-federation.sh && \
  bash scripts/start-federation.sh'

# Use a single sequential receive case before increasing concurrency.
CASES='10:1' scripts/benchmark_nocean_federation.sh

# If healthy, increase one bounded step at a time.
CASES='20:5' scripts/benchmark_nocean_federation.sh
CASES='20:10' scripts/benchmark_nocean_federation.sh
CASES='20:20' scripts/benchmark_nocean_federation.sh

# Optional above-default escalation. Stop on a bounded deadline; preparation
# itself may reproduce the stall before the timed receive begins.
CASES='40:40' scripts/benchmark_nocean_federation.sh
```

Before the timed receive, require five healthy converged members, record an
idle interval CPU baseline, and confirm all public and loopback information
routes answer. During the receive, sample process CPU rather than relying on a
single instantaneous value; continuously time `/v1/info`; retain signed status,
operation height, pending mempool size, finalized cursor, reconstructed-DAG
size, and AlephBFT repeat counters. Stop the case after a bounded deadline if
HTTPS becomes unavailable. Keep the benchmark directory private because it can
contain bearer tokens.

The amplification bug is reproduced even if every wallet command succeeds: CPU
bursts, DAG growth, and `UnitBroadcast` depth must be compared with the number
of accepted operations. The stronger liveness failure is reproduced when
operation or public requests time out, or wallet preparation stops making
progress while the journal is already converged. Continue observing after
stopping load: recovery synchronized with queue drainage, without a mint
restart, distinguishes this failure from proxy misconfiguration, database
corruption, or permanent deadlock.

**Required instrumentation and regression:** expose active versus idle unit
counts, empty versus payload-bearing units, activity-ID cardinality and lease
age, CDK retry-queue depth, per-peer send-queue depth, event-loop delay, and HTTP
handler latency. A five-member process test should submit a small bounded number
of sequential
swaps, require a bounded number of empty units per payload and bounded queue
depth, and continuously require public and signed private routes to respond.
The test must wait beyond the network-activity grace period and prove that all
members return to the normal idle cadence. A separate network-pump test should
delay one healthy peer beyond the 25 ms completion grace and measure whether
the retry policy creates avoidable duplicate delivery work.

### 9. Federation discovery/import can hang after a healthy config response

**Severity:** Medium  
**Status:** Confirmed in the Nocean setup; needs revalidation

`GET /federation/v1/config` returned HTTP 200 with a valid configuration, but
`cdk-cli federation add --mint-url ...` hung during post-discovery
import/validation. Importing the same immutable config from a file succeeded.

**Most likely cause:** the problem is after config retrieval, probably member
metadata/keyset validation or incomplete fanout cancellation, not the discovery
route itself. The exact import path must be rerun after the wallet fanout work.

## Detailed performance evidence

### 10. Sequential token preparation degrades with wallet fragmentation

**Status:** Observed, not precisely quantified

The bounded local preparation run produced 240 independent tokens before it was
stopped. Progress slowed materially over the run. Only 84 new consensus entries
were added while producing those 240 tokens, because many sends could use exact
local proofs without an online swap.

The slowdown therefore cannot be attributed solely to consensus journal growth.
The most likely contributors are increasing wallet proof fragmentation, coin
selection, repeated CLI startup and SQLite work, plus occasional federated swaps.
A future benchmark must separately count offline sends and online pre-send swaps
and record per-send latency against wallet proof count and journal height.

### 11. Remote sustained throughput and latency remain poor

**Status:** Confirmed for the tested Nocean revision

The Nocean run sustained only about 0.27 confirmed swaps/s at concurrency 5 and
10, with failure rates rising to 30-47.5%. A one-wave concurrency-20 burst
reached 0.566 swaps/s but had approximately 32-35 second successful latency.

The dominant observed limitation was not raw rejection by consensus. Operations
continued to enter the journal while wallets failed to collect threshold shares
through member catch-up/readiness churn. Wallet fanout and catch-up orchestration
must be fixed and the same remote matrix repeated before treating these numbers
as a consensus-engine capacity limit.

### 12. Status and recovery work can scale with journal and report size

**Status:** Partly resolved; residual risk confirmed by the status hang

Status, digest, certificate, and startup paths previously reloaded or replayed
unchanged journal state. Large payment-observation reports also make signed
status responses grow with workload history. The new caches remove major
repeated computation, but the member-1 status failure shows that bounded status
latency still needs a process-level load test.

Operator endpoints should expose summaries by default and retrieve detailed
rows or reports only on demand. Cache invalidation must remain tied to journal,
application, certificate, and observation mutation rather than polling.

### 13. Payment-report quote age can be nonsensical

**Status:** Confirmed with the local fakewallet configuration; production
payment backends need revalidation

Signed status in both focused local federations reported newly created mint and
melt quotes with `quote_age_secs` values around 1.784 billion seconds, close to
the current Unix timestamp rather than the quotes' actual age of minutes. The
same reports marked the quotes `stale=false`.

The most likely explanation is a missing or zero creation timestamp in the
fakewallet quote/report path, followed by age calculation against the Unix
epoch. The trace did not establish whether real payment backends populate the
field correctly. Even if test-only, the value makes local stale-payment
diagnostics misleading and can hide age-calculation regressions.

Status generation should represent an unavailable creation timestamp as
unknown rather than computing an epoch-sized age. Add unit and process coverage
for fresh, stale, terminal, and missing-timestamp quotes across fakewallet and at
least one real payment-backend fixture, and assert consistency between
`quote_age_secs` and `stale`.

## Resolved and verified performance defects

### Healthy live consensus was mistaken for recovery-worthy lag

Under modest swap load, a normal one-operation propagation gap caused the
peer-sync loop to mark a member lagging and start REST journal catch-up even
though its healthy live AlephBFT session was already delivering that operation.
In the corrected process-level reproduction, one member entered lagging 27
times, catch-up 15 times, and certificate verification 3 times during otherwise
convergent load.

The peer-sync loop now leaves delivery to AlephBFT while readiness is healthy
and both the live session and network input are installed. Startup and genuinely
non-healthy states retain fail-closed REST recovery. The post-fix five-member
regression converged with zero height skew and zero lag/catch-up transitions.

The erased-database failure in Issue 1 exposes the necessary boundary on this
fix: the optimization is correct only after startup has established that the
local persisted history and live session are continuous with quorum peer state.
A new empty member must not default into the healthy condition that suppresses
its required recovery.

### Effective federation config replayed the full journal on every read

The cache was consulted only after loading and deserializing the journal. BLS
key decoding and subgroup checks made repeated config reads increasingly
expensive. The cache is now checked before database access and invalidated after
an applied keyset rotation. Focused regressions verify one replay for repeated
unchanged reads and correct rotation invalidation.

### AlephBFT backup persistence had quadratic write amplification

Every backup flush appended to one growing SQL BLOB. At 130 operations, roughly
1.3 MB of logical backup produced about 3.34 GB of cumulative SQL expression
output per member. Backups are now stored as ordered independent rows, with
legacy migration and exact restart-prefix suppression. Identical chunks remain
valid and are not content-deduplicated.

### Repeated digest and certificate verification reloaded unchanged state

Materialized-state digests and validated catch-up-certificate quorum results are
now cached until relevant mutation. Detailed materialized rows are fetched only
after a digest mismatch. Focused cache, mutation, and migration tests pass.

### CPU runaway during idle/retry state

Before these fixes, the reproduced problematic state used approximately 8-17%
CPU per mint. After the fixes, local idle and accepted-but-unapplied states used
approximately 0.0-0.1% per mint, including repeated signed digest checks. This
proves the runaway CPU defect is fixed; it does not prove that all throughput or
liveness defects are fixed.

The fresh Nocean failure in Issue 8 is distinct. It showed consensus-message
amplification, queue backpressure, and HTTP starvation at moderate CPU rather
than journal-length-dependent idle CPU. Fixing the earlier replay and cache
costs therefore does not resolve or contradict the new observation.

## Recommendations from the focused local confirmation

### Make readiness a quorum-derived claim

Treat `ready` as a lease derived from recent signed quorum evidence, not as a
default local state. The lease must cover peer height, order and materialized
digests, catch-up certificates, active AlephBFT session, finalized cursor, and
successful traffic exchange in that session. Expiry, missing startup evidence,
height lag, or session mismatch must fail closed. Preserve a separate local-
consistency field so operators can distinguish internally valid storage from a
member that is current and able to participate.

### Separate recovery from live consensus activation

Startup should first classify the local checkpoint. A missing or untrusted
checkpoint must recover journal, finalized items, application state,
certificates, session state, and AlephBFT backup before installing live network
input. A trusted but behind checkpoint may use live AlephBFT only after proving
session continuity with quorum peers. Session rotation must require quorum
evidence and must not allow one member to unilaterally advertise a session that
its peers reject.

### Make server completion cancellation-safe and client completion bounded

Once a server has durably accepted an operation, request cancellation must not
control whether application and response persistence finish. On the client,
threshold success and wallet persistence should permit a bounded return while
surplus member work continues independently. A surplus request may be detached,
but never cancelled in a way that strands an accepted journal entry. Add a hard
post-success process-exit bound and expose separate metrics for threshold
decision, wallet commit, response drain, and exit.

### Treat session and application skew as first-class alerts

The strict status command should fail when members disagree on active session,
when any member has accepted/applying rows older than a short bound, or when a
member's signed status is unavailable even if public information remains
healthy. Summaries should show per-member session, journal height, applied
height, accepted/applying counts, peer-view age, and status latency without
requiring large payment-report serialization.

### Keep correctness probes separate from capacity benchmarks

Retain a small deterministic matrix for erased recovery, intact recovery,
session rotation, accepted-but-unapplied replay, slow/unresponsive surplus
members, consecutive melts, and post-success CLI exit. Run capacity curves only
after those invariants pass. Every process benchmark should impose a hard wall
clock and report partial client output, wallet persistence, journal state,
application state, and session state when the bound expires.

## Positive performance findings

- Initial empty-federation startup is fast at 1.303 seconds.
- Intact local restart replay is fast: 1.673 seconds for one member and 2.958
  seconds for all five at 316 operations.
- Parallel local receive throughput reached 25.09 client-completed committed
  swaps/s with no failures at parallelism 20.
- All five members converged 1.710 seconds after those clients completed.
- In the focused run, 20 receives overlapping an intact behind member's restart
  all completed in 0.56-0.81 seconds and converged on all five members.
- The CPU fixes eliminated journal-length-dependent idle spin in the reproduced
  state.

These results establish that normal restart and local swap throughput are not
the immediate blockers. Fresh recovery, session-aware readiness, cancellation-
safe minority application, bounded wallet completion, melt concurrency, and
status liveness are.

## Cross-milestone dependency summary

The roadmap and milestone checklists above are authoritative. In condensed
order, the dependencies are:

1. Investigate and fix fresh-member startup recovery, then make readiness depend on recent quorum
   height, digest, certificate, and live-session evidence.
2. Make strict operator status reject AlephBFT session divergence once the
   readiness evidence model is proven, even when
   journal height and digests match.
3. Prove, then implement, cancellation-safe server application and bounded client exit after
   threshold success; eliminate accepted-but-unapplied minority stalls.
4. Reproduce and explain consecutive melt failure/stall behavior before fixing the
   one-wallet, one-offline-member case.
5. Complete the cadence/backpressure causal chain, then bound empty-unit
   amplification and preserve HTTP liveness under small remote
   workloads.
6. Reproduce and explain the private-status hang with lock, permit, handler-latency, session,
   and application-state instrumentation.
7. Revalidate typed catch-up/ambiguous wallet outcomes with one and two slow,
   catching-up, unresponsive, and session-incompatible members.
8. Revalidate federation discovery/import after wallet fanout changes and make
   no production change if the current code invalidates the report.
9. Repeat controlled local and remote capacity curves only after the correctness
   and liveness fixes above.

## Cross-milestone regression matrix

This matrix is a shared reference. A test counts toward milestone completion
only when its checklist records the exact command, result, and relevant state
assertions.

- Fresh member at histories of 1, 100, 1,000, and 10,000 operations; measure
  time through exact digest/certificate convergence and assert non-healthy
  startup state.
- A cheap approximately 35-operation erased-member CI regression that waits
  beyond partial recovery and rejects any unsupported session transition.
- Intact one-member and full-cluster restart at the same heights.
- Equal-height, equal-digest members with deliberately mismatched active
  sessions; strict status and single-member readiness must fail.
- Session rotation under load with continuous assertions that every member
  remains in a quorum-compatible session and continues applying journal rows.
- Repeated 3-of-5 fanout with one and two slow/catching-up members.
- Repeated 3-of-5 fanout with one accepted-but-unapplied, status-unresponsive,
  or session-incompatible member. Threshold success must return within a
  bounded post-success interval without cancelling server application.
- A 43-wallet-style adverse-deadline wave that separately asserts user-visible
  result, wallet persistence, CLI exit, journal commitment, per-member
  application, and session convergence.
- Ten or more wallets performing at least two consecutive melts concurrently.
- One wallet performing two consecutive standard batches while one member is
  offline, with a hard melt-confirmation deadline and cross-member observation
  comparison.
- Continuous signed status polling during mint, swap, melt, restart, and catch-up
  load, with bounded latency and admission-permit accounting.
- Fresh and stale payment reports with valid and missing quote timestamps;
  reported age and stale classification must remain consistent.
- Fixed-wave receive tests with at least five repetitions per concurrency,
  measuring client completion and signed convergence separately.
- A fresh-state `N=10, parallelism=1` remote receive test with continuous public
  route probes, event-loop delay, activity-hint cardinality, empty/payload unit
  counts, and per-peer `UnitBroadcast` queue depth.
- Synchronized CPU, RSS, SQLite lock time, disk I/O, journal height, AlephBFT
  session/round, response-cache, and catch-up-transition telemetry.

Hard wall-clock limits should guard exploratory measurements. A single case
should normally stop after two minutes and report its partial height and state;
longer recovery tests should be explicit soak tests rather than accidental
benchmark stalls.
