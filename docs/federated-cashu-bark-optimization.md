# Federated Cashu Bark optimization checklist and plan

Date: 2026-07-20

Status: proposed; baseline measured on the Nocean five-member federation.

Related documents:

- `docs/federated-cashu-bark-frost-implementation-plan.md`
- `docs/federated-cashu-implementation-plan.md`
- `docs/federated-cashu-bdk-frost-implementation-plan.md`

## Executive summary

The latest successful 10,123-sat BOLT11 receive took approximately 101 seconds
from the mint quote's recorded creation time to Cashu issuance. Approximately
58 seconds elapsed after the first post-payment federation activity became
visible. No rate-limit error occurred for this receive, so its latency was
primarily federation orchestration rather than Lightning routing or Ark rate
limiting.

The main optimization is to separate two different kinds of waiting:

1. External settlement discovery may use a persisted, backend-independent
   probing schedule with bounded exponential backoff.
2. Internal federation progress must be event-driven and must not wait for the
   external probing schedule after an operation has made progress.

This separation should be implemented in one generic federated mint-quote
driver. Bark keeps its explicit state machine and security checks, but does not
own generic timers, rate-limit interpretation, or retry policy. Future
federated funding backends should receive the same behavior automatically.

The first optimization release should preserve the current trust model. In
particular, it should retain distinct-member threshold evidence and must not
make one member's Bark database or Ark response authoritative.

## Measured baseline

All times are UTC. Consensus times below are bounded by the ten-second AlephBFT
status-report cadence unless a more precise Bark or database timestamp exists.

| Stage | Time | Elapsed from quote creation | Evidence |
|---|---:|---:|---|
| Quote and invoice recorded | 18:18:19 | 0s | Durable `mint_quote.created_time` |
| Quote setup operations finalized | by 18:18:22 | about 3s | Receive intent, receive prepared, and mint quote were ordered |
| First post-payment consensus activity | about 18:19:02 | about 43s | AlephBFT resumed after an idle interval |
| HTLC evidence quorum and `receive_htlcs_ready` | by 18:19:12 | about 53s | Five matching member observations and the actor transition were ordered |
| Ark claim started | 18:19:32.792 | about 74s | Bark receive log |
| Actor Bark database settled | 18:19:32.974 | about 74s | `bark_settled_lightning_receive.settled_at` |
| Other four Bark databases settled | 18:19:38.597-18:19:38.627 | about 80s | Member-local settled-receive records |
| Bark result-evidence quorum | by about 18:19:42 | about 84s | Three `Succeeded` observations were ordered |
| Generic mint-payment quorum | by about 18:19:52 | about 94s | Three `MintQuotePayment` observations were ordered |
| Cashu signatures issued | 18:20:00 | 101s | Durable issuance and blind-signature timestamps |

The exact external payer settlement time is not present in the production
logs. The approximately 40-second interval between quote setup and new
consensus activity therefore cannot yet be divided reliably between payer
delay and backed-off settlement discovery.

### Consensus work observed for one receive

The receive produced 22 accepted federation operations:

- 3 quote and invoice setup operations;
- 5 accepted HTLC observations, although the threshold was 3;
- 4 claim-plan and claim-transition operations;
- 5 successful Bark result observations;
- 4 generic mint-payment observations; and
- 1 mint operation.

The same mint operation was also carried by four additional finalized units as
a duplicate operation because all five wallet requests submitted it. An
unrelated Bark refresh plan and five refresh observations were interleaved with
the receive finalization.

## Confirmed latency sources

### 1. Successful progress still waits for the base polling delay

`MintQuoteObservationSchedule::finish` resets `consecutive_no_progress` after
progress, but then calculates and persists the five-second base delay anyway.
The outer observer also wakes on a five-second interval. A state transition can
therefore wait approximately five to ten seconds before the next internal
transition is attempted.

This delay is appropriate for repeated external probes that return no new
evidence. It is not appropriate after an accepted federation operation has
changed the state.

### 2. Bark result evidence is followed by a second payment quorum

After `receive_claimed`, members independently validate the claimed result and
submit matching `Succeeded` Bark observations. Once that threshold exists,
each member separately creates a generic `MintQuotePayment` observation. The
mint waits for another distinct-member threshold before becoming paid.

The second quorum added approximately ten seconds in the measured receive. The
first Bark result quorum can serve as the source evidence for the ordinary mint
quote payment transition without weakening the current observation policy.

### 3. Every member submits evidence before learning that quorum exists

