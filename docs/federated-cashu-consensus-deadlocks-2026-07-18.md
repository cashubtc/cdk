# Federated Cashu unresolved consensus deadlocks — 2026-07-18

## Audit scope and terminology

This is the remediation ledger for federation failures that can stop a live
AlephBFT session, keep an accepted value operation pending indefinitely,
prevent a member from recovering, or starve required federation work. It keeps
completed milestone evidence alongside open findings and combines previously
reported incidents with a source review of the current federation, wallet,
FROST/BDK, payment-observation, recovery, and worker code.

In this document, **deadlock** includes:

1. **Session halt:** finalized ordering cannot advance and the member enters
   `Halted`.
2. **Value deadlock:** consensus remains healthy, but a quote, proof set, UTXO,
   or on-chain intent can never reach its next terminal state.
3. **Recovery deadlock:** a non-ready member cannot acquire or apply the
   evidence needed to rejoin.
4. **Starvation/resource deadlock:** a worker or serialized application path can
   remain occupied indefinitely, or an unbounded queue can remove enough
   members to lose quorum.

Evidence labels used below:

- **Source-proven:** the complete failure path is present in current code and
  does not require an unverified assumption about an external library.
- **Process-reproduced:** a repository report records the failure in a running
  federation.
- **High-confidence hazard:** the CDK-side path is present, but one external or
  timing precondition still needs an executable regression.

This is not a claim that all items are mutex cycles. The current review did not
find a new server-side Rust mutex lock-order cycle comparable to the resolved
Nocean incident. Most current failures are trust-boundary, async wait,
state-machine, scheduling, or recovery-source problems.

## Ranked findings

The 21 remediation milestones were initially sorted by highest applicable
severity and then by confidence. Completed and skipped milestones retain their
rank for audit history. Ties are ordered by blast radius and exploitability.
Confidence measures the documented failure mechanism, not whether it has
already occurred in a particular deployment. For finding 14 (`R4`), the failure
is well established but its root cause is not.

| Rank | Legacy ID | Finding | Severity | Confidence | Reach | Milestone status |
|---:|---|---|---|---:|---|---|
| 1 | `F1` | A single peer's equal-height status is treated as halt authority | Critical | 99% | Session/global if repeated against all honest members | Completed |
| 2 | `F2` | Semantically invalid Byzantine operations halt finalized application | Critical | 99% | Session/global | Not started |
| 3 | `P4` | Recovery-critical outgoing lookup ID is persisted after the side effect | Critical safety / High liveness | 99% | Quote/proofs and possible external-payment double spend | Not started |
| 4 | `P5` | One member can reorg or quarantine accepted on-chain funds | Critical | 99% | On-chain quote/funds; all on-chain liquidity if repeated | Not started |
| 5 | `P1` | Accepted melt author has a permanent payment veto | High | 99% | Quote/proofs; malicious or failed author veto | Not started |
| 6 | `P2` | One backend call or poison quote blocks the single observer worker | High | 99% | All payment repair on one member; quorum loss if shared | Not started |
| 7 | `S1` | DKG availability is all-member, not Byzantine-threshold | High availability | 99% | Federation setup/key ceremony | Not started |
| 8 | `F3` | Local BDK reconciliation is inside the fatal apply boundary | High | 98% | Session/member; quorum loss if repeated | Not started |
| 9 | `W2` | Melt-status recovery lets one invalid share poison an honest threshold | High availability / Medium scope | 98% | Wallet melt recovery | Not started |
| 10 | `F4` | Apply and backup retries have no terminal budget | High | 97% | Session/member starvation | Not started |
| 11 | `P3` | Exact evidence quorum is not backed by an enforced evidence model | High | 97% | Quote/proofs | Not started |
| 12 | `R1` | Recovery candidate fetch is pinned to the first source | High | 97% | Startup recovery | Not started |
| 13 | `A1` | Aleph queues lack bounded backpressure | High security gate | 97% | Process/quorum resource exhaustion | Not started |
| 14 | `R4` | Finalized-item catch-up response-count mismatch remains unexplained | High investigation | 95% failure / 55% cause | Member recovery | Not started |
| 15 | `R2` | Opaque Aleph backup can pin startup to unusable bytes | High | 85% | Startup/live-session recovery | Not started |
| 16 | `P7` | Staged melt commits need complete crash/replay proof | High pending proof | 75% | Quote/proofs | Not started |
| 17 | `W1` | Member metadata validation can wait forever | Medium | 99% | Wallet operations | Not started |
| 18 | `W3` | Broadcast worker has the same sequential backend head-of-line failure | Medium | 98% | On-chain settlements | Not started |
| 19 | `P6` | Unbounded pending-quote population amplifies wallet-request DoS | Medium | 97% | Resource starvation | Not started |
| 20 | `R3` | Retry queue is serviced only when fresh traffic is idle | Medium | 95% | Network/rejoining member | Not started |
| 21 | `A2` | Payment-observation submit readiness remains implicit | Medium | 85% | Consensus backlog and invalid-state amplification | Not started |

### Most likely common cause

The most important systemic cause is a mismatch between the Byzantine trust
boundary and the application failure policy:

> AlephBFT is allowed to order authenticated member data, but several later
> layers treat false member claims or invalid operation contents as proof that
> the local node is corrupt. They then halt the node instead of rejecting the
> member's item or isolating the member/source.

That pattern explains the pre-fix form of finding 1, directly explains open
finding 2, and amplifies finding 8. For unexplained payment stalls while
consensus stays healthy, findings 5, 6, and 11 are more likely than an AlephBFT
failure. For on-chain outputs becoming stuck, finding 4 is the most likely
cause; the accidental version is a lagging/inconsistent chain backend, while a
Byzantine member can deliberately create the same state.

## Binding development contract

This ledger is also the remediation plan. The development constraints in
`docs/federated-cashu-implementation-plan.md` are binding for every milestone
below. If a proposed correction conflicts with them, record the conflict and
skip the milestone; do not work around the constraint.

### Scope boundary

- Work on exactly one numbered milestone at a time, in the ranked order below.
  Only one milestone may be `Confirming`, `Fixing`, or `Evaluating`.
- Federation code, federation-only feature-gated branches, federation tests,
  federation schemas/migrations, and these federation documents are in scope.
- Standalone CDK code and behavior are read-only. Running standalone tests is
  required where useful, but editing a shared or standalone path to make a
  federation fix work is prohibited. A mixed file may be changed only where
  the changed branch is unambiguously federation-only and standalone behavior
  is unchanged. If a fix requires changing shared standalone semantics, record
  that fact and skip the milestone.
- Preserve the public Cashu protocol and existing standalone behavior unless
  federation is explicitly enabled. Federation remains BLS-only, using
  `KeySetVersion::Version02` and keyset prefix `02`; version or algorithm
  mismatches must fail closed.
- Do not do unrelated cleanup. Reuse existing CDK validation and business
  logic. Extract a small shared operation only when it removes demonstrated
  duplication without changing standalone behavior; speculative abstractions
  and broad refactors are out of scope.
- Keep federation coupling thin and explicit. The centralized wallet connector
  owns member fan-out, agreement, fallback, share validation, aggregation, and
  ambiguous-result handling. Mint domain code owns Cashu validation and
  deterministic application. Federation code owns ordering, quorum evidence,
  durable replay, and recovery; it must not create a second implementation of
  standalone mint behavior.
