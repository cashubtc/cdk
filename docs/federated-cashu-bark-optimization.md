# Federated Cashu Bark BOLT11 optimization implementation plan

Status: Milestones 0, 0A, and 1 complete; Milestone 2 is next

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

### Execution location

Run every test federation, repeated performance sample, fault injection, and
adverse-ordering workload on the local computer, not on the Nocean server. Use
a local five-member release federation because it is faster, easier to reset,
and produces cleaner comparisons. A local federation may still use the real
Signet Ark server, faucet, and store when the milestone needs end-to-end Bark
evidence. Nocean is limited to read-only field evidence and a narrowly scoped
final deployment smoke check after all local gates pass; it is not the test or
benchmark host.

Use the repository runner for the repeatable five-member topology:

```bash
scripts/run_local_bark_federation.sh fresh
BUILD=0 scripts/run_local_bark_federation.sh status
BUILD=0 scripts/run_local_bark_federation.sh runtime-metrics
scripts/analyze_local_bark_federation.sh
```

It builds release `cdk-mintd` and `cdk-cli`, renders the same Bark federation
control script used by the deployment helper, runs all five public member URLs
on loopback, and confines generated configs, secrets, databases, and logs to
the ignored `target/local-bark-federation` directory. Use `BUILD=0` after the
first build. `fresh` performs a new DKG ceremony and replaces only that
disposable target directory's generated federation state.

The analysis helper reads only the disposable local databases and emits
operation classes, duplicate counts, quote timing, redacted Bark movements,
and convergence digests. It never prints invoices, payment hashes, preimages,
seeds, private configs, or wallet proofs. Run it before restarting a candidate:
Bark replay currently refreshes old movement completion timestamps, so those
rows are authoritative for state but not immutable timing evidence after a
restart.

### Repeatable 2nd.dev Signet payment workflow

Use these workflows for every real-payment sample. They were verified against
`signet.2nd.dev` on 2026-07-21. Keep the five mint processes and the local CLI
on the local computer; only the test Lightning payment crosses to 2nd.dev.
Never record the raw invoice, preimage, payment hash, browser cookie, or wallet
state in this document or a commit.

#### Incoming: faucet pays the federation invoice

1. After every `fresh` DKG, import that ceremony's generated public config into
   a clean local CLI wallet before requesting money. A wallet retained from an
   earlier ceremony has a stale aggregate key; treating one new member share
   as its old aggregate correctly fails DLEQ verification. Import the current
   config with:

   ```bash
   target/release/cdk-cli \
     --work-dir "$WALLET_DIR" --non-interactive --unit sat \
     federation add \
     --public-config target/local-bark-federation/member-1/dkg-final/federation-public.json \
     --local-mint-url "$MINT_URL"
   ```

2. Start a local CLI mint against the local five-member candidate and keep it
   waiting. Capture the BOLT11 invoice outside the repository. The repeatable
   command is:

   ```bash
   target/release/cdk-cli \
     --work-dir "$WALLET_DIR" --non-interactive --unit sat \
     mint "$MINT_URL" "$AMOUNT_SAT" --wait-duration 600
   ```

   Run the faucet helper from a second terminal while this command waits.
3. Run `scripts/second_signet_faucet.sh "$INVOICE"`. The helper validates the
   Signet invoice, copies it to the macOS clipboard, and opens the exact faucet
   page. It never reads or persists browser cookies.
4. Reload the faucet before each payment. A stale page can still show the user
   menu after its OAuth session has expired; a click that never changes `Send
   sats` to `Processing...` is the tell. If the reloaded page shows `Sign in
   with GitHub`, sign in, wait for the faucet to return, and rerun the helper so
   the intended invoice—not a value retained by the page—is on the clipboard.
5. Select `Lightning`, paste the invoice, and click `Send sats`. The underlying
   endpoint is `POST /api/v1/faucet/lightning` with an `invoice` JSON field, but
   it returns HTTP 401 without the authenticated browser session; do not work
   around that boundary by copying cookies into a script.
6. Record the click and success times, then let the local CLI drive quote GET
   and POST mint fan-out. Verify issuance and five-member convergence locally.

#### Outgoing: federation pays a store invoice

1. Create a fresh invoice without navigating the store UI:
   `scripts/second_signet_store_invoice.sh --json 1000`. Valid product amounts
   are 1,000, 5,000, and 10,000 sats. The public helper calls
   `POST /api/v1/order/create` and returns only `order_id` and the Signet BOLT11
   invoice.
2. Extract the order ID and invoice in the shell; keep both outside the
   repository.

   ```bash
   store_order="$(scripts/second_signet_store_invoice.sh --json 1000)"
   ORDER_ID="$(jq -r .order_id <<<"$store_order")"
   INVOICE="$(jq -r .invoice <<<"$store_order")"
   ```

3. Start the bounded store-status check in the background. It mirrors the
   store's one-second status cadence for at most ten minutes and never prints
   the returned payment hash.

   ```bash
   scripts/second_signet_store_status.sh --wait "$ORDER_ID" &
   STORE_STATUS_PID=$!
   ```

