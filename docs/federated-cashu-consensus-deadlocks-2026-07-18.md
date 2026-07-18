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
| 2 | `F2` | Semantically invalid Byzantine operations halt finalized application | Critical | 99% | Session/global | Completed |
| 3 | `P4` | Recovery-critical outgoing lookup ID is persisted after the side effect | Critical safety / High liveness | 99% | Quote/proofs and possible external-payment double spend | Completed |
| 4 | `P5` | One member can reorg or quarantine accepted on-chain funds | Critical | 99% | On-chain quote/funds; all on-chain liquidity if repeated | Completed |
| 5 | `P1` | Accepted melt author has a permanent payment veto | High | 99% | Quote/proofs; malicious or failed author veto | Completed |
| 6 | `P2` | One backend call or poison quote blocks the single observer worker | High | 99% | All payment repair on one member; quorum loss if shared | Completed |
| 7 | `S1` | DKG availability is all-member, not Byzantine-threshold | High availability | 99% | Federation setup/key ceremony | Completed |
| 8 | `F3` | Local BDK reconciliation is inside the fatal apply boundary | High | 98% | Session/member; quorum loss if repeated | Confirming |
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
| 22 | `DB-001` | Stateful apply-time content errors remain ambiguous and halt the session | Critical | 99% | Session/global | Not started |
| 23 | `DB-002` | Payment backends do not uniformly honor a caller-supplied recovery key | Medium liveness | 95% | Quote liveness; standalone follow-up | Not started |
| 24 | `DB-003` | First observer can fix incorrect immutable deposit fields permanently | Critical | 99% | On-chain quote/funds | Not started |

Discovered bugs are appended after the existing ranked milestones in discovery
order with their legacy IDs preserved; `DB-001` was recorded on 2026-07-18
from milestone 2.

### Most likely common cause

The most important systemic cause is a mismatch between the Byzantine trust
boundary and the application failure policy:

> AlephBFT is allowed to order authenticated member data, but several later
> layers treat false member claims or invalid operation contents as proof that
> the local node is corrupt. They then halt the node instead of rejecting the
> member's item or isolating the member/source.

That pattern explains the pre-fix form of finding 1, directly explains the
confirmed pure-content mechanism of finding 2 (now resolved; its stateful
apply-time residual is `DB-001`), and amplifies finding 8. For unexplained
payment stalls while consensus stays healthy, findings 5, 6, and 11 are more
likely than an AlephBFT failure. For on-chain outputs becoming stuck, finding
4 is the most likely cause; the accidental version is a lagging/inconsistent
chain backend, while a Byzantine member can deliberately create the same
state.

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

### DB-001 — Stateful apply-time content errors remain ambiguous and halt the session

- **Discovery date and originating milestone:** 2026-07-18, milestone 2 (`F2`)
  confirmation.
- **Status:** Queued
- **Proposed severity and confidence:** Critical; 99% source-proven.
- **Faulty/Byzantine actor or backend precondition:** one Byzantine member
  places an authorized operation whose pure content is valid but whose ordered
  state dependencies are invalid in its own Aleph unit.
- **Observed behavior, violated invariant, exact code references, and
  evidence:** the milestone 2 confirmation harness proved that one invalid
  finalized operation aborts whole-unit application and halts honest members.
  The pure-content variant is addressed by milestone 2's classification fix.
  The stateful variant remains: apply paths return ambiguous (non-definitive)
  errors for deterministic untrusted-content failures, which persist `Failed`
  and halt the essential finalized-unit task. Cited paths: chain-observation
  deposit/trap errors (`crates/cdk/src/mint/federation/chain_observation.rs:40-142`,
  `FederationError::ChainObservationInvalid`), on-chain quote validation
  (`crates/cdk/src/mint/federation.rs:2011-2078`), and melt state traps
  (`crates/cdk/src/mint/federation.rs:2788-2814` returning `Custom`,
  `crates/cdk/src/mint/federation.rs:2893-2945` returning `Internal` and
  `InvalidPaymentRequest`). `Error::is_definitive_failure`
  (`crates/cdk-common/src/error.rs:785-950`) recognizes only
  `InactiveFederatedKeyset` and `RejectedOperation` among federation errors.
- **Why distinct from an existing finding:** milestone 2 covers pure
  operation-local validation moved into finalized classification. This bug is
  the remaining error-semantics surface: deterministic stateful content
  failures must become typed, definitive per-item rejections while genuine
  local corruption (`Internal`, `Custom`, `Database`) must stay halt-worthy.
  It requires its own confirmation harness per operation family and an audit
  of each `Custom`/`Internal` apply error at the consensus boundary.
- **Likely simplest federation-only correction and required proof:** replace
  free-form `Custom`/`Internal` errors at federation apply boundaries with
  typed content errors, extend the federation definitive-failure set to cover
  them, and prove with one harness per affected operation family that a
  Byzantine stateful-invalid operation is persisted as a canonical per-item
  rejection while later items and honest members continue.
- **Proposed position in the milestone order:** immediately after the current
  ranked milestones (position 22), preserving legacy IDs.

### DB-002 — Payment backends do not uniformly honor a caller-supplied recovery key

- **Discovery date and originating milestone:** 2026-07-18, milestone 3 (`P4`)
  confirmation.
- **Status:** Queued
- **Proposed severity and confidence:** Medium liveness; 95% source-proven.
- **Faulty/Byzantine actor or backend precondition:** a payment backend whose
  Bolt12/Custom `make_payment` does not treat the caller-supplied `quote_id`
  as a durable lookup key (fake wallet returns a random `CustomId`), or a
  crash after the external call before any secondary handle write.
- **Observed behavior, violated invariant, exact code references, and
  evidence:** milestone 3's fix persists `PaymentIdentifier::QuoteId` before
  the external side effect so a failed handle write can never authorize a
  payment or compensation. For backends that do not resolve `QuoteId`, an
  in-flight payment remains unobservable after a crash (liveness dead end,
  not a safety failure). The fake wallet's Bolt12/Custom `make_payment`
  returns `PaymentIdentifier::CustomId(Uuid::new_v4())`
  (`crates/cdk-fake-wallet/src/lib.rs:787-832`), while CLN already persists
  and echoes the caller's `quote_id` (`crates/cdk-cln/src/lib.rs:450-504`,
  `:774-778`) and the on-chain contract documents the echo
  (`crates/cdk-common/src/payment.rs:330-337`). The standalone
  (non-federation) melt payment flow has the same side-effect-first ordering
  question as the fixed federation branch; it is read-only under the
  milestone scope boundary.
- **Why distinct from an existing finding:** `P4` (milestone 3) fixes the
  mint-side ordering so safety never depends on a post-side-effect write.
  This bug tracks the remaining backend-contract and standalone-flow work,
  which cannot be done federation-only.
- **Likely simplest federation-only correction and required proof:** none —
  aligning backend behavior (fake wallet Bolt12/Custom echoing `QuoteId` as
  CLN does) and the standalone melt flow is shared-code work outside the
  federation scope boundary. Required proof if ever scheduled: per-backend
  crash-after-payment recovery tests showing the caller key always resolves
  the attempt.
- **Proposed position in the milestone order:** after `DB-001` (position 23);
  recorded for completeness and for upstream standalone CDK follow-up.

### DB-003 — First observer can fix incorrect immutable deposit fields permanently

- **Discovery date and originating milestone:** 2026-07-18, milestone 4 (`P5`)
  confirmation.