- Use Fedimint as a reference for safety properties and failure handling, not as
  an architecture template. Import a primitive or dependency only for a
  specific recorded gap that existing workspace facilities cannot satisfy.
- Prefer dedicated federation modules behind existing feature gates over
  scattering federation conditionals. Keep handlers thin: parse, authenticate,
  delegate, and translate errors. HTTP/private REST endpoints are transport and
  diagnostics, not an alternate consensus path. Aleph orders authenticated
  data; it does not prove business validity. The committed journal is
  authoritative; a local projection or backend response is not quorum evidence.

### Safety properties that a fix must preserve

- No usable signature share, state transition with economic effect, or
  externally visible critical side effect may be authorized by one member or
  exposed before the required consensus finality.
- Operation, quote, intent, and evidence identifiers must be canonical and
  stable. Deterministic application must be idempotent under duplicate delivery,
  replay, restart, and catch-up.
- Payment side effects must have a designated, durable, replay-safe owner. A
  local database row or one payment backend's answer is not proof of a
  federation decision.
- Threshold aggregation requires compatible member views and cryptographically
  valid shares. A minority is diagnostic and cannot veto an honest quorum. Two
  incompatible payment quorums remain unresolved and fail closed.
- Startup, catch-up, DKG, rotation, roster, keyset, transcript, and federation
  configuration mismatches fail closed; they must never silently fall back to
  standalone operation.
- Wall-clock time, randomness, unordered collections, network availability, and
  local backend state cannot influence deterministic application unless the
  relevant value was committed by consensus.

### Cut-through federation policy

There is no deployed federation format to preserve. When a milestone changes
an experimental federation wire type, endpoint, configuration, database row,
migration, fixture, or test, update all federation producers and consumers
together and regenerate disposable artifacts. Do not add compatibility
adapters, dual reads/writes, legacy modes, or fallback paths. This permission
applies only to experimental federation surfaces; it does not permit changes to
public Cashu/NUT compatibility or standalone CDK behavior.

### Prohibited shortcuts

- No production key-provisioning shortcuts, fake shares, automatic ceremony
  finalization, test-only consensus in production paths, bypass flags, or
  weakening of authentication or replay protection.
- No result before finality, first-success acceptance where a threshold is
  required, or payment-backend calls during deterministic replay.
- Do not hide a liveness defect with unbounded queues, retry storms, arbitrary
  timeout increases, serialized tests, relaxed assertions, or unmeasured
  periodic consensus traffic.
- Do not introduce a new consensus or cryptographic dependency without an
  explicit recorded justification. Builds without federation must continue to
  work.
- Temporary instrumentation must be isolated and removed before the milestone
  is kept unless it becomes bounded, opt-in, documented production
  observability. Logs and fixtures must never contain secrets, private shares,
  seed material, credentials, personally identifying information, or local
  machine paths.

## Milestone execution protocol

Each ranked finding is one milestone. Its `Required correction` is a hypothesis
to test, not authorization to edit code before the bug is confirmed. Every
milestone must produce a fresh, executed pre-fix demonstration before any
production code is edited. Static source analysis, confidence labels, an old
incident report, or an existing test that was not run in the current worktree
is supporting evidence only.

The only accepted confirmation modes are:

1. a focused test harness run that deterministically exhibits the reported
   failure or violated invariant; or
2. a local multi-member federation deployment using fakewallet-backed mints
   that exhibits the failure, with the relevant member and federation state
   captured before teardown.

A harness may use controlled fault injection or a faithful backend test double
when that is necessary to reproduce Byzantine, crash, timeout, or payment
backend behavior. Merely compiling a test, asserting source structure, or
showing that a suspicious branch is reachable is not a demonstration. If
neither accepted mode can reproduce the issue unambiguously, the milestone must
be marked `Skipped — not confirmed`; production code must remain unchanged.

### Status vocabulary

Use exactly one of these milestone states:

- `Not started`
- `Confirming`
- `Skipped — not confirmed`
- `Confirmed`
- `Fixing`
- `Skipped — fix failed`
- `Evaluating`
- `Completed`
- `Skipped — blocked by scope or environment`
- `Reopened`

Update both the ranked table and the detailed milestone record whenever the
state changes. Record observations as they occur so an interrupted run remains
auditable. Do not start milestone N+1 until milestone N is `Completed` or one
of the `Skipped` states.

### Bootstrap before milestone 1

1. Read this entire document and the implementation plan.
2. Inspect the worktree and preserve all pre-existing or unrelated changes.
3. If this planning document is not yet committed, commit it by itself with a
   conventional documentation commit before changing production code. This is
   the baseline from which milestone status changes are audited.
4. Record the relevant feature set, test environment, and any unavailable
   dependency. A sandbox failure to bind `127.0.0.1:0` is environmental: run
   localhost-listener or real-network federation tests with the required
   elevated sandbox permission from the first attempt, rather than changing
   code or weakening the test.

### Gate 1 — confirm the bug before editing production code

1. Set the milestone to `Confirming` and read every cited code path, existing
   regression, and incident report relevant to the finding.
2. Before editing production code, run either a focused deterministic test
   harness or a local multi-member deployment of fakewallet-backed mints. The
   run must exhibit the exact failure or violated invariant on the unmodified
   pre-fix implementation and fail for the expected reason.
3. For crash, restart, network, Byzantine-member, wallet-request, payment-
   backend, or process failures, use bounded fault injection that reproduces the
   actual transition. The harness must exercise runtime behavior; a source-
   reading argument or code-shape assertion does not pass this gate.
4. Record the exact command, configuration, fault injection, result, observed
   member/federation state, code references, and why the result is unambiguous.
   Preserve the failing regression harness as part of a successful milestone.
   For a local fakewallet deployment, retain a deterministic automated
   regression derived from the reproduction whenever practical.
5. Only after this recorded pre-fix run succeeds at demonstrating the bug may
   the milestone become `Confirmed` and production editing begin.
6. If the claimed failure cannot be demonstrated, is already fixed, is flaky,
   or depends on an unavailable environment that cannot be replaced by a
   faithful bounded harness, make no production change. Set
   `Skipped — not confirmed`, record the attempted commands and evidence,
   commit the documentation/test result, and continue.

### Gate 2 — attempt one minimal federation-only fix

1. Set the milestone to `Confirmed`, then `Fixing`.
2. State the smallest plausible correction and the files it should touch before
   editing. Favor deletion, a local guard, bounded retry/backpressure, or reuse
   of an existing verified primitive over a new subsystem.
3. Make one bounded fix attempt that addresses only the confirmed mechanism.
   Do not expand the milestone to repair adjacent architecture, opportunistic
   findings, or standalone behavior.
4. Re-run the exact harness or local-deployment scenario that demonstrated the
   bug. This is the **fix check**, not the broader evaluation. The milestone
   must therefore contain direct red-before-green evidence from the same
   scenario. If it still fails, fails for a new reason, or requires a wider fix,
   revert only this milestone's changes without disturbing user or prior-
   milestone work. Set `Skipped — fix failed`, record why, commit the resulting
   milestone record and any useful non-production regression, and continue.

### Gate 3 — evaluate only a fix that passed its fix check

Set the milestone to `Evaluating` only after the confirmation regression passes.
Then run, in proportion to the change:

1. formatting and the narrow unit/integration test for the corrected path;
2. neighboring federation operation tests and negative/Byzantine cases;
3. restart, duplicate-delivery, replay, and catch-up tests for stateful changes;
4. adverse ordering, partial failure, minority-fault, bounded saturation, and
   cleanup tests where the finding involves distributed or worker behavior;
5. relevant standalone/no-federation build or test checks as read-only proof
   that standalone behavior remains unchanged; and
6. the affected real CLN, LND, BDK, or other production backend path when the
   defect depends on it. The fake wallet remains useful for ordinary local
   development but is not sufficient proof for a backend-specific milestone.

Record exact commands and outcomes, including tests not run and why. Never
weaken an assertion, increase a timeout, serialize tests, or suppress a failure
to obtain a pass. If evaluation fails, do not cascade into a larger repair:
revert the production fix, set `Skipped — fix failed`, record the failure, make
the milestone's documentation/test commit, and continue.

### Gate 4 — keep, document, and commit a successful milestone

A milestone is `Completed` only when the original failure is confirmed, the
minimal fix passes its fix check, and all required evaluation passes. Before
committing:

1. update the detailed record with confirmation evidence, fix rationale, files
   changed, commands/results, residual risks, and observations;
2. update the ranked-table status and any affected finding text;
3. include the regression, production fix, federation migration/fixture changes
   if any, and documentation in one coherent conventional commit;
4. stage only milestone-scoped files, inspect the staged diff, and leave
   unrelated worktree changes untouched; and
5. record the conventional commit subject in the milestone record. The commit
   hash remains discoverable from Git history; do not create a second commit
   merely to write a commit's own hash into this file.

Skipped milestones also end with one milestone-scoped commit recording the
attempt and evidence. Never combine two milestones in one commit, amend a prior
milestone with later work, or continue with uncommitted milestone changes.

## Milestone working-record requirements

The record directly below every numbered finding is the authoritative work log.
Keep it concise but evidence-bearing:

- **Confirmation:** the executed pre-fix harness or local fakewallet deployment,
  exact commands/configuration, injected fault, and salient output/member state;
  or the attempted runs and reason it was not confirmed.
- **Minimal fix:** hypothesis, changed files, and why it is the least complex
  correction.
- **Fix check and evaluation:** exact commands/results and anything not run.
- **Commit subject:** the milestone-specific conventional commit subject.
- **Observations:** unexpected behavior, residual risk, and scope decisions.

Do not rewrite failed attempts as if they did not happen. Append dated notes to
the milestone's observations when a later run reopens it.

## Discovered bugs

Record every distinct potential bug discovered while executing a milestone
here immediately, even if it is not fixed. Do not silently fold it into the
active milestone or expand the active fix.

For each discovered bug, append a section named `DB-NNN` containing:

- discovery date and originating milestone;
- status (`Observed`, `Confirmed`, `Queued`, `Resolved`, or `Not a bug`);
- proposed severity and confidence;
- faulty/Byzantine actor or backend precondition;
- observed behavior, violated invariant, exact code references, and evidence;
- why it is distinct from an existing finding;
- likely simplest federation-only correction and required proof; and
- proposed position in the milestone order.

Finish or skip the active milestone before starting the discovered bug. If the
new bug invalidates the active fix or presents an immediate critical safety
risk, revert/skip the active milestone, document that dependency, and promote
the discovered bug to the next milestone. Otherwise queue it by severity and
confidence after the existing ranked milestones. Renumbering must preserve the
legacy ID and discovery history.

No additional bugs have yet been recorded through this milestone workflow.

## Detailed findings in ranked order

### 1. A single peer's equal-height status is treated as halt authority (`F1`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with an executed five-member
federation harness. First, the pre-existing behavior harness was run unchanged:

`cargo test -p cdk-axum --features federation rest_peer_client_halts_on_equal_height_digest_mismatch -- --nocapture`

It passed (`1 passed`) by observing that one peer's incompatible equal-height
digest changed the honest member to `Halted` and stopped its live session and
inbound network. The harness was then converted into the required desired-
behavior regression with a five-member BFT configuration, one liar, and four
honest member states. The pre-fix command was:

`cargo test -p cdk-axum --features federation single_equal_height_digest_liar_does_not_halt_four_honest_members -- --nocapture`

It failed with exit code 101 at the first honest member: `one liar halted honest
member 1`. This is direct red evidence through `sync_once_from_member`; no
production code had been edited.

**Minimal fix:** Implemented one federation-only change in
`crates/cdk-axum/src/federation/sync_loop.rs`: retain the typed
`CatchUpDigestMismatch` error for peer isolation, reconnect backoff, and
diagnostics, but remove the terminal AlephBFT halt side effect from the
equal-height single-peer mismatch branch. The existing outer sync round already
continues to other members after this error. Only the sync loop, its federation
regression, and this ledger changed; no standalone path changed.

**Fix check and evaluation:** Exact fix check passed on 2026-07-18:

`cargo test -p cdk-axum --features federation single_equal_height_digest_liar_does_not_halt_four_honest_members -- --nocapture`

Result: `1 passed; 0 failed`. All four honest five-member states returned the
typed mismatch error while their AlephBFT live sessions and inbound networks
remained active, and a neighboring valid mint-quote operation was materialized
after each rejected liar response.

Evaluation passed:

- `cargo fmt -p cdk-axum -- --check` — passed. Stable rustfmt reported only the
  repository's existing warnings for nightly-only formatting options.
- `cargo test -p cdk-axum --features federation equal_height -- --nocapture` —
  `2 passed; 0 failed`.
- `cargo test -p cdk-axum --features federation peer_sync_round -- --nocapture`
  — `4 passed; 0 failed`, including failed-peer backoff and configured-peer
  catch-up.
- `cargo test -p cdk-axum --features federation` — `493 passed; 0 failed; 2
  ignored`, including five-member faults, restart/replay, checkpoint catch-up,
  and session lifecycle tests. It was run with localhost-listener permission.
- `cargo check -p cdk-axum --no-default-features` — passed, proving the
  standalone/no-federation build remains unchanged.

A local fakewallet deployment was not run because the accepted focused harness
directly exercised the faulty peer status, live-session teardown, five-member
BFT configuration, and neighboring progress without a payment-backend
precondition.

**Commit subject:** `fix: prevent single-peer digest mismatch halt`

**Milestone observations:** 2026-07-18 — Planning baseline committed as
`docs: add federation deadlock remediation milestones`. The existing runtime
harness `rest_peer_client_halts_on_equal_height_digest_mismatch` exercises
`sync_once_from_member` with one authenticated peer reporting an incompatible
equal-height digest while an AlephBFT live session is active. It was selected
for the mandatory pre-fix run. The five-member red regression confirms the
finding's equal-height attack.

The separate terminal branch after a successful checkpoint catch-up with no
operation or finalized-cursor progress was audited but not changed. Unlike the
confirmed path, it is downstream of a locally stored, signature-verified
checkpoint quorum and strict catch-up page validation. Empty/no-progress pages
are rejected earlier by `checked_checkpoint_page_cursors`; invalid source
history is rejected before mutation and another source can be used. The full
suite's `checkpoint_page_cursor_checks_reject_no_progress_and_overflow` and
`checkpoint_catch_up_rejects_bad_source_then_uses_second_source` tests passed.
No executed harness demonstrated that one peer can reach the later terminal
branch. It is therefore not claimed as a confirmed part of `F1` and was not
modified. If runtime evidence later demonstrates peer-only reachability, append
it as a discovered bug and give it an independent milestone.