4. Give the invoice to the local CLI melt command against the local
   five-member federation:

   ```bash
   target/release/cdk-cli \
     --work-dir "$WALLET_DIR" --non-interactive --unit sat \
     melt --mint-url "$MINT_URL" --method bolt11 --invoice "$INVOICE"
   wait "$STORE_STATUS_PID"
   ```

5. Record quote creation, POST melt, Bark/Ark settlement, Cashu finalization,
   CLI response, and store `PAID` times separately; then verify local
   convergence, balances, quote/saga state, and VTXO accounting.

If either public store response schema changes, stop and re-verify the current
page implementation before changing the helper. The browser store remains the
fallback: open `https://signet.2nd.dev/store`, choose a product, click `Buy
now`, and copy the displayed invoice.

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
- focused local harnesses, a local five-member release federation, the local
  `cdk-cli`, real Signet payment demonstrations, and a final Nocean deployment
  smoke check.

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
0 therefore captured the fresh successful outgoing samples below before any
outgoing optimization.

### Local release baseline: three incoming and three outgoing successes

Milestone 0 established a clean five-member release baseline on local loopback
at source `43f19254c595`, with the Bark fork at `e68252989858`, SQLite,
threshold `t = 3`, generic payment threshold `q = 4`, and the public Signet Ark
server. The release binary hashes were
`acf9920803eb69c290f09f5805b0a5b3d923c0ca513ded8b4f14b115c513b344`
for `cdk-mintd` and
`431b323d2065bae53a5cc50e00c01710a297ae87f5c323efc955476e620bc0b2`
for `cdk-cli`.

The first production-like local sample found the maintenance defect recorded
below. To measure the payment path rather than a known broken refresh, the
three-pair baseline used the same fresh federation with automatic refresh
disabled only in its ignored generated local configs. This is a
maintenance-isolated baseline, not evidence that disabling refresh is an
acceptable deployment fix.

Incoming samples used 10,000-sat invoices and the authenticated Signet faucet:

| Sample | Faucet click to success | Faucet success to CLI issuance | Durable quote to issued | Accepted operations | Duplicate finalized items |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 96.753s | 40.176s | 157s | 22 | 3 |
| 2 | 41.717s | 31.738s | 102s | 22 | 4 |
| 3 | 103.056s | 45.142s | 165s | 22 | 3 |
| Median | 96.753s | 40.176s | 157s | 22 | 3 |

The faucet stage is external Lightning routing and dominates the variance. The
post-success stage is the wallet-observed bound from faucet success through
federation detection, claim, result quorum, quote payment, POST mint, and
balance persistence. The durable quote duration additionally includes payer
interaction before the faucet click. With only three samples, p95 is not
reported; the observed maxima are shown directly.

Each incoming accepted exactly one intent, one preparation, one mint quote,
five HTLC observations, the three ordered ready/claim transitions plus one
claim plan, five result observations, four generic payment observations, and
one mint: 22 operations. Bark rows show member evidence hydration beginning at
different scheduler deadlines and the accepted terminal result converging
across all members. The remaining approximately 32-45 seconds after faucet
success is therefore a named external-probe/federation-finality/Cashu-issuance
tail, not payer routing.

Outgoing samples paid fresh 1,000-sat store invoices with a 25-sat fee:

| Sample | CLI process wall time | Durable quote to paid | Accepted operations | Duplicate finalized items | Store result |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 24.819s | 37s | 16 | 4 | `PAID` |
| 2 | 30.961s | 34s | 16 | 4 | `PAID` |
| 3 | 23.693s | 37s | 15 | 4 | `PAID` |
| Median | 24.819s | 37s | 16 | 4 | `PAID` |

The durable quote can finish materializing on all members after the first
wallet response, which explains its coarse whole-second duration differing
from the client process clock. Samples one and two accepted five generic melt
observations; sample three reached `q = 4` before the fifth entered consensus
and therefore used 15 operations. All three accepted one quote, one melt
reservation, one send plan, HTLC-cosigned/payment-initiated/payment-succeeded
transitions, five Bark result observations, and four or five generic payment
observations.

Across the six successful lifecycles the five mint processes consumed 40.89
aggregate CPU-seconds, or 6.815 seconds per lifecycle across all members. RSS
rose from approximately 52 MiB per member before the series to 84-88 MiB after
the series. The existing broad signature-verification counter increased, but
it mixes consensus, checkpoint, and signing verification and is not claimed as
a Bark-only measure.

The threshold-three FROST coordinator sends exactly three commitment requests,
three signature-share requests, and three aggregate-result notifications for
each custody action: nine authenticated messages. A healthy incoming claim and
a healthy outgoing HTLC construction each use one such action, so the six
successful samples used 54 FROST messages. The aggregate-result cache grew once
per action on every member, confirming the session count. A failed send plus
revocation uses two actions and therefore 18 messages.

### Disposable Ark-call attribution sample

A fourth incoming/outgoing pair used temporary, identifier-free call-site
probes in `cdk-bark`; the probes were reverted and the clean release binary was
rebuilt before Milestone 0 completed. It is excluded from the latency medians.

The 10,000-sat incoming lifecycle made 25 Ark-facing Bark-facade calls:

- one invoice-start call;
- four receive-status polls from each of five members, 20 total;
- one actor HTLC preparation and two follower hydrations, exactly the three
  evidence producers that entered consensus;
- one actor claim call.

It accepted 20 operations because evidence reached threshold after three HTLC
observations; it still accepted all five terminal result observations and four
generic payment observations. The 1,000-sat outgoing success made eight
Ark-facing calls: one HTLC construction, one payment initiation, one terminal
failure check from each member, and one actor success check. It accepted 16
operations. No rate-limit response occurred.

These are facade-call counts, not an assertion that every facade call maps to
one transport request inside Bark. They identify the owning CDK call path and
the exact work visible at the integration boundary. Milestone 4 must measure
the underlying status and hydration transport calls before changing their
policy.

### Local terminal failure and safe revocation baseline

A fresh, correctly signed Signet invoice for a deliberately unreachable node
exercised terminal failure without transferring funds. The client returned
`Payment failed` in 49 seconds. The lifecycle accepted 22 operations and four
duplicate finalized items:

- one melt quote and one melt reservation;
- send and revocation action plans;
- send planned, HTLCs cosigned, payment initiated, payment failed, revocation
  planned, and revocation completed transitions;
- five terminal-failure and five recovered-result member observations; and
- four generic failed-melt observations.

All five members converged on the same 135-operation journal and application
digests immediately after the sample. The failed quote remained unpaid, the
wallet balance remained 26,925 sats, and the canonical Bark projection held
exactly 26,925 sats in four spendable VTXOs with no reserved or pending-spend
value. The failed movement recorded intended value `-1000`, effective value
zero, and fee zero. Aggregate member CPU for this 49-second failure/revocation
path was 22.03 seconds. The disposable invoice generator, raw invoice, client
output, and temporary probes were removed.

### Critical maintenance blocker found before isolation

With deployment-equivalent automatic refresh enabled, the first received
10,000-sat VTXO had approximately 44 blocks remaining while the configured
refresh margin was 144. Refresh therefore began immediately, reserved the only
spendable VTXO, and made a subsequent melt quote fail with zero spendable Bark
value. The payment itself had succeeded; maintenance had captured its output.

The refresh added six accepted operations: one plan plus five member
observations. Non-actor members attempted to publish the actor's plan and
logged author mismatches. The actor then retried execution every five seconds.
A disposable diagnostic replay confirmed Bark failed to decode its own stored
round participation with `wrong msgpack marker Array16`; 101 identical
never-started rows accumulated while the retry loop ran.

The confirmed serialization cause is in the Bark fork's positional MessagePack
encoding: optional `SerdeRoundParticipation` fields are omitted with
`skip_serializing_if`, shifting later optional custody and federation fields
into the wrong positional slots on decode. CDK also needs to ensure only the
deterministic plan actor publishes a newly proposed maintenance plan. This is a
production liveness and liquidity blocker, not a payment-performance tuning
candidate, and is isolated as Milestone 0A.

### Current consensus-entry cardinality

Let `n` be the member count, `t` the Bark/FROST threshold, and `q` the generic
payment-observation threshold. The active Nocean public config was checked on
2026-07-21 and has `n = 5`, `t = 3`, and `q = 4`; signed status independently
reported `required=4` for both mint and melt payment observations.

These counts cover Bark and payment-observation entries. Incoming quote
creation, outgoing melt quote/reservation, the final Cashu mint operation, raw
AlephBFT protocol packets, private FROST messages, Ark RPCs, and wallet HTTP
fan-out are reported separately.

| Flow | Minimum formula | Nocean minimum (`t=3`, `q=4`) | Current 5-member range |
| --- | ---: | ---: | ---: |
| Incoming success | `6 + 2t + q` | 16 | 16-21 |
| Outgoing success | `4 + t + q` | 11 | 11-14 |
| Outgoing failure plus revocation | `6 + 2t + q` | 16 | 16-21 |

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

- Run at least three equivalent deterministic or controlled local samples and
  three equivalent local-federation Signet samples for a noisy end-to-end
  comparison. Compare medians and report p95 when the sample count is
  sufficient; never report only the best trace.
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
3. **Local federation and real Signet.** Run the exact release binaries and
   source commit as five local members against the public Ark server. Use the
   local CLI, Signet faucet, and Signet store. Require final convergence across
   all five local members. Nocean is used only for the final deployment smoke
   check after these measurements pass.

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

The driver owns persisted schedules, active-attempt leases and coalescing,
accepted-operation wakes, optional backend event wakes, wallet-request wakes,
and terminal cleanup. Backend advancement owns protocol state and returns only
the reason it cannot continue. It does not own generic timers.

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
| 0 | Establish reproducible incoming and outgoing baselines with causal timelines | Complete | `7403e90c0` |
| 0A | Restore maintenance round persistence, actor ownership, and payment liquidity | Complete | `32f01bbf9` (CDK), `aa98f568` (Bark) |
| 1 | Remove unnecessary inter-operation delay with one generic mint/melt advancement driver | Complete | recorded in the milestone commit; hash added by the next update |
| 2 | Determine and implement the smallest safe Bark incoming/outgoing event wake, or reject it | Not started | - |
| 3 | Minimize consensus messages by deriving Cashu payment from accepted Bark result quorum | Not started | - |
| 4 | Reduce excess Ark probes and observations with threshold-first polling and measured backoff | Not started | - |
| 5 | Remove duplicate wallet submissions and any proven redundant serial Bark operation | Not started | - |
| 6 | Isolate Bark maintenance from payment hot paths and terminalize permanent failures | Not started | - |
| 7 | Tune consensus timing only if post-reduction evidence still shows a material delay | Not started | - |
| 8 | Pass sustained local, CLI, browser-based Signet, and Nocean smoke gates | Not started | - |

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
- Three local incoming and three local outgoing successes now provide clean
  release baselines, and one controlled failure proves exact-value revocation.