- **Status:** Queued
- **Proposed severity and confidence:** Critical; 99% source-proven and
  demonstrated.
- **Faulty/Byzantine actor or backend precondition:** one Byzantine observer
  is the first to report a real public outpoint but claims an incorrect value
  or block while keeping the correct allocated script.
- **Observed behavior, violated invariant, exact code references, and
  evidence:** the first deposit observation fixes the UTXO's immutable
  txout/allocation/block fields
  (`crates/cdk-common/src/federation/chain_observation.rs:635-658`); every
  later honest observation then mismatches and quarantines the deposit
  (`:660-669`), and `Quarantined` has no recovery epoch (`:415-435`).
  Executed pre-fix demonstration on 2026-07-18 (harness removed after the
  run): `first_lying_observer_cannot_brick_deposit_fields` FAILED with
  `left: Quarantined, right: Spendable` after three honest members reported
  the true fields against one first liar.
- **Why distinct from an existing finding:** milestone 4 fixes
  single-observer reorg authority and makes one-observer quarantines
  repairable by the honest quorum when the recorded fields are honest
  (liar-later). The liar-first case cannot be repaired that way because the
  recorded fields themselves are the liar's and honest evidence is never
  stored once conflicting
  (`FederationChainObservationState::validate` enforces evidence/field
  identity at `:958-1010`). Repair requires quorum-gated field establishment
  or quorum-authorized field adoption with conflicting-evidence retention —
  a different serialization/validation surface.
- **Likely simplest federation-only correction and required proof:** retain
  per-observer field votes (including conflicting evidence), fix fields only
  once `required_observers` agree on one set, and adopt a quorum-backed set
  over a minority-fixed one while keeping the recorded conflict diagnostic;
  post-liability remains sticky. Required proof: the removed harness above
  plus adversarial ordering (liar first/last, liar changing story, quorum
  splitting across two false sets) and restart/replay convergence.
- **Proposed position in the milestone order:** after `DB-002` (position 24).

No other additional bugs have yet been recorded through this milestone workflow.

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

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with an executed five-member BFT
federation harness (threshold 3, one Byzantine injector, four honest members)
run against the unmodified pre-fix worktree:

`cargo test -p cdk-axum --features federation byzantine_balance_overflow_operation_does_not_halt_finalized_application -- --nocapture`

The harness signs a structurally valid mint operation whose two outputs are
`u64::MAX` each (balance-audit overflow) with the Byzantine member's own key,
then queues it plus a valid mint quote operation atomically through
`FederationConsensusRuntime::submit_operation` on that member's own consensus
data provider. That is the exact Byzantine capability: public admission
(`submit_federation_envelope*`) validates and rejects the operation, but a
member can place application data in its own Aleph unit without admission. A
fourth valid operation was submitted by an honest member through the public
path for the next unit.

Pre-fix result: exit code 101, `FAILED` in 3-4s. The first honest member
halted with the recorded reason:

`AlephBFT live session 0 stopped after a non-retryable finalized-payload apply
failure: Federation operation balance audit amount overflow for mint`

This is direct red evidence of the full reported path: finalized
classification accepted the authorized operation, journal preparation aborted
the whole unit at `envelope.operation.balance_audit()?`
(`crates/cdk-common/src/federation/journal.rs:1164`), the non-retryable error
escaped the finalized-unit apply loop, and the essential-task supervisor
halted the session. The valid same-unit and next-unit operations never became
journal entries. No production code had been edited; only the regression test
in `crates/cdk-axum/src/federation/tests.rs` was added.

**Minimal fix:** One federation-only change in cdk-common. A new pure,
config-free operation validator `FederationOperation::validate_pure()`
(`crates/cdk-common/src/federation/operation.rs`) runs every validity rule
that depends only on canonical operation contents: per-payload `validate()`
for address allocations, chain observations, transaction proposals, and
broadcast intents/observations, plus the existing operation-local
`balance_audit()`. Checks requiring the effective keyset configuration (which
keyset rotation changes mid-session) or ordered journal state are
deliberately excluded and remain in admission and accepted-state apply.
Finalized classification
(`crates/cdk-common/src/federation/consensus.rs`,
`FederationAlephBftFinalizationHandler::is_invalid_finalized_operation`) now
assigns the pre-existing canonical per-item status
`RejectedInvalidOperation` to any operation failing member authorization or
`validate_pure()`, instead of letting it reach journal preparation. Rejected
items are already excluded from journal appends, persist as finalized items,
and replay identically; no standalone code, wire format, or error semantics
changed. This is the least complex correction: it reuses the existing
rejection status and downstream filtering, and removes the halt by moving
validation earlier rather than by reclassifying errors.

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact final
three-phase scenario. Red runs (production fix stashed,
`git stash push` of only `operation.rs` + `consensus.rs`):

`cargo test -p cdk-axum --features federation byzantine_balance_overflow_operation_does_not_halt_finalized_application -- --nocapture`

FAILED each time at the halt assertion in 10-11s with the same reason
(`AlephBFT live session 0 stopped after a non-retryable finalized-payload
apply failure: Federation operation balance audit amount overflow for mint`),
verified against the final harness after its synchronization hardening. Green
runs (fix restored): three consecutive `1 passed` (~32-41s each). The green
scenario proves, in one test: the invalid item is recorded
`RejectedInvalidOperation` on every member with no journal entry; the valid
same-unit mint quote applies on all online members; the valid next-unit
operation applies; no honest member halts and live sessions/inbound networks
stay active; a restarted member replays its durable state with the rejection
intact and applies a post-restart operation; the partitioned fifth member
catches up from peers, fails closed before catch-up, becomes ready, and
reproduces the same rejection, journal digests, and materialized quotes; all
five members converge on the same journal projection. A cdk-common
classification unit test
(`aleph_bft_finalization_handler_rejects_balance_overflow_operation_per_item`)
pins the same-unit/next-unit per-item statuses deterministically.

Broader evaluation on 2026-07-18, all against the post-fix worktree:

- `cargo fmt -p cdk-common -p cdk-axum -- --check` — stable rustfmt reported
  the repository's existing nightly-only option warnings plus three
  pre-existing unformatted regions in
  `crates/cdk-common/src/federation/operation.rs` (verified identical at the
  pristine milestone baseline by stashing all changes and re-running the
  check). The milestone's added code is format-clean; the pre-existing
  regions were reverted out of this milestone's diff to avoid unrelated
  cleanup.
- `cargo test -p cdk-common --features federation` — `422 passed; 0 failed`
  lib plus integration targets, including the new classification unit test and
  neighboring finalization, journal, catch-up, and duplicate/replay tests.
- `cargo test -p cdk-axum --features federation` — `494 passed; 0 failed; 2
  ignored` on each of the last two consecutive runs, including the new
  regression, five-member faults, below-finality conflict/recovery,
  restart/replay, checkpoint and suffix catch-up, session lifecycle, and
  private-route negative cases. One earlier pre-hardening run failed a single
  test (identity not captured); the only suite delta is the new regression,
  whose assertions originally raced per-member finalization lag and the
  submit-waiter timeout. The regression was synchronized (per-member terminal
  waits, tolerant submission helper), after which it passed three focused
  runs and both full-suite runs.
- `cargo check -p cdk-common --no-default-features` and `cargo check -p
  cdk-axum --no-default-features` — passed; the changed module is entirely
  `#[cfg(feature = "federation")]`-gated, so standalone/no-federation builds
  exclude it by construction and standalone behavior is unchanged.
