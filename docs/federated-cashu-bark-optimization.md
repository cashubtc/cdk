# Federated Cashu Bark BOLT11 optimization implementation plan

Status: Milestone 0 in progress; production baselines recorded; no optimization
milestone implemented

First recorded: 2026-07-20

Plan contract established: 2026-07-21

## Required project context

Before starting any milestone, read
[`federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md).
This Bark plan is subordinate to that federation plan and inherits its project
vision, architecture boundaries, safety invariants, cut-through policy, and
development discipline. If the two documents conflict, the federation plan
governs.

Apply those priorities here: preserve consensus-before-side-effects and
threshold authority; preserve public Cashu compatibility and standalone CDK
behavior; reuse existing CDK domain logic while keeping federation
orchestration thin; and prefer deleting measured work or delay over adding new
machinery. Each change must remain narrow, evidenced, tested, documented, and
independently committed.

## Purpose

This is the living implementation plan, evidence record, and development
contract for making incoming and outgoing Bark BOLT11 payments faster and more
efficient in federated CDK.

The goal is not merely to make a favorable trace look faster. The goal is to
reduce the actual work and waiting in the normal payment lifecycle while
preserving federation payment authority, threshold custody, replay, recovery,
and standalone CDK behavior. Every retained optimization must identify a
measured cause, change the smallest owning component, and demonstrate the
predicted improvement with an equivalent before/after test.

Simple solutions are preferred. Remove unnecessary operations, distinctions,
polls, timers, and branches before adding concurrency, caches, queues, tuning
knobs, or new protocols. A milestone that does not produce its predicted
improvement is rejected and its production changes are reverted.

Related documents:

- `docs/federated-cdk-performance-optimizations.md`
- `docs/federated-cashu-bark-consensus-review-2026-07-21.md`
- `docs/federated-cashu-bark-frost-implementation-plan.md`
- `docs/federated-bark-outgoing-lightning-failure-2026-07-20.md`

## Goal and scope

This plan covers the complete federated Bark BOLT11 lifecycle:

- incoming invoice creation, payment detection, HTLC evidence, threshold claim,
  Cashu quote payment, and mint issuance;
- outgoing melt planning, threshold HTLC construction, payment initiation,
  success or safe failure/revocation, and Cashu melt finalization;
- wallet request fan-out and duplicate submission behavior;
- Ark polling, Bark mailbox or notification wakes, retry and repair behavior;
- accepted federation operation count, duplicate finalized items, AlephBFT
  latency, authenticated FROST traffic, Ark RPC volume, and member CPU;
- restart, replay, checkpoint restore, member loss, ambiguous payment state,
  expired work, and terminal cleanup; and
- focused local harnesses, the local `cdk-cli`, the five-member Nocean
  deployment, and real Signet payment demonstrations.

The first optimization release must not:

- weaken distinct-member evidence or make an actor's Bark database authoritative;
- merge consensus boundaries merely because two operations are adjacent;
- parse Ark rate-limit strings or introduce backend-specific retry policy into
  the generic federation observer;
- remove polling repair before an event path has proven complete restart and
  disconnect behavior;
- tune AlephBFT globally before avoidable serial operations and duplicate
  submissions have been removed; or
- change ordinary standalone mint or wallet behavior.

## Current evidence

### Production incoming baseline A: completed 10,123-sat receive

The latest fully measured successful receive took approximately 101 seconds
from durable quote creation to Cashu issuance. Approximately 58 seconds elapsed
after the first post-payment federation activity became visible. No Ark
rate-limit error occurred in this trace, so its post-payment delay was primarily
inside federation orchestration rather than Lightning routing or server
throttling.

All times below are UTC. Consensus times are bounded by the ten-second
AlephBFT status-report cadence when no more precise record exists.

| Stage | Time | Elapsed from quote creation | Evidence |
| --- | ---: | ---: | --- |
| Quote recorded | 18:18:19 | 0s | Durable `mint_quote.created_time` |
| Setup operations finalized | by 18:18:22 | about 3s | Receive intent, receive preparation, and mint quote ordered |
| First post-payment consensus activity | about 18:19:02 | about 43s | AlephBFT activity after an idle interval |
| HTLC evidence quorum and ready transition | by 18:19:12 | about 53s | Five matching observations plus transition |
| Ark claim started | 18:19:32.792 | about 74s | Bark receive log |
| Actor Bark database settled | 18:19:32.974 | about 74s | Durable Bark settlement row |
| Other Bark projections settled | 18:19:38.597-18:19:38.627 | about 80s | Four member-local records |
| Bark result quorum | by about 18:19:42 | about 84s | Matching succeeded observations |
| Generic mint-payment quorum | by about 18:19:52 | about 94s | Matching `MintQuotePayment` observations |
| Cashu signatures issued | 18:20:00 | 101s | Durable issuance and blind signatures |

The exact payer settlement time is absent. The approximately 40-second interval
between quote setup and new consensus activity cannot yet be divided reliably
between payer delay and backed-off external discovery. Milestone 0 must make
that boundary observable before any polling change is evaluated.

This receive created 22 accepted operations from invoice setup through mint:

- receive intent, receive preparation, and mint quote: 3;
- HTLC-ready member observations: 5;
- ready, claim-plan, preimage-disclosure, and claimed transitions: 4;
- claimed-result member observations: 5;
- generic mint-payment observations: 4; and
- mint issuance: 1.

Four additional finalized units carried the identical mint operation because
all five wallet requests submitted it. They did not create extra accepted
journal entries, but they still consumed consensus and network work.

### Production incoming baseline B: paid 50,000-sat quote without issuance

A second production trace separates successful Bark settlement from wallet
issuance behavior:

- the quote was recorded at 19:27:25 UTC;
- a wallet quote-status request at 19:28:15 triggered the actor's claim;
- Bark completed the external claim RPC and received the Ark output in about
  170 milliseconds;
- the accepted journal contains all five HTLC-ready observations, the ready
  transition, claim plan, preimage disclosure, claim completion, all five
  claimed-result observations, and four matching generic paid observations;
- all five public members now return the quote as `PAID` for 50,000 sats; and
- the quote has zero issued value, zero blind signatures, and no accepted mint
  operation referencing it.

This trace does not show a backend failure. It shows that the Bark receive and
payment-quorum path can complete while the client-side issuance step never
enters consensus. The optimization harness must therefore measure and report
`quote paid -> POST mint received -> signatures returned` separately. Server
latency work must not claim to fix a missing wallet POST.

The trace also confirms that a fast Ark claim can be surrounded by much more
federation work than the claim itself.

### Outgoing evidence

The consensus review establishes the current operation shape, but a clean
successful outgoing latency baseline has not yet been captured with the stage
precision required by this contract.

The recovered 1,000-sat outgoing incident is useful only as failure-path
evidence:

- melt quote, melt reservation, and send plan took approximately nine seconds
  to become accepted;
- the actor constructed and persisted the Ark HTLC package approximately two
  seconds later;
- an invalid physical-change-output assertion then prevented the accepted
  `SendHtlcsCosigned` transition; and
- recovery later exercised terminal failure, revocation, and failed-melt
  observation paths.

Because that lifecycle contained a defect, restarts, and an expired invoice,
its total duration is not a successful-payment performance baseline. Milestone
0 must capture fresh successful outgoing payments before outgoing optimization.

### Current consensus-entry cardinality

Let `n` be the member count, `t` the Bark/FROST threshold, and `q` the generic
payment-observation threshold. Nocean normally has `n = 5`, `t = 3`, and
`q = 3`.

These counts cover Bark and payment-observation entries. Incoming quote
creation, outgoing melt quote/reservation, the final Cashu mint operation, raw
AlephBFT protocol packets, private FROST messages, Ark RPCs, and wallet HTTP
fan-out are reported separately.

| Flow | Minimum formula | 3-of-5 minimum | Current 5-member range |
| --- | ---: | ---: | ---: |
| Incoming success | `6 + 2t + q` | 15 | 15-21 |
| Outgoing success | `4 + t + q` | 10 | 10-14 |
| Outgoing failure plus revocation | `6 + 2t + q` | 15 | 15-21 |

The two production incoming traces reached the upper part of the range because
all five members submitted each Bark observation family and four members then
submitted the generic payment observation.

### Current scheduler and polling behavior

The implementation has one backend-neutral `FederationPaymentObserver`, but
its scheduling behavior is not yet symmetric:

- the observer wakes every five seconds and scans unfinished mint and pending
  melt quotes;
- mint quotes use a persisted schedule with a five-second base delay, doubling
  to a five-minute cap, a five-second request cooldown, a fifteen-second attempt
  lease, and deterministic twenty-percent jitter;
- a successful mint advancement resets its no-progress counter but still
  persists another base delay;
- accepted internal progress can therefore wait for the next five-second
  observer wake and then another scheduled delay;
- melt quotes do not use the persisted per-quote schedule and are reconsidered
  on every five-second repair scan; and
- Bark-specific incoming and outgoing advancement is called inside the generic
  observer, but it does not return a scheduling reason that distinguishes
  external evidence from internal consensus wait.

This is the first confirmed unnecessary-delay target. Backoff is necessary to
protect the shared Ark server, especially because all co-located members appear
under one public address. Applying external no-progress backoff to internal
federation progress is not necessary.

### Bark event and stream evidence

The current `cdk-bark` payment adapter returns a permanently pending payment
event stream and reports `is_payment_event_stream_active() == false`.

The Bark fork itself does have relevant primitives:

- an authenticated mailbox subscription stream;
- an incoming-Lightning mailbox message carrying a payment hash;
- an outgoing-Lightning-finished mailbox message carrying terminal success or
  failure information;
- wallet movement notifications; and
- bounded reconnect backoff for dropped mailbox and round streams.

Ordinary Bark mailbox processing performs wallet mutations and may claim an
incoming payment directly, so it cannot simply be enabled inside a federation.
The useful open question is whether a narrow facade can expose mailbox data as
a wake-only hint while the existing federation probe, validation, consensus,
and claim plan remain authoritative. Milestone 2 answers that question with
code and live evidence. Until then, polling remains required.

## Optimization invariants

Every milestone must preserve all of the following:

- A member-local Ark response, mailbox message, Bark row, clock value, or
  notification is never payment authority by itself.
- Matching evidence comes from distinct authenticated members according to the
  configured threshold.
- Consensus accepts the exact claim, send, or revocation plan before preimage
  disclosure, FROST signing, payment initiation, claim, or revocation side
  effects.
- Deterministic apply performs no Ark, chain, clock, random, notification, or
  member-local Bark I/O.
- The existing actor and failover rules continue to prevent duplicate external
  payment, claim, and revocation side effects.
- Replay, restart, checkpoint restore, and catch-up converge to the identical
  Cashu quote, saga, Bark projection, VTXO, and liability state.
- Terminal or issued quotes and terminal Bark actions leave all automatic
  polling and maintenance schedules.
- Generic scheduler changes apply to future federated funding backends and to
  federated BDK where the same state contract exists, without adding Bark
  branches to shared handlers.
- Standalone payment processors and standalone wallet flows remain unchanged.
- Logs, metrics, and benchmark artifacts contain no invoice, preimage, secret,
  credential, private member config, or unbounded quote/action label.

## Implementation contract

This work inherits the safety, cut-through, testing, and observability rules
from the required federation plan linked above. Those rules govern throughout;
the following evidence and discipline rules add Bark-specific requirements for
every milestone.

### Evidence before production changes

- Record the symptom, exact owning call path, invocation count, wait or work
  duration, and current lifecycle effect before changing behavior.
- Code inspection identifies a hypothesis. A direct counter, structured
  timeline, controlled fault, or equivalent measurement confirms it.
- Change one independently measurable cause at a time. Do not combine timer,
  message-count, and consensus-cadence changes into one candidate.
- Use release binaries for performance conclusions. Debug builds are only for
  correctness and fault diagnosis.
- Use the same source base, topology, threshold, database, Ark server, wallet,
  payment amount class, and host conditions for a before/after pair.
- Record all attempted samples in execution order, including failures, stopped
  runs, rate limits, timeouts, and rejected candidates.
- Temporary instrumentation may be added while measuring a milestone, but it
  must be reverted before that milestone is completed. Any observability that
  remains is a deliberate production feature and must be separately justified,
  documented, bounded, low-cardinality, and low-overhead.

### Keep-or-revert rule

- Run at least three equivalent local samples and three equivalent Signet or
  Nocean samples for a noisy end-to-end comparison. Compare medians and report
  p95 when the sample count is sufficient; never report only the best trace.
- A latency candidate is retained when its directly targeted median stage
  improves by at least 15%, the direct delay/work counter moves as predicted,
  and no other primary stage regresses by more than 5%.
- A protocol-work candidate may instead qualify by removing the exact predicted
  operations, duplicate items, Ark probes, or messages while end-to-end latency
  does not regress by more than 5%.
- A correctness or liveness fix is retained when the recorded failure becomes
  reproducibly impossible and performance does not regress materially.
- If the direct counter does not move as predicted or the result misses the
  gate, revert production changes and mark the milestone or candidate
  `Rejected`. Commit the evidence so the same speculation is not repeated.
- Never obtain a faster result by weakening quorum, signature, payment,
  preimage, amount, expiry, replay, finality, readiness, or convergence checks;
  increasing operation timeouts; disabling recovery; or adding a benchmark-only
  fast path.

### Simplicity and ownership

- Remove a wait or operation at its owning layer. Do not compensate for it with
  extra tasks elsewhere.
- Prefer one small advancement outcome and one scheduler over per-backend timer
  implementations.
- Prefer accepted-operation notifications over periodic inspection for
  internal progress.
- Prefer deriving state from evidence already accepted by consensus over
  creating a second consensus vote about the same fact.
- Prefer deterministic publisher selection and an existing operation ID over a
  distributed deduplication service.
- Do not add a general cache, event bus, workflow engine, compatibility adapter,
  or configuration surface unless a confirmed cause cannot be removed more
  simply.
- Centralize the few retained timing constants. Configuration is added only
  after a real deployment demonstrates that one policy cannot serve the
  supported environments.

### Milestone, documentation, and commit discipline

- A milestone is complete only when its success criteria pass, this document
  contains its dated progress record, and coherent code, tests, measurements,
  and documentation are committed together.
- Use a conventional commit. Record the intended subject before commit and the
  resulting hash in the next documentation update; do not amend historical
  measurements merely to insert the current commit's hash.
- Rejected milestones end with production code reverted and a documentation
  commit that records the evidence and decision.
- Update the milestone status table and result log before each milestone commit.
- Start each later milestone from the committed result of its predecessor. Do
  not hide attribution by combining milestone commits.

### Newly discovered work

If implementation or benchmarking exposes another material performance,
stability, safety, or operability issue:

1. Record the symptom separately from the suspected cause.
2. Add a newly numbered milestone before the final release gate.
3. Give it current evidence, preconditions, a checklist, focused success
   criteria, and the same keep-or-revert rule.
4. Confirm the cause before changing production behavior.
5. Commit its result independently, even if the hypothesis is rejected.

Critical safety or liveness work may be prioritized immediately, but it still
receives its own evidence, tests, plan update, and commit.

## Measurement model

### Required latency stages

Incoming traces must record:

1. wallet quote request received;
2. receive intent accepted;
3. invoice prepared;
4. mint quote accepted and invoice returned;
5. payer submits the invoice to the faucet or Lightning payer;
6. first Ark-visible payment signal or first authoritative positive probe;
7. HTLC evidence quorum accepted;
8. claim plan accepted;
9. preimage disclosure accepted;
10. Ark claim completed;
11. Bark result quorum accepted;
12. quote marked paid;
13. wallet POST mint received;
14. mint operation accepted; and
15. signatures returned and wallet balance persisted.

Outgoing traces must record:

1. store invoice obtained;
2. wallet melt-quote request received and returned;
3. wallet POST melt received;
4. melt reservation accepted;
5. send plan accepted;
6. HTLC package FROST signing started and completed;
7. `SendHtlcsCosigned` accepted;
8. Ark payment initiation started and completed;
9. `PaymentInitiated` accepted;
10. first terminal Ark signal or authoritative terminal probe;
11. Bark result quorum accepted;
12. quote paid, or safe failure and revocation completed;
13. Cashu melt saga finalized; and
14. wallet result and change persisted.

Use monotonic clocks for local spans. Consensus state contains only accepted
protocol data, never local duration. Correlation uses bounded operation class,
member ID, and redacted operation identifiers outside metric labels.

### Required efficiency counters

For each lifecycle, record:

- accepted operations by kind;
- duplicate finalized items carrying an already accepted operation;
- payload-bearing and empty AlephBFT units;
- raw private consensus messages and bytes when available;
- FROST request, commitment, share, abort, retry, and byte counts;
- Ark RPCs by bounded method and result class, per member and total;
- mailbox notifications, reconnects, lag, wake-only probes, and repair wakes;
- scheduler wake reason, advancement outcome, pump steps, and lease collision;
- member CPU-seconds and peak RSS;
- Cashu quote payment and issuance counts; and
- warnings for rate limit, actor mismatch, invalid replay, stale action,
  non-retryable signing session, expiry, recovery-required state, or audit
  disagreement.

### Benchmark environments

Use all three layers. A milestone may use a smaller layer only when its direct
counter cannot be exercised meaningfully at a higher one.

1. **Deterministic focused harness.** Script backend outcomes and accepted
   operations with fake time. Prove exact scheduling, operation count,
   idempotency, restart, and adverse ordering without network noise.
2. **Local release federation.** Run five local mint processes with SQLite,
   threshold three, normal AlephBFT, the local release `cdk-cli`, and either a
   controlled Bark/Ark test facade or Signet Bark. Capture server-only and warm
   persistent-wallet latency separately from cold CLI startup.
3. **Nocean and real Signet.** Deploy the exact release binaries and source
   commit. Use the real five-member deployment, public Ark server, local CLI,
   Signet faucet, and Signet store. Require final convergence across all five
   members.

### Reproducible Signet demonstrations

The Signet faucet accepts a BOLT11 invoice generated by the federation. The
Signet store exposes fresh outgoing invoices for 1,000, 5,000, and 10,000-sat
test purchases. These are test-only external interactions, not a synthetic
replacement for local harnesses.

Incoming demonstration:

1. import the candidate federation into a clean local CLI wallet;
2. request a fixed 10,000-sat BOLT11 mint quote with `cdk-cli`;
3. submit the returned invoice through `https://signet.2nd.dev/`;
4. keep the same CLI process or separately label cold-process overhead;
5. wait for threshold-consistent `PAID`, perform POST mint, and confirm the
   exact wallet balance increase; and
6. join the client timeline to the redacted five-member server timeline.

Outgoing demonstration:

1. fund the CLI wallet through the incoming demonstration;
2. obtain a fresh 1,000 or 5,000-sat invoice from
   `https://signet.2nd.dev/store`;
3. melt through the federated wallet with `cdk-cli`;
4. confirm terminal settlement in both the store and federation;
5. verify actual fee, returned change, and exact wallet balance; and
6. join the wallet, Bark, Ark, FROST, and consensus timelines.

Browser timing is evidence for the human-visible boundary only. Server metrics
remain authoritative for internal attribution. Do not commit invoices, wallet
databases, browser state, or payment secrets.

## Target architecture

### One generic payment advancement driver

The smallest useful shared contract is one advancement result:

```text
Progressed
AwaitingExternalEvidence
AwaitingFederationEvidence
Complete
```

It has the following meaning:

| Result | Scheduler action |
| --- | --- |
| `Progressed` | Advance again immediately in the same bounded cooperative pump |
| `AwaitingExternalEvidence` | Persist and apply external no-progress backoff |
| `AwaitingFederationEvidence` | Wait for relevant accepted-operation notification; retain a slow repair fallback |
| `Complete` | Remove schedule state and stop all automatic work |

The driver owns persisted schedules, active-attempt leases, duplicate wake
cooldowns, accepted-operation wakes, optional backend event wakes, wallet
request wakes, and terminal cleanup. Backend advancement owns protocol state
and returns only the reason it cannot continue. It does not own generic timers.

The immediate pump is bounded initially to eight advancements per wake. If it
reaches the bound, it yields and schedules a cooperative continuation. The
bound is a fairness guard, not an artificial delay.

This driver must cover both mint and melt quotes. Federated BDK and future
federated backends use the same outcomes where applicable. Bark-specific phase
logic remains in the Bark incoming and outgoing orchestrators.

### Wake sources

The driver merges these hints without treating any as payment truth:

1. wallet GET quote or POST mint/melt;
2. accepted federation operation referencing the quote or Bark action;
3. Bark mailbox or backend event, if Milestone 2 proves a safe integration;
4. persisted external-probe deadline; and
5. a slow bounded repair scan for restart and lost notification recovery.

### Initial polling candidate

The current five-second exponential policy remains unchanged in Milestone 1 so
the internal-progress change can be attributed independently. After threshold-
first observation exists, Milestone 4 evaluates this centralized candidate:

| Condition | Candidate behavior |
| --- | --- |
| Fresh external wait | Probe after 2s |
| Continued no progress | 4s, 8s, 16s, 32s, 64s, 128s, 256s, then 300s cap |
| Background collision spreading | deterministic +/-20% jitter |
| Wallet GET or POST | immediate if no probe is active and none completed in the last 1s |
| Accepted relevant operation | immediate internal pump; no external probe unless the new state requests one |
| Mailbox/backend hint | immediate authoritative probe under the same lease |
| Progress into a new external-wait phase | reset that phase's no-progress counter |
| Restart | resume persisted deadline, phase, counter, and expired-lease recovery |
| Terminal state | remove schedule immediately |

A wallet request wakes a quote but does not reset a no-progress counter. Error
text, including `local_rate_limited`, never changes the schedule. The direct
Ark-RPC count and server error classes decide whether the two-second candidate
is safe; if it increases rate limiting or has no latency benefit, keep the
slower measured policy.

## Milestone status

Allowed states are `Not started`, `In progress`, `Complete`, and `Rejected`.

| Milestone | Goal | Status | Commit |
| --- | --- | --- | --- |
| 0 | Establish reproducible incoming and outgoing baselines with causal timelines | In progress | - |
| 1 | Remove unnecessary inter-operation delay with one generic mint/melt advancement driver | Not started | - |
| 2 | Determine and implement the smallest safe Bark incoming/outgoing event wake, or reject it | Not started | - |
| 3 | Minimize consensus messages by deriving Cashu payment from accepted Bark result quorum | Not started | - |
| 4 | Reduce excess Ark probes and observations with threshold-first polling and measured backoff | Not started | - |
| 5 | Remove duplicate wallet submissions and any proven redundant serial Bark operation | Not started | - |
| 6 | Isolate Bark maintenance from payment hot paths and terminalize permanent failures | Not started | - |
| 7 | Tune consensus timing only if post-reduction evidence still shows a material delay | Not started | - |
| 8 | Pass sustained local, Nocean, CLI, and browser-based Signet release gates | Not started | - |

## Milestone 0: causal baseline

### Goal

Measure successful incoming, successful outgoing, and safe outgoing failure
with enough precision to attribute payer/routing wait, Ark discovery, Bark/FROST
work, consensus finality, Cashu finalization, wallet fan-out, and issuance. No
production scheduling or protocol optimization is allowed in this milestone.
This is an evidence gate, not a production-observability deliverable: use
existing logs and durable state first, add isolated disposable probes only for
material gaps, and leave no instrumentation behind merely to satisfy this
milestone.

### Current evidence

- One issued 10,123-sat receive took about 101 seconds and 22 accepted
  operations.
- One 50,000-sat receive reached `PAID`; the external claim RPC took about 170
  milliseconds, but no client mint operation was submitted.
- The consensus review provides exact minimum formulas and 5-member ranges.
- No clean successful outgoing stage timeline exists yet.
- Existing logs omit the exact payer-settlement-to-first-positive-probe
  boundary and do not provide one redacted joined lifecycle.

### Checklist

- [ ] Reconstruct a redacted joined timeline from existing logs and durable
      state for every material stage observable today.
- [ ] Measure accepted operation kinds, duplicate finalized items, Ark RPCs,
      observer attempts, FROST traffic, maintenance overlap, CPU, and RSS using
      existing evidence or one-off measurements; do not add permanent counters
      solely for this baseline.
- [ ] For any material evidence gap, add the smallest isolated disposable probe
      or local analysis tool, capture the result, and revert or delete it before
      completing the milestone.
- [ ] Extend a deterministic test harness to script incoming and outgoing Bark
      outcomes and fake time.
- [ ] Run at least three release incoming and three release outgoing
      lifecycles on a five-member local deployment using the local CLI.
- [ ] Run at least three incoming and outgoing Signet lifecycles against the
      same candidate on Nocean, using the faucet and store.
- [ ] Include one outgoing terminal failure and safe revocation, but report it
      separately from successful latency.
- [ ] Record cold CLI, warm wallet, server-only, payer-controlled, and external
      routing time separately.
- [ ] Confirm exact five-member convergence, quote accounting, balances, VTXO
      state, and absence of safety alerts after every sample.
- [ ] Confirm that no temporary instrumentation or one-off analysis artifact
      remains in production code.

### Success criteria

- Every material second and every accepted operation in both successful flows
  belongs to a named stage.
- External payer delay is not conflated with server detection or federation
  progress.
- Incoming and outgoing baselines include accepted operation count, duplicate
  finalized items, Ark RPC count, FROST messages, CPU, and wallet-visible
  latency.
- All runs finish converged and no instrumentation logs sensitive material.
- The milestone is committed as
  `test: establish federated Bark payment performance baseline`.

### Progress record — 2026-07-21

Milestone 0 is `In progress`. The two production incoming traces and consensus
cardinality review are recorded above. Source review confirmed the current
five-second scan, asymmetric mint/melt scheduling, inactive CDK payment event
stream, and available Bark mailbox primitives. The outgoing successful
baseline, joined measurements, reproducible harness, and repeated local/Signet
samples remain open. Permanent observability work is not required: disposable
probes are allowed only where existing evidence is insufficient and must be
removed before completion. No production optimization has been made.

## Milestone 1: generic progress pump and inter-operation wakes

### Goal

Remove the five-to-ten-second waits that currently occur between successful
internal state transitions. External no-progress probing keeps bounded
backoff; accepted federation progress does not.

### Preconditions

- Milestone 0 identifies the current delay between at least two internal
  transitions and records the exact scheduler outcome that caused it.

### Checklist

- [ ] Introduce the four backend-independent advancement results without
      exposing Bark semantic phases to the generic observer.
- [ ] Use one persisted schedule implementation for federated mint and melt
      quotes.
- [ ] Restrict exponential backoff to `AwaitingExternalEvidence`.
- [ ] On `Progressed`, continue in a bounded eight-step cooperative pump.
- [ ] On `AwaitingFederationEvidence`, register a wake for accepted operations
      that reference the quote/action.
- [ ] Keep one slow repair scan for lost notifications, startup races, and
      restart; do not use it as the normal progress clock.
- [ ] Keep the current external 5s-to-300s backoff constants during this
      milestone to isolate the internal-delay change.
- [ ] Let wallet GET/POST wake the same driver; do not reset persisted backoff.
- [ ] Remove schedules for issued mint quotes and terminal melt/action states.
- [ ] Verify federated BDK follows the generic contract and has no Bark branch.
- [ ] Verify standalone payment observation is unchanged.

### Tests

- [ ] Multiple scripted internal transitions complete without advancing fake
      time by five seconds between each one.
- [ ] No-progress external evidence follows the persisted sequence.
- [ ] Restart preserves deadline, counter, phase, and lease recovery.
- [ ] A wallet wake does not reset no-progress state.
- [ ] A relevant journal apply wakes the exact quote immediately.
- [ ] An unrelated operation does not wake every quote.
- [ ] A missed wake is recovered by the slow repair scan.
- [ ] The pump yields at eight steps and resumes cooperatively.
- [ ] Concurrent wallet, timer, journal, and backend wakes produce at most one
      active attempt per quote/member.
- [ ] Pending melts no longer run every five seconds independently of their
      per-quote state.

### Success criteria

- Internal transition wait attributable to the repair/backoff loop falls by at
  least 80% and reaches zero scheduled-delay events in the direct counter.
- Ark RPC volume does not increase because external probing constants are
  unchanged.
- Incoming and outgoing targeted median post-evidence latency improve by at
  least 15%, or the change is reverted.
- Restart, convergence, federated BDK, and standalone regressions pass.
- The milestone is committed as
  `perf: drive federated payment progress without polling delays`.

## Milestone 2: Bark mailbox/event wake feasibility and implementation

### Goal

Determine whether the supported Bark/Ark stack can wake incoming and outgoing
federated payments from a stream or long-lived notification. If it can, add the
smallest wake-only integration. If it cannot safely preserve cursor, replay,
and federation boundaries, retain polling and record the rejection.

### Current evidence

- `cdk-bark` advertises no active payment event stream.
- Bark exposes authenticated mailbox subscription and both incoming-payment
  and outgoing-finished message kinds.
- Ordinary mailbox processing mutates the local Bark wallet and may directly
  claim or settle a payment, which is incompatible with federation consensus.
- Bark also exposes wallet movement notifications and an Ark status API with a
  wait flag, but neither is currently integrated into federated CDK.

### Checklist

- [ ] Trace the incoming and outgoing mailbox message origin, authentication,
      delivery, cursor, reconnect, redelivery, and expiry behavior in the
      supported Ark server.
- [ ] Determine whether all members can subscribe without exceeding server
      connection or rate limits.
- [ ] Determine whether raw decode-only subscription can advance a durable
      cursor without invoking ordinary local claim/settlement side effects.
- [ ] Evaluate, in order, the smallest candidates:
  - [ ] expose a payment-hash wake from the existing mailbox stream;
  - [ ] expose a federation-safe Bark notification after mailbox decode;
  - [ ] use the server's bounded wait/long-poll status mode; or
  - [ ] retain ordinary polling.
- [ ] Treat every stream item only as a hint to run the normal authoritative
      member probe.
- [ ] Reuse the generic driver and attempt lease; do not create a Bark retry
      loop in the adapter.
- [ ] Use bounded reconnect backoff and cancellation.
- [ ] On lag, cursor loss, disconnect, restart, or unsupported message, fall
      back to the persisted polling repair path.
- [ ] Measure persistent connections, reconnects, event-to-probe latency, Ark
      RPC reduction, and server rate-limit errors.

### Tests

- [ ] Incoming mailbox hint wakes only the matching quote.
- [ ] Outgoing success and failure hints wake only the matching melt.
- [ ] A forged, stale, duplicate, reordered, or unknown hint cannot authorize
      payment or mutate deterministic state.
- [ ] Stream disconnect and cursor replay do not lose or duplicate settlement.
- [ ] Lagged notification forces a bounded repair probe.
- [ ] Restart before and after a delivered hint converges identically.
- [ ] Disabling the stream changes latency only, not eventual correctness.

### Success criteria

- If kept, median Ark-signal-to-authoritative-probe latency improves by at
  least 50%, background Ark status RPCs fall materially, and no rate-limit or
  reconnect amplification appears.
- If no safe narrow cursor/notification boundary exists, no production event
  code remains; the evidence is committed and Milestone 4 proceeds with
  polling.
- A kept implementation is committed as
  `perf: wake federated Bark payments from mailbox hints`.
- A rejected result is committed as
  `docs: record federated Bark event wake investigation`.

## Milestone 3: minimize consensus messages using accepted Bark result quorum

### Goal

Remove the second generic member-by-member payment-observation quorum when an
accepted threshold of Bark result observations already proves the exact same
incoming or outgoing terminal fact.

This is the first consensus-message reduction milestone. It changes no Bark
threshold and does not make an actor result authoritative.

### Expected direct reduction

One accepted Bark result family may replace the generic payment family only
when it contains enough distinct members for both authorities. Define
`r = max(t, q)`. Nocean has `t = q = r = 3`.

| Flow | Current minimum | Candidate minimum | General reduction | Nocean reduction |
| --- | ---: | ---: | ---: | ---: |
| Incoming success | `6 + 2t + q` | `6 + t + r` | `min(t, q)` | 15 -> 12, save 3 |
| Outgoing success | `4 + t + q` | `4 + r` | `min(t, q)` | 10 -> 7, save 3 |
| Outgoing failure/revocation | `6 + 2t + q` | `6 + t + r` | `min(t, q)` | 15 -> 12, save 3 |

Cashu quote creation/reservation and final mint issuance remain separately
counted and are not hidden by this table.

### Checklist

- [ ] Specify the exact deterministic mapping from a threshold-matched Bark
      result key to mint-quote paid, melt paid, or safely failed state.
- [ ] Require `max(t, q)` matching result observers when the same result family
      supplies both Bark and Cashu payment authority; never assume `t == q`.
- [ ] Bind quote, method, payment hash, action, amount, fee, source scope,
      result hash, terminal status, and predecessor phase.
- [ ] Apply the ordinary quote/saga transition when the threshold-completing
      Bark observation is accepted, in the same transaction or one
      deterministic derived apply step.
- [ ] Reuse existing Cashu quote accounting and saga functions instead of
      duplicating Bark-specific payment business logic.
- [ ] Remove Bark's second `MintQuotePayment` and `MeltQuotePayment`
      submission rounds.
- [ ] Preserve generic payment-observation operations for backends that do not
      already produce a threshold-authoritative result family.
- [ ] Publish quote/saga notifications only after commit.
- [ ] Make catch-up and replay derive the identical state without Bark or Ark
      I/O.

### Tests

- [ ] `r - 1` matching Bark results cannot pay or fail a quote.
- [ ] The threshold-completing result applies the ordinary quote/saga change
      exactly once.
- [ ] Later matching observations are idempotent.
- [ ] Conflicting amount, fee, hash, result, scope, action, or terminal status
      cannot combine.
- [ ] Two incompatible threshold groups fail closed.
- [ ] Observation reordering converges to the same result.
- [ ] Restart before notification and catch-up after quorum preserve payment.
- [ ] Incoming issuance cannot exceed accepted net Bark value.
- [ ] Outgoing success, fee change, failed revocation, and proof rollback retain
      existing Cashu behavior.

### Success criteria

- The direct operation counter falls by exactly `min(t, q)` for all three flows,
  which is three entries in the Nocean 3-of-5 configuration.
- No generic Bark-backed payment observation appears after result quorum.
- Median result-quorum-to-quote-final latency improves by at least 15%, with no
  other primary stage regressing by more than 5%.
- Non-Bark federation backends and standalone flows are unchanged.
- The milestone is committed as
  `perf: derive Cashu payment from Bark result quorum`.

## Milestone 4: threshold-first Ark probing and measured backoff

### Goal

Use only the number of healthy Ark observers needed for threshold evidence,
retain deterministic fallback for liveness, and evaluate a faster fresh-payment
backoff without recreating the shared-IP rate-limit incident.

### Checklist

- [ ] Deterministically select the required observer count from accepted action
      data: `t` for Bark-only evidence and `max(t, q)` for a result family that
      also supplies Cashu payment authority after Milestone 3.
- [ ] Permit primaries to probe immediately according to the generic schedule.
- [ ] Delay non-primary observers for a measured fallback window, initially six
      seconds, and cancel fallback when accepted matching evidence reaches
      threshold.
- [ ] If fallback is needed, expand deterministically without changing the
      evidence key or actor plan.
- [ ] Before every Ark call, check whether this member already has accepted
      matching evidence for the current epoch; if so, return
      `AwaitingFederationEvidence` without Ark I/O.
- [ ] Do not submit new evidence after threshold if it has not already entered
      consensus.
- [ ] Apply the same primary/fallback principle to outgoing terminal-failure
      observation where multiple members must poll.
- [ ] Evaluate the centralized 2s-to-300s polling candidate only after primary
      selection is active.
- [ ] Keep wallet and mailbox wakes under the same one-second duplicate cooldown
      and active-attempt lease.
- [ ] Do not parse server errors or reset backoff on wallet requests.
- [ ] Record status and prepare/hydration Ark RPCs separately.

### Tests

- [ ] Healthy Nocean incoming evidence uses three primary observers for every
      current evidence family because `t = q = 3`.
- [ ] One unavailable primary activates fallback and completes safely.
- [ ] Two unavailable primaries leave the quote pending without unsafe claim.
- [ ] Accepted evidence suppresses all later Ark I/O by that member and epoch.
- [ ] Restart preserves primary selection and evidence suppression.
- [ ] Wallet fan-out during backoff does not cause five simultaneous probes.
- [ ] A mailbox hint during an active lease does not duplicate a probe.
- [ ] Selection is identical on all members and during replay.

### Success criteria

- Healthy incoming observation families contain exactly their required
  observer count in at least 95% of repeated samples; surplus observations have
  an explained race bounded to already-entered consensus items.
- Healthy observer Ark RPC volume falls by at least 40% from the five-member
  baseline.
- One unavailable primary still completes within the measured fallback budget.
- The faster polling candidate is kept only if fresh-payment detection improves
  by at least 15% without increasing rate-limit errors or total Ark RPCs.
- The milestone is committed as
  `perf: use threshold-first Bark payment observation`.

## Milestone 5: duplicate submission and serial-operation reduction

### Goal

Remove remaining consensus work that does not represent a distinct safety
boundary. Address wallet fan-out duplicates first, then evaluate each adjacent
Bark operation independently. Do not introduce opaque operation batches.

### Checklist

- [ ] Count accepted entries, duplicate finalized items, units, protocol
      messages, and bytes separately.
- [ ] For identical wallet-fanned mint or melt operations, let every member
      register a waiter for the stable operation ID while one deterministic
      publisher submits first.
- [ ] Add a short deterministic fallback publisher order for liveness.
- [ ] Prove every waiter receives the same accepted result and cancellation
      cannot suppress fallback.
- [ ] Recount incoming setup, outgoing setup, and mint/melt submission work.
- [ ] Audit each serial Bark boundary against the invariant it protects:
  - [ ] receive intent -> receive prepared;
  - [ ] HTLC-ready -> claim plan;
  - [ ] claim plan -> preimage disclosure -> claimed;
  - [ ] send plan -> HTLCs cosigned -> payment initiated -> terminal result;
  - [ ] failure -> revocation plan -> revocation completed.
- [ ] For each boundary, document the crash or equivocation that requires it.
- [ ] Evaluate only a boundary whose accepted data can deterministically carry
      the next state without crossing an external side effect or signing
      authorization boundary.
- [ ] Evaluate materializing the mint quote from accepted receive preparation
      only if invoice, lookup ID, amount, expiry, and requester binding remain
      deterministic and replay-safe.
- [ ] Keep reductions as direct derived apply behavior; do not create a generic
      Bark batch envelope.
- [ ] Stop after the first retained serial-operation reduction, commit it, and
      add another milestone for further independent reductions.

### Tests

- [ ] Five simultaneous identical wallet requests normally produce one
      publisher submission and one accepted operation.
- [ ] Publisher failure triggers fallback without changing the operation ID.
- [ ] Waiter restart and reconnect recover the accepted result.
- [ ] Every removed boundary has crash-before, crash-after, replay, reorder,
      conflicting-plan, and exact-side-effect tests.
- [ ] No preimage, signature share, Ark payment, claim, or revocation occurs
      before its retained accepted boundary.

### Success criteria

- Healthy wallet fan-out produces no surplus finalized item for an already
  accepted mint/melt operation, except a bounded measured fallback race.
- Raw consensus traffic and duplicate-item counters fall as predicted.
- Any serial operation removed has a written safety proof and exact count
  reduction; otherwise it remains.
- End-to-end latency does not regress by more than 5%.
- The initial milestone is committed as
  `perf: deduplicate federated wallet operation submission`.

## Milestone 6: maintenance isolation and terminal cleanup

### Goal

Prevent refresh, old receives, expired HTLCs, invalid reservations, and
non-retryable signing sessions from competing with active payments or polling
forever.

### Current evidence

- Production receive traces interleaved unrelated refresh operations with the
  payment hot path.
- Members have logged actor/author mismatches during refresh.
- Expired incoming work continued to wake and fail long after it could no
  longer make progress.
- Five-second maintenance attempts can overlap payment-related FROST and Ark
  work.

### Checklist

- [ ] Check accepted actor/member role before any actor-only Bark side effect.
- [ ] Restrict non-actors to member-scoped observations.
- [ ] Prevent concurrent execution of the same maintenance action.
- [ ] Give active receive, send, claim, and revocation work priority over
      routine refresh.
- [ ] Classify expired HTLC, invalid reserved input, exhausted attempt lineage,
      and non-retryable FROST session into explicit terminal or operator-required
      states.
- [ ] Remove terminal actions from automatic selection and Ark polling.
- [ ] Keep authenticated operator retry only where its replay contract is safe
      and documented.
- [ ] Apply external no-progress backoff to routine maintenance instead of a
      permanent five-second loop.
- [ ] Measure maintenance overlap, Ark RPCs, FROST requests, and payment-stage
      latency before and after.

### Tests

- [ ] Only the accepted actor executes refresh.
- [ ] Every member can submit its own evidence without author mismatch.
- [ ] An expired receive becomes terminal and performs no later Ark RPC.
- [ ] A non-retryable FROST session is not retried automatically.
- [ ] Active payment finalization proceeds while refresh is due.
- [ ] Restart does not reactivate terminal automatic work.

### Success criteria

- Healthy maintenance produces no author mismatch or duplicate actor side
  effect.
- Terminal actions produce zero automatic retries after terminalization.
- Routine maintenance adds no accepted operation or measurable wait to the
  payment critical path in the repeated healthy sample.
- The milestone is committed as
  `fix: isolate federated Bark maintenance from payments`.

## Milestone 7: measured consensus timing evaluation

### Goal

Tune AlephBFT or private transport only if operation reduction, event/internal
wakes, and maintenance isolation still leave a confirmed consensus-stage delay.
The default result of this milestone may be `Rejected` with no production
change.

### Preconditions

- Milestones 1-6 have committed outcomes.
- New baselines show submit-to-finality or finality-to-apply is a material
  remaining share of healthy payment latency.
- Accepted operation and duplicate counts are already at their justified
  minimums.

### Checklist

- [ ] Measure per-operation queue, proposal, first-unit, finality, application,
      and response latency for active payment operations.
- [ ] Count payload-bearing and empty units, items per unit, private messages,
      bytes, retries, and CPU.
- [ ] Confirm whether upstream active/idle delay, batching, or dissemination is
      responsible; do not infer from a five-second constant alone.
- [ ] Evaluate one parameter or algorithmic cause at a time under loopback and
      injected transport delay.
- [ ] Include idle, saturation, one-member-slow, one-member-offline, restart,
      rotation, and catch-up cases.
- [ ] Reject any candidate that amplifies empty/helper units, CPU, retries, or
      idle work.

### Success criteria

- A kept change improves the targeted consensus stage by at least 15%, moves
  its direct counter as predicted, and respects the 5% regression budget.
- No empty-unit, private-message, retry, CPU, restart, or convergence
  amplification appears.
- A kept result is committed as
  `perf: tune measured federation payment consensus latency`.
- A rejected result is committed as
  `docs: record Bark payment consensus timing evaluation`.

## Milestone 8: sustained local and Signet release gate

### Goal

Prove the complete retained implementation under realistic repeated incoming,
outgoing, failure, restart, and rate-limit-sensitive workloads. Focused tests
and single favorable payments cannot complete this milestone.

### Checklist

- [ ] Build release `cdk-mintd` and federation-enabled `cdk-cli` from the exact
      candidate commit and record source revision and binary hashes.
- [ ] Run the deterministic harness through every progression, no-progress,
      wake, restart, and terminal path.
- [ ] Start a fresh local five-member SQLite federation with threshold three.
- [ ] Run at least:
  - [ ] 20 sequential and 20 parallel scripted incoming lifecycles;
  - [ ] 20 sequential and 20 parallel scripted outgoing successes;
  - [ ] 10 outgoing failures with revocation;
  - [ ] one primary observer offline;
  - [ ] two surplus members offline after invoice creation;
  - [ ] actor restart before and after each external side-effect boundary;
  - [ ] all-member restart after Bark settlement but before Cashu finalization;
  - [ ] wallet GET/POST fan-out during backoff; and
  - [ ] old expired work plus new healthy payments.
- [ ] Run at least five incoming and five outgoing real Signet samples with the
      local CLI, faucet, and store.
- [ ] Deploy the exact candidate to Nocean only after local gates pass.
- [ ] Repeat at least five incoming and five outgoing Nocean samples, including
      one slow or offline surplus member.
- [ ] Verify exact journal, application, finalized-item, Bark projection,
      materialized-state, quote, saga, VTXO, and balance convergence.
- [ ] Verify idle CPU and automatic-work counters return to a bounded floor.
- [ ] Run formatting, focused Bark/CDK/federation/storage tests, affected
      standalone tests, workspace check, strict Clippy, and restart/catch-up
      suites.
- [ ] Append every sample and result to this document.

### Release targets

Targets exclude payer choice time and external Lightning routing before the
payment becomes observable to the Ark server.

| Measurement | Recorded baseline | Initial target | Stretch target |
| --- | ---: | ---: | ---: |
| Wallet quote request to invoice ready | below 10s but not precisely instrumented | below 5s p95 | below 3s p95 |
| Ark-visible incoming evidence to ready quorum | up to about 10s | below 5s p95 | below 3s p95 |
| Ready quorum to actor Bark settlement | about 20s | below 10s p95 | below 6s p95 |
| Bark result quorum to quote paid | about 10s | below 3s p95 | below 2s p95 |
| Quote paid to signatures returned when POST exists | about 8s in baseline A | below 5s p95 | below 3s p95 |
| Ark-visible incoming payment to Cashu issuance | about 58s | 15-25s p95 | 10-15s p95 |
| Outgoing POST melt to terminal wallet result | Milestone 0 required | below 25s p95 | below 15s p95 |

Efficiency targets for a healthy 3-of-5 lifecycle:

- exactly the required Bark observer count per evidence family in at least 95%
  of samples;
- no second generic payment-observation quorum for Bark;
- one normal publisher for a wallet-fanned identical operation;
- no member with accepted matching evidence performs another Ark probe for the
  same epoch;
- no terminal quote/action remains scheduled;
- no `local_rate_limited`, actor mismatch, non-retryable signing retry,
  recovery-required, payment-observation conflict, invalid-operation, or audit
  alert; and
- all members return to bounded idle CPU with no growing automatic-work counter.

### Success criteria

- Every correctness, failure, restart, and convergence case passes.
- Repeated incoming and outgoing p95 values meet the initial targets or every
  remaining material delay has a measured external cause outside CDK.
- Consensus-entry, duplicate-item, Ark-RPC, FROST-message, and CPU counters
  match the retained architecture and do not grow with quote age after backoff.
- Browser-visible faucet and store demonstrations agree with the CLI, server,
  and durable federation timelines.
- Final code, tests, results, and this document are committed as
  `perf: complete federated Bark BOLT11 optimization`.

## End-to-end verification matrix

- [ ] Healthy 3-of-5 incoming and outgoing success.
- [ ] One selected observer offline before external payment.
- [ ] Two non-threshold members offline after setup.
- [ ] Actor crash before evidence quorum.
- [ ] Actor crash after accepted claim/send plan.
- [ ] Actor crash after preimage disclosure or payment initiation.
- [ ] Actor crash after external settlement but before result acceptance.
- [ ] Member and all-member restart while backoff is persisted.
- [ ] Restart after Bark settlement but before Cashu mint/melt finalization.
- [ ] Wallet GET fan-out during a backed-off quote.
- [ ] Wallet POST mint/melt fan-out after terminal notification.
- [ ] Mailbox disconnect, lag, duplicate, stale cursor, and unsupported message.
- [ ] Ark timeout, unavailable, ambiguous, and rate-limited response without
      error-string control flow.
- [ ] Old expired receive plus a new successful receive.
- [ ] Routine refresh becoming due during an active payment.
- [ ] Failed outgoing revocation with exact value recovery.
- [ ] Federated BDK on-chain quote regression.
- [ ] Standalone BOLT11 backend regression.
- [ ] SQLite and PostgreSQL replay/convergence parity where applicable.
- [ ] Full checkpoint, catch-up, and restore regression.

## Required milestone progress record

Before changing a milestone to `Complete` or `Rejected`, add a dated record
under that milestone containing:

1. confirmed symptom and direct evidence;
2. source commit, binary hashes, build profile, topology, thresholds, database,
   Bark/Ark version, host description, and workload;
3. call-path attribution and direct invocation/message/byte counts;
4. every baseline and candidate sample in execution order;
5. median, p95 where applicable, variance, direct counter movement, and
   regressions;
6. focused correctness, adverse-ordering, restart/replay, distributed, and
   standalone test commands and results;
7. explicit `Kept` or `Rejected` decision against this contract;
8. temporary instrumentation reverted and any retained production metrics
   explicitly justified as bounded, low-cardinality, and low-overhead;
9. document status and result-log update; and
10. milestone commit subject and, once available, commit hash.

Raw invoices, payment hashes, preimages, wallet databases, browser state,
credentials, member private configs, and machine-specific paths stay outside
the repository.

## Result log

Append future evidence here rather than rewriting historical results.

| Date | Source | Environment | Flow | Result | Interpretation |
| --- | --- | --- | --- | --- | --- |
| 2026-07-20 | pre-optimization deployment | Nocean, five members, Bark Signet | 10,123-sat incoming success | about 101s quote-to-issuance; about 58s after first visible post-payment activity; 22 accepted operations; four duplicate finalized mint items | Baseline confirms serial federation delays and excess observation/submission work; no rate-limit error in this trace |
| 2026-07-20 | pre-optimization deployment | Nocean, five members, Bark Signet | 50,000-sat incoming paid, not issued | actor claim RPC about 170ms; five HTLC observations, five result observations, four generic paid observations; all members `PAID`; no mint operation or signatures | Bark settlement succeeded; missing client POST issuance is a separate wallet-visible stage and must not be attributed to Ark or consensus failure |
| 2026-07-21 | static consensus review | 3-of-5 model | incoming/outgoing cardinality | incoming success minimum 15, outgoing success 10, outgoing failure/revocation 15 Bark/payment entries; practical five-member ranges reach 21/14/21 | Establishes exact consensus-reduction counters before production changes |

## Completion checklist

- [ ] External probing, internal progress, and wallet issuance are measured and
      scheduled as separate concerns.
- [ ] No successful internal transition waits for external no-progress backoff.
- [ ] Mint and melt use one backend-independent advancement driver.
- [ ] Bark event support is either safely integrated as wake-only or explicitly
      rejected with evidence.
- [ ] Fresh payments remain responsive without permanent high-frequency polling.
- [ ] Healthy evidence uses threshold observers and accepted evidence suppresses
      repeat Ark I/O.
- [ ] Bark result quorum directly finalizes the ordinary Cashu payment state.
- [ ] Wallet fan-out does not multiply normal consensus submissions.
- [ ] Every remaining serial Bark operation protects a documented distinct
      safety or side-effect boundary.
- [ ] Maintenance does not compete with active payments and terminal work stops.
- [ ] Repeated local, Nocean, CLI, faucet, and store measurements satisfy the
      retained milestone gates.
- [ ] Restart, replay, catch-up, member loss, rate limit, threshold failure,
      federated BDK, and standalone regressions pass.
- [ ] Production observability explains every material delay without logging
      secrets or high-cardinality identifiers.