- A disposable probe run attributes exact Ark-facing facade calls and confirms
  the current five-member polling amplification.

### Checklist

- [x] Reconstruct a redacted joined timeline from existing logs and durable
      state for every material stage observable today.
- [x] Measure accepted operation kinds, duplicate finalized items, Ark RPCs,
      observer attempts, FROST traffic, maintenance overlap, CPU, and RSS using
      existing evidence or one-off measurements; do not add permanent counters
      solely for this baseline.
- [x] For any material evidence gap, add the smallest isolated disposable probe
      or local analysis tool, capture the result, and revert or delete it before
      completing the milestone.
- [x] Extend a deterministic test harness to script incoming and outgoing Bark
      outcomes and fake time.
- [x] Run at least three release incoming and three release outgoing
      lifecycles on a five-member local deployment using the local CLI.
- [x] Run at least three incoming and outgoing Signet lifecycles against the
      same local five-member candidate, using the faucet and store.
- [x] Include one outgoing terminal failure and safe revocation, but report it
      separately from successful latency.
- [x] Record cold CLI, warm wallet, server-only, payer-controlled, and external
      routing time separately.
- [x] Confirm exact five-member convergence, quote accounting, balances, VTXO
      state, and absence of safety alerts after every sample.
- [x] Confirm that no temporary instrumentation or one-off analysis artifact
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

Milestone 0 is `Complete`, decision `Kept` as a baseline-only milestone. The
two production incoming traces, consensus cardinality review, three local
incoming successes, three local outgoing successes, one safe outgoing failure,
one disposable attribution pair, stage medians, operation/duplicate counts,
FROST message cardinality, Ark-facing call counts, CPU, RSS, accounting, and
five-member convergence are recorded above. Source review confirmed the
current five-second scan, asymmetric mint/melt scheduling, inactive CDK
payment event stream, and available Bark mailbox primitives. No production
scheduling or protocol behavior changed.

The execution location was corrected during this milestone: a planned remote
sample was stopped before any federation mint quote or payment was submitted,
and all repeated tests now target a local five-member release federation. The
active Nocean public config and signed status corrected the assumed generic
payment threshold from three to four. Faucet UI/OAuth behavior and the public
store order APIs were verified; several disposable 1,000-sat store orders were
created but deliberately left unpaid while checking response schemas. The
three redacted helpers in `scripts/` passed shell syntax, invalid-input, live
invoice-creation, and pending-status smoke checks. The faucet helper retains no
authentication material.

The shared deployment control renderer was extracted so local and Nocean Bark
topologies cannot drift. A clean local release build, two DKG ceremonies, and a
stop/restart cycle exposed and fixed two setup-to-runtime handoff defects: all
setup members are now stopped before any finalized runtime starts, and the
selected tmux session name is passed explicitly to every pane. The corrected
`fresh` run reached five-member readiness without a manual restart. Generated
private material and runtime state remained under the ignored target tree.

Focused checks passed:

```text
cargo test -p cdk-axum --features federation-bark \
  receive_actor_advances_from_cache_only_after_matching_quorum
cargo test -p cdk-axum --features federation-bark payment_schedule
cargo test -p cdk-sqlite --features federation \
  federation_bark_projection_persistence
```

The first command passed the cached-evidence/no-extra-Ark-I/O decision test;
the second passed five fake-time persistence, lease, cooldown, jitter, and
backoff tests; the third passed the full scripted incoming, outgoing success,
failure, revocation, replay, and idempotent projection lifecycle on SQLite.

Temporary call probes and the temporary unreachable-invoice generator were
reverted. The clean release binary was rebuilt after the probe run. The only
retained tooling is `scripts/analyze_local_bark_federation.sh`, a bounded
read-only helper that emits no payment or custody secrets. The intended
milestone commit subject is
`test: establish federated Bark payment performance baseline`.

## Milestone 0A: restore maintenance persistence and liquidity

### Goal

Make routine Bark refresh replayable and actor-correct before optimizing the
payment path. A due refresh must not permanently reserve all spendable value,
retry a deterministic serialization failure every five seconds, or prevent a
new outgoing payment.

### Confirmed cause and scope

- Bark serializes `SerdeRoundParticipation` positionally with
  `rmp_serde::to_vec` while optional fields are conditionally omitted. A record
  with no unblinded mailbox ID but with custody/federation fields cannot be
  decoded into the same field layout and fails with `wrong msgpack marker
  Array16`.