All five members submitted initial accepted HTLC evidence and all five later
submitted successful result evidence. The extra observations do not strengthen
a 3-of-5 threshold after quorum, but they consume Ark capacity, federation
bandwidth, journal entries, and consensus units.

The existing post-observation quorum check cannot prevent this race because all
members may query and submit before the first three observations are applied.

### 4. Bark maintenance competes with the receive hot path

At receive completion, all members attempted to refresh the new VTXOs. Members
2-5 logged `Federation operation author mismatch` while claiming member 1's
role. A refresh plan and five refresh observations were ordered while the mint
quote was still becoming paid.

Maintenance must distinguish actor-only execution from member-local
observation before calling Bark. Expired or permanently invalid actions must
also become terminal instead of consuming a repair attempt forever.

### 5. Bark has no active payment event stream

The federated Bark payment adapter returns a permanently pending stream and
reports that payment events are inactive. Authoritative settlement discovery
therefore depends on explicit probes. Wallet requests can wake a quote, but
they do not eliminate the need for a bounded repair path.

## Optimization invariants

Every optimization in this plan must preserve the following properties.

- No member-local Ark response, Bark SQLite row, clock value, or notification
  becomes federation payment authority by itself.
- Required evidence still comes from distinct authenticated federation
  members according to the configured threshold.
- Consensus accepts the exact claim plan before preimage disclosure, threshold
  signing, or an externally visible claim side effect.
- Deterministic apply performs no Ark, chain, clock, random, or member-local
  database I/O.
- Restart, replay, catch-up, and checkpoint restore converge to the same quote,
  Bark, and liability state.
- Ordinary standalone mints and non-Bark payment processors retain their
  current behavior.
- Federated BDK and future federated funding backends receive generic scheduler
  improvements without acquiring Bark-specific branches.
- Backoff does not inspect or parse backend-specific rate-limit errors.
- No completed or terminal quote remains in a probing or maintenance schedule.

## Target architecture

### One generic federated mint-quote driver

The generic driver should own:

- the persisted external-probe schedule;
- per-quote attempt leases and duplicate-probe cooldowns;
- a per-quote notification or equivalent bounded wake mechanism;
- wallet-request, backend-event, and accepted-operation wake sources;
- a bounded immediate progress pump; and
- terminal schedule cleanup.

The driver should consume a small backend-independent outcome from one
advancement attempt:

```text
Progressed
AwaitingExternalEvidence
AwaitingFederationEvidence
Complete
```

The outcomes have the following scheduling semantics:

| Outcome | Driver behavior |
|---|---|
| `Progressed` | Retry immediately in the same bounded pump; do not apply external backoff |
| `AwaitingExternalEvidence` | Persist and apply the external-probe schedule |
| `AwaitingFederationEvidence` | Wait for an accepted-operation notification, with a slow repair fallback |
| `Complete` | Remove all schedule state and stop |

The immediate pump must be bounded, for example to eight advancements per
wake. Reaching the bound schedules another cooperative wake rather than
recursing or monopolizing the runtime.

### Wake sources

The driver should merge these wake sources without allowing any of them to
supply settlement truth directly:

1. A wallet GET quote or POST mint request.
2. A backend payment event, when a backend supports one.
3. Application of a federation operation that references the quote or its
   funding action.
4. The persisted external-probe deadline.
5. A slow repair scan for missed notifications and restart recovery.

Notifications only request a new authoritative advancement attempt. Existing
backend and consensus validation remains responsible for the result.

### Suggested external-probe policy

The initial policy should favor a fast fresh-payment experience without
returning to an unbounded five-member polling loop.

| Quote age or trigger | Probe timing |
|---|---|
| Quote creation | Immediate |
| First 30 seconds | Every 2 seconds for primary observers |
| 30 seconds to 2 minutes | Every 5 seconds |
| After 2 minutes without progress | 10s, 20s, 40s, 80s, 160s, then 300s cap |
| Wallet request | Immediate when no probe ran during the last 1 second |
| Backend event | Immediate, subject only to an active attempt lease |

Use deterministic jitter for background probes. A wallet request must wake a
backed-off quote without resetting its persisted no-progress counter.

The precise constants should remain centralized and configurable only if a
real operational need appears. Do not spread timing constants across handlers
or payment backends.

## Implementation checklist

### Phase 0: observability and reproducible baseline

- [ ] Add one structured per-quote latency timeline covering:
  - [ ] public request received;
  - [ ] receive intent accepted;
  - [ ] invoice prepared;
  - [ ] mint quote accepted and returned;
  - [ ] first backend evidence observed;
  - [ ] backend evidence quorum accepted;
  - [ ] claim planned;
  - [ ] preimage disclosed;
  - [ ] claim succeeded;
  - [ ] result evidence quorum accepted;
  - [ ] quote marked paid;
  - [ ] mint request received; and
  - [ ] mint signatures issued.