**Severity:** Critical

**Confidence:** 99% — source-proven

**Finding status:** Resolved on 2026-07-18 with red-before-green runtime proof

**Attacker/fault:** One authenticated Byzantine peer, or one faulty peer status
implementation

At the planning baseline, `sync_once_from_member` compared the local order
status with one peer at a time. When operation counts were equal and the peer
reported another order digest, the receiver immediately called the full Aleph
halt helper:

- `crates/cdk-axum/src/federation/sync_loop.rs:1492-1509`
- `crates/cdk-axum/src/federation/sync_loop.rs:157-205` iterates every peer
  independently.

`validate_peer_status` checks version, federation ID, member ID, finalized
cursor presence, and roster shape, but cannot establish that the reported
count/digest is true:

- `crates/cdk-axum/src/federation/peer_client.rs:1159-1203`

That halt was not a diagnostic flag. It removed the live session and inbound
queue, incremented the halt epoch, marked the member `Halted`, and shut down the
supervisor:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1457-1475`

`Halted` is then explicitly excluded from quorum reconciliation:

- `crates/cdk-axum/src/federation/sync_loop.rs:1068-1078`

A Byzantine peer could therefore report the honest member's current operation
count with any other digest and halt it. The five-member pre-fix harness
demonstrated that authority against the first honest member.

The corrected branch now returns the typed `CatchUpDigestMismatch` without
calling the halt helper. The outer peer-sync round records the failed source,
applies bounded reconnect backoff, and continues its member loop. A minority
source remains diagnostic while the honest member's live consensus resources
stay available.

**Most likely cause when observed on a pre-fix build:** if the halt reason
contains `peer ... digest mismatch`, this path is definitive. In a non-
adversarial deployment, the trigger is more likely a stale/incorrect status
projection or prior divergence than a forged response.

**Correction and proof:** a single equal-height peer mismatch is now rejected as
source-local evidence and cannot halt the receiver. The retained five-member
regression injects one liar against all four honest member states, verifies each
live session and inbound network remains active, and verifies a neighboring
operation progresses. A terminal divergence decision remains reserved for
independently verified local or quorum evidence.

### 2. Semantically invalid Byzantine operations halt finalized application (`F2`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Critical

**Confidence:** 99% — source-proven and previously reported

**Finding status:** Open, source-proven and previously reported

**Attacker/fault:** One Byzantine member able to place application data in its
own Aleph unit

The current code correctly repairs the older structural variant: finalized
payload metadata is reconstructed from the authenticated Aleph creator,
session, round, and unit hash, while empty or oversized payloads are rejected:

- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:713-728`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1738-1748`

The semantic variant remains. Normal public/local submission calls the full
pure admission validator:

- `crates/cdk-axum/src/federation/operation_pipeline.rs:33-61`
- `crates/cdk/src/mint/federation.rs:649-703`

A member does not need that route to put data in its own Aleph unit. Finalized
classification checks the unit envelope and operation authorization, then marks
every unique authorized operation `Accepted`; it does not call the mint's pure
operation validator:

- `crates/cdk-common/src/federation/consensus.rs:1838-1940`

Journal preparation then runs `balance_audit()` for the entire finalized delta.
One malformed value operation aborts preparation instead of becoming a
per-operation rejection:

- `crates/cdk-common/src/federation/journal.rs:1135-1204`, especially line 1164
- `crates/cdk-common/src/federation/consensus.rs:2639-2656`

Concrete source-proven payloads include a swap/melt amount overflow or negative
balance audit, and stateful operations whose dependencies are invalid, such as
an unknown allocation, future chain-observation epoch, inconsistent on-chain
quote, or invalid transaction proposal. The apply paths return
`FederationError`, `InvalidPaymentRequest`, `Internal`, or `Custom` for many of
these cases:

- chain observations: `crates/cdk/src/mint/federation/chain_observation.rs:40-142`
- on-chain quote validation: `crates/cdk/src/mint/federation.rs:2011-2078`
- melt state traps: `crates/cdk/src/mint/federation.rs:2788-2814` and
  `crates/cdk/src/mint/federation.rs:2893-2945`

The definitive-failure classifier recognizes only two federation errors
(`InactiveFederatedKeyset` and `RejectedOperation`) as deterministic terminal
rejections; most federation/content errors, `InvalidPaymentRequest`, `Internal`,
`Payment`, `Database`, and `Custom` are ambiguous:

- `crates/cdk-common/src/error.rs:785-950`

An ambiguous apply error is persisted as `Failed` and returned. The finalized
unit task returns the error to its essential-task supervisor, which halts the
session:

- `crates/cdk-axum/src/federation/node_runtime.rs:124-194`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1729-1767`

Because all honest members finalize the same item, deterministic content errors
can halt all of them. This is an application-layer Byzantine-tolerance failure,
not failed Aleph finality.

**Most likely cause when observed:** all members halt at the same accepted
operation index and that operation is `Failed`, with a chain-observation,
transaction-policy, balance-audit, `InvalidPaymentRequest`, or saga-state error.

**Required correction:** run all pure deterministic validation during finalized
classification; persist a canonical per-item rejection; make untrusted content
errors definitive; continue with later items. Tests must put an invalid item
before a valid item in the same unit and another valid item in the next unit,
then prove live/restart/catch-up convergence.