- Every failed retry inserts another identical never-started round row; 101
  rows accumulated in the disposable reproduction.
- The accepted refresh plan reserves the spendable VTXO before execution, so
  the retry loop removes the federation's outgoing liquidity.
- CDK lets non-actors publish an actor-authored newly proposed maintenance
  plan, producing deterministic author mismatches and surplus submissions.
- Terminal refresh replay re-applies the earlier accepted plan after its input
  is already spent. The Bark facade rejected that valid later state, so every
  member restarted Bark-degraded even though the accepted journal and local
  output agreed. The actor also retained its confirmed round row after the
  terminal transition.
- The generated 144-block refresh margin equalled the public Ark server's
  144-block VTXO lifetime. A successful refresh output was therefore already
  inside the next refresh window and immediately started another round. The
  generated default is now 72 blocks. Custom policy remains an explicit
  federation configuration decision; this fix does not make existing accepted
  public configs fail startup.
- The serialization fix belongs in the configured `../bark-cdk` fork; actor
  publication belongs in CDK. No automatic-refresh disable is an acceptable
  production fix.

### Checklist

- [x] Add a Bark regression fixture for every optional-field combination in
      round participation, including the failing custody-without-mailbox case.
- [x] Choose the smallest stable encoding fix and document compatibility with
      existing valid rows.
- [x] Recover or safely discard only malformed never-started federation rows;
      do not reinterpret an ambiguous started round.
- [x] Prove a restarted wallet resumes or cleanly re-proposes the same accepted
      refresh without duplicating value or custody side effects.
- [x] Restrict new maintenance-plan publication to the deterministic accepted
      actor; non-actors may still submit their own observations.
- [x] Ensure a permanent maintenance failure becomes operator-visible and
      stops hot-loop retries without silently releasing an unsafe reservation.
- [x] Keep the generated refresh margin strictly inside the public Ark
      server's advertised VTXO lifetime without invalidating existing accepted
      federation configs.
- [x] Re-enable automatic refresh in a fresh local five-member release
      federation.
- [x] Receive funds whose VTXO is inside the refresh margin, complete refresh,
      then pay a store invoice from the refreshed value.
- [x] Restart before and after round persistence and verify identical Bark
      projection, VTXO value, action phase, journal, and application digests.
- [x] Measure refresh operations, attempts, FROST messages, Ark calls, CPU, and
      payment latency; confirm no five-second unbounded growth remains.
- [x] Revert all temporary instrumentation before completion.

### Success criteria

- The positional decode failure is reproducibly impossible for newly written
  rows and the compatibility decision is covered by fixtures.
- A due refresh reaches a terminal accepted state and preserves exact value
  across all five members and restart.
- Only the accepted actor publishes and executes the refresh plan; no author
  mismatch or duplicate actor side effect appears.
- The refreshed federation can immediately create and complete an outgoing
  payment.
- No retry, row, operation, or CPU counter grows after terminal completion.
- A refreshed output is not immediately eligible for another refresh solely
  because the generated margin equals the server's complete VTXO lifetime.
- The milestone is committed as
  `fix: restore federated Bark refresh persistence`.

### Progress record — 2026-07-21

Milestone 0A is `Complete`, decision `Kept` as a correctness and liveness fix.
The corresponding Bark fork changes are committed as `aa98f568`.
The Bark fork now writes named MessagePack fields, decodes the current schema
first, and recognizes only the exact historical malformed layout. Fixtures
cover all eight combinations of the three optional participation fields. The
compatibility path deletes only a never-submitted interactive federation row
with no mailbox, no new VTXOs, no sent forfeits, and both custody fields; an
otherwise identical row with sent forfeits fails closed as ambiguous.

An accepted refresh now resumes the one persisted round whose complete inputs,
output policy and amount, custody session, and request commitment match the
accepted plan. That lookup happens before fee/capability planning, so a member
that already has the round response does not repeat those Ark reads. A spent
input is allowed only while replaying its already accepted plan; creating a
new round still requires an available input. The accepted terminal transition
acknowledges and removes the actor's confirmed row. Regression tests cover
spent-input replay, complete persisted-participation binding, exact legacy-row
handling, and restart persistence.

CDK now gates a new maintenance proposal on the deterministic actor before
calling the Bark planner. Non-actors perform neither proposal Ark I/O nor
publication, but still observe an accepted plan. A one-input/one-output refresh
therefore contributes five accepted journal entries: one plan, three distinct
trigger observations, and one completion transition. Its custody path uses
one threshold Schnorr operation and one virtual MuSig2 forfeit operation. With
three accepted signers, the source-derived request count is nine authenticated
Schnorr messages (commitment, share, and aggregate-result notification per
signer) plus six virtual-MuSig2 messages (commitment and partial per signer).
No new consensus vote or custody retry layer was introduced.

The fresh five-member Signet run received and issued exactly 10,000 sats. The
sole actor's first refresh movement began at 01:30:56 UTC, survived a clean
restart while unconfirmed with one movement and one round row, and reached the
accepted `refresh_completed` phase after public Signet confirmation. All five
members then held the same 10,000-sat spendable replacement. External block
production dominated the refresh wall time and is reported separately from
implementation latency.