- [ ] Use monotonic durations for local latency measurement; do not put local
      wall-clock measurements into deterministic consensus state.
- [ ] Correlate domain stages with federation operation IDs so AlephBFT
      lifecycle traces can be joined to a quote without logging secrets.
- [ ] Add fixed-label histograms for invoice creation, payment detection,
      backend finalization, paid transition, and issuance.
- [ ] Record external probe attempts, progress outcomes, immediate-pump steps,
      and repair wakes with bounded labels.
- [ ] Add a script or operator command that prints a redacted timeline for one
      quote or action.
- [ ] Capture at least three successful baseline receives before changing
      scheduling behavior.

Exit criteria:

- A receive can be divided unambiguously into payer/backend waiting,
  federation consensus, Bark claim, paid transition, and Cashu issuance.
- Metrics and logs contain no invoice, preimage, secret, peer credential, or
  unbounded quote label.

### Phase 1: generic internal progress pump

- [ ] Introduce the backend-independent advancement outcome described above.
- [ ] Restrict exponential backoff to `AwaitingExternalEvidence`.
- [ ] On `Progressed`, retry immediately in a bounded cooperative loop.
- [ ] On `AwaitingFederationEvidence`, wake on relevant accepted journal
      application instead of the five-second external polling cadence.
- [ ] Retain a slow repair scan for lost notifications and restarts.
- [ ] Persist only the state needed for external no-progress backoff and active
      attempt leases.
- [ ] Remove schedule state as soon as the quote becomes issued or terminal.
- [ ] Keep request-triggered wakes backend-independent.
- [ ] Verify that federated BDK follows the same generic scheduling rules.
- [ ] Verify that standalone payment processing is unchanged.

Tests:

- [ ] Multiple internal transitions complete without advancing a fake clock by
      five seconds between each transition.
- [ ] A no-progress external backend follows the persisted probe sequence.
- [ ] Restart preserves the next external-probe deadline and attempt counter.
- [ ] A wallet request wakes a backed-off quote but does not reset its counter.
- [ ] A journal notification wakes `AwaitingFederationEvidence` immediately.
- [ ] A missed notification is recovered by the repair scan.
- [ ] The bounded pump yields after its maximum number of steps.
- [ ] Concurrent wallet, timer, and federation wakes perform at most one active
      probe per quote and member.

Exit criteria:

- No successful internal state transition waits for the persisted external
  backoff or the five-second repair interval.
- Existing federation restart and convergence tests continue to pass.

### Phase 2: reuse Bark result evidence for mint payment

- [ ] Define the exact mapping from a threshold-matched Bark `Succeeded`
      observation key to the associated mint quote payment.
- [ ] When the third matching result observation is applied, update the mint
      quote payment in the same deterministic transaction or through one
      deterministic derived transition.
- [ ] Remove the second member-by-member `MintQuotePayment` observation round
      for federated Bark receives.
- [ ] Preserve the generic `MintQuotePayment` path for backends that do not
      already have an accepted threshold result-evidence family.
- [ ] Preserve payment identity, amount, fee, quote binding, result hash, and
      liability accounting checks.
- [ ] Publish the existing quote-paid notification only after the transaction
      commits.
- [ ] Make replay and catch-up derive the identical paid quote without Ark or
      Bark I/O.

Tests:

- [ ] Two matching result observations do not pay a 3-of-5 quote.
- [ ] The third matching observation pays it exactly once.
- [ ] A fourth or fifth observation is idempotent.
- [ ] Conflicting amount, fee, payment hash, result hash, source scope, or quote
      evidence cannot combine into quorum.
- [ ] Reordered accepted observations converge to the same paid state.
- [ ] Restart between result quorum and notification does not lose payment.
- [ ] The quote cannot be issued for more than the accepted net Bark value.

Exit criteria:

- A completed Bark result quorum directly authorizes the ordinary mint quote
  paid transition.
- No second payment-observation quorum appears in a successful receive trace.

### Phase 3: threshold-first Ark observation

- [ ] Deterministically select `threshold` primary observers for each receive
      action from accepted federation data.
- [ ] Let primary observers probe immediately according to the generic policy.
- [ ] Delay non-primary observers by a short deterministic fallback interval.
- [ ] Cancel fallback work when accepted matching evidence reaches threshold.
- [ ] Never perform Ark I/O after the local member already has matching
      accepted evidence for the current observation epoch.
