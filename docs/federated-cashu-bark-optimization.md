# Federated Cashu Bark BOLT11 optimization implementation plan

Status: Milestones 0, 0A, 1, 3, 4, 4A, 5, 6, 6A, 6B, and 6C complete; Milestones 2 and 7 rejected; Milestone 8 in progress

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
   The helper targets the ordinary macOS clipboard used for manual paste. The
   controlled Chrome session has a separate automation clipboard; do not read
   that clipboard and assume it contains the new invoice. When automating the
   form, fill the exact invoice returned by the current CLI process and verify
   the field before clicking.
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
AwaitingObserverFallback
Complete
```

It has the following meaning:

| Result | Scheduler action |
| --- | --- |
| `Progressed` | Advance again immediately in the same bounded cooperative pump |
| `AwaitingExternalEvidence` | Persist and apply external no-progress backoff |
| `AwaitingFederationEvidence` | Wait for relevant accepted-operation notification; retain a slow repair fallback |
| `AwaitingObserverFallback` | Preserve an exact six-second fallback deadline; accepted matching evidence may wake the driver but cannot reset or advance that deadline |
| `Complete` | Remove schedule state and stop all automatic work |

The driver owns persisted schedules, active-attempt leases and coalescing,
accepted-operation wakes, optional backend event wakes, wallet-request wakes,
deterministic observer-fallback rounds, and terminal cleanup. Backend
advancement owns protocol state and returns only the reason it cannot
continue. It does not own generic timers. `AwaitingObserverFallback` is a
backend-independent scheduling reason even though Bark is its first caller:
the generic driver knows neither Ark nor Bark phases, while the Bark
orchestrator decides whether matching accepted evidence exists and which
member a round selects.

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

### Retained polling policy

Milestone 4 retained the centralized five-second exponential policy after the
two-second candidate failed its RPC-volume gate. Observer fallback is a
separate exact six-second deadline that starts only after the first matching
accepted observation; unpaid invoices therefore do not expand from three to
all five observers merely because they remain unpaid.

| Condition | Candidate behavior |
| --- | --- |
| Fresh external wait | Probe after 5s |
| Continued no progress | 10s, 20s, 40s, 80s, 160s, then 300s cap |
| Background collision spreading | deterministic +/-20% jitter |
| Wallet GET or POST | wake the same driver subject to its existing per-kind cooldown and active lease |
| Accepted relevant operation | immediate internal pump; no external probe unless the new state requests one |
| Mailbox/backend hint | immediate authoritative probe under the same lease |
| Progress into a new external-wait phase | reset that phase's no-progress counter |
| Restart | resume persisted deadline, phase, counter, and expired-lease recovery |
| Terminal state | remove schedule immediately |
| Unselected observer, no matching evidence | wait for accepted federation evidence; do not start fallback |
| Unselected observer, matching evidence below threshold | preserve a 6s fallback deadline, then add one deterministic observer per round |

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
| 1 | Remove unnecessary inter-operation delay with one generic mint/melt advancement driver | Complete | `690b9a861` |
| 2 | Determine and implement the smallest safe Bark incoming/outgoing event wake, or reject it | Rejected | `9f0fe6467` |
| 3 | Minimize consensus messages by deriving Cashu payment from accepted Bark result quorum | Complete | `f09a8d40d` |
| 4 | Reduce excess Ark probes and observations with threshold-first polling and measured backoff | Complete | `c248a441f` |
| 4A | Make accepted Bark custody signer plans recover safely from unavailable signers | Complete | `fix: recover federated Bark signing from unavailable members` |
| 5 | Remove duplicate wallet submissions and audit serial Bark operations | Complete | `4148162c7` |
| 6 | Isolate Bark maintenance from payment hot paths and retire safely expired receives | Complete | `fix: isolate federated Bark maintenance from payments` |
| 6A | Make accepted external-action actor leases replay-stable | Complete | `fix: make federated Bark actor leases replay-stable` |
| 6B | Back off observer-worker failures so lagging members can recover quietly | Complete | `fix: back off failed federation payment observer rounds` |
| 6C | Reconcile a paid federated melt after the wallet loses its aggregate response | Complete | `fix: retain ambiguous wallet melts for recovery` |
| 7 | Tune consensus timing only if post-reduction evidence still shows a material delay | Rejected | `docs: record Bark payment consensus timing evaluation` |
| 8 | Pass sustained local, CLI, browser-based Signet, and Nocean smoke gates | In progress | - |

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

- [x] Trace the incoming and outgoing mailbox message origin, authentication,
      delivery, cursor, reconnect, redelivery, and expiry behavior in the
      supported Ark server.
- [x] Determine whether all members can subscribe without exceeding server
      connection or rate limits: the required threshold-custody authorization
      is unavailable, and an all-member connection budget is not specified.
- [x] Determine whether raw decode-only subscription can advance a durable
      cursor without invoking ordinary local claim/settlement side effects: it
      avoids those effects but cannot authenticate the payment mailbox.
- [x] Evaluate, in order, the smallest candidates:
  - [x] reject a payment-hash wake from the existing mailbox stream;
  - [x] reject a federation-safe notification after mailbox decode;
  - [x] reject all-member bounded wait/long-poll status mode; and
  - [x] retain ordinary persisted polling.
- [x] Preserve the rule that any future stream item may only hint the normal
      authoritative member probe; no stream authority was added.
- [x] Retain the generic driver, attempt lease, and persisted polling repair
      path without adding a Bark retry or reconnect loop.
- [x] Reject the candidates before adding persistent connections, so reconnect
      and event-to-probe measurements are not applicable.

### Tests

- [x] No implementation candidate survived the authorization and bounded-load
      preconditions, so the stream-specific runtime matrix is not applicable;
      the source and existing upstream coverage are recorded below.

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

### Evaluation record — 2026-07-21

Decision: **Rejected.** Keep the Milestone 1 persisted polling repair path and
add no production event or long-poll code in this milestone.

The supported Bark/Ark source does provide the signal content. Ark stores an
`IncomingLightningPaymentMessage` when an invoice becomes claimable and a
`LightningSendFinishedMessage` containing the preimage after a successful
send. `SubscribeMailbox` first drains stored messages newer than the supplied
checkpoint, then follows a server watch. Checkpoints are strictly ascending.
The client reconnect policy uses bounded 1-to-30-second equal-jitter backoff
and stops after five reconnects that make no progress.

The signal cannot be consumed safely by independent federation members at the
current trust boundary:

- `SubscribeMailbox` requires a fresh `MailboxAuthorization`, a BIP340 Schnorr
  signature over the exact mailbox identifier and expiry.
- Federation receive and outgoing HTLC intents use a mailbox identifier derived
  from the FROST custody public key. A member's operational Bark seed derives a
  different mailbox key, so no member can authorize that subscription alone.
- Authorizing the custody mailbox would require a new threshold signing round
  for every authorization/expiry or a new delegated read capability. Either is
  a protocol and key-management change, not a contained latency optimization.
- Replacing the plan mailbox with an operational member mailbox would require
  accepted binding, cursor ownership, catch-up, and failover rules. It would
  also make notification availability depend on one member.

The ordinary Bark mailbox handler is not a safe shortcut. Incoming messages
call `try_claim_lightning_receive`; outgoing success messages can settle local
state from the preimage; failure messages drive the outgoing action; and the
wallet checkpoint can advance despite an individual payment-processing error.
The raw `subscribe_mailbox_messages` API avoids those mutations, but it does
not solve custody-mailbox authorization.

Ark's status RPCs also support `wait=true`: incoming status waits on a broadcast
with a 30-second database fallback, while outgoing status waits on a broadcast
and the configured invoice poll. They have no durable client cursor or replay.
Using them from all five members would hold five requests per active quote and
could add five Ark facade calls when a wait releases. The disposable Milestone
0 probe already showed that the ordinary successful outgoing path needed one
terminal check per member plus one actor success check. Long-polling all members
therefore adds work in the common path and has no declared public-server
connection budget. An actor-only outgoing or threshold-first incoming wait can
be reconsidered in Milestone 4 after observer selection is implemented and
measured; doing it first would confound that result.

No test federation was needed because every production candidate failed the
authorization or bounded-load precondition before runtime evaluation. The
stream-specific correctness matrix is consequently not applicable. Existing
upstream source tests cover correct, missing, mismatched, and expired mailbox
authorization, mailbox history delivery, `wait=true`, and the 30-second status
fallback. A direct `cargo test -p ark-lib mailbox` attempt could not reach the
tests because the native `libbitcoinkernel` test dependency lacked Boost, and
the local environment has no `nix` executable for the repository shell. This
environment failure does not weaken the source-level authorization rejection.

Source versions were CDK `690b9a861` and Bark `aa98f5685d70`, with the same
five-member, 3-of-5 Bark/FROST and four-observer Cashu model used by the local
release baseline. No binary, database, invoice, payment, or host was involved;
there are therefore no new latency samples, message counts, binary hashes, or
host metrics to report. No production code or temporary instrumentation was
added. Milestone 4 remains the evidence-gated place to measure narrowed polling.
The intended milestone commit subject is
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

- [x] Specify the exact deterministic mapping from a threshold-matched Bark
      result key to mint-quote paid, melt paid, or safely failed state.
- [x] Require `max(t, q)` matching result observers when the same result family
      supplies both Bark and Cashu payment authority; never assume `t == q`.
- [x] Bind quote, method, payment hash, action, amount, fee, source scope,
      result hash, terminal status, and predecessor phase.
- [x] Apply the ordinary quote/saga transition when the threshold-completing
      Bark observation is accepted, in the same transaction or one
      deterministic derived apply step.
- [x] Reuse existing Cashu quote accounting and saga functions instead of
      duplicating Bark-specific payment business logic.
- [x] Remove Bark's second `MintQuotePayment` and `MeltQuotePayment`
      submission rounds.
- [x] Preserve generic payment-observation operations for backends that do not
      already produce a threshold-authoritative result family.
- [x] Publish quote/saga notifications only after commit.
- [x] Make catch-up and replay derive the identical state without Bark or Ark
      I/O.

### Tests

- [x] `r - 1` matching Bark results cannot pay or fail a quote.
- [x] The threshold-completing result applies the ordinary quote/saga change
      exactly once.
- [x] Later matching observations are idempotent.
- [x] Conflicting amount, fee, hash, result, scope, action, or terminal status
      cannot combine.
- [x] Two incompatible threshold groups fail closed.
- [x] Observation reordering converges to the same result.
- [x] Restart before notification and catch-up after quorum preserve payment.
- [x] Incoming issuance cannot exceed accepted net Bark value.
- [x] Outgoing success, fee change, failed revocation, and proof rollback retain
      existing Cashu behavior.

### Success criteria

- The minimum-path operation formula falls by exactly `min(t, q)` for all three
  flows, which is three entries with Nocean's `t = 3` and `q = 4`. A live run
  may remove more entries when surplus generic observers would otherwise race
  into consensus; the complete generic family must disappear.
- No generic Bark-backed payment observation appears after result quorum.
- Median result-quorum-to-quote-final latency improves by at least 15%, with no
  other primary stage regressing by more than 5%.
- Non-Bark federation backends and standalone flows are unchanged.
- The milestone is committed as
  `perf: derive Cashu payment from Bark result quorum`.

### Evaluation record — 2026-07-21

Decision: **Kept.** The confirmed redundant work was a second generic Cashu
payment-observation family after Bark had already accepted the same exact
terminal result from distinct members. The retained implementation derives
ordinary payment-observation rows deterministically from the accepted Bark
result records. It does not submit another consensus operation.

Materialization starts only after the configured Bark/FROST threshold `t` is
present. The existing Cashu payment policy then evaluates those rows at its
own threshold `q`, so the effective authority is `max(t, q)` without assuming
that the thresholds are equal. The derived key binds quote, BOLT11 method,
action, predecessor terminal phase, payment hash, Ark source scope, amount,
fee, result hash, and success/failure state. Duplicate reports from one member
remain one vote; the earliest exact accepted result per member supplies that
member's deterministic row.

Incoming result application, derived rows, ordinary mint-quote accounting,
journal status, and post-commit payment notification share one transaction.
Outgoing result evidence commits with the Bark projection first, then reuses
the existing melt observation and saga finalization helper; a crash between
those steps replays the same idempotent rows and leaves the journal operation
nonterminal until saga finalization succeeds. Generic non-Bark payment
operations are unchanged. The Axum observer skips only the redundant generic
probe for a Bark-backed BOLT11 quote after Bark advancement.

The candidate was built in release mode from CDK base `9f0fe646746d` plus this
milestone and Bark `aa98f5685d70`. The release binary hashes were
`e5529b0363b106d89524347a9fe0c3694c7401cb86ac3825f57af1a0f100ad3f`
for `cdk-mintd` and
`431b323d2065bae53a5cc50e00c01710a297ae87f5c323efc955476e620bc0b2`
for `cdk-cli`. Measurements used five local loopback members, SQLite,
`t = 3`, `q = 4`, a fresh DKG, the public Signet Ark service, the authenticated
faucet, the public store API, and a disposable local CLI wallet. No Nocean
benchmark or mutation was performed.

Three incoming 10,000-sat samples completed in execution order:

| Sample | Faucet click to issued | Durable quote to issued | Accepted operations | Duplicate finalized items |
| --- | ---: | ---: | ---: | ---: |
| 1 | 28.405s | 147s | 18 | 4 |
| 2 | 28.190s | 64s | 18 | 4 |
| 3 | 27.648s | 67s | 18 | 4 |
| Median | 28.190s | 67s | 18 | 4 |

The first durable duration includes deliberate operator delay while rejecting
a stale automation-clipboard value; it remains recorded but is not external
payment latency. The click-to-issued median improved 15.6% from Milestone 1's
33.4-second post-submit/detection median. Each receive accepted exactly one
intent, one preparation, one quote, one claim plan, three transitions, ten Bark
member observations, and one mint. It accepted no generic payment operation:
18 entries versus 23 in every Milestone 1 sample, a five-entry or 21.7%
reduction under the observed all-member race.

Three outgoing 1,000-sat store samples completed in execution order:

| Sample | Durable quote to paid | Accepted operations | Duplicate finalized items | Bark debit | Store |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 24s | 11 | 4 | 1,025 sat | `PAID` |
| 2 | 25s | 11 | 4 | 1,025 sat | `PAID` |
| 3 | 24s | 11 | 4 | 1,025 sat | `PAID` |
| Median | 24s | 11 | 4 | 1,025 sat | `PAID` |

The outgoing median improved 7.7% from Milestone 1's 26 seconds and did not
regress. Each melt accepted one quote, one reservation, one plan, three Bark
transitions, five Bark results, and no generic payment operation: 11 entries
versus the prior median 16, a five-entry or 31.3% reduction in the observed
all-member race. The minimum formulas still reduce by the predicted three
entries; live savings are larger because all five generic observations had
previously entered consensus. FROST custody traffic remains one unchanged
nine-message action per successful receive or send. Ark call sites and polling
constants are unchanged, and no rate-limit error appeared.

All five members converged after every sample. Before restart each had 87
identical applied operations and identical order/application digests. A full
release restart left those 87 application operations and digests unchanged;
five later finalized items had no operation ID and were empty AlephBFT units,
not replayed payment work. All three mint quotes remained issued, all three
melt quotes remained paid, and replay performed no external Bark or Ark
payment side effect. The existing analysis warning was reconfirmed: Bark
refreshes movement completion timestamps during later replay, so the immutable
quote and accepted-operation records, not post-restart movement timestamps,
are the retained evidence.

Focused and full gates passed:

```text
cargo check -p cdk --features federation
cargo check -p cdk-axum --features federation-bark
cargo test -p cdk --features federation \
  bark_result_records_materialize_only_after_distinct_inner_quorum --lib