That run also demonstrated that the former generated 144-block margin equalled
the public server's 144-block VTXO lifetime: a replacement confirmed with about
141 blocks remaining and was immediately due again. Two later accepted rounds
reproduced the churn without increasing row count beyond one. The renderer now
uses 72 blocks. This remains safely above the 18-block emergency margin and
18-block outgoing CLTV bound while leaving a 72-block post-refresh window. The
change applies to newly generated configs and deliberately does not invalidate
an already accepted public policy.

After the old-policy disposable run was stopped at a terminal transition, the
released value paid a fresh 1,000-sat Signet store invoice. Bark recorded a
single successful send from 02:48:33 to 02:49:08 UTC with a 25-sat fee; the
Cashu melt quote reached `PAID` in 50 seconds and the store returned `PAID`.
The CLI and every member ended at exactly 8,975 spendable sats
(`10,000 - 1,000 - 25`). The exact feature-complete release was then restarted:
all five members reported the same 57-operation order and application digests,
zero pending round rows, zero pending movements, and no recent reconciliation,
maintenance, rate-limit, or panic error.

The post-restart bounded runtime sample reported 6.57-6.67 CPU-seconds and
109,360-119,904 KiB RSS per member while replaying the complete local history.
The observed permanent-error retry sequence before repair was 5, 10, 20, 40,
80, 160, then 300 seconds; after repair no failure counter grew. Temporary
diagnostic logging was removed before the final release build.

Focused and full checks passed:

```text
cd ../bark-cdk && nix develop --command just check
cd ../bark-cdk && nix develop --command just unit federation::tests
cargo test -p cdk-bark --features federation-bark
cargo test -p cdk-axum --features federation-bark bark_maintenance::tests
cargo test -p cdk-axum --features federation-bark \
  only_refresh_authorizes_an_output_locator
cargo fmt -p cdk-bark -p cdk-axum -- --check
```

The intended milestone commit subject is
`fix: restore federated Bark refresh persistence`.

## Milestone 1: generic progress pump and inter-operation wakes

### Goal

Remove the five-to-ten-second waits that currently occur between successful
internal state transitions. External no-progress probing keeps bounded
backoff; accepted federation progress does not.

### Preconditions

- Milestone 0 identifies the current delay between at least two internal
  transitions and records the exact scheduler outcome that caused it.

### Checklist

- [x] Introduce the four backend-independent advancement results without
      exposing Bark semantic phases to the generic observer.
- [x] Use one persisted schedule implementation for federated mint and melt
      quotes.
- [x] Restrict exponential backoff to `AwaitingExternalEvidence`.
- [x] On `Progressed`, continue in a bounded eight-step cooperative pump.
- [x] On `AwaitingFederationEvidence`, register a wake for accepted operations
      that reference the quote/action.
- [x] Keep one slow repair scan for lost notifications, startup races, and
      restart; do not use it as the normal progress clock.
- [x] Keep the current external 5s-to-300s backoff constants during this
      milestone to isolate the internal-delay change.
- [x] Let wallet GET/POST wake the same driver; do not reset persisted backoff.
- [x] Remove schedules for issued mint quotes and terminal melt/action states.
- [x] Verify federated BDK follows the generic contract and has no Bark branch.
- [x] Verify standalone payment observation is unchanged.

### Tests

- [x] Multiple scripted internal transitions complete without advancing fake
      time by five seconds between each one.
- [x] No-progress external evidence follows the persisted sequence.
- [x] Restart preserves deadline, counter, phase, and lease recovery.
- [x] A wallet wake does not reset no-progress state.
- [x] A relevant journal apply wakes the exact quote immediately.
- [x] An unrelated operation does not wake every quote.
- [x] A missed wake is recovered by the slow repair scan.
- [x] The pump yields at eight steps and resumes cooperatively.
- [x] Concurrent wallet, timer, journal, and backend wakes produce at most one
      active attempt per quote/member.
- [x] Pending melts no longer run every five seconds independently of their
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

### Progress record — 2026-07-21

Milestone 1 is `Complete`, decision `Kept`. The confirmed cause was not one
payment-method-specific timer but the observer contract: successful Bark
transitions, federation waits, and genuine external no-progress all collapsed
to one integer result and one delayed loop. Mint quotes had a persisted
schedule; melts were rescanned independently every five seconds. Accepted
journal progress therefore commonly waited for another timer even though the
next input was already local.

The retained implementation replaces that split with one v2 mint/melt
schedule and four backend-independent advancement results. `Progressed` runs
again immediately, bounded to eight cooperative steps. An exact applied
operation wake advances only the quote named by a mint, melt, payment, chain,
or Bark operation. `AwaitingFederationEvidence` waits for that wake and keeps
only a 60-second repair deadline. `AwaitingExternalEvidence` alone increments
the persisted 5, 10, 20, 40, 80, 160, then 300-second backoff with deterministic
20% member jitter. `Complete` removes the exact schedule. The v1 mint schedule
is migrated on first read; deadlines, counters, wait reason, operation cursor,
generation, and leases survive restart.