References: [2026-07-17 security audit](federated-cashu-security-audit-2026-07-17.md#sec-2026-07-17-03--semantically-invalid-finalized-operations-halt-the-session)
and [July 13 review R2](federated-cashu-security-review-jul-13.md#r2-byzantine-finalized-aleph-unit-can-halt-honest-members).

### 3. Recovery-critical outgoing lookup ID is persisted after the side effect (`P4`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Critical safety / High liveness

**Confidence:** 99% — source-proven

**Finding status:** Open, source-proven

**Fault:** Database failure after an external payment call, especially Bolt12

The saga first claims `PaymentAttempted`, calls the external backend, and only
then tries to persist the returned lookup ID:

- `crates/cdk/src/mint/melt/melt_saga/mod.rs:526-550`

For Bolt12 this ID is explicitly documented as the only durable handle to the
in-flight payment, but the write is best-effort. Failure is logged and ignored:

- `crates/cdk/src/mint/melt/melt_saga/mod.rs:990-1049`

Startup recovery treats `PaymentAttempted` with no lookup ID as evidence that
payment was never sent and compensates/releases the saga:

- `crates/cdk/src/mint/start_up_check.rs:693-703`

The result is both a liveness dead end (the backend can no longer be polled) and
a safety failure (proofs can be released while the external payment later
settles).

**Required correction:** persist an idempotency/lookup key before the external
side effect, or require a backend API whose caller-supplied durable key can
always recover the attempt. A failed recovery-handle write must never authorize
compensation.

### 4. One member can reorg or quarantine accepted on-chain funds (`P5`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Critical

**Confidence:** 99% — source-proven; accidental variant previously reported

**Finding status:** Open, source-proven; accidental false-reorg variant previously
observed

**Attacker/fault:** One Byzantine chain observer, lagging Esplora/BDK source, or
inconsistent chain view

Observer identity is now correctly bound to operation authorization:

- `crates/cdk-common/src/federation/operation.rs:1801-1854`

But authorization is the only vote required for the fail-closed transition.
`apply_reorg` verifies that the outpoint, recorded block, and epoch match, then a
single observation sets the UTXO to `Reorged` and raises the liability alarm if
Cashu liability exists:

- `crates/cdk-common/src/federation/chain_observation.rs:760-812`

There is no chain proof or observer quorum. A Byzantine member knows the public
outpoint/block and can submit this operation for every federation UTXO.

A single conflicting deposit also permanently quarantines the UTXO. The first
observer fixes immutable txout/allocation/block fields; a later mismatch calls
`quarantine`:

- `crates/cdk-common/src/federation/chain_observation.rs:600-724`
- `crates/cdk-common/src/federation/chain_observation.rs:1073-1077`

A Byzantine observer can be first for a real public outpoint but claim an
incorrect value or block while retaining the correct allocated script. Honest
evidence then quarantines the output. `Quarantined` has no recovery epoch, and a
post-liability reorg also has no automatic recovery:

- `crates/cdk-common/src/federation/chain_observation.rs:415-433`

Pre-liability epoch recovery is implemented, but one attacker can repeat a
false reorg each time honest observations recover the next epoch. The normal
observer itself produces a reorg whenever a formerly observed output is absent
from its current local result or not reported in the same block:

- `crates/cdk-axum/src/federation/payment_observation.rs:182-228`

This explains the previously observed accidental version: a backend false
negative or lagging chain source has the same authority as a real reorg. See
[BDK reorg incident](federated-cashu-bdk-reorg-problem.md).

**Most likely cause when observed:** in an honest deployment, chain-source lag
or inconsistent results; under the stated Byzantine threat model, a deliberate
single-member veto.

**Required correction:** require threshold observations or independently
verified chain evidence for reorg/quarantine; make recovery authority symmetric
with failure authority; retain a sticky liability alarm for safety, but add a
quorum-authorized repair path.

### 5. Accepted melt author has a permanent payment veto (`P1`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 99% — source-proven

**Finding status:** Open, source-proven

**Attacker/fault:** Byzantine accepted author, author crash after finality, or
author payment backend permanently unavailable

For off-chain/custom melts, payment ownership is derived from the authorization
stored on the accepted journal envelope. Only that member may transition a
`SetupComplete` saga into an external payment attempt; non-authors only poll.
Failed or unpaid observations are also accepted only from the actor:

- `crates/cdk/src/mint/federation.rs:1359-1413`
- durable claim before external I/O:
  `crates/cdk/src/mint/melt/melt_saga/mod.rs:526-550`

There is no actor lease, failover, threshold takeover, or consensus-authorized
replacement. If the author stops after its operation wins final order, the
quote and proofs can remain pending forever even though a full consensus and
signature quorum remains online.

The author can be selected in two non-obvious ways:

1. The federated wallet fans the same melt request to the complete member
   roster (`crates/cdk/src/wallet/federation.rs:2181-2201`). Each member can
   authorize the same operation.
2. Operation IDs deliberately exclude authorization
   (`crates/cdk-common/src/federation/operation.rs:1905-1939`). The first ordered
   envelope for that operation ID therefore determines durable authorship. A
   Byzantine member can copy a valid operation body seen in gossip, authorize
   it itself, and try to be ordered first without changing the operation ID.

The historical static actor selector that chose an offline roster member was
fixed by using the accepted author. That fix removed the observed selection
bug, but it did not make the accepted author fail over. See
[performance plan Issue 6](federated-cashu-performance-and-issues.md#6-repeated-concurrent-melts-fail-or-stall).

**Most likely cause when observed:** consensus and quote status are healthy,
the saga remains `SetupComplete` or `PaymentAttempted`, and the journal envelope
author is offline, Byzantine, or cannot use the payment backend.

**Required correction:** introduce a durable, consensus-visible attempt lease
with a backend idempotency key fixed before external I/O. Rotate authority after
a bounded lease only when another member can safely query/continue the same
payment. Test author crash before claim, after claim, during backend I/O, and
after backend success.

### 6. One backend call or poison quote blocks the single observer worker (`P2`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 99% — source-proven

**Finding status:** Open, source-proven

**Fault:** Hung backend future, repeatable backend error, poison quote, or panic
inside a backend implementation

The observer scans quote IDs sequentially:

- mint loop: `crates/cdk-axum/src/federation/payment_observation.rs:234-263`
- on-chain, mint, then melt scans:
  `crates/cdk-axum/src/federation/payment_observation.rs:310-354`

The single spawned task awaits either an entire round or one event inline. It
cannot service payment events while a round is stuck:

- `crates/cdk-axum/src/federation/payment_observation.rs:357-394`

Backend calls have no outer timeout:

- incoming status: `crates/cdk/src/mint/federation.rs:1257-1266`
- outgoing attempt/status: `crates/cdk/src/mint/federation.rs:1377-1392`
- `make_payment` and status check:
  `crates/cdk/src/mint/melt/melt_saga/mod.rs:882-894` and
  `crates/cdk/src/mint/melt/melt_saga/mod.rs:1051-1068`

A hung call therefore blocks every later quote and all event repair on that
member. A persistent `Err` is also head-of-line blocking: `?` aborts the round,
the outer loop logs it, and the next round starts from the same ordering and can
fail on the same quote again. The quote-scoped active-payment guard prevents
duplicate local attempts, but its RAII lifetime includes the hung backend await
(`crates/cdk/src/mint/federation.rs:1195-1212` and `1368-1375`).

The worker's `JoinHandle` is discarded and it is not supervised. A backend
panic terminates observation until process restart.

One affected member is tolerable only if enough other members can observe the
same backend. A shared backend bug or enough independently hung members prevents
payment quorum and leaves all dependent proofs pending.

**Required correction:** per-call deadlines with ambiguity-preserving results,
per-quote bounded concurrency, error isolation so one quote cannot abort a
round, circuit-breaker/oldest-pending metrics, and worker supervision. A timeout
must never be translated into `Failed`; it must remain pending/unknown and be
polled through a durable idempotency handle.

### 7. DKG availability is all-member, not Byzantine-threshold (`S1`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High availability

**Confidence:** 99% — source-proven design limitation

**Finding status:** Open design limitation, source-proven

**Attacker/fault:** Any one offline or Byzantine configured member

The DKG driver waits for readiness acknowledgements from every configured
member, then reports exhaustion after a finite number of attempts:

- `crates/cdk-axum/src/federation/dkg_driver.rs:519-550`

The common ceremony state similarly requires all commitments/reveals, all
incoming secret-share contributions, all transcript signatures, and all
activations:

- `crates/cdk-common/src/federation/dkg_driver.rs:303-370`
- `crates/cdk-common/src/federation/dkg_driver.rs:448-524`

This is not an infinite internal loop—the run fails boundedly—but federation
setup or a required new ceremony cannot complete while one member withholds.
That is an explicit `n-of-n` availability boundary inside a system otherwise
designed to tolerate `f` faults.

**Required correction/design decision:** either document all-member DKG as an
operator-mediated setup requirement with roster reconfiguration, or implement
an exclusion/restart protocol whose final roster and threshold are agreed and
signed before activation.

### 8. Local BDK reconciliation is inside the fatal apply boundary (`F3`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 98% — source-proven

**Finding status:** Open, source-proven

**Fault:** Local BDK database/engine failure, incompatible accepted transaction,
or persistent local projection corruption

After journal entries become terminal and their application frontiers are
marked, every finalized-unit application reconciles the consensus on-chain
projection into member-local BDK state:

- `crates/cdk-axum/src/federation/operation_pipeline.rs:686-738`

Reconciliation advances local revealed counters and applies every accepted
signed transaction to each matching local BDK wallet:

- `crates/cdk-axum/src/lib.rs:3016-3073`

Every reconciliation error is erased to `cdk::Error::Custom`. `Custom` is
non-definitive and non-retryable, so the essential finalized-unit task exits and
the live session halts under finding 2's failure policy. The same reconciliation is
also required during startup:

- `crates/cdk-axum/src/lib.rs:2994-3005`
- `crates/cdk-axum/src/federation/runtime_components.rs:218-226`

This produces a particularly difficult recovery shape: the consensus business
entry may already be `Applied`, so replay skips that mutation, but local BDK
reconciliation runs again and can fail at the same point. A disk/BDK failure can
halt one member; a deterministic incompatibility in an accepted transaction can
halt every member.

**Most likely cause when observed:** one member halts immediately after an
on-chain allocation or signed transaction while peers remain healthy, and the
halt follows `accepted federation on-chain state could not be reconciled
locally`.

**Required correction:** preserve typed reconciliation errors; separate local
projection health from the consensus state-machine decision; provide bounded
retry/rebuild and a BDK-degraded state that blocks local signing without
misclassifying deterministic consensus history.

### 9. Melt-status recovery lets one invalid share poison an honest threshold (`W2`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High availability / Medium scope

**Confidence:** 98% — source-proven and previously reported

**Finding status:** Open, source-proven and previously reported

**Attacker/fault:** One Byzantine member returns matching metadata with malformed
or invalid BLS `c`

Normal melt response aggregation has the original blinded outputs and verifies
each member share before selecting a threshold:

- `crates/cdk/src/wallet/federation.rs:1541-1608`
- `crates/cdk/src/wallet/federation.rs:1338-1538`

Status recovery groups responses by metadata that deliberately excludes the
actual signature value, then parses and interpolates every `c` in the selected
group without share verification:

- grouping view: `crates/cdk/src/wallet/federation.rs:2387-2409` and
  `2447-2477`
- aggregation: `crates/cdk/src/wallet/federation.rs:1708-1821`

Thus one Byzantine response can join a group containing an honest threshold and
make parsing or final aggregation fail. The wallet does not discard that member
and retry a valid honest subset, so melt recovery remains a dead end even when
enough honest shares are available.

**Required correction:** durably retain/recover the original blinded change
outputs; verify each share against its public share; ignore invalid members;
continue collecting until a valid threshold is obtained.

Reference: [July 13 review R4](federated-cashu-security-review-jul-13.md#r4-melt-status-recovery-aggregates-unverified-bls-shares).

### 10. Apply and backup retries have no terminal budget (`F4`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 97% — source-proven

**Finding status:** Open, source-proven

**Fault:** A database that remains locked/busy or repeatedly returns
`ConcurrentUpdate`

The operation pipeline holds one global application mutex while it applies a
finalized unit:

- `crates/cdk-axum/src/federation/operation_pipeline.rs:577-582`

Journal replay retries recognized storage contention forever with exponential
backoff capped only by delay, not by attempts or elapsed time:

- `crates/cdk-axum/src/federation/node_runtime.rs:140-194`

The finalized-unit wrapper has another unbounded retry loop:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1772-1811`

Aleph backup persistence independently retries a locked/busy DB forever and can
block the backup writer expected by the live protocol:

- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:1272-1313`

Retryability includes explicit lock/concurrent variants and string matching for
SQLite `locked`/`busy` messages:

- `crates/cdk-axum/src/lib.rs:2091-2112`

Preserving order means the item must not be skipped, but an indefinitely held
application boundary lets finalized and network work accumulate without a
bounded degraded transition.

**Required correction:** add attempt/elapsed budgets, expose the blocked index
and oldest retry age, reject new backlog at a bound, and transition to an
operator-repairable degraded/halted state while retaining the exact item for
replay.

### 11. Exact evidence quorum is not backed by an enforced evidence model (`P3`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 97% — source-proven configuration gap

**Finding status:** Open, source-proven configuration/design gap

**Fault:** Independent payment backends, backend-specific proof/ID formats,
non-canonical result ordering, or honest members with different visibility

Every member probes its local payment processor using the quote's accepted
lookup ID:

- `crates/cdk/src/mint/federation.rs:1257-1308`

For a Lightning invoice created by one member's independent node, other members
normally cannot query that invoice. The proposer alone observes payment and the
configured member quorum never forms. Even with a shared backend, the probe
selects the last newly returned payment ID, making backend return ordering part
of consensus evidence (`crates/cdk/src/mint/federation.rs:1270-1296`).

Matching is exact, including amount, issued amount/fee reserve, payment ID, and
proof:

- mint: `crates/cdk-common/src/federation/operation.rs:339-350`
- melt: `crates/cdk-common/src/federation/operation.rs:659-670`

Apply leaves the quote unchanged until an exact group reaches the configured
count; competing threshold groups also remain unresolved:

- `crates/cdk/src/mint/federation.rs:1883-1925`
- `crates/cdk-common/src/federation/operation.rs:1118-1137`

Configuration validates only that the count is between the consensus threshold
and member count. It does not enforce shared read-only access, an origin proof,
a threshold gateway, or an on-chain proof:

- `crates/cdk-common/src/federation/config.rs:499-520`
- [payment trust model](federated-cashu-payment-trust-model.md#policy-surface)

An operator may also configure an `n-of-n` payment quorum, deliberately giving
one failed observer/backend a veto.

**Most likely cause when observed:** multiple accepted observation rows exist
for a quote but are split by payment ID/proof/amount, or only the quote creator
can see the invoice. Consensus stays healthy.

**Required correction:** commit and validate an actual evidence trust model;
canonicalize settlement identity independently of backend iteration order; and
reject configurations whose processors cannot share/verify the accepted lookup
evidence.

### 12. Recovery candidate fetch is pinned to the first source (`R1`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 97% — source-proven

**Finding status:** Open, source-proven

**Attacker/fault:** One Byzantine member among a quorum that honestly advertises
the same recovery candidate

Startup discovery requires enough peers to advertise one checkpoint ID, but
then chooses `peers.first()` as the sole source. Any fetch error, omitted
checkpoint, or summary mismatch returns without trying another advertising
peer:

- `crates/cdk-axum/src/federation/sync_loop.rs:438-523`

Peer ordering is stable, so the same Byzantine first source can withhold every
round even though the threshold group necessarily includes honest peers able to
serve the object.

**Required correction:** shuffle/rotate deterministically, try every advertising
source within a bounded round, penalize false advertisers, and persist only a
fully verified candidate.

### 13. Aleph queues lack bounded backpressure (`A1`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High security gate

**Confidence:** 97% — source-proven and previously reported

**Finding status:** Open security release gate, source-proven

Core channels are unbounded:

- inbound/outbound network pair:
  `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:152-184`
- authenticated inbound sender:
  `crates/cdk-axum/src/federation/aleph_bft_network.rs:80-86`
- finalized units:
  `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:1396-1400`
- essential-task and activity events:
  `crates/cdk-axum/src/federation/aleph_bft_session.rs:750-795`

An authenticated flood, blocked application lock, or stuck backup writer can
make memory growth independent of the bounded pending-operation mempool. OOM or
scheduler collapse can remove enough members to stop quorum.

**Required correction:** bounded queues and byte budgets, backpressure/fail-fast
semantics at private REST admission, fairness per member, and saturation tests
that do not drop already finalized work.

Reference: [security audit SEC-2026-07-17-05](federated-cashu-security-audit-2026-07-17.md#sec-2026-07-17-05--alephbft-channels-do-not-provide-bounded-backpressure).

### 14. Finalized-item catch-up response-count mismatch remains unexplained (`R4`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High investigation

**Confidence:** 95% that the failure exists; 55% in the current root-cause hypotheses

**Finding status:** Open investigation; process-reproduced

An offline member has rejected catch-up because reconstructed finalized items
produced a different number of journal entries than the response carried.
Recorded pairs include 6 versus 10, 0 versus 2, and 1 versus 2. One run stayed
unready for a 120-second bound; later clean-state runs retried and converged.

Finalized items and journal operations are not inherently one-to-one: duplicate,
invalid, payload-free, recovery, and non-operation items may advance the
finalized cursor without appending an operation. It remains unknown whether
page construction crosses incompatible unit/journal boundaries or the rejected
response is actually malformed.

Do not weaken the equality/digest checks without proving the intended mapping.
The required investigation and exact reproductions are recorded in
[Performance plan U8](federated-cashu-performance-and-issues.md#milestone-u8-validate-finalized-item-catch-up-response-count-mismatch)
and the [Nocean recovery report](federated-cashu-nocean-deadlock.md#cause-of-the-catch-up-mismatch).

### 15. Opaque Aleph backup can pin startup to unusable bytes (`R2`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High

**Confidence:** 85% — CDK source pinning is proven; upstream backup failure needs a regression

**Finding status:** Open high-confidence hazard

**Attacker/fault:** Authenticated peer serves size-bounded but semantically
unusable upstream Aleph backup bytes

Backup recovery downloads chunks from one member and validates only transfer
completion, byte count, and chunk count before writing them to the database:

- `crates/cdk-axum/src/federation/sync_loop.rs:1533-1609`

When `replace_existing` is false, any nonempty stored backup prevents another
fetch (`1540-1547`). Decode/session/DAG validation happens later inside the
upstream Aleph session. If those bytes fail there, the session can halt, while
the stored nonempty backup prevents ordinary alternate-source recovery.

The exact malformed-backup-to-upstream-failure behavior still needs a focused
test, so this is not labeled fully source-proven.

**Required proof/correction:** inject an unusable first source and valid second
source; validate in a disposable decoder before durable replacement; record
source provenance; and automatically retry another peer after startup failure.

### 16. Staged melt commits need complete crash/replay proof (`P7`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** High pending proof

**Confidence:** 75% — residual crash/replay proof obligation, not a reproduced defect

**Finding status:** Open proof obligation; high-confidence hazard, not a new reproduced
defect

Payment evidence, payment attempt, quote/saga mutation, and terminal journal
application intentionally cross multiple durable transactions. The broad
missing-rollback issue was fixed, but the remaining crash points are not all
covered by proof that replay reaches one identical terminal result without a
duplicate payment.

The highest-risk current seams are the paid observation requiring a pending saga
(`crates/cdk/src/mint/federation.rs:2788-2814`) and the accepted melt path
mapping unexpected terminal states to non-definitive `Internal`
(`crates/cdk/src/mint/federation.rs:2893-2945`). Those errors feed finding 2's
halt boundary.

**Required proof:** crash after every evidence, claim, backend result,
lookup-ID, quote, saga, proof-state, and application-state commit; restart and
catch up; assert no duplicate external payment and identical final state.

Reference: [code/security review P5](federated-cashu-code-and-security-review.md#p5-staged-federation-apply-transactions-need-replay-safety-verification).

### 17. Member metadata validation can wait forever (`W1`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Medium

**Confidence:** 99% — source-proven and previously reported

**Finding status:** Open, source-proven and previously reported

**Fault:** A Byzantine/hung connector placed before enough healthy connectors
under bounded fanout

`member_request_timeout` is documented as the per-member fanout timeout:

- `crates/cdk/src/wallet/federation.rs:64-82`

But active-keyset validation awaits `get_mint_info()` and `get_mint_keys()` with
no timeout:

- `crates/cdk/src/wallet/federation.rs:735-848`

With bounded concurrency, early hung calls occupy all slots and later healthy
members are never scheduled. Any wallet operation that first validates active
keysets can wait indefinitely despite a healthy threshold.

**Required correction:** apply one configured deadline to each metadata attempt,
count timeout as a member failure, and continue scheduling until a valid
threshold or the finite roster is exhausted.

The older claim that a cached threshold pins signing permanently is not current
for mint/melt/swap: those methods validate the cache as a gate but fan out the
actual operation to the full roster (`crates/cdk/src/wallet/federation.rs:2053-2074`,
`2181-2232`). It is therefore not listed as a separate open deadlock.

### 18. Broadcast worker has the same sequential backend head-of-line failure (`W3`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Medium

**Confidence:** 98% — source-proven

**Finding status:** Open, source-proven

**Fault:** One hung BDK/backend call, one persistent transaction error, or worker
panic

One task iterates every accepted signed transaction sequentially. Confirmation,
wallet readiness, and broadcast have no surrounding timeout; a returned error
aborts the round before later transactions:

- `crates/cdk-axum/src/federation/broadcast.rs:65-167`
- detached worker: `crates/cdk-axum/src/federation/broadcast.rs:188-210`

One member's failure is normally covered by other broadcasters, but a shared
BDK/chain backend issue or enough affected members leaves all later on-chain
payments pending. A panic permanently removes the detached worker until process
restart.

**Required correction:** per-transaction isolation and deadlines, fair cursors,
worker supervision, and explicit ambiguous-broadcast handling that always
rechecks the exact txid before retrying.

### 19. Unbounded pending-quote population amplifies wallet-request DoS (`P6`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Medium

**Confidence:** 97% — source-proven resource hazard

**Finding status:** Open, source-proven resource hazard

**Attacker/fault:** A wallet that creates many valid unpaid quotes

Pending quote capacity and expiry are explicitly not enforced:

- `docs/federated-cashu-payment-trust-model.md:127-132`

Every observer round loads all mint and melt quotes, filters them in memory, and
then probes qualifying records:

- `crates/cdk-axum/src/federation/payment_observation.rs:323-352`

Consensus mempool and waiter limits bound unfinalized submissions, but they do
not bound already accepted unpaid quote rows. A wallet can therefore create a
scan cost can starve all later payments and increase status/report size.
growing durable scan set. In combination with finding 6, an early poison quote
or high scan cost can starve all later payments and increase status/report size.
scan cost can starve all later payments and increase status/report size.

This is not yet a reproduced hard deadlock by itself. It is an externally
driven resource path that can create one.

**Required correction:** consensus-defined capacity/expiry or an indexed,
bounded work queue; per-wallet/rate admission; cursor-based fair scans; and
metrics for quote count, scan age, and oldest unresolved quote.

### 20. Retry queue is serviced only when fresh traffic is idle (`R3`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Medium

**Confidence:** 95% — source-proven and previously reported

**Finding status:** Open, source-proven and previously reported

The Aleph network pump drains up to its fresh-message budget first. It services
the retry queue only if `messages_drained == 0`:

- `crates/cdk-axum/src/federation/aleph_bft_network.rs:419-515`, especially
  `476-511`

Under sustained outbound traffic, failed deliveries can receive no service
until the bounded retry FIFO evicts them. A temporarily disconnected member can
then depend on unproven protocol rebroadcast/backup behavior to rejoin.

**Required correction:** reserve a retry budget every tick or use a fair queue;
retain hard bounds; expose oldest retry age and eviction count; test sustained
fresh traffic through partition/heal and restart.

Reference: [code/security review P4](federated-cashu-code-and-security-review.md#p4-aleph-network-retries-can-starve-restart-recovery-needs-an-explicit-contract).

### 21. Payment-observation submit readiness remains implicit (`A2`)

**Milestone status:** Not started

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** None yet

**Severity:** Medium

**Confidence:** 85% — policy bypass is proven; deadlock effect is indirect

**Finding status:** Open policy gap, source-proven

Payment observation submission calls the internal consensus path and does not
apply the normal public-submit readiness check:

- `crates/cdk-axum/src/lib.rs:1731-1749`

The normal observer currently performs an authoritative-read readiness check
before its scan, which narrows the reachable surface. Other local handler/event
paths and future reuse can still submit while a live session is Lagging or
CatchingUp. That can add stale evidence and consume mempool/waiter capacity at a
member already unable to serve wallet work. It also amplifies finding 2 if an
observation is valid structurally but invalid against the receiver's ordered
state.

This is not a standalone proven deadlock. The readiness contract must be made
explicit and rate-bounded. See
[code/security review P13](federated-cashu-code-and-security-review.md#p13-payment-observation-submit-bypasses-public-federation-readiness).

## Faulty-wallet threat boundary and non-findings

A malformed public wallet request does **not** by itself have the same power as
a Byzantine member. Honest member routes validate authorization, operation-local
structure, balances, keysets, quote terms, and transaction-specific rules before
putting an envelope into their local consensus mempool
(`crates/cdk-axum/src/federation/operation_pipeline.rs:33-61`). Duplicate spends
and conflicts are intended to become deterministic rejections.

The wallet-controlled mechanisms that remain are narrower:

- creating many valid unpaid quotes (finding 19);
- fanning the same melt to all members, allowing the first ordered author to
  become the permanent actor (finding 5);
- choosing/encountering a member whose backend can create a quote that other
  members cannot observe (finding 11); and
- cancelling an HTTP request after admission. Cancellation alone does not undo
  accepted consensus work; the operation remains durably pending/finalized.

A Byzantine member is more powerful because it can bypass its own public
admission path and place an authorized operation directly in its Aleph unit
(finding 2).

The FROST on-chain signing scheduler was also audited. Coordinator leases and
signer subsets rotate across the roster and the complete threshold-combination
space (`crates/cdk-common/src/federation/liveness.rs:106-141`), so one tolerated
Byzantine signer can delay attempts but is not a permanent signing veto. A
signer that withholds aggregate-result acknowledgement can make one coordinator
attempt fail (`crates/cdk-axum/src/federation/frost_signing.rs:165-183`), but a
later lease can select an honest signer set. This is a bounded DoS amplifier,
not currently classified as an open deadlock.

## Diagnostic priority for the next halt or stall

| Symptom | Most likely finding | First evidence to capture |
|---|---|---|
| All members halt at the same operation index | 2 (`F2`) | Finalized item, journal application state, exact typed apply error |
| A peer sync rejects an equal-height digest mismatch but the member stays live | Completed 1 (`F1`) | Signed peer statuses from the entire roster at the same frontier; confirm the source enters backoff |
| A member still halts with `peer ... digest mismatch` | Pre-fix build or regression of 1 (`F1`) | Build/commit identity, signed peer statuses, live-session teardown state |
| One member halts after an on-chain operation; peers continue | 8 (`F3`) | BDK reconciliation error and local BDK DB state |
| Session remains live but application index/backlog stops growing | 10 (`F4`) / 13 (`A1`) | Blocked operation index, DB lock age, queue depth/bytes |
| Melt proofs stay pending with healthy consensus | 5 (`P1`), then 6 (`P2`), 11 (`P3`), or 3 (`P4`) | Accepted envelope author, saga state, lookup ID, per-member backend observations |
| Mint quote is paid externally but never becomes paid in federation | 11 (`P3`), then 6 (`P2`) | Per-member lookup visibility and exact evidence group differences |
| UTXO changes to `Reorged`/`Quarantined` | 4 (`P5`) | First observation author, block/txout fields, every member's raw chain-source result |
| Restarted member never adopts a known checkpoint | 12 (`R1`), 15 (`R2`), or 14 (`R4`) | Advertising peers, selected source, transferred bytes, page cursors/counts |
| Wallet hangs before sending value operation | 17 (`W1`) | In-flight `get_mint_info`/`get_mint_keys` member calls and fanout slots |
| Melt succeeded but wallet cannot recover change | 9 (`W2`) | All raw member shares, selected metadata group, original blinded outputs |

## Previously reported items no longer open in current code

- The Nocean materialized-cache/consensus mutex inversion was removed. See
  [incident report](federated-cashu-nocean-deadlock.md).
- The FROST on-chain internal-settlement apply halt was corrected and coordinated
  recovery was verified. See
  [incident report](federated-cashu-frost-onchain-internal-settlement-halt.md).
- Structural Aleph payload impersonation was corrected by reconstructing unit
  metadata from authenticated Aleph context. Finding 2 is the remaining
  semantic variant.
- Payment/chain observer identity impersonation was corrected by binding claimed
  observer to operation authorization
  (`crates/cdk-common/src/federation/operation.rs:1827-1851`). Finding 4 remains
  because one correctly identified observer still has unilateral failure
  authority.
- Pre-liability reorg epoch recovery exists. Finding 4 narrows the open problem to
  unilateral/repeatable reorgs, permanent quarantine, and post-liability
  recovery.
- Mint quote provenance is now checked before mint payment and mint application
  by `require_prior_applied_mint_quote_in_entries`
  (`crates/cdk/src/mint/federation.rs:1840-1868`). The earlier provenance item is
  not open.
- The older catch-up-certificate missing-digest soft-skip was tied to a removed
  certificate architecture; the current code uses checkpoint candidates/quorums
  and no longer contains those certificate symbols. It is not carried forward
  as an open current-code bug.
- The federation-wide melt-observation mutex was replaced by a quote-scoped
  guard and durable CAS. Finding 6 is the remaining sequential-worker/backend-
  I/O head-of-line path, not the removed mutex.
- Session publication/teardown races, pending-operation session rebinding,
  false-ready non-retryable apply failure, waiter/pending persistence ordering,
  and restart revalidation of materialized-state mismatch are retained as
  regression history, not current open findings.

## Closure rule

Do not remove a finding merely because a patch exists. Close it only when:

1. the exact adversarial/fault precondition was first demonstrated by an
   executed pre-fix test harness or local federation of fakewallet-backed mints,
   and the same scenario is retained as a deterministic regression;
2. a valid neighboring operation progresses after the injected failure;
3. restart and catch-up preserve the result;
4. all relevant members converge on journal, finalized cursor, application
   frontier, order digest, and materialized-state digest; and
5. the owning incident, audit, security gate, or performance-plan status is
   updated with the same evidence; and
6. the milestone record and ranked table say `Completed`, and the scoped
   regression, fix, evaluation record, and documentation are present in that
   milestone's conventional commit.