cargo test -p cdk --features federation --lib
# 787 passed
RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation-bark --lib
# 531 passed, 2 intentionally ignored
cargo test -p cdk-sqlite --features federation \
  federation_bark_projection_persistence
```

The focused test covers duplicate observers, `t - 1`, `q - 1`, `max(t, q)`,
observation ordering, incoming and outgoing row materialization, and conflicts
in action, quote, terminal state, payment hash, source scope, amount, fee, and
result hash. Existing projection and saga tests cover success proof binding,
failure/revocation binding, exact fee/change, rollback, competing quorums,
restart, and catch-up. Clippy passes for the candidate after allowing the ten
already present workspace lint findings; none is introduced by this milestone.

No temporary instrumentation was added or retained. The intended milestone
commit subject is `perf: derive Cashu payment from Bark result quorum`.

## Milestone 4: threshold-first Ark probing and measured backoff

### Goal

Use only the number of healthy Ark observers needed for threshold evidence,
retain deterministic fallback for liveness, and evaluate a faster fresh-payment
backoff without recreating the shared-IP rate-limit incident.

### Checklist

- [x] Deterministically select the required observer count from accepted action
      data: `t` for Bark-only evidence and `max(t, q)` for a result family that
      also supplies Cashu payment authority after Milestone 3.
- [x] Permit primaries to probe immediately according to the generic schedule.
- [x] Delay non-primary observers for a measured fallback window, initially six
      seconds, and cancel fallback when accepted matching evidence reaches
      threshold.
- [x] If fallback is needed, expand deterministically without changing the
      evidence key or actor plan.
- [x] Before every Ark call, check whether this member already has accepted
      matching evidence for the current epoch; if so, return
      `AwaitingFederationEvidence` without Ark I/O.
- [x] Do not submit new evidence after threshold if it has not already entered
      consensus.
- [x] Apply the same primary/fallback principle to outgoing terminal-failure
      observation where multiple members must poll.
- [x] Evaluate the centralized 2s-to-300s polling candidate only after primary
      selection is active.
- [x] Keep wallet and mailbox wakes under the same existing duplicate cooldown
      (five seconds for mint requests, request-driven melts) and active-attempt
      lease.
- [x] Do not parse server errors or reset backoff on wallet requests.
- [x] Record status and prepare/hydration Ark RPCs separately.

### Tests

- [x] Healthy incoming HTLC evidence uses three primary observers; a Bark
      result family carrying Cashu authority uses four because `t = 3`, `q =
      4`, and `max(t, q) = 4`.
- [ ] One unavailable primary activates fallback and completes the entire
      payment safely. The observer fallback completed its evidence phase, but
      the existing fixed FROST signer plan then blocked the claim; Milestone
      4A owns that separate defect.
- [x] Two unavailable primaries cannot produce an unsafe claim under the exact
      threshold checks. A complete live isolation test remains part of
      Milestone 4A because the same fixed signer plan prevents a useful
      end-to-end result.
- [x] Accepted evidence suppresses all later Ark I/O by that member and epoch.
- [x] Restart preserves primary selection and evidence suppression.
- [x] Wallet fan-out during backoff does not cause five simultaneous probes.
- [x] A mailbox hint during an active lease does not duplicate a probe.
- [x] Selection is identical on all members and during replay.

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

### Progress record — 2026-07-21

Milestone 4 is `Complete`, decision `Kept`, with the downstream custody-signer
recovery defect isolated as Milestone 4A. The confirmed amplification was that
all five members were eligible to poll Ark even though healthy incoming HTLC
evidence needs `t = 3` observers and terminal result evidence needs
`max(t, q) = 4`. A first implementation started every non-primary member's
fallback deadline when the quote was created. It was rejected before retention:
a slowly paid invoice allowed all five deadlines to expire and recreated the
five-member polling fan-out.

The retained implementation uses an actor-first cyclic observer roster from
accepted action data. It starts with exactly the required threshold and expands
by one member per persisted fallback round. An unselected member waits for
federation evidence while zero matching observations exist. The first matching
accepted observation starts an exact six-second fallback deadline; later
wallet or operation wakes preserve that deadline instead of moving it earlier
or later. Progress resets the round. Version-2 schedules migrate in memory to
version 3, while the existing storage key, active-attempt lease, wallet
cooldown, external 5-to-300-second backoff, deterministic jitter, and
60-second repair path remain shared by mint and melt. No response text or rate
limit class influences scheduling.

Two retained incoming samples against real Signet used only members 1-3 for
Ark access; members 4-5 made zero status or hydration calls even though faucet
routing took 51.535 and 55.920 seconds. Status calls were 13 and 14, versus 20
in the five-member baseline, a 35% and 30% raw reduction. The eligible caller
set fell exactly 40%, from five members to three. The longer external routing
allowed an extra selected-member status probe, so the cadence-sensitive raw
call count did not meet 40%; it still moved in the predicted direction without
rate-limit errors. Both samples used one invoice call and three total
prepare/hydration calls, reached issuance in 58 and 63 seconds of durable quote
time, and accepted exactly 15 operations with four duplicate wallet items.

Three retained 1,000-sat outgoing samples completed in 23, 22, and 22 seconds
of durable quote time (22-second median) and 23.20, 22.91, and 21.78 seconds of
CLI wall time (22.91-second median). Exactly the actor-first three members made
one terminal-failure status call each, the actor made one success-status call,
and the other two members made none. Ark-facing facade calls fell from eight
to six (25%), terminal-status calls fell from six to four (33%), and accepted
operations fell from 11 to 10. Each store order reached `PAID`; exact fee and
wallet accounting held; all five projections converged with four duplicate
wallet submissions and no rate-limit warning.

The two-second external-backoff candidate was then tested on a fresh local
five-member release federation. Its incoming sample took 59 seconds of durable
quote time and 66.473 seconds from faucet submission to success, with 17 status
calls. Relative to the retained samples' 60.5-second durable median and
13.5-call median, its roughly 2.5% latency difference was noise below the 15%
gate while status traffic increased about 26%. The candidate was rejected and
the five-second base restored.

The unavailable-primary experiment paused member 2 after an incoming quote.
Members 1 and 3 accepted matching HTLC evidence; after the six-second deadline,
member 4 made one status/hydration attempt and supplied the third observation.
Consensus progressed through HTLC-ready and claim-plan acceptance, proving the
observer fallback. The claim then named the pre-existing fixed signer set
`[1, 2, 3]` and could not sign while member 2 was absent. Restarting that member
did not recover attempt 1 because the FROST session was already non-retryable.
No preimage-disclosure or claimed transition was accepted. This safe but stuck
state is not caused by observer selection and is recorded as Milestone 4A.

Temporary, identifier-free Ark-call probes were used to separate invoice,
status, preparation, hydration, and outgoing status calls. They were reverted
before the final build; the marker is absent from the final `cdk-mintd`
binary. No production instrumentation, secret, invoice, payment hash,
preimage, browser state, or machine-specific path remains.

The exact final candidate was built in release mode with Rust 1.95.0 from CDK
base `f7d843576598` plus this milestone and Bark `aa98f5685d70`. Binary hashes
were `a19eccd524d510a8e20e390ba20f67832a84398c6b6acba0b3f14c1f3802778c`
for `cdk-mintd` and
`431b323d2065bae53a5cc50e00c01710a297ae87f5c323efc955476e620bc0b2`
for `cdk-cli`. A final clean-binary 1,000-sat store smoke completed in 23
seconds, charged exactly 25 sats, reached store `PAID`, and added exactly ten
accepted operations. The five members converged at 40 application operations
with identical order and application digests. A no-build all-member restart
left the operation count and both digests unchanged; its one new finalized
item contained no application operation.

Focused and full gates passed:

```text
cargo test -p cdk-axum --features federation-bark \
  bark_observer::tests --lib