Wallet requests use the same driver without resetting the external counter.
The existing five-second mint-request cooldown remains in place to avoid
turning wallet fan-out into Ark amplification. Melts preserve their prior
request-driven GET/POST behavior. In both cases a 15-second persisted lease
admits at most one active attempt per quote/member; concurrent callers wait for
that attempt to finish instead of racing it or returning inconsistent
`Pending`/`Paid` snapshots. Backend events and accepted operations use the
same lease. No error text or Bark phase appears in the scheduler.

The measured candidate was built in release mode from CDK base `32f01bbf9b43`
plus this milestone and Bark `aa98f5685d70`, using Rust 1.95.0 on a local ARM64
host. The topology was five loopback members, threshold three, SQLite per
member, the public Signet Ark service, and the local CLI. The Ark server version
is not exposed by the configured client interface. The final release hashes
were `14ae930bcbc9d05b868e8224615b4778f0abe663dfef7ce060d6a4df8a4742a6`
for `cdk-mintd` and
`431b323d2065bae53a5cc50e00c01710a297ae87f5c323efc955476e620bc0b2`
for `cdk-cli`.

Three incoming samples were executed in order with 10,000-sat faucet
payments. The first quote was deliberately excluded from raw quote-duration
statistics because faucet reauthentication delayed payment by about nine
minutes; its first detected Bark movement to issuance was 24.0 seconds. The
next two browser-submit-to-issuance bounds were 33.4 and 42.4 seconds. The
24.0/33.4/42.4 post-submit-or-detection median was 33.4 seconds, 16.9% below
the 40.176-second baseline post-payment median. Durable quote durations for
the two clean payer samples were 67 and 66 seconds. Each completed incoming
used 23 accepted operations: one more than the 22-operation baseline because
all five generic observations arrived in these samples. That 4.5% increase is
inside the milestone regression budget; Milestone 3 owns removing the generic
observation round entirely.

Three 1,000-sat outgoing samples completed in 27, 31, and 28 seconds of CLI
wall time. Their durable quote-to-paid times were 26, 31, and 26 seconds, a
26-second median and a 29.7% reduction from the 37-second baseline. Each used
16 accepted operations, equal to the baseline median. After the final
concurrency correction, the exact rebuilt candidate paid one additional
1,000-sat store invoice in 27 seconds; its durable quote reached `PAID` in 26
seconds, used 16 accepted operations and four duplicate finalized items, and
the wallet balance changed exactly from 26,925 to 25,900 sats including the
25-sat fee. The store also reported `PAID`.

The direct scheduler counter is zero delayed deadlines between scripted
`Progressed` steps: all eight allowed steps run without fake-time advancement,
then yield. External automatic-probe constants and call sites are unchanged.
The exact candidate produced no `local_rate_limited`, resource-exhaustion, or
panic log. Accepted Bark observation cardinality remained five HTLC/result
observations where required, and outgoing consensus cardinality did not grow.
Underlying Ark transport RPC counts remain a Milestone 4 measurement because
the Bark facade does not expose a one-call/one-transport mapping; no claim is
made beyond the unchanged CDK call sites and the observed absence of rate
limiting.

The restart probe left one unpaid quote active. Before restart every member
persisted one external-wait schedule with no-progress counter three. After a
release restart the same quote remained scheduled on every member with
counters four or five according to its persisted jittered deadline and zero
active leases; it did not reset. After the exact payment smoke, all five
members converged at 136 accepted operations with identical order and
application digests. The local CLI balance was exact. A bounded post-replay
sample reported 6.79-7.08 CPU-seconds and 110,032-119,840 KiB RSS per member,
with no growing automatic-work or recovery counter.

Focused and full gates passed:

```text
cargo fmt -p cdk-axum -- --check
cargo check -p cdk-axum --features federation
cargo check -p cdk-axum --features federation-bark
cargo test -p cdk-axum --features federation-bark federation::payment
cargo test -p cdk-axum --features federation-bark payment_observer
cargo test -p cdk-axum --features federation-bark pending_federated_melt
cargo test -p cdk-axum --features federation-bark \
  duplicate_process_melt_aleph_bft_pending_requests_join_one_queue_item
RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation-bark --lib
cargo test -p cdk-bark --features federation-bark
```

The full `cdk-axum` run passed 529 tests with two intentional ignores. Its
existing deeply nested concurrent-route stress requires an 8 MiB test-thread
stack; the ordinary 2 MiB Tokio test stack overflows before an assertion. No
production stack setting or test annotation was added. Federated BDK compiles
through the generic path without a Bark branch, and the scheduler is entered
only when federation configuration exists, leaving standalone observation
unchanged. Temporary diagnostic probes were not retained. The intended
milestone commit subject is
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
`r = max(t, q)`. Nocean has `t = 3`, `q = 4`, and `r = 4`.

| Flow | Current minimum | Candidate minimum | General reduction | Nocean reduction |
| --- | ---: | ---: | ---: | ---: |
| Incoming success | `6 + 2t + q` | `6 + t + r` | `min(t, q)` | 16 -> 13, save 3 |
| Outgoing success | `4 + t + q` | `4 + r` | `min(t, q)` | 11 -> 8, save 3 |
| Outgoing failure/revocation | `6 + 2t + q` | `6 + t + r` | `min(t, q)` | 16 -> 13, save 3 |

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
  which is three entries with Nocean's `t = 3` and `q = 4`.
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