- [ ] Do not submit evidence after quorum if the operation has not already
      entered consensus.
- [ ] Rotate or expand the primary set deterministically when liveness evidence
      shows that quorum has not been reached.
- [ ] Keep observer selection independent of backend error strings.

Tests:

- [ ] A healthy 3-of-5 receive performs three primary Ark probes rather than
      five.
- [ ] One unavailable primary causes a fallback observer to restore liveness.
- [ ] Two unavailable primaries leave the quote pending without unsafe claim.
- [ ] Accepted local evidence suppresses all later Ark probes by that member.
- [ ] Restart preserves the accepted-evidence suppression behavior.
- [ ] Primary selection is identical on every member and during replay.

Exit criteria:

- Healthy receives use only the required number of Ark observers.
- The fallback path still completes with any threshold of healthy members.

### Phase 4: Bark maintenance isolation and terminalization

- [ ] Check actor/member role before calling Bark maintenance methods with
      actor-only side effects.
- [ ] Ensure non-actors create only observations that claim their own member
      identity.
- [ ] Prevent all members from concurrently executing the same refresh action.
- [ ] Give active receive and claim work priority over routine refresh work.
- [ ] Add per-action maintenance leases or equivalent in-process exclusion
      where repeated timers can overlap.
- [ ] Classify expired HTLCs, invalid reserved inputs, and exhausted retry
      lineages into explicit safe terminal or operator-required states.
- [ ] Stop automatic retries for non-retryable FROST sessions.
- [ ] Remove terminal actions from automatic maintenance selection.
- [ ] Retain explicit authenticated operator retry only for states whose retry
      contract is safe and documented.
- [ ] Slow routine maintenance after no progress rather than running every five
      seconds forever.

Tests:

- [ ] Only the accepted actor performs refresh execution.
- [ ] Every member may submit valid member-scoped evidence without an author
      mismatch.
- [ ] A permanently expired receive becomes terminal and causes no more Ark
      probes.
- [ ] A non-retryable FROST session is not retried automatically.
- [ ] Receive finalization proceeds while unrelated maintenance is pending.
- [ ] Restart does not reactivate terminal automatic work.

Exit criteria:

- No `Federation operation author mismatch` occurs in a healthy maintenance
  round.
- Routine maintenance does not add unrelated operations to a receive's latency
  critical path.

### Phase 5: remove redundant submissions and setup operations

- [ ] Let all public members register a waiter for a deterministic operation ID
      while only one deterministic publisher initially places it into
      consensus.
- [ ] Add a short deterministic fallback publisher sequence for liveness.
- [ ] Apply this to wallet-fanned-out mint requests without changing the public
      Cashu API.
- [ ] Measure duplicate finalized items before and after the change.
- [ ] Evaluate materializing the mint quote atomically with accepted
      `ReceivePrepared` state.
- [ ] If quote materialization can be deterministic and replay-safe, remove the
      separate quote operation from Bark invoice preparation.
- [ ] Do not introduce a generic opaque operation batch merely to save one
      Bark setup step.

Tests:

- [ ] Five simultaneous identical mint requests produce one normal consensus
      submission in the healthy case.
- [ ] Publisher failure triggers a fallback without changing the operation ID.
- [ ] All public request waiters receive the same accepted result.
- [ ] Quote preparation replay produces the same invoice, payment hash, expiry,
      requester key, and quote state.

Exit criteria:

- Healthy wallet fan-out no longer produces four duplicate mint items.
- Invoice setup uses no more consensus transitions than its security model
  requires.

### Phase 6: optional Bark/Ark event wakeup

- [ ] Determine whether the supported Ark protocol exposes a stable mailbox,
      stream, or long-poll signal for incoming HTLC readiness.
- [ ] If available, expose it through Bark as a wake-only payment event stream.
- [ ] Reconnect with bounded backoff and cancellation.
- [ ] Treat stream data only as a hint to run the normal authoritative probe.
- [ ] Retain polling repair for disconnects, lag, and restarts.
- [ ] Do not require an Ark protocol extension for the first optimization
      release.

Exit criteria:

- When supported, an event wakes a fresh payment immediately without replacing
  independent member verification.
- Disabling or losing the event stream only affects latency, not correctness or
  eventual completion.

### Phase 7: consensus tuning only after operation reduction

- [ ] Measure submit-to-finalization and finalization-to-application latency per
      operation after Phases 1-5.
- [ ] Confirm whether AlephBFT's initial five-second unit delay or later
      round-aware delays materially affect active payment latency.