# 3 passed
cargo test -p cdk-axum --features federation-bark \
  payment_schedule::tests --lib
# 14 passed
cargo test -p cdk-axum --features federation-bark \
  bark_incoming::tests --lib
# 6 passed
cargo test -p cdk-axum --features federation-bark \
  bark_outgoing::tests --lib
# 1 passed
RUST_MIN_STACK=8388608 cargo test -p cdk-axum \
  --features federation-bark --lib
# 539 passed, 2 intentionally ignored
cargo test -p cdk --features federation --lib
# 787 passed
cargo test -p cdk-sqlite --features federation \
  federation_bark_projection_persistence
cargo check -p cdk-axum --features federation-bark
cargo clippy -p cdk-axum --features federation-bark --lib -- \
  -D warnings [pre-existing lint categories allowed]
cargo fmt -p cdk-axum -- --check
git diff --check
```

The intended milestone commit subject is
`perf: use threshold-first Bark payment observation`.

## Milestone 4A: availability-aware custody signing and retry recovery

### Goal

Allow an accepted Bark claim or send plan to recover safely when a named FROST
signer is unavailable, without choosing signers from divergent local liveness
views, reusing nonce material, repeating an external side effect, or weakening
the accepted-plan authorization boundary. This milestone is a prerequisite for
the sustained member-loss gate in Milestone 8, but it is independent of Ark
observer selection.

### Current evidence

- Observer fallback supplied a missing third HTLC observation while one primary
  was offline, and consensus accepted the next two receive transitions.
- The accepted claim plan still named the fixed first-three signer set. The
  available threshold could not substitute another member.
- Restoring the signer did not recover the existing session; attempt 1 remained
  in a non-retryable state and automatic retries repeated the same failure.
- No preimage or claim transition was accepted, so the failure was safe but not
  live.

The first implementation candidate exposed one additional adapter boundary:
Bark wraps custody failures in its execution error, so downcasting the final
error did not reliably retain the selected-signer failure class. The revised
candidate records that class directly at the custody adapter and consumes it
once at the owning execution boundary. It never parses error text or changes
behavior based on Ark rate-limit responses. A failed refresh also proved that
the accepted attempt must be copied into Bark's member-local round session;
otherwise a later consensus-authorized retry finds the failed old round rather
than creating a fresh custody session.

### Checklist

- [x] Trace signer-set construction, accepted plan validation, FROST session
      identity, attempt numbering, nonce lifecycle, and retry ownership for
      receive claims, outgoing sends, revocations, and maintenance.
- [x] Prove why the fixed signer set is required or replace it with an accepted,
      deterministic roster/attempt plan that every member validates from the
      same operation data. Never select signers directly from local health
      observations.
- [x] Define a consensus-authorized timeout or failure transition that can move
      to the next deterministic signer roster and a fresh attempt identifier.
- [x] Burn or quarantine every prior attempt's nonce commitments and shares;
      never reuse them after timeout, restart, reorder, or catch-up.
- [x] Keep the action, quote, payment hash, amount, fee, source, output set, and
      external side-effect identity unchanged across signer retries.
- [x] Make an already-completed side effect idempotently discoverable before a
      replacement attempt can execute it.
- [x] Terminalize truly non-retryable sessions or expose a narrowly scoped,
      authenticated operator recovery action with an explicit replay contract.
- [x] Reuse the existing action/plan machinery where it can express the safety
      boundary; do not build a second generic retry scheduler.

### Tests

- [x] One initially selected signer offline: the next accepted deterministic
      roster completes incoming claim and outgoing send.
- [x] Two unavailable signers leave the action safely pending with no preimage,
      signature, claim, payment, or revocation side effect.
- [x] Signer restart before and after commitment/share submission either
      resumes safely or advances once to a fresh attempt.
- [x] Actor crash, conflicting retry proposal, delayed old share, replay,
      catch-up, and all-member restart cannot reuse a nonce or execute the side
      effect twice.
- [x] All members derive the same signer roster and attempt identifier from
      accepted data under reordered local notifications.
- [x] Healthy payments preserve their current signer count, operation count,
      and latency within the 5% regression budget.

### Success criteria

- One unavailable signer completes within a measured bounded retry budget with
  the same threshold and consensus-before-side-effects guarantees.
- Two unavailable signers remain safe and bounded rather than polling or
  retrying a permanently non-retryable session.
- Restart and catch-up converge on one attempt/result with no nonce reuse and
  no duplicate Bark/Ark side effect.
- The milestone is committed as
  `fix: recover federated Bark signing from unavailable members`.

### Candidate findings and progress

The candidate retains the original action plan and adds only one canonical
`SigningAttemptAdvanced` transition. Signers are selected by a deterministic,
threshold-sized cyclic window over the accepted member roster. A five-member,
threshold-three federation therefore uses `[1,2,3]`, `[2,3,4]`, `[3,4,5]`,
`[1,4,5]`, and `[1,2,5]`, then stops. The choice never reads a local health
view. Every retry preserves the action, quote, payment hash, amounts, fees,
inputs, outputs, request commitment, and external action identity while using
a fresh FROST attempt/session. Participants accept only the current
consensus-authorized attempt; delayed older requests remain rejected. Failed
commitment collection uses the existing best-effort nonce abort, and durable
participant state prevents an abandoned session from becoming reusable.

The first live candidate safely reproduced the old failure: member 3 was
paused, attempts against `[1,2,3]` remained non-retryable, and no preimage or
claim was accepted. After preserving the custody failure class at the adapter,
the same already-paid action recovered without a new invoice. Consensus
accepted attempts 2, 3, and 4 in order; attempt 4 selected `[1,4,5]`, accepted
`ReceivePreimageDisclosed` and `ReceiveClaimed`, marked the quote paid, and
issued exactly 10,000 sats. Restarting the offline member caught it up from 3
to 21 operations and restored identical order/application digests on all five
members. The actor restart-to-success interval was about 24 seconds; the
565-second quote duration includes the deliberate outage, diagnosis, rebuild,
and restart and is not a payment-latency sample.

The same accepted-attempt adapter is used by incoming claims, outgoing HTLC
cosigning, failed-send revocation, refresh, and emergency exit. A refresh whose
attempt-1 Bark round had failed accepted attempt 2, created one distinct Bark
round instead of reopening the failed movement, and later completed through
the authenticated exact-action retry. All five members applied the result and
agreed on one spendable 10,000-sat replacement VTXO.

The final release candidate then paid two 1,000-sat store invoices. The healthy
control used attempt 1, the normal three signers, ten accepted operations, and
completed in 33 seconds. In the fault sample member 3 was confirmed offline
before quote creation. Attempts 1, 2, and 3 all included that member and failed
without initiating the Lightning payment; consensus accepted each exact next
roster. Attempt 4 selected `[1,4,5]`, paid the store once, reached
`PaymentSucceeded`, and reduced the accepted Bark balance by exactly 1,025
sats in 31 seconds. The recovery therefore added three explicit authorization
operations but no measurable external-payment delay relative to the adjacent
healthy control.

The wallet command did not receive a final four-member aggregate response even
though the fault payment and quote were durably paid. Restoring member 3 exposed
two separate pre-existing restart defects. Replay recomputed the accepted
plan's actor lease from the later journal tip and rejected immutable history;
before that failure terminated the process, a due payment schedule also retried
the member's readiness error roughly every millisecond. Neither defect is
signer-attempt state or duplicate Bark execution. They are isolated as
Milestones 6A and 6B rather than folded into this recovery protocol. The
wallet's separate failure to reconcile the paid melt after losing the aggregate
response is recorded as Milestone 6C. The earlier incoming recovery already
proved offline-member journal/application catch-up and identical all-member
digests.

Decision: `Kept`. One deterministic roster pass is the smallest protocol that
restores one-fault custody liveness while preserving consensus authorization.
There is no local health election, timeout scheduler, error-string policy, or
unbounded retry. Bark's relevant Ark cosign and round exchanges are explicitly
idempotent; the accepted action identity stays fixed while only the custody
session attempt changes.

Focused validation currently passes:

- canonical transition, bounded roster, and Bark operation tests: 20 passed;
- complete `cdk-bark` federation test suite: 13 passed;
- SQLite Bark projection persistence through retry and durable checkpoint: 1
  passed;
- FROST transport retry/idempotency/stale-attempt regression: 1 passed;
- current-attempt validation across durable preimage and exit checkpoints: 1
  passed; and
- full serial `cdk-axum` `federation-bark` suite: 550 passed and 2 intentional
  stress tests ignored; the two localhost-bind tests denied by the sandbox
  passed separately with loopback permission; and
- `cdk-axum` all-target `federation-bark` check and exact locked release
  `cdk-mintd`/federation `cdk-cli` builds: passed.

Temporary attempt tracing was removed before final validation. The retained
failure classification is a one-shot adapter flag because Bark deliberately
wraps custody errors; it is not performance instrumentation and contains no
backend error text.

## Milestone 5: duplicate submission and serial-operation reduction

### Goal

Remove remaining consensus work that does not represent a distinct safety
boundary. Address wallet fan-out duplicates first, then evaluate each adjacent
Bark operation independently. Do not introduce opaque operation batches.

### Checklist

- [x] Treat every proposed consensus-boundary removal as an investigation
      before attempting an implementation. First record the invariant the
      boundary protects, the accepted data available on every member, and the
      Byzantine-actor, crash, replay, catch-up, and recovery cases. Attempt a
      change only after that evidence shows the same security model can be
      preserved; otherwise retain the boundary and record why.
- [x] Count accepted entries, duplicate finalized items, units, protocol
      messages, and bytes separately.
- [x] For identical wallet-fanned mint or melt operations, let every member
      register a waiter for the stable operation ID while one deterministic
      publisher submits first.
- [x] Add a short deterministic fallback publisher order for liveness.
- [x] Prove every waiter receives the same accepted result and cancellation
      cannot suppress fallback.
- [x] Recount incoming setup, outgoing setup, and mint/melt submission work.
- [x] Audit each serial Bark boundary against the invariant it protects:
  - [x] receive intent -> receive prepared;
  - [x] HTLC-ready -> claim plan;
  - [x] claim plan -> preimage disclosure -> claimed;
  - [x] send plan -> HTLCs cosigned -> payment initiated -> terminal result;
  - [x] failure -> revocation plan -> revocation completed.
- [x] For each boundary, document the crash or equivocation that requires it.
- [x] Evaluate only a boundary whose accepted data can deterministically carry
      the next state without crossing an external side effect or signing
      authorization boundary.
- [x] Investigate materializing the mint quote from accepted receive
      preparation before attempting the change. Proceed only if invoice,
      lookup ID, amount, expiry, and requester binding remain deterministic,
      replay-safe, and equally protected against a Byzantine preparation
      actor.
- [x] Investigate whether `ReceiveHtlcsReady` can be derived from the exact
      accepted HTLC-observation quorum before attempting to remove its
      operation. The investigation must prove that every member can obtain and
      validate the complete VTXO artifacts from accepted data; an observation
      hash or trust in the actor is not sufficient, and enlarging observations
      must be included in the message/byte comparison.
- [x] Investigate each terminal bookkeeping transition separately before
      attempting to derive it from an accepted Bark result quorum. Name the
      exact transition, prove deterministic reconstruction and ordering on
      replay and catch-up, and retain every transition that separates an
      external side effect, signing authorization, preimage disclosure, or
      recovery decision.
- [x] Investigate whether a universally verifiable success proof can safely
      replace any terminal member observations before attempting that change.
      A valid Lightning preimage alone is not enough: the proof must bind the
      exact accepted action and demonstrate the corresponding federated Bark
      balance transition even with a Byzantine actor or colluding payee. If it
      cannot preserve the current independent-member evidence model, retain
      `max(t, q)` result observations.
- [x] Keep reductions as direct derived apply behavior; do not create a generic
      Bark batch envelope.
- [x] Stop after the first retained serial-operation reduction, commit it, and
      add another milestone for further independent reductions. No serial
      Bark boundary met the removal gate, so no follow-up reduction milestone
      was created.

### Tests

- [x] Five simultaneous identical wallet requests normally produce one
      publisher submission and one accepted operation.
- [x] Publisher failure triggers fallback without changing the operation ID.
- [x] Waiter restart and reconnect recover the accepted result.
- [x] No serial boundary was removed, so removal-specific crash, replay,
      reorder, conflicting-plan, HTLC-reconstruction, and terminal-binding
      tests are not applicable.
- [x] No preimage, signature share, Ark payment, claim, or revocation occurs
      before its retained accepted boundary.

### Success criteria

- Healthy wallet fan-out produces no surplus finalized item for an already
  accepted mint/melt operation, except a bounded measured fallback race.
- Raw consensus traffic and duplicate-item counters fall as predicted.
- Any serial operation removed has a written safety proof and exact count
      reduction; otherwise it remains.
- No retained reduction weakens consensus-before-side-effects, threshold
  authority, independent result validation, replay determinism, or recovery.
- End-to-end latency does not regress by more than 5%.
- The initial milestone is committed as
  `perf: deduplicate federated wallet operation submission`.

### Findings and implementation

The wallet already fans the same stable `Mint`, `Melt`, or `Swap` operation ID
to all five members. Previously, every member immediately published its own
signed envelope. The retained implementation applies only to these generic
wallet-value operations: a deterministic operation-ID rotation chooses the
first publisher, and later publishers wait in three-second rank slots for the
same accepted result before falling back. Quote creation, payment evidence,
Bark operations, and chain observations retain their existing independent
authorship. This is backend-independent and therefore also applies to
federated BDK and future funding backends without payment-method branching.

Waiters use the existing bounded operation-result notification registry. Five
concurrent waiters received byte-identical accepted responses; cancellation
removed the registration; notification-state loss timed out and safely
resubmitted the stable operation ID as a duplicate; and a live five-member
AlephBFT run with the rank-zero member offline completed through rank-one
fallback after three seconds with exactly one finalized item.

The live same-process A/B comparison isolates the publication policy from Ark
and Lightning variability:

| Measure | Five direct publishers | Designated publisher | Change |
| --- | ---: | ---: | ---: |
| Finalized data items | 5 | 1 | -80.0% |
| Application-bearing Aleph messages | 164 | 44 | -73.2% |
| Application-bearing bytes | 176,300 | 47,344 | -73.1% |
| Five-run median messages | 168 | 28 | -83.3% |
| Five-run median bytes | 180,600 | 30,128 | -83.3% |
| Five-run median finality | 2,137ms | 2,048ms | -4.2% |

The exact real-release melt envelope was 1,897 bytes in both cases. Direct
fan-out produced five finalized items, five data units, one accepted entry,
four duplicates, and 11,309 stored finalized JSON bytes. Designated
publication produced one item, one unit, one accepted entry, no duplicates,
and 2,254 stored bytes. A real incoming mint and three real outgoing melts also
left the federation-wide duplicate counter unchanged at 11.

Those outgoing payments became durable in 29, 34, and 33 seconds, versus the
Milestone 4 median of 22 seconds. This is not a controlled regression: the Ark
server began returning an explicit outage response when the attempted
same-federation baseline restart followed. The controlled A/B above instead
shows that the candidate removes work without a median finality regression.
Individual pairs varied in both directions, while the five-run candidate
median was 4.2% faster. Keep the reduction; do not attribute the
external-interval difference to designated publication.

### Serial-boundary audit

No adjacent Bark operation is safely redundant with the data currently
accepted by consensus. Removing one would cross a side effect or discard data
needed for deterministic replay, catch-up, or recovery:

| Boundary | Distinct invariant and retained reason |
| --- | --- |
| `ReceiveIntent` -> `ReceivePrepared` | The intent fixes requester, destination, policy, and secret-dependent authority before invoice creation. Preparation then commits the exact Ark invoice result after that external side effect, so crash recovery cannot create or substitute a different receive. |
| `ReceivePrepared` -> mint quote | Preparation contains invoice, payment hash, amounts, fee, expiry, and external ID, but not the requester's NUT-19 public key. The quote cannot be reconstructed with the same requester binding without adding backend-specific schema and apply coupling, so the generic `MintQuote` remains. |
| HTLC observations -> `ReceiveHtlcsReady` | Observations bind status, amount, fee, and a result hash, not the complete canonical VTXO artifacts and transitions. Enlarging each threshold observation would duplicate the large artifacts and create new validation coupling; the actor transition remains. |
| `ReceiveHtlcsReady` -> `ReceiveClaimPlan` | The plan fixes exact input reservations, output locator, signer roster, and signing attempt before FROST and preimage use. It is the authorization and crash-recovery boundary. |
| `ReceiveClaimPlan` -> `PreimageDisclosed` -> `ReceiveClaimed` | Disclosure is the durable ambiguous-side-effect marker after threshold authorization; claimed commits canonical outputs, spent pending inputs, and spendable VTXO transitions after the external claim. Merging either edge loses exact crash recovery. |
| `SendPlan` -> `SendHtlcsCosigned` | The plan reserves inputs and binds invoice, fee, change, revocation locator, signer roster, and attempt before FROST. Cosigned then persists the exact signed HTLC package and VTXO transitions so a crash resumes without re-signing or changing inputs. |
| `SendHtlcsCosigned` -> `PaymentInitiated` -> terminal result | Initiated records exact Ark acceptance before terminal polling and resolves ambiguous initiation without double-paying. Success or failure separately commits the terminal result and exact VTXO effects. |
| Failure -> `RevocationPlan` -> completed | The plan fixes recovery inputs, outputs, signer roster, and attempt before revocation FROST; completion commits the exact recovered balance transition. |

A Lightning preimage verifies payment of the invoice but does not prove the
corresponding federated Bark balance transition. It therefore cannot replace
the retained independent `max(t, q)` terminal observations. Milestone 3 already
derives ordinary Cashu payment state from that accepted Bark result quorum, so
there is no additional generic payment vote to remove. The audit deliberately
retains all serial Bark boundaries instead of increasing protocol coupling or
weakening consensus-before-side-effects.

All temporary A/B changes were reverted. The final source contains only the
generic publication coordinator and durable regression tests; it contains no
temporary production instrumentation.

The operation-submission future is boxed at its existing single entry point.
This keeps the larger coordinator state off concurrent handler stacks without
spreading coordination logic or changing the response contract. The existing
eight-request mint/swap/melt idempotency test caught the unboxed stack growth
and passes with the contained form.

Validation completed with the final source:

- focused publisher, waiter, cancellation, restart, and offline-primary tests:
  5 passed;
- full serial `cdk-axum` `federation-bark` library suite with an 8 MiB Rust
  test-thread stack: 547 passed and 2 intentional stress tests ignored;
- `cdk-axum` doc tests and all-target build check: passed;
- final locked release `cdk-mintd` and federated `cdk-cli` builds: passed.

Strict Clippy remains blocked by existing warnings in unrelated CDK and Axum
modules under the current toolchain. Both the dependency-inclusive and
`--no-deps` runs reached only pre-existing findings; neither identified the new
publisher module. No unrelated lint cleanup was folded into this milestone.

## Milestone 6: maintenance isolation and terminal cleanup

### Goal

Prevent refresh and safely expired incoming work from competing with active
payments or polling forever. Accepted invalid reservations, exhausted signer
attempts, and non-retryable FROST sessions require a consensus-authorized
recovery protocol and belong exclusively to Milestone 4A; this milestone must
not guess from backend error text or silently abandon accepted custody work.

### Current evidence

- Production receive traces interleaved unrelated refresh operations with the
  payment hot path.
- Members have logged actor/author mismatches during refresh.
- Expired incoming work continued to wake and fail long after it could no
  longer make progress.
- Five-second maintenance attempts can overlap payment-related FROST and Ark
  work.
- The automatic loop and authenticated operator retry called the same
  maintenance driver without sharing a per-member execution lease.
- Actor checks, member-scoped observations, and terminal maintenance-phase
  exclusion were already correct after Milestone 0A and should be reused, not
  replaced.

### Checklist

- [x] Check accepted actor/member role before any actor-only Bark side effect.
- [x] Restrict non-actors to member-scoped observations.
- [x] Prevent concurrent execution of the same maintenance action.
- [x] Give active receive, send, claim, and revocation work priority over
      routine refresh.
- [x] Retire an expired unpaid receive only after a 15-minute grace period and
      only when no accepted HTLC evidence exists.
- [x] Assign invalid reserved inputs, exhausted attempt lineage, and
      non-retryable FROST recovery to Milestone 4A instead of parsing errors or
      adding a second retry policy here.
- [x] Remove terminal actions from automatic selection and Ark polling.
- [x] Keep authenticated operator retry only where its replay contract is safe
      and documented.
- [x] Apply external no-progress backoff to routine maintenance instead of a
      permanent five-second loop.
- [x] Measure maintenance overlap, Ark RPCs, FROST requests, and payment-stage
      latency before and after.

### Tests

- [x] Only the accepted actor executes refresh.
- [x] Every member can submit its own evidence without author mismatch.
- [x] An expired receive becomes terminal before any Bark runtime or Ark call.
- [x] Active payment finalization proceeds while routine refresh is due.
- [x] Completion removes the persisted payment schedule, so restart does not
      reactivate terminal automatic work.
- [ ] Non-retryable FROST recovery tests are owned by Milestone 4A and remain a
      prerequisite for the sustained member-loss gate.

### Success criteria

- Healthy maintenance produces no author mismatch or duplicate actor side
  effect.
- Terminal actions produce zero automatic retries after terminalization.
- Routine maintenance adds no accepted operation or measurable wait to the
  payment critical path in the repeated healthy sample.
- The milestone is committed as
  `fix: isolate federated Bark maintenance from payments`.

### Progress record — 2026-07-21

Milestone 6 is `Complete`, decision `Kept`. The implementation is contained in
the existing Bark maintenance driver, incoming advancement path, and runtime
component holder. It adds no payment-method branch, rate-limit parser, second
scheduler, configuration knob, or new protocol state.

Automatic maintenance and authenticated exact-action retry now share one
per-member mutex, so they cannot execute the same accepted action concurrently
inside a member. The existing accepted actor checks remain immediately before
refresh, exit registration, exit publication, and exit confirmation side
effects. Non-actors still apply accepted state locally and publish only their
own member observation. Terminal maintenance phases remain absent from the
automatic selector.

Routine maintenance now reads the accepted projection and yields before Bark
maintenance work whenever any nonterminal receive, send, claim, failure, or
revocation action is active. An unpaid receive is exempt only after its
accepted expiry plus a 15-minute grace period and only if no accepted HTLC
evidence exists. The incoming driver makes the same decision before applying
the receive plan or calling any Bark/Ark status path, returns `Complete`, and
thereby removes the exact persisted quote schedule. Accepted evidence always
fails closed and keeps the action live; it is never discarded merely because
the invoice clock elapsed.

The fixed five-second maintenance interval is replaced by a single bounded
no-progress sequence of 5, 10, 20, 40, 80, 160, then 300 seconds. Any accepted
maintenance progress resets it to five seconds. A deterministic test and a
temporary identifier-free live probe both measured six attempts per member in
the first five minutes, versus 60 under the old loop: 30 rather than 300 total
five-member scans, a 90% reduction. At the cap, an idle member performs one
scan per five minutes rather than 60. Errors and ordinary no-work use the same
policy; no server error string changes the delay.

The fresh local five-member release run then created a 10,000-sat invoice and
paid it through the authenticated Signet faucet. External payer processing
dominated the 320-second durable quote interval. Once Bark detected the
payment, the actor's receive movement completed in 18.1 seconds. The lifecycle
accepted 15 operations with zero duplicate finalized items, used the expected
single threshold claim signing action, issued exactly 10,000 sats, and
converged on the same 15-operation order and application digests on all five
members.

A maintenance deadline fired while that quote remained in
`ReceivePrepared`. It returned without a maintenance plan, reservation, Ark
call, or maintenance FROST action; the payment subsequently completed. After
the payment was terminal, a clean-binary restart allowed due refresh work to
publish one plan and five member observations. This demonstrates priority,
not a global maintenance disable: routine work yields during the payment and
resumes afterward. No actor/author mismatch, duplicate maintenance item,
rate-limit response, or maintenance error appeared.

Invalid reservations and exhausted or non-retryable FROST attempts are not
safe to infer from redacted backend errors. Their recovery must change the
accepted signer attempt while preserving nonce and side-effect safety, so the
availability injection and automatic-retry failure remain recorded under
Milestone 4A. Keeping that protocol work in one milestone avoids coupling the
generic maintenance backoff to Bark error variants.

Temporary attempt logging was removed before the final source validation and
clean release rebuild. Regression coverage includes the exact idle delay
sequence, active/evidenced payment priority, actor ownership, expiry grace and
accepted-evidence exception, and persisted schedule removal on `Complete`.
The focused tests passed, the full serial federation-enabled `cdk-axum`
library suite passed 551 tests with two intentional stress/benchmark ignores,
and all-target checking plus doc tests passed. The exact clean release binary
also restarted the five-member state with identical member digests before
routine post-payment refresh resumed.

## Milestone 6A: replay-stable external-action actor leases

### Goal

Validate an accepted Bark action plan against the actor lease at the exact
consensus index where the plan was ordered. Later journal growth must not
change the actor expected for immutable history during catch-up or restart.

### Current evidence

Member 3 rejoined after missing the offline outgoing payment and downloaded
the complete 52-entry journal, but application stopped at operation 41. The
first unapplied entry was the accepted `LightningSend` plan. When it was first
ordered, the author still held the lease and actor 5 was valid. Replay used the
later journal tip, advanced the lease epoch, expected a different actor, marked
the accepted plan failed, and terminated Bark startup with
`actor, action, signer set, or attempt is not deterministic`.

### Checklist

- [x] Separate the actor lease's decision index from the current local journal
      tip.
- [x] Live admission derives the actor from the next possible operation index.
- [x] Accepted application and catch-up use the accepted plan entry's exact
      operation index.
- [x] Preserve the existing eight-item deterministic takeover cadence for new
      work.
- [x] Add a regression proving later journal growth rotates new authority but
      cannot alter the actor accepted at an earlier index.
- [x] Restart the offline member and verify all application and Bark state
      converges without another external payment.
- [x] Rebuild/restart all members and verify the historical plan remains valid.

### Success criteria

- The previously failed accepted plan applies on member 3 without changing its
  actor, action ID, signer set, attempt, or result.
- All five members converge on identical journal, application, materialized,
  Bark projection, quote, VTXO, and balance state.
- New actor takeover still rotates from consensus-ordered distance exactly as
  before.
- The milestone is committed as
  `fix: make federated Bark actor leases replay-stable`.

### Candidate findings and progress

The retained change makes the decision index explicit. Live admission still
uses the next possible journal index, preserving the eight-item takeover
cadence for new plans. Application and catch-up instead validate an immutable
accepted plan at that entry's own index. A focused regression proves that a
decision at index 41 retains its original actor after enough later entries
exist to rotate a new decision at index 49.

The exact release binary restarted all five persisted members. Member 3
reapplied the formerly failed plan and the remaining suffix without changing
the accepted actor, signer roster, action, or result. All members became ready
with 52 of 52 journal entries applied, zero failed entries, identical order and
application digests, five identical Bark action projections, and identical
federation Bark VTXO accounting of 7,950 sats spendable and 41,025 sats spent.
The journal remained at 52 entries and the Bark movement set did not grow, so
replay did not initiate another external payment. Focused unit testing and the
federation-enabled `cdk-axum` all-target check passed.

Decision: `Kept`. Historical authorization now depends only on immutable
consensus data; the live takeover policy and external action machinery are
otherwise unchanged.

## Milestone 6B: observer-worker failure backoff and recovery priority

### Goal

Prevent a payment schedule whose deadline is already due from hot-looping when
the observer worker fails before it can claim and advance that schedule. This
must remain a generic worker-health guard, not a Bark error parser or a second
per-quote scheduler.

### Current evidence

After the Milestone 4A outgoing fault sample, the restored member correctly
entered `lagging` while it had 11 accepted operations to apply. Its persisted
payment deadline was already due. `scheduled_quotes` rejected the authoritative
read before claiming the quote, `next_schedule_delay` continued returning zero,
and the outer worker retried the same readiness error roughly every millisecond.
The log grew to tens of megabytes and peer checkpoint publication timed out,
amplifying the separate replay failure diagnosed in Milestone 6A.

### Checklist

- [x] Add one contained outer-worker failure delay; do not modify backend or
      payment-method code.
- [x] Use a short exponential sequence that remains responsive after recovery
      and caps at a quiet bounded interval.
- [x] Reset the worker failure sequence after any successful wake, including a
      successful no-op.
- [x] Keep the persisted per-quote schedule as the only quote-age/backoff
      authority.
- [x] Do not inspect error strings or special-case lagging, Ark, Bark, BDK, or
      Lightning.
- [x] Prove repeated pre-claim errors do not spin, and recovery is noticed
      within the first delay after the failing condition clears.
- [x] Restart the lagging live member and verify application/digest convergence
      without duplicate payment or observation.
- [x] Remove temporary instrumentation before completion.

### Success criteria

- A persistent outer-worker failure produces the exact bounded retry sequence,
  not an immediate loop.
- Once the underlying condition clears, ordinary schedule processing resumes
  and the failure counter resets without losing a wake.
- The live offline-member sample catches up to identical journal, application,
  materialized-state, Bark projection, and balance state.
- The milestone is committed as
  `fix: back off failed federation payment observer rounds`.

### Candidate findings and progress

The retained guard lives only around the existing observer worker. Consecutive
failed rounds wait 1, 2, 4, 8, 16, then at most 30 seconds. Any successful wake,
including a no-op, resets the counter. The delay is in memory and does not
replace, reset, or add fields to the persisted per-quote schedule. Panics use
the same bounded path. No error text, backend, payment method, or federation
readiness variant affects the policy.

A focused test proves the exact capped sequence and reset. The exact release
candidate then stopped member 3, ordered one unpaid receive intent,
`ReceivePrepared`, and mint quote through the remaining four members, and
restarted member 3 three entries behind. Its pre-claim readiness failures
waited at 1, 2, and 4 seconds rather than looping every millisecond. Successful
control wakes reset the sequence as designed. Catch-up completed with all five
members at 55 of 55 applied entries, zero failed entries, identical order and
application digests, identical Bark projections, and unchanged 7,950-sat
spendable / 41,025-sat spent accounting. No external Bark movement or payment
was added. The federation-enabled all-target check and exact release build
passed.

Decision: `Kept`. This is a worker-health circuit breaker for an exceptional
failure path, not a second payment scheduler, so normal payment latency and
backend polling cadence are unchanged.

## Milestone 6C: lost-response wallet melt reconciliation

### Goal

Ensure a wallet that submitted a federated melt can discover the canonical paid
result after losing the aggregate HTTP response, without paying again or
requiring knowledge of Bark.

### Current evidence

The offline-member outgoing sample paid once and all five mint databases now
agree on the paid quote and exact 1,025-sat reduction. The test wallet lost the
aggregate response and still reports 8,975 sats rather than 7,950. Its second
melt quote remains locally `UNPAID`, all 8,975 sats remain locally `UNSPENT`,
and `cdk-cli check-pending` reports no orphaned pending proofs. The wallet
therefore has no durable local marker that lets its ordinary recovery path ask
for the already-paid quote.

### Checklist

- [x] Reproduce the lost-response boundary without killing unrelated wallet
      work and identify where the quote/proof recovery marker must become
      durable.
- [x] Reuse the generic wallet saga and quote-status machinery; do not add a
      Bark- or federation-specific balance repair path.
- [x] Reconcile the canonical paid result idempotently after restart without
      resubmitting the Lightning payment.
- [x] Add focused crash/lost-response regression coverage for standalone and
      federated clients where their request semantics differ.
- [x] Verify the original test wallet reaches the exact 7,950-sat balance or
      document why its pre-fix state lacks sufficient durable identity.

### Success criteria

- A fresh lost-response reproduction repairs to the canonical paid quote and
  spent proofs on restart/check-pending.
- Recovery cannot make another melt request or duplicate the external payment.
- Ordinary standalone wallet behavior remains unchanged or gains the same
  generic safety property.

### Candidate findings and progress

The loss happened after the saga and inputs had correctly become
`MeltRequested`/`Pending`. When the aggregate POST failed, the immediate status
check still returned `Unpaid`; the wallet treated that stale observation as a
terminal failure, reverted the inputs, released the quote, and deleted the only
durable recovery identity. The federation later paid the already accepted
request. This was a generic wallet ambiguity bug, not a Bark or federation
connector failure.

The retained change preserves the existing saga after an ambiguous POST when
the quote is `Unpaid` or `Unknown`. Status polling and startup recovery can then
observe `Paid` and finalize the exact stored inputs and change outputs. An
explicit `Failed` status remains terminal. An `Unpaid` saga is compensated only
after its stored quote expires, preventing a stale pre-consensus status from
releasing proofs while also bounding genuinely abandoned work. Recovery never
calls POST melt.

Focused generic wallet tests prove that an ambiguous response followed by
`Unpaid` retains the saga, inputs, and operation link; a later `Paid` status
finalizes them with exactly one POST call; and an unexpired `Unpaid` status
remains pending while an expired one compensates. All 67 wallet melt tests and
the federation-enabled all-target check passed.

The exact release CLI then paid a healthy 1,000-sat store control with one
member offline in 50 seconds. For the fault sample, the CLI was terminated ten
seconds after submitting a second 1,000-sat store melt. Before recovery the
wallet durably held one `melt_requested` saga and exactly 1,025 sats of inputs
in `Pending`; the store reached `PAID`. The next ordinary CLI startup recovered
one operation, finalized the wallet to exactly 5,900 sats, and removed the saga.
The federation journal stayed at 81 entries and four melt operations before and
after wallet recovery, proving that it issued no second melt request. After the
offline member rejoined, all five members had 81 of 81 entries applied, zero
failed entries, identical order/application digests, and identical 5,900-sat
spendable Bark projections.

The original pre-fix fault had already deleted its saga and operation link, so
normal recovery could not safely reconstruct its transaction or change outputs.
For continued disposable testing only, its two locally stale proofs were
matched against the canonical mint proof-state table and marked spent, restoring
the exact 7,950-sat balance before the new samples. No production repair path is
added for identity that old code discarded.

Decision: `Kept`. The fix removes an unsafe terminal interpretation and reuses
the existing saga/status path; it adds no resubmission loop, backend branch, or
normal-path delay.

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

- [x] Measure queue, data-provider pickup, first peer delivery, finality,
      application, notification, and response with the existing test-only
      per-operation recorder.
- [x] Reuse the controlled Milestone 5 five-member A/B for payload items,
      application-bearing private messages, bytes, and repeated finality.
- [x] Compare consensus latency with healthy whole-payment latency before
      changing global consensus parameters.
- [x] Stop at the precondition gate: the measured consensus stage is not a
      material remaining share, so loopback/injected-delay parameter trials
      would optimize a non-bottleneck.
- [x] Retain the already-tested idle, offline-primary, restart, rotation,
      catch-up, slow/failed signer, and lagging-member behavior from Milestones
      5, 4A, and 6A-6C; no new candidate exists to amplify helper units, CPU,
      retries, or idle work.

### Success criteria

- A kept change improves the targeted consensus stage by at least 15%, moves
  its direct counter as predicted, and respects the 5% regression budget.
- No empty-unit, private-message, retry, CPU, restart, or convergence
  amplification appears.
- A kept result is committed as
  `perf: tune measured federation payment consensus latency`.
- A rejected result is committed as
  `docs: record Bark payment consensus timing evaluation`.

### Findings and decision

The precondition for consensus tuning is false. A clean five-member loopback
probe on the retained code recorded one operation at these boundaries:

| Boundary | Elapsed from submission | Increment |
| --- | ---: | ---: |
| Queue | 0ms | - |
| Data provider yielded the one payload | 109ms | 109ms |
| First peer delivery | 117ms | 8ms |
| First finality | 1,202ms | 1,085ms |
| First apply-loop receipt | 1,220ms | 18ms |
| First durable apply and notification | 1,224ms | 4ms |
| Submitter response | 1,364ms | 140ms after first apply |

The recorder is shared across the five in-process members, so it also observed
five finality/apply paths and 32 operation-bearing private deliveries before
the submitter returned. It is test-only instrumentation already present in the
tree; no temporary or production instrumentation was added.

More importantly, the controlled five-run Milestone 5 comparison already
isolated the only retained consensus-traffic change. Designated publication
reduced median application-bearing messages from 168 to 28 and bytes from
180,600 to 30,128 while median finality improved from 2,137ms to 2,048ms. That
2.048-second median is only 7.3% of the 28.190-second healthy incoming baseline
and 8.5% of the 24-second healthy outgoing baseline. Even deleting the entire
measured consensus interval could not meet this milestone's 15% improvement
gate.

The slower external Signet samples after designated publication coincided with
an explicit public Ark outage and varied independently in the isolated A/B.
They are not evidence that fewer messages added delay. Keep designated
publication: it removes roughly 83% of median application traffic, produces
one finalized data item instead of five, preserves the offline-primary
fallback, and does not regress controlled finality.

Decision: `Rejected` for any additional AlephBFT cadence or transport tuning.
The remaining whole-payment delay is outside this already-small consensus
stage, so changing global cadence, batching, or dissemination would add risk
and complexity without evidence of a material Bark payment benefit.

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
| 2026-07-21 | CDK `690b9a861`, Bark `aa98f5685d70` source audit | supported Bark/Ark mailbox and status APIs; no federation run | event-wake feasibility | payment mailboxes require threshold-custody Schnorr authorization unavailable to individual members; normal mailbox handling mutates wallet state; all-member long-poll has no durable replay or declared connection budget | Reject production event code; retain the persisted generic polling repair path and evaluate narrowed observer polling in Milestone 4 |
| 2026-07-21 | Milestone 3 candidate based on `9f0fe646746d`, Bark `aa98f5685d70` | local release, five loopback members, SQLite, real Signet | three 10,000-sat incoming and three 1,000-sat outgoing successes | incoming click-to-issued 28.405/28.190/27.648s, median 28.190s, 18 operations each; outgoing durable 24/25/24s, median 24s, 11 operations each; four duplicates per sample; all members converged across restart | Accepted Bark results now materialize the ordinary Cashu evidence at `max(t, q)`; the generic consensus family disappeared, removing five live entries per payment and improving or preserving latency |
| 2026-07-21 | Milestone 4 candidate based on `f7d843576598`, Bark `aa98f5685d70` | local release, five loopback members, SQLite, real Signet | two 10,000-sat incoming and three 1,000-sat outgoing successes | incoming: only three Ark callers, 13/14 status calls, 15 operations, 58/63s durable; outgoing: three failure-status plus one actor success-status call, 10 operations, 23/22/22s durable; four duplicate wallet items; all members converged | Keep actor-first threshold observers and evidence-gated six-second fallback; shared-IP Ark eligibility falls from five members to three without a latency or rate-limit regression |
| 2026-07-21 | disposable two-second candidate, reverted | fresh local release federation, SQLite, real Signet | one 10,000-sat incoming success | 59s durable, 66.473s faucet-submit-to-success, 17 status calls, 15 operations, four duplicates | Reject two-second base: latency difference was below the 15% gate while status calls rose about 26% against the retained candidate median |
| 2026-07-21 | Milestone 4 availability injection | local release, five loopback members, one selected member paused | incoming observer fallback followed by claim | fallback member supplied the third HTLC observation after six seconds; HTLC-ready and claim plan were accepted; fixed signer set then stalled and restored signer encountered a non-retryable attempt; no preimage or claim accepted | Observer fallback works, but accepted custody plans cannot recover from a missing fixed signer; isolate as Milestone 4A before sustained release testing |
| 2026-07-21 | exact clean Milestone 4 release binaries | local release, five loopback members, SQLite, real Signet | one 1,000-sat outgoing success and all-member restart | store `PAID` in 23s; exact 25-sat fee; 10 accepted operations; three duplicate wallet items; 40 operations and identical digests on all members before and after restart; no temporary probe in binary | Final rebuild and replay confirm the retained source, cardinality, accounting, terminal cleanup, and convergence without disposable instrumentation |
| 2026-07-21 | Milestone 5 designated wallet publication | local release plus controlled live five-member AlephBFT A/B | one incoming mint, three outgoing melts, five repeated direct-publication controls, and offline-primary fault injection | finalized identical wallet items 5 -> 1; application-bearing messages 164 -> 44 and bytes 176,300 -> 47,344 in the first recorded run; five-run median messages 168 -> 28, bytes 180,600 -> 30,128, and finality 2,137 -> 2,048ms; offline primary fell back after 3s with one item; real-flow duplicate counter stayed at 11 | Keep the generic `Mint`/`Melt`/`Swap` publisher coordinator. The slower external Signet interval preceded an explicit Ark outage and is not a controlled candidate regression; repeated isolated A/B had no median regression. No serial Bark boundary passed the safety-removal gate. |
| 2026-07-21 | Milestone 6 maintenance isolation candidate | local release, fresh five-member loopback federation, SQLite, real Signet | five-minute idle window plus one 10,000-sat incoming success and clean restart | idle scans 300 -> 30 across five members in five minutes; a maintenance deadline during `ReceivePrepared` produced zero maintenance operations and zero custody actions; payment accepted 15 operations, zero duplicate items, completed Bark receive in 18.1s after detection, issued exactly 10,000 sats, and converged on all members; post-terminal refresh resumed | Keep one shared maintenance lease, payment-priority yield, safely expired receive retirement, and no-progress backoff. External payer processing dominated the 320s quote interval. Invalid reservations and non-retryable signer attempts remain exclusively in Milestone 4A rather than driving error-string policy. |
| 2026-07-21 | Milestone 4A signer recovery | exact local release, five loopback members, one member offline, real Signet | recover one already-paid 10,000-sat incoming claim, one refresh, one healthy 1,000-sat outgoing control, and one 1,000-sat outgoing fault | incoming accepted attempts 2/3/4, selected `[1,4,5]`, issued exactly 10,000 sats, and reconverged after catch-up; refresh attempt 2 completed one replacement; healthy outgoing attempt 1 took 33s; offline outgoing attempts 1/2/3 failed safely, attempt 4 selected `[1,4,5]`, paid once in 31s, and charged exactly 1,025 sats | Keep the bounded consensus-authorized signer rotation. It restores one-fault custody liveness without local health selection, error-string policy, duplicate payment, or latency regression. |
| 2026-07-21 | Milestone 6A discovery | exact local release, member restored after outgoing fault | replay 11 accepted operations | journal catch-up reached the current 52-entry head, but application stopped on the accepted outgoing plan at index 41; replay used the later tip to rotate its historical actor lease and rejected actor 5 even though four members had already accepted and executed that exact plan | Bind replay validation to the accepted plan's own operation index while retaining tip-based lease rotation for new admission. Future journal growth must not rewrite historical authority. |
| 2026-07-21 | Milestone 6A retained candidate | exact local release, persisted five-member fault state | all-member restart and historical action replay | all members ready with 52/52 entries applied, zero failed entries, identical order/application digests and Bark projections, 7,950 sats spendable and 41,025 sats spent; no new journal entry, movement, or external payment | Keep exact-index replay validation. New admission still uses the live next index, while immutable history can no longer change authority as the journal grows. |
| 2026-07-21 | Milestone 6B discovery | same restored member before replay terminated | due payment schedule while authoritative reads reject `lagging` | the pre-claim readiness error retried roughly every millisecond; logs grew to tens of megabytes and checkpoint publications timed out | Outer observer failure occurs before the persisted quote claim can advance its deadline. Add one generic in-memory worker failure backoff; retain the persisted schedule as the only per-quote authority. |
| 2026-07-21 | Milestone 6B retained candidate | exact local release, one member stopped while three unpaid-receive setup entries were ordered | lagging-member restart and catch-up | failed worker rounds waited 1/2/4s with reset on successful control wakes; all members reached 55/55 applied entries with identical digests and Bark projections; accounting and external movement counts were unchanged | Keep the contained 1s-to-30s outer worker backoff. It removes the exceptional hot loop without changing normal quote schedules, polling, or payment latency. |
| 2026-07-21 | Milestone 6C discovery | same paid outgoing fault sample after all-member convergence | wallet lost aggregate melt response | mint and Bark state agree on one paid 1,025-sat spend, but the wallet retains an `UNPAID` quote and 8,975 locally unspent sats; `check-pending` finds nothing | The request was not durably represented in the wallet recovery state before the response was lost. Investigate one generic lost-response/saga repair path without resubmitting payment. |
| 2026-07-21 | Milestone 6C retained candidate | exact local release CLI, five loopback members with one offline, real Signet | healthy 1,000-sat control plus client termination 10s after a second 1,000-sat melt submission | healthy CLI/store success in 50s; interrupted wallet retained one saga and 1,025 pending sats; store reached `PAID`; next CLI startup recovered one operation to exact 5,900-sat balance; journal remained 81 and melt operations remained four during recovery; all members converged after catch-up | Keep ambiguous `Unpaid`/`Unknown` melts pending until paid, failed, or expired. Recovery uses GET status only and cannot repeat the external payment. |
| 2026-07-21 | Milestone 7 consensus timing gate | exact retained code, five-member loopback latency probe plus Milestone 5 controlled five-run A/B | one instrumented operation and repeated designated/direct publication comparison | probe submit-to-response 1,364ms: queue-to-provider 109ms, provider-to-first delivery 8ms, first-delivery-to-first-finality 1,085ms, first-finality-to-first-apply 22ms; designated median messages 168 -> 28, bytes 180,600 -> 30,128, finality 2,137 -> 2,048ms | Reject further global consensus tuning because controlled finality improved and is only 7.3-8.5% of healthy whole-payment latency. Keep designated publication; the slower external sample coincided with a public Ark outage, not the message reduction. |

## Completion checklist

- [x] External probing, internal progress, and wallet issuance are measured and
      scheduled as separate concerns.
- [x] No successful internal transition waits for external no-progress backoff.
- [x] Mint and melt use one backend-independent advancement driver.
- [x] Bark event support is either safely integrated as wake-only or explicitly
      rejected with evidence.
- [x] Fresh payments remain responsive without permanent high-frequency polling.
- [x] Healthy evidence uses threshold observers and accepted evidence suppresses
      repeat Ark I/O.
- [x] Bark result quorum directly finalizes the ordinary Cashu payment state.
- [x] Wallet fan-out does not multiply normal consensus submissions.
- [x] Every remaining serial Bark operation protects a documented distinct
      safety or side-effect boundary.
- [x] Maintenance does not compete with active payments and safely terminal
      work stops.
- [ ] Repeated local federation, CLI, faucet, and store measurements satisfy
      the retained milestone gates; the Nocean deployment smoke check passes.
- [ ] Restart, replay, catch-up, member loss, rate limit, threshold failure,
      federated BDK, and standalone regressions pass.
- [ ] Production observability explains every material delay without logging
      secrets or high-cardinality identifiers.