- [ ] Healthy incoming HTLC evidence uses three primary observers; a Bark
      result family carrying Cashu authority uses four because `t = 3`, `q =
      4`, and `max(t, q) = 4`.
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
      local five-member federation, local CLI, faucet, and store.
- [ ] Deploy the exact candidate to Nocean only after local gates pass.
- [ ] Perform only a narrowly scoped Nocean deployment smoke check; do not run
      the repeated workload or member-failure matrix on the remote server.
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
| 2026-07-21 | active public config and signed status | Nocean, read-only field evidence | threshold correction | Bark/FROST threshold is 3; mint and melt payment-observation threshold is 4; active minimums are therefore 16/11/16, not 15/10/15 | Milestone 3 must use `max(t, q) = 4`; the three-operation reduction remains unchanged |
| 2026-07-21 | workflow verification | local browser and shell against 2nd.dev Signet | payment-helper smoke, no payment submitted | faucet login succeeded through GitHub OAuth; unauthenticated faucet API returned 401; public store create/status returned a redacted Signet invoice and `PENDING`; disposable orders were left unpaid | Repeated samples can avoid store UI navigation while faucet authentication remains correctly confined to the browser |
| 2026-07-21 | local federation runner | local release, five loopback members, Bark Signet | DKG and runtime readiness, no payment submitted | clean release build passed; corrected `fresh` completed DKG and reached five-member quorum without intervention; restart reached readiness in about 12s | The same rendered control topology can now be reset and measured locally without using Nocean as a benchmark host |
| 2026-07-21 | `43f19254c595`, Bark `e68252989858` | local release, five loopback members, SQLite, refresh isolated, real Signet | three 10,000-sat incoming successes | faucet routing 96.753/41.717/103.056s; post-faucet CLI issuance 40.176/31.738/45.142s; 22 accepted operations each; duplicate items 3/4/3 | External routing is noisy; the median 40.176s post-payment tail and repeated 22-operation path are the server optimization baseline |
| 2026-07-21 | same baseline | local release, same federation and wallet, real Signet | three 1,000-sat outgoing successes | CLI wall 24.819/30.961/23.693s; durable quote-to-paid 37/34/37s; accepted operations 16/16/15; four duplicate items each; all store orders `PAID` | Current successful outgoing median is 24.819s wallet-visible; one late fifth generic observation is avoidable work |
| 2026-07-21 | same baseline | local release, same federation | unreachable 1,000-sat outgoing failure | 49s; 22 accepted operations; four duplicates; two nine-message FROST actions; exact 1,025-sat reservation restored; all members converged; balance unchanged | Failure and revocation are live, exact-value, and safe; they are a separate latency class from successful sends |
| 2026-07-21 | disposable probes, reverted | local release, same federation, real Signet | one incoming and one outgoing attribution pair | incoming: 25 Ark-facing calls, 20 accepted operations; outgoing: eight Ark-facing calls, 16 accepted operations; no rate limit | Five members made 20 incoming status polls before three hydrated evidence; successful outgoing made five member terminal polls plus one actor success poll |
| 2026-07-21 | disposable maintenance diagnosis, reverted | local release, automatic refresh enabled | receive followed by outgoing attempt | refresh reserved the only 10,000-sat VTXO; outgoing quote failed; 101 malformed never-started round rows accumulated; MessagePack decode failed every five seconds | Bark optional-field positional serialization and non-actor plan publication are critical liveness blockers isolated as Milestone 0A |
| 2026-07-21 | Milestone 1 candidate based on `32f01bbf9b43`, Bark `aa98f5685d70` | local release, five loopback members, SQLite, real Signet | three incoming and three outgoing successes | incoming post-submit/detection 24.0/33.4/42.4s, median 33.4s; outgoing durable 26/31/26s, median 26s; incoming 23 operations each; outgoing 16 each; no rate limit | Generic operation wakes and the immediate bounded pump reduced targeted medians 16.9% incoming and 29.7% outgoing without changing external automatic-backoff constants |
| 2026-07-21 | exact final Milestone 1 binary | same local federation after release rebuild/restart | one 1,000-sat outgoing smoke | 27s CLI, 26s durable, 16 accepted operations, four duplicates, exact 1,025-sat balance reduction, store `PAID`, all five members converged | Request coalescing preserved payment latency and cardinality while fixing concurrent `Pending`/`Paid` response races |

## Completion checklist

- [x] External probing, internal progress, and wallet issuance are measured and
      scheduled as separate concerns.
- [x] No successful internal transition waits for external no-progress backoff.
- [x] Mint and melt use one backend-independent advancement driver.
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
- [ ] Repeated local federation, CLI, faucet, and store measurements satisfy
      the retained milestone gates; the Nocean deployment smoke check passes.
- [ ] Restart, replay, catch-up, member loss, rate limit, threshold failure,
      federated BDK, and standalone regressions pass.
- [ ] Production observability explains every material delay without logging
      secrets or high-cardinality identifiers.