- [ ] Prefer reducing serial consensus operations over globally accelerating
      empty units.
- [ ] If still necessary, introduce a reviewed activity-aware production delay
      configuration with load, restart, partition, and recovery tests.
- [ ] Verify that tuning does not cause empty-unit amplification or network
      rebroadcast pressure.

Exit criteria:

- Any consensus timing change is justified by production measurements after
  redundant operations have been removed.

## Latency targets

Targets exclude time spent by the payer choosing to pay and external Lightning
routing before the payment becomes observable at the Ark server.

| Measurement | Current baseline | Initial target | Stretch target |
|---|---:|---:|---:|
| Wallet request to invoice ready | bounded below 10s, not precisely instrumented | below 5s p95 | below 3s p95 |
| Ark-visible HTLC to `receive_htlcs_ready` | up to about 10s | below 5s p95 | below 3s p95 |
| `receive_htlcs_ready` to Bark actor settlement | about 20s | below 10s p95 | below 6s p95 |
| Bark actor settlement to quote paid | about 20s | below 5s p95 | below 3s p95 |
| Quote paid to Cashu issuance | about 8s | below 5s p95 | below 3s p95 |
| Ark-visible HTLC to Cashu issuance | about 58s | 15-25s p95 | 10-15s p95 |

These targets must be evaluated over repeated receives. A single favorable
trace is not sufficient to mark a phase complete.

## End-to-end verification matrix

- [ ] 3-of-5 healthy receive with a freshly created quote.
- [ ] One primary observer offline before payment.
- [ ] Two non-threshold members offline after invoice creation.
- [ ] Actor crash before HTLC evidence quorum.
- [ ] Actor crash after claim plan acceptance.
- [ ] Actor crash after preimage disclosure but before claim result acceptance.
- [ ] Member restart while external probe backoff is persisted.
- [ ] All members restart after Bark settlement but before Cashu minting.
- [ ] Wallet GET fan-out during a backed-off quote.
- [ ] Wallet POST mint fan-out after paid notification.
- [ ] Ark timeout, unavailable response, and ambiguous response without parsing
      server-specific error strings.
- [ ] Old expired receive coexisting with a new successful receive.
- [ ] Bark refresh becoming due during an active receive.
- [ ] Federated BDK on-chain quote regression.
- [ ] Standalone BOLT11 backend regression.
- [ ] SQLite and PostgreSQL replay/convergence parity where applicable.
- [ ] Full federation checkpoint, catch-up, and restore regression.

For each receive, collect:

- total latency and every domain-stage latency;
- Ark probes per member and total Ark probes;
- accepted operations and duplicate finalized items;
- AlephBFT units with and without data;
- maintenance actions overlapping the receive;
- quote payment and issuance counts; and
- any retry, actor mismatch, stale action, or non-retryable session warning.

## Rollout plan

1. Deploy Phase 0 instrumentation without changing behavior and collect a
   multi-receive baseline.
2. Deploy Phase 1 generic state-pump behavior behind a federation-only runtime
   switch if a rollback seam is needed.
3. Verify Bark and federated BDK behavior before enabling the new driver for
   all federation funding backends.
4. Deploy Phase 2 quorum reuse and compare quote-paid latency and operation
   counts against the baseline.
5. Deploy Phase 3 threshold-first observation and verify Ark request volume and
   fallback liveness.
6. Deploy Phase 4 maintenance fixes before relying on automatic refresh in
   production.
7. Complete Phase 5 submission and setup deduplication after the receive path
   is stable.
8. Consider Phases 6 and 7 only if measured latency remains above target.

Rollback must preserve the journal and quote state. Scheduler changes may be
disabled, but accepted Bark operations, payment evidence, and liabilities must
never be deleted or rewritten.

## Completion checklist

- [ ] No internal progress transition waits for external probe backoff.
- [ ] Fresh quotes are detected quickly without permanent high-frequency
      polling.
- [ ] Healthy 3-of-5 Bark receives use three primary Ark observers.
- [ ] Bark result quorum pays the quote without a second payment quorum.
- [ ] Wallet fan-out does not place five identical mint operations into
      consensus.
- [ ] Bark maintenance performs actor-only side effects and does not generate
      author-mismatch warnings.
- [ ] Expired and non-retryable actions stop automatically.
- [ ] An Ark-visible payment reaches Cashu issuance within the initial p95
      target over repeated Nocean tests.
- [ ] Restart, replay, catch-up, threshold failure, and backend regression tests
      pass.
- [ ] Production metrics can explain every material receive delay without
      logging secrets or unbounded identifiers.