- `cargo clippy -p cdk-common --features federation --all-targets -- -D
  warnings` — passed. `cargo clippy -p cdk -p cdk-axum --features federation
  --all-targets -- -D warnings` fails on 9 pre-existing lints inside `crates/cdk`
  sources (`mint/federation.rs`, `wallet/federation.rs`, `melt/`) that this
  milestone does not touch; none are in the changed files.

Not run: a local fakewallet multi-process deployment (the accepted focused
harness exercised the exact finalized-application boundary, five-member BFT
fault, restart replay, and peer catch-up without a payment-backend
precondition), and real CLN/LND/BDK backend paths (the defect is in the
consensus classification boundary, not backend-dependent).

**Commit subject:** `fix: reject semantically invalid finalized operations per item`

**Milestone observations:** 2026-07-18 — Milestone 1 is committed and the
worktree was clean before starting confirmation. Production code remains
untouched while the finalized-unit admission/application boundary and existing
Byzantine-operation harnesses are inspected.

2026-07-18 — Confirmation harness recorded above. Scope decision: the
confirmed mechanism is the pure-content variant (operation-local validation
such as `balance_audit()` aborting whole-unit finalized preparation). The
finding text also lists stateful apply-time traps (unknown allocation, melt
saga state, inconsistent on-chain quote) whose ambiguous error types
(`FederationError`, `InvalidPaymentRequest`, `Internal`, `Custom`) feed the
same halt boundary. Making those untrusted content errors definitive is a
separate error-semantics change; it is recorded as discovered bug `DB-001`
and queued after this milestone rather than folded into this fix.

2026-07-18 — Fix and evaluation complete; milestone kept. Observations during
the fix: (1) `verify_authorization` already binds claimed observer/proposer
identity to the envelope author at classification, so no additional identity
check was needed. (2) Keyset-version and rotation checks must stay out of
classification because keyset rotation changes the effective configuration
mid-session; misclassifying them as ambiguous apply errors is part of
`DB-001`. (3) The journal's retained `balance_audit()?` on the catch-up entry
path is defense-in-depth: honest journals can no longer contain pure-invalid
operations, and a forged peer journal fails with a typed catch-up error that
moves to another source (the milestone-1 audited behavior). (4) Public
admission does not run `balance_audit()` for mint/melt/swap requests, so an
honest member can still enqueue a pure-invalid operation that classification
later rejects; the impact is bounded by the pending-mempool limit and the
submitter's waiter timeout, and unifying admission with `validate_pure()` was
left out of scope as non-minimal. (5) The audit's required test "inconsistent
mint and melt quote operations are rejected without halting" is not covered by
this milestone; it depends on `DB-001`'s definiteness work. (6) Nine
pre-existing clippy lints in `crates/cdk` sources fail `-D warnings` with the
federation feature set; they are unrelated to this change and were left
untouched. (7) The first green regression version asserted finalized-item
statuses on members whose apply loop could still lag the Byzantine unit; one
focused run exposed that race. Assertions now synchronize each member on the
terminal status of every neighboring operation first, and the next-unit
submission uses the tolerant pending-timeout helper. Red evidence was
re-captured against the hardened harness, and all later focused and
full-suite runs are green.

**Severity:** Critical

**Confidence:** 99% — source-proven and previously reported

**Finding status:** Resolved for the confirmed pure-content mechanism on
2026-07-18 with red-before-green runtime proof; the remaining stateful
apply-time definiteness surface is tracked as discovered bug `DB-001`

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

The corrected classification now runs every pure, deterministic, operation-local
check (`FederationOperation::validate_pure()`: per-payload `validate()` rules
plus `balance_audit()`) when assigning the finalized item status, so untrusted
pure-invalid contents become a canonical per-item `RejectedInvalidOperation`
and the containing unit and later units continue. The retained five-member
regression injects one member-signed balance-overflow operation before a valid
operation in the same unit plus another valid operation in the next unit, and
proves per-item rejection, neighboring progress, session survival, journal
convergence, restart-replay preservation, and peer catch-up reproduction. A
cdk-common unit test pins the same classification statuses deterministically.
Deterministic apply-time business failures that still return ambiguous
`FederationError`, `InvalidPaymentRequest`, `Internal`, or `Custom` (for
example the chain-observation, on-chain-quote, and melt-saga traps above)
remain open as `DB-001`.

**Most likely cause when observed:** all members halt at the same accepted
operation index and that operation is `Failed`, with a chain-observation,
transaction-policy, balance-audit, `InvalidPaymentRequest`, or saga-state error.
On a post-fix build a balance-audit or other pure-content payload no longer
halts; a same-index halt with a stateful apply error indicates `DB-001`.

**Required correction:** run all pure deterministic validation during finalized
classification; persist a canonical per-item rejection; make untrusted content
errors definitive; continue with later items. Tests must put an invalid item
before a valid item in the same unit and another valid item in the next unit,
then prove live/restart/catch-up convergence. The first two clauses are
implemented and proven by this milestone; the definiteness clause is `DB-001`.

References: [2026-07-17 security audit](federated-cashu-security-audit-2026-07-17.md#sec-2026-07-17-03--semantically-invalid-finalized-operations-halt-the-session)
and [July 13 review R2](federated-cashu-security-review-jul-13.md#r2-byzantine-finalized-aleph-unit-can-halt-honest-members).

### 3. Recovery-critical outgoing lookup ID is persisted after the side effect (`P4`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with two executed focused harnesses
against the unmodified pre-fix worktree, using a file-backed SQLite mint
database and a SQLite trigger as the bounded fault injection
(`BEFORE UPDATE OF request_lookup_id ON melt_quote ... RAISE(ABORT, 'p4
injected lookup id write failure')`), plus a Bolt12 fake-wallet payment
processor whose returned lookup ID is a random backend-generated
`CustomId`:

`cargo test -p cdk --features federation,mint --lib federation_lookup_id_write_failure_never_authorizes_side_effect_or_compensation -- --nocapture`

FAILED with the full violation chain in one assertion message:

`P4 violated: external payment proceeded without a durable recovery handle
(payment status: Some(Paid), quote lookup id: None, saga after attempt:
Some(Melt(PaymentAttempted))); startup recovery then compensated (proof
states: [None, None, None], saga present: false)`

This exhibits the exact reported failure end to end on the unmodified code:
the saga claimed `PaymentAttempted`, the external payment settled (`Paid`),
the best-effort lookup-ID write failed and was ignored, and
`recover_from_incomplete_melt_sagas` treated the missing handle as
"payment never sent", releasing all three pending proofs and deleting the
saga while the external payment kept the funds. The companion regression
also failed pre-fix:

`cargo test -p cdk --features federation,mint --lib federation_lookup_id_is_durable_before_external_payment -- --nocapture`

FAILED: the durable handle recorded for the settled payment was a
backend-generated `CustomId(76fd9294-...)`, not a caller-supplied key
(`QuoteId(019f7658-...)`) recorded before the side effect. No production
code had been edited; only the test harnesses and a test-only
`cdk-sql-common` dev-dependency were added.

**Minimal fix:** One federation-only change in
`crates/cdk/src/mint/melt/melt_saga/mod.rs`
(`attempt_external_payment_for_federation_observation`, `#[cfg(feature =
"federation")]`). The claim transaction now atomically compares-and-sets
`SetupComplete -> PaymentAttempted` AND persists the caller-supplied
`PaymentIdentifier::QuoteId(quote.id)` as `quote.request_lookup_id` when the
quote has none (Bolt12/custom quotes; Bolt11 already carries its payment
hash). If that write fails, the whole claim rolls back and no external
payment is attempted, so a failed recovery-handle write can never authorize
a side effect or a later "never sent" compensation. The post-payment
best-effort write of the backend-native lookup id was removed from this
branch: the durable handle is now fixed before the side effect, and the
response's backend-native id remains observation evidence only.
`persist_pending_payment_lookup_id` is unchanged for its two standalone
callers. No standalone code or behavior changed; no backend, trait, schema,
or wire format changed. Contract-honoring backends (CLN) already resolve
`QuoteId` back to the attempt; the fake backend's remaining
contract-violation liveness gap is `DB-002`.

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact two
confirmation scenarios. Pre-fix runs (recorded above): both FAILED with the
violation evidence. Post-fix runs:

`cargo test -p cdk --features federation,mint --lib federation_lookup_id -- --nocapture`

`2 passed; 0 failed`. The trap scenario now fails closed at the
recovery-handle write (`p4 injected lookup id write failure`), rolls the
claim back to `SetupComplete`, performs no payment, and recovery's
`SetupComplete` compensation is then legitimate (proofs released, saga
deleted, quote unpaid). The durable-handle scenario now records
`PaymentIdentifier::QuoteId(quote.id)` before the side effect, settles
`Paid`, and recovery does not compensate (proofs stay pending, saga remains
`PaymentAttempted`).

Broader evaluation on 2026-07-18:

- `cargo fmt -p cdk -- --check` — clean for the milestone's code after
  formatting the new tests; no pre-existing lines churned.
- `cargo test -p cdk --features federation,mint --lib` — `775 passed; 0
  failed`, including both new harnesses and the full melt-saga test file.
- `cargo test -p cdk --lib` (default features: mint+wallet, no federation)
  — `540 passed; 0 failed`, proving standalone melt sagas unchanged.
- `cargo test -p cdk-axum --features federation` — `494 passed; 0 failed; 2
  ignored`, including below-finality melt recovery through the changed
  branch (exactly one payment actor, one backend call).
- `CDK_TEST_DB_TYPE=memory cargo test -p cdk-integration-tests --test
  wallet_saga -- --test-threads 1` — `9 passed; 0 failed`. An earlier
  invocation without `CDK_TEST_DB_TYPE` failed every test at the harness's
  environment check (`Database type set: NotPresent`); that is the documented
  harness requirement, not a regression.
- `cargo check -p cdk --no-default-features` — passed.
- `cargo clippy -p cdk --features federation --all-targets -- -D warnings` —
  fails only on the same 9 pre-existing lints recorded in milestone 2 (none
  in the changed code).

Not run: the CLN/LND regtest deployment. The defect is mint-side
claim/persist ordering, not backend-behavior-specific; the fake backend
faithfully models the contract-violating case (random native id, `Paid`),
and CLN's contract-honoring behavior (persists `quote_id -> payment_hash`,
echoes `PaymentIdentifier::QuoteId`) is source-verified at
`crates/cdk-cln/src/lib.rs:450-504` and `:774-778`.

**Commit subject:** `fix: persist federation melt recovery handle before payment`

**Milestone observations:** 2026-07-18 — Milestone 2 is committed
(`d1e560db9`) and the worktree was clean before starting confirmation.
Production code remains untouched while the melt-saga claim/persist ordering,
the startup-recovery compensation path, and the standalone/federation sharing
boundary are inspected.

2026-07-18 — Confirmation recorded above. Scope analysis: the vulnerable
flow is the federation-gated branch
(`MeltSaga::attempt_external_payment_for_federation_observation`,
`#[cfg(feature = "federation")]`), so a federation-only fix is possible
without touching standalone behavior. The CLN backend already treats the
caller-supplied `quote_id` as a durable recovery handle for Bolt12 (it
persists `quote_id -> payment_hash` and echoes `PaymentIdentifier::QuoteId`),
and the on-chain backend contract documents the same echo; the fake wallet
does not for Bolt12/Custom. Backend-contract alignment and the analogous
standalone-branch ordering question are out of scope here and recorded as
`DB-002`.

2026-07-18 — Fix and evaluation complete; milestone kept. Observations during
the fix: (1) the first fix iteration kept the post-payment best-effort write,
but it overwrote the caller key with the fake backend's random `CustomId`;
the write was removed from the federation branch instead, leaving
`persist_pending_payment_lookup_id` untouched for its two standalone callers
(lines 802/814). (2) With the claim-transaction handle, federation
`PaymentAttempted` sagas always carry a lookup id, so the startup-recovery
"no lookup id -> compensate" branch cannot fire for them; no shared
startup-recovery change was needed. (3) The actor's melt-payment observation
still carries the backend-native payment id, matching pre-fix behavior for
both fake and CLN backends; no observation-semantics change. (4) The
harness's SQLite trigger fault injection requires a file-backed database
(the in-memory pool is single-connection); the test-only `cdk-sql-common`
dev-dependency drives DDL through the public `Pool`/`query` helpers.

**Severity:** Critical safety / High liveness

**Confidence:** 99% — source-proven

**Finding status:** Resolved on 2026-07-18 with red-before-green runtime
proof; the remaining backend-contract and standalone-flow work is tracked as
discovered bug `DB-002`

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

The corrected federation claim transaction now persists the caller-supplied
`PaymentIdentifier::QuoteId(quote.id)` atomically with the
`SetupComplete -> PaymentAttempted` compare-and-set whenever the quote has no
lookup id. A failed handle write rolls the whole claim back before any external
payment, and a federation `PaymentAttempted` saga therefore always carries a
durable handle, so startup recovery's "never sent" compensation can no longer
fire for it. The retained harnesses prove both directions: with the write
fault-injected, the attempt fails closed, no payment occurs, and the later
`SetupComplete` compensation is legitimate; with a writable database, the
caller key is durable before the `Paid` response and recovery does not
compensate.

**Required correction:** persist an idempotency/lookup key before the external
side effect, or require a backend API whose caller-supplied durable key can
always recover the attempt. A failed recovery-handle write must never authorize
compensation. The first and third clauses are implemented and proven by this
milestone; uniform backend honoring of the caller key (fake wallet
Bolt12/Custom) and the analogous standalone-branch ordering are `DB-002`.

### 4. One member can reorg or quarantine accepted on-chain funds (`P5`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with three executed focused
harnesses against the unmodified pre-fix worktree, driving the exact cited
apply paths (`FederationChainObservationState::apply_deposit` /
`apply_reorg`) with the nocean incident's policy profile
(`required_observers = 4`, `minimum_confirmations = 1`):

`cargo test -p cdk-common --features federation --lib single_observer_reorg_does_not_reorg_or_block_credit_without_quorum -- --nocapture`

FAILED: `assertion failed: one observer must not reorg a quorum-established
UTXO — left: Reorged, right: Spendable`. A quorum-established, liability-
issued UTXO transitioned to `Reorged` (with the liability alarm raised and
quote credit blocked downstream) from a single observer's reorg report —
the same authority that the 2026-07-17 nocean incident reproduced
accidentally.

`cargo test -p cdk-common --features federation --lib single_conflicting_observer_cannot_permanently_quarantine_established_utxo -- --nocapture`

FAILED: `assertion failed: an honest quorum must repair a one-observer
quarantine — left: Quarantined, right: Spendable`. One conflicting deposit
(value mismatch) permanently quarantined a quorum-established UTXO; three
honest members resubmitting the true fields with deeper confirmations did
not recover it.

A third temporary harness (removed after the run, to be re-created as
`DB-003`'s own milestone regression) also FAILED pre-fix:
`first_lying_observer_cannot_brick_deposit_fields` — `left: Quarantined,
right: Spendable` — one first observer fixed incorrect immutable fields and
the honest quorum's true evidence only quarantined the deposit. No
production code had been edited; only the two retained harness tests in
`crates/cdk-common/src/federation/chain_observation.rs` were added.

**Minimal fix:** One federation-only change in
`crates/cdk-common/src/federation/chain_observation.rs` making failure and
recovery authority symmetric with spendability authority:

1. `apply_reorg` records reorg evidence but transitions to `Reorged` (and
   raises the post-liability alarm) only when distinct current-epoch reorg
   observers reach the UTXO's `required_observers`; minority evidence is
   diagnostic only.
2. `has_current_epoch_reorg` now reports that quorum (new helper
   `matching_current_epoch_reorg_observers`), so the deposit-path state
   recompute and recovery-epoch logic use the same threshold.
3. `utxo_is_spendable_at_or_before` blocks quote credit on a reorg-evidence
   quorum instead of on any single reorg report.
4. Pre-liability quarantines self-repair: after matching deposit evidence is
   inserted, a `Quarantined` UTXO with no issued liability whose matching
   confirmed observers reach `required_observers` recomputes state normally;
   post-liability quarantines stay sticky with the alarm.
5. `next_deposit_observation_epoch` keeps the current epoch for
   pre-liability quarantines so honest observers keep reporting (they
   previously stopped, making repair impossible).

No standalone code, backend, emission logic, wire format, or schema changed;
three existing cdk-common tests and two mint-level tests that encoded the
single-observer authority were updated to the corrected quorum semantics and
now also pin the minority-no-transition behavior. The liar-first variant
(incorrect immutable fields fixed before honest evidence exists) is `DB-003`.

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact two
retained confirmation scenarios. Pre-fix runs (recorded above): both FAILED
with the violation evidence. Post-fix:

`cargo test -p cdk-common --features federation --lib single_ -- --nocapture`

`2 passed; 0 failed` (`single_observer_reorg_does_not_reorg_or_block_credit_without_quorum`,
`single_conflicting_observer_cannot_permanently_quarantine_established_utxo`).
The reorg scenario proves one reorg report neither transitions nor alarms
nor blocks credit, while a full reorg quorum still fails closed; the
quarantine scenario proves one conflict still fails closed but three honest
members resubmitting the true fields repair the UTXO and restore credit.

Broader evaluation on 2026-07-18:

- `cargo fmt -p cdk-common -p cdk -- --check` — clean for the milestone's
  code; only the pre-existing `operation.rs` regions recorded in milestone 2
  remain (verified identical to the pristine baseline; a write-mode fmt run
  that churned them was reverted).
- `cargo test -p cdk-common --features federation` — `424 passed; 0 failed`,
  including both new harnesses and the three quorum-updated existing tests.
- `cargo test -p cdk --features federation,mint --lib` — `775 passed; 0
  failed`, including the two quorum-updated mint-level chain-observation
  tests (minority reorg no-transition, quorum transition, restart replay,
  materialized-state digest convergence).
- `cargo test -p cdk --lib` (default features, no federation) — `540
  passed; 0 failed`, standalone unchanged.
- `cargo test -p cdk-axum --features federation` — `494 passed; 0 failed; 2
  ignored` on the final run. One earlier run failed
  `aleph_bft_live_session_latency_probe_records_seven_member_bft_operation`
  (a latency timing assertion unrelated to chain observation); it passed
  standalone (7.69s) and in the final full run.
- `cargo check -p cdk-common --no-default-features` and `cargo check -p cdk
  --no-default-features` — passed.
- `cargo clippy -p cdk-common --features federation --all-targets -- -D
  warnings` — passed.

Not run: the multi-process BDK/Esplora regtest deployment. The defect and
fix live in the consensus chain-observation state machine (observer-quorum
authority at the apply boundary), independent of any chain backend; the
harnesses drive the exact apply paths that failed on nocean, and the
false-negative emission path (`payment_observation.rs`) is intentionally
unchanged because a single false emission is now harmless diagnostic
evidence.

**Commit subject:** `fix: require observer quorum for reorg and repair quarantine`

**Milestone observations:** 2026-07-18 — Milestone 3 is committed
(`2001841d0`) and the worktree was clean before starting confirmation.
Production code remains untouched while the reorg/quarantine authority
boundary, the normal observer's false-negative reorg path, and the BDK reorg
incident report are inspected.

2026-07-18 — Confirmation recorded above. Scope decision: the confirmed
mechanism for this milestone is (a) single-observer reorg authority
(transition, liability alarm, and the `utxo_is_spendable_at_or_before`
credit veto) and (b) permanent one-observer quarantine with no recovery
path. The liar-first variant — a Byzantine observer fixing incorrect
immutable fields before honest evidence exists — needs quorum-gated or
adoptive field establishment, a different evidence-model change with its own
serialization/validation surface; it is recorded as discovered bug `DB-003`
with the executed demonstration above and queued after this milestone.

2026-07-18 — Fix and evaluation complete; milestone kept. Observations
during the fix: (1) `reorg_observations` holds one evidence slot per member,
so a member re-reporting in a newer epoch replaces its older vote and each
epoch needs its own quorum — the updated repeat test pins this. (2) The
deposit recompute keeps `Reorged` while recovering below the matching
quorum, so epoch-advance recovery is unchanged. (3) Quarantine-on-conflict
was intentionally retained (fail-closed while truth is undecided); only the
recovery side changed for the pre-liability case. (4) Three pre-existing
cdk-common tests and two mint-level tests encoded the single-observer
authority and were updated to assert both the new minority-no-transition
invariant and the retained quorum fail-closed behavior — no assertion was
weakened toward the old authority. (5) The cdk-axum observer emission loops
needed no change: they consume `next_deposit_observation_epoch`, which now
keeps pre-liability quarantines reportable.

**Severity:** Critical

**Confidence:** 99% — source-proven; accidental variant previously reported

**Finding status:** Resolved for the confirmed reorg and established-UTXO
quarantine mechanisms on 2026-07-18 with red-before-green runtime proof; the
liar-first field-establishment variant is tracked as discovered bug `DB-003`

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

The corrected state machine now makes failure and recovery authority
symmetric with spendability authority: `Reorged` transitions, the liability
alarm, and the quote-credit reorg veto each require the UTXO's
`required_observers` in current-epoch reorg evidence, and pre-liability
quarantines self-repair once the matching honest quorum resubmits, while
conflicts still fail closed and post-liability states stay sticky. A single
false emission — Byzantine or accidental — is now diagnostic evidence only.
The retained regressions prove one report cannot reorg, alarm, block credit,
or permanently quarantine, that full quorums still fail closed, and that
restart replay and materialized digests converge.

**Most likely cause when observed:** in an honest deployment, chain-source lag
or inconsistent results; under the stated Byzantine threat model, a deliberate
single-member veto. On a post-fix build a single report can no longer reorg,
alarm, block credit, or permanently quarantine; a UTXO stuck `Quarantined`
after first observations points to `DB-003`.

**Required correction:** require threshold observations or independently
verified chain evidence for reorg/quarantine; make recovery authority symmetric
with failure authority; retain a sticky liability alarm for safety, but add a
quorum-authorized repair path. The quorum and symmetry clauses are implemented
and proven by this milestone; the liar-first field-establishment case is
`DB-003`, and a post-liability quorum-authorized manual repair path remains
future work recorded in `DB-003`'s correction sketch.

### 5. Accepted melt author has a permanent payment veto (`P1`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with an executed focused harness
against the unmodified pre-fix worktree, driving the exact cited probe path
(`Mint::probe_federation_melt_quote_payment`,
`crates/cdk/src/mint/federation.rs:1359-1413`) with a two-member runtime:

`cargo test -p cdk --features federation,mint --lib offline_melt_author_cannot_veto_payment_beyond_bounded_lease -- --nocapture`

FAILED with the veto evidence in one assertion message:

`the accepted author's offline veto left the quote pending: follower payment
calls 0, follower probe returned None`

The harness accepted a melt quote and a melt operation authored by member 1
on both members, then took member 1 (the accepted author) offline — it never
probes. The follower's consensus frontier was advanced 8 journal items past
the accepted melt. The follower — with a healthy fake payment backend and
the pending quote/saga fully materialized — made 0 backend calls and its
probe returned `None`: only the envelope author may transition the saga, and
no failover exists. The quote and proofs cannot reach a terminal state
despite a full payment-capable member being online, which is the reported
permanent veto. No production code had been edited; only the regression test
in `crates/cdk/src/mint/federation/tests/melt.rs` was added.

**Minimal fix:** One federation-only change in
`crates/cdk/src/mint/federation.rs`. A durable, consensus-visible attempt
lease replaces the permanent author veto:
`FEDERATION_MELT_PAYMENT_ACTOR_LEASE_ITEMS` (8 accepted journal items)
bounds the accepted envelope author's exclusive payment authority, and
`Mint::federation_melt_payment_lease_actor` derives the current actor from
consensus-ordered state only — the melt operation's journal index, the
member's journal tip, and the roster — so every member with the same
frontier computes the same single actor. `probe_federation_melt_quote_payment`
now treats the computed lease actor (not always the envelope author) as the
only member allowed to claim and attempt the external payment or report its
own outcome; all other members keep polling. Takeover is safe because the
caller-supplied idempotency key is durable before any external I/O
(milestone 3), Bolt11 attempts reuse the same invoice payment hash, and
contract-honoring backends deduplicate by the caller key. No standalone
code, wire format, schema, or config changed; on-chain FROST settlement and
the member-local active-payment guard are untouched.

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact two
retained scenarios. Pre-fix runs (production change stashed): both FAILED —
`offline_melt_author_cannot_veto_payment_beyond_bounded_lease` with the
recorded veto message, and
`lease_takeover_after_author_backend_success_keeps_matching_paid_evidence`
at its takeover expectation. Post-fix:

`cargo test -p cdk --features federation,mint --lib offline_melt_author_cannot_veto -- --nocapture`
`cargo test -p cdk --features federation,mint --lib lease_takeover_after_author_backend_success -- --nocapture`

Both `1 passed`. The first proves the author alone holds the first lease
epoch (follower probe `None`, zero attempts) and that after 8 journal items
the follower claims the lease, attempts the payment, and reports `Paid`
under the durable payment hash with its saga at `PaymentAttempted`. The
second proves a takeover after the author's successful backend call reports
matching `Paid` evidence under the same payment id instead of splitting the
observation quorum. The author-crash-after-claim and during-backend-I/O
points are locally equivalent to crash-before-claim because each member owns
its saga row and compare-and-set claim; they are covered by the same code
path.

Broader evaluation on 2026-07-18:

- `cargo fmt -p cdk -- --check` — clean for the milestone's code.
- `cargo test -p cdk --features federation,mint --lib` — `777 passed; 0
  failed`, including both new regressions and the pre-existing
  `accepted_melt_author_is_the_only_payment_actor` (author-only within the
  first lease epoch is pinned).
- `cargo test -p cdk --lib` (default features, no federation) — `540
  passed; 0 failed`, standalone unchanged.
- `cargo test -p cdk-axum --features federation` — `494 passed; 0 failed; 2
  ignored`, including the below-finality melt recovery test (exactly one
  payment actor and one backend call while the author completes within its
  lease epoch).
- `cargo check -p cdk --no-default-features` — passed.
- `cargo clippy -p cdk --features federation --all-targets -- -D warnings` —
  fails only on the same 9 pre-existing lints recorded in milestone 2 (none
  in the changed code).

Not run: the multi-process CLN/LND regtest deployment. The defect and fix
live in member-local actor election from consensus-ordered state; the
harnesses drive the exact probe path with a faithful backend double. A
bounded residual remains: if the author completes its backend payment but
dies before its observation is accepted, a later lease actor can pay the
same invoice externally a second time (the observation evidence still
matches on the payment id, and the quote finalizes once). This is the
inherent cross-node cost of lease-based takeover for Lightning and is
bounded by the lease; it is recorded in the finding text.

**Commit subject:** `fix: rotate federation melt payment authority after bounded lease`

**Milestone observations:** 2026-07-18 — Milestone 4 is committed
(`7a6960cca`) and the worktree was clean before starting confirmation.
Production code remains untouched while the accepted-author payment
ownership boundary, author-selection races, and the performance plan's
Issue 6 history are inspected.

2026-07-18 — Confirmation recorded above. (1) The lease is deliberately
measured in consensus-ordered journal positions, not wall time: deterministic
application cannot consume local clocks, and a fully idle federation has no
concurrent work competing for the same payment, so expiry on the next
journal item is sufficient. (2) The first fix iteration used the counting
backend double, which always returns `Pending`; the harnesses now use
fakewallet-backed mints whose invoice settles `Paid`, and red evidence was
re-captured against the final harness. (3) The takeover reuses the same
durable idempotency key, so takeover and author evidence match on payment
id for Bolt11 and for contract-honoring backends; mismatched fake/custom
backend ids remain finding 11's (`P3`) exact-matching surface, not a new
one. (4) The quiet-frontend stall trade-off and the bounded cross-node
double-pay residual for Lightning (author succeeds but dies before
observing) are recorded in the evaluation; no smaller safe variant exists
without a new consensus operation type, which would exceed a minimal fix.

**Severity:** High

**Confidence:** 99% — source-proven

**Finding status:** Resolved on 2026-07-18 with red-before-green runtime
proof for off-chain/custom melts; the bounded Lightning cross-node residual
is recorded in the milestone evaluation

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
after backend success. This is implemented and proven by this milestone:
`FEDERATION_MELT_PAYMENT_ACTOR_LEASE_ITEMS` (8 accepted journal items) bounds
the author's exclusive epoch, the actor rotates deterministically through the
roster from consensus-ordered state, and the takeover continues under the
durable caller-supplied key persisted before external I/O (milestone 3). The
retained regressions prove crash before claim (offline author vetoed, then the
follower paid under the lease) and crash after backend success (matching paid
evidence); crash after claim and during backend I/O share the same local
claim path. A bounded residual remains for Lightning: if the author completes
its backend payment but dies before its observation is accepted, a later lease
actor can pay the same invoice externally a second time; evidence still
matches on the payment id and the quote finalizes once.

### 6. One backend call or poison quote blocks the single observer worker (`P2`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with an executed focused harness
against the unmodified pre-fix worktree, driving the real
`FederationPaymentObserver::run_round` on a local-finality federation mint
state whose fake backend rejects every attempt and every status check for
the first pending melt quote (fake-invoice `pay_err` + `check_err`) while
settling the second quote normally:

`cargo test -p cdk-axum --features federation poisoned_melt_quote_cannot_starve_observer_round -- --nocapture`

FAILED with `poisoned quote must not abort the observer round: Internal`.
An instrumented run of the identical scenario recorded the full head-of-line
shape: `first=Err("Internal Error") second=Err("Internal Error")
healthy_observations=0` — two consecutive rounds aborted on the same
poisoned quote (`?` propagation out of the sequential melt scan), and the
healthy quote queued behind it collected zero paid observations. This is the
reported starvation: one poison quote blocks all payment repair on the
member, and if the backend fault is shared, payment quorum is lost. No
production code had been edited; only the regression test and fixture
helpers in `crates/cdk-axum/src/federation/tests.rs` were added.

**Minimal fix:** One federation-only change in
`crates/cdk-axum/src/federation/payment_observation.rs`:

1. Per-quote probe deadline and isolation: every mint/melt quote payment
   probe is wrapped in `tokio::time::timeout`
   (`FEDERATION_PAYMENT_OBSERVER_PROBE_DEADLINE`, 10s) and
   `AssertUnwindSafe::catch_unwind`. A backend error, deadline expiry, or
   panic is logged (bounded, per quote) and the quote is skipped pending —
   never translated into a terminal failure and always retried next round
   through its durable idempotency handle. Consensus-submission and local
   database errors on the round's own path still propagate for the outer
   loop's bounded retry.
2. Worker supervision: the spawned loop wraps each tick/event iteration in
   `catch_unwind`, so a panicking round or backend future is logged and the
   worker continues instead of dying silently until process restart.

No standalone code, probe semantics, wire format, or config changed; the
on-chain scan's existing readiness isolation and the quote-scoped
active-payment guard are untouched. Per-quote bounded concurrency,
circuit-breaker/oldest-pending metrics, and the remaining sequential
scan-cost surface are deferred (see milestone observations).

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact
confirmation scenario. Pre-fix run (recorded above): FAILED with the abort
and `healthy_observations=0`. Post-fix:

`cargo test -p cdk-axum --features federation poisoned_melt_quote_cannot_starve_observer_round -- --nocapture`

`1 passed`: the round isolates the poisoned quote (backend attempt and
status checks all rejected), still submits the healthy quote's paid
observation, and the poisoned quote remains `Pending` (never a terminal
failure).

Broader evaluation on 2026-07-18:

- `cargo fmt -p cdk-axum -- --check` — clean for the milestone's code.
- `cargo test -p cdk-axum --features federation poisoned_melt` and
  `catching_up_member_blocks` — both `1 passed`; the fail-closed readiness
  gate while catching up is unchanged.
- `cargo test -p cdk-axum --features federation` — `495 passed; 0 failed; 2
  ignored`, including the new regression and all neighboring observation,
  below-finality, and event-repair tests.
- `cargo check -p cdk-axum --no-default-features` — passed; the changed
  module is entirely federation-gated.
- `cargo clippy -p cdk-axum --features federation --all-targets -- -D
  warnings` — fails only on the same 9 pre-existing `crates/cdk` lints
  recorded in milestone 2 (none in the changed code).

Not run: a deliberately hung backend wall-clock demonstration (the deadline
is pinned structurally by the timeout wrapper and the error/panic isolation
is demonstrated; the CLN/LND regtest deployment is unchanged by this
member-local worker behavior).

**Commit subject:** `fix: isolate per-quote observer probe failures behind deadlines`

**Milestone observations:** 2026-07-18 — Milestone 5 is committed
(`0878cae32`) and the worktree was clean before starting confirmation.
Production code remains untouched while the observer worker's scan,
deadline, error-isolation, and supervision structure is inspected.

2026-07-18 — Confirmation recorded above.

2026-07-18 — Fix and evaluation complete; milestone kept. Observations
during the fix: (1) the harness's poison shape uses the fake invoice's
`pay_err` + `check_err`, which fails both the attempt and every later status
check — the finding's "repeatable backend error" shape; a single
`pay_err`-only quote only costs one aborted round, because the later poll
returns `Unknown`. (2) Consensus-submission and local database errors still
propagate to the outer loop's bounded retry rather than being skipped;
only per-quote probe failures, timeouts, and panics are isolated. (3)
Per-quote bounded concurrency (parallel scans), circuit-breaker and
oldest-pending metrics, and the growing scan-set cost of finding 19 (`P6`)
were left out as non-minimal; the sequential scan remains, now with
per-quote isolation. (4) The panicking-backend case is covered structurally
by `catch_unwind` at both the probe and the loop level; no separate panic
regression was added since a panicking backend double would exercise the
same isolation branch as the timeout.

**Severity:** High

**Confidence:** 99% — source-proven

**Finding status:** Resolved on 2026-07-18 with red-before-green runtime
proof; per-quote bounded concurrency and metrics remain future work as
recorded in the milestone record

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

The corrected observer now bounds every per-quote probe with a deadline and
`catch_unwind`, isolates a backend error, timeout, or panic to that quote
(which stays pending and is retried through its durable idempotency handle),
and supervises the worker loop with per-iteration panic recovery, so one
poison quote can no longer abort a round, starve later quotes, or kill the
worker. The retained regression proves two consecutive aborted rounds and
zero healthy observations pre-fix become one isolated skip, a submitted
healthy paid observation, and a pending poisoned quote post-fix.

**Required correction:** per-call deadlines with ambiguity-preserving results,
per-quote bounded concurrency, error isolation so one quote cannot abort a
round, circuit-breaker/oldest-pending metrics, and worker supervision. A timeout
must never be translated into `Failed`; it must remain pending/unknown and be
polled through a durable idempotency handle. The deadline, ambiguity-preserving
skip, per-quote error isolation, and worker supervision clauses are implemented
and proven by this milestone; bounded concurrency and metrics remain future
work as recorded in the milestone record.

### 7. DKG availability is all-member, not Byzantine-threshold (`S1`)

**Milestone status:** Completed

**Confirmation:** Confirmed on 2026-07-18 with an executed three-member DKG
harness against the unmodified pre-fix worktree, using the repository's
authenticated-route driver federation with member 3's route replaced by a
503 fallback:

`cargo test -p cdk-axum --features federation dkg_cannot_complete_while_one_configured_member_withholds -- --nocapture`

The run exhibits the reported boundary end to end on the unmodified code:

1. `wait_for_federation_dkg_readiness` (called directly with the mock
   transport client, `max_ticks: 3`, `retry_delay: 10ms`) returned
   `federation DKG readiness exhausted after 3 attempts with 2/3 members
   ready` — the all-member readiness barrier fails boundedly with two of
   three members acknowledging and one withholding.
2. The error message carries the counts but no operator remedy; the
   regression fails at `readiness error must name the operator roster
   remedy`, because no actionable guidance exists.
3. Driving every tick for the two online members for four rounds never
   reached `Complete`, and each tick's progress stalled below the
   all-member counts (`commitment_count 2 < member_count 3`), confirming the
   ceremony phases themselves require all configured members.

No production code had been edited; only the regression test in
`crates/cdk-axum/src/federation/tests/dkg_driver.rs` was added
(`wait_for_federation_dkg_readiness` was made `pub(crate)` so the harness
could exercise the exact barrier with the mock transport).

**Minimal fix:** The finding's documentation correction, not the
exclusion/restart protocol (a new consensus-visible roster-change subsystem,
explicitly not minimal). Two parts: (1) the bounded readiness exhaustion
error in `crates/cdk-axum/src/federation/dkg_driver.rs` now carries the
operator remedy — DKG requires all configured roster members for the entire
ceremony, and recovery is operator roster reconfiguration plus a new
ceremony with a new ceremony id, not indefinite retry; (2) a new operator
section "All-member availability is an operator-mediated setup requirement"
in `docs/federated-cashu-dkg-implementation-plan.md` documenting the
requirement, why it is inherent to the protocol, the recovery procedure
(drain failure, agree/sign a replacement roster, fresh ceremony id, no
roster reuse across ceremonies), and that the signed in-band
exclusion/restart protocol remains future work. The all-member ceremony
design is unchanged. `wait_for_federation_dkg_readiness` was made
`pub(crate)` so the harness exercises the exact barrier.

**Fix check and evaluation:** Fix check on 2026-07-18 with the exact
confirmation scenario. Pre-fix run (recorded above): FAILED at the missing
remedy guidance while the boundary behavior was demonstrated. Post-fix:

`cargo test -p cdk-axum --features federation dkg_cannot_complete_while_one_configured_member_withholds -- --nocapture`

`1 passed`: the readiness barrier still fails boundedly at `2/3` with no
completion path for the online members, and the error now names the roster
remedy. The all-member boundary is pinned as intended behavior, not
weakened.

Broader evaluation on 2026-07-18:

- `cargo fmt -p cdk-axum -- --check` — clean.
- `cargo test -p cdk-axum --features federation dkg_` — `57 passed; 0
  failed`, including the neighboring readiness, restart-at-every-phase,
  partition-resume, and timeout tests.
- `cargo test -p cdk-axum --features federation` — `496 passed; 0 failed; 2
  ignored`.
- `cargo check -p cdk-axum --no-default-features` — passed; the changed
  code is entirely federation-gated.

Not run: a live multi-process DKG ceremony (the accepted harness drives the
real readiness barrier and tick state machine over authenticated routes;
the finding's confidence was already source-proven and the correction is
documentation plus error text).

**Commit subject:** `fix: surface all-member DKG roster remedy in readiness errors`

**Milestone observations:** 2026-07-18 — Milestone 6 is committed
(`a105c4582`) and the worktree was clean before starting confirmation.
Production code remains untouched while the DKG readiness gate, ceremony
state machine, and exhaustion behavior are inspected.

2026-07-18 — Confirmation recorded above. Correction choice: the finding
explicitly offers two corrections — document the all-member requirement as
an operator-mediated setup requirement with roster reconfiguration, or
implement a signed exclusion/restart protocol. The exclusion/restart
protocol is a new consensus-visible roster-change subsystem and is not a
minimal fix; the documentation-plus-actionable-error correction is attempted
first, as the finding allows. The all-member ceremony design itself is
retained (a DKG cannot produce a member's key share without that member's
contribution, so the boundary is inherent to the protocol, not an
implementation accident).

2026-07-18 — Fix and evaluation complete; milestone kept. The production
`run_federation_dkg_driver` builds its own real network transport, so the
harness drives `wait_for_federation_dkg_readiness` directly with the mock
transport client (newly `pub(crate)`) and the tick loop through the existing
driver-federation harness. The pinned regression now guards both the
bounded all-member failure and the presence of the operator remedy.

2026-07-18 — Follow-up to the same finding: mid-ceremony member loss now
aborts the other members' drivers with a bounded, actionable timeout. The
run config gained an overall `ceremony_timeout` (600s production default,
small values in tests), and ending the bounded tick loop without completion
now returns an error naming the phase and the members still awaited
(`missing_federation_dkg_members_for_phase`) with the roster remedy,
logged as a clean abort, instead of returning an ambiguous incomplete
outcome. The loop was extracted into
`run_federation_dkg_driver_loop_with_client` so the harness drives it with
the mock transport;
`dkg_mid_ceremony_member_loss_aborts_others_with_timeout_naming_missing_members`
pins the abort with a 100ms test timeout. The all-member requirement and
the documented recovery procedure are unchanged.

**Severity:** High availability

**Confidence:** 99% — source-proven design limitation

**Finding status:** Addressed as documented operator requirement on
2026-07-18 with runtime confirmation; the signed exclusion/restart protocol
remains future work as recorded in the milestone record

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

The boundary is now confirmed by an executed harness and documented as an
operator-mediated setup requirement: the bounded readiness exhaustion error
names the remedy (operator roster reconfiguration plus a fresh ceremony id,
not indefinite retry), and
`docs/federated-cashu-dkg-implementation-plan.md` carries the operational
requirement and recovery procedure. The signed in-band exclusion/restart
protocol remains future work; the retained regression pins both the bounded
all-member failure and the presence of the remedy.

**Required correction/design decision:** either document all-member DKG as an
operator-mediated setup requirement with roster reconfiguration, or implement
an exclusion/restart protocol whose final roster and threshold are agreed and
signed before activation. The first option is implemented and proven by this
milestone; the second remains future work.

### 8. Local BDK reconciliation is inside the fatal apply boundary (`F3`)

**Milestone status:** Confirming

**Confirmation:** Pending

**Minimal fix:** Not attempted

**Fix check and evaluation:** Not run

**Commit subject:** Not assigned

**Milestone observations:** 2026-07-18 — Milestone 7 is committed
(`16d8e3842`) and the worktree was clean before starting confirmation.
Production code remains untouched while the reconciliation call sites,
error erasure, and startup recovery shape are inspected.

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
| All members halt at the same operation index | `DB-001` (pre-fix builds: 2 (`F2`)) | Finalized item, journal application state, exact typed apply error |
| A peer sync rejects an equal-height digest mismatch but the member stays live | Completed 1 (`F1`) | Signed peer statuses from the entire roster at the same frontier; confirm the source enters backoff |
| A member still halts with `peer ... digest mismatch` | Pre-fix build or regression of 1 (`F1`) | Build/commit identity, signed peer statuses, live-session teardown state |
| One member halts after an on-chain operation; peers continue | 8 (`F3`) | BDK reconciliation error and local BDK DB state |
| Session remains live but application index/backlog stops growing | 10 (`F4`) / 13 (`A1`) | Blocked operation index, DB lock age, queue depth/bytes |
| Melt proofs stay pending with healthy consensus | 11 (`P3`), 19 (`P6`), or `DB-002`; pre-fix builds: 5 (`P1`), 6 (`P2`) | Accepted envelope author, lease epoch vs journal tip, saga state, lookup ID, per-member backend observations |
| Mint quote is paid externally but never becomes paid in federation | 11 (`P3`), then 6 (`P2`) | Per-member lookup visibility and exact evidence group differences |
| UTXO changes to `Reorged`/`Quarantined` | Post-fix: quorum evidence or 24 (`DB-003`); pre-fix builds: 4 (`P5`) | Every member's reorg/deposit evidence, first observation author, block/txout fields, every member's raw chain-source result |
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
