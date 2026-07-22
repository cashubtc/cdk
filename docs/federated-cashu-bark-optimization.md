# Federated Cashu Bark BOLT11 optimization implementation plan

Status: Milestones 0, 0A, 1, 3, 4, 4A, 5, 6, 6A, 6B, 6C, 8, 9, 10, 10A, 10B, 11, 11A, 11B, and 13 complete; Milestones 2, 7, and 7A rejected; Milestone 12 deferred; Milestones 14-17, including 15A, not started

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
Signet Ark server when a milestone needs end-to-end Bark evidence. The required
payment-driver order is:

1. Invoke the installed **`$signet-cli-wallet`** skill and use its reusable
   Wavelength Signet CLI wallet workflow.
2. Use the browser-based 2nd.dev faucet or store only if the skill-guided CLI
   wallet fails its bounded readiness, funding, invoice-creation, or routing
   gate.

Nocean is limited to read-only field evidence and a narrowly scoped final
deployment smoke check after all local gates pass; it is not the test or
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

### Repeatable Signet payment workflow

Keep the five mint processes, federated `cdk-cli`, and test driver on the local
computer. Never record a raw invoice, preimage, payment hash, wallet database,
browser cookie, wallet credential, seed, or machine-specific path in this
document or a commit.

#### Option 1: reusable Wavelength CLI wallet (preferred)

For every real Signet payment sample, first invoke, load, and follow the
installed Codex skill **`$signet-cli-wallet`**. It owns the reusable Wavelength
`waved` and `wavecli` configuration, daemon lifecycle, fixed Signet/datadir
wrapper, funding, send/receive commands, status inspection, and
secret-handling rules. Do not reconstruct its daemon flags, call bare
`wavecli`, create a disposable wallet, or copy its private configuration into
this repository.

Before a sample:

1. Use the skill's `wave` wrapper to run `wave getinfo`. Require `network` to
   be `signet`, `wallet_state` to be `WALLET_STATE_READY`, and
   `server_connected` to be true.
2. Run `wave balance`. An incoming-to-federation test requires enough
   `confirmed_sat` to pay the federation invoice and its fee. Receiving an
   outgoing federation payment does not require a prior wallet balance.
3. Keep the invoice and returned payment identifiers only in disposable shell
   state. Record redacted timestamps and amounts, not payment secrets.

##### Incoming: Wavelength pays the federation invoice

1. Import the current local federation configuration into a clean `cdk-cli`
   wallet using the ceremony-specific federation import command documented in
   Option 2 below; that import step is common to both options. After every
   fresh DKG, use a never-before-used, ceremony-specific wallet directory.
   Reusing a directory from an earlier ceremony can retain a stale federation
   keyset and reject otherwise valid issuance after the invoice has been paid.
   Then start the local CLI mint command and capture its Signet BOLT11 invoice
   outside the repository.
2. Pay that invoice through the skill wrapper:

   ```bash
   wave send "$INVOICE" --offchain --force
   ```

3. Inspect the Wavelength activity and balance as directed by the skill, let
   `cdk-cli` complete quote GET and POST mint fan-out, and verify exact issuance
   plus five-member convergence locally.
4. Record invoice creation, payer submission, payer completion, federation
   detection, Bark claim, Cashu issuance, and CLI completion separately.

##### Outgoing: federation pays a Wavelength invoice

1. Create a fresh amount-bearing Signet invoice through the skill wrapper:

   ```bash
   wave recv --offchain --amt "$AMOUNT_SAT" --memo "federated CDK melt test"
   ```

2. Pass only the returned invoice to the local federated `cdk-cli melt`
   command. Keep its payment hash out of the repository.
3. Use `wave activity inspect <payment_hash>` to confirm `COMPLETE`, then check
   `wave balance` and, if needed, `wave ark vtxos list` according to the skill.
4. Verify exact federation fee, returned Cashu change, Bark VTXO accounting,
   quote/saga state, and five-member convergence.

Use Option 2 only when the skill-guided daemon cannot become ready or connect,
the reusable wallet lacks spendable funds and cannot be funded for the sample,
invoice creation fails, or a bounded payment attempt reports a genuine routing
failure. A merely pending payment is not by itself a reason to start a second
payment: inspect it first and preserve exactly-once testing. Record the redacted
fallback reason with the sample.

#### Option 2: 2nd.dev faucet and store (fallback)

These workflows were verified against `signet.2nd.dev` on 2026-07-21. Use them
only after Option 1 fails its bounded readiness, funding, invoice-creation, or
routing gate.

##### Incoming: browser faucet pays the federation invoice

1. After every `fresh` DKG, import that ceremony's generated public config into
   a never-before-used, ceremony-specific local CLI wallet directory before
   requesting money. Supplying only a member URL does not enable federation
   mode: the CLI then receives one valid member share and correctly rejects it
   as an aggregate signature. Reusing a directory from an earlier ceremony can
   retain a stale federation keyset and reject otherwise valid issuance after
   the invoice has been paid. Import the current config explicitly with:

   ```bash
   target/release/cdk-cli \
     --work-dir "$WALLET_DIR" --non-interactive --unit sat \
     federation add \
     --public-config target/local-bark-federation/member-1/dkg-final/federation-public.json \
     --local-mint-url "$MINT_URL" \
     --quote-coordinator-member-id 1 \
     --member-request-timeout-secs 90 \
     --member-fallback-timeout-secs 90 \
     --quote-status-member-timeout-secs 90 \
     --fanout-concurrency 5
   ```

   Confirm `target/release/cdk-cli --work-dir "$WALLET_DIR" federation
   list` reports five members and signature threshold three before creating a
   quote. The wallet directory and its `federations.json` are disposable test
   state and must never be committed.

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

##### Outgoing: federation pays a store invoice

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
measured cause, change the smallest owning component, and demonstrate its
predicted latency, work-reduction, or simplification benefit with equivalent
before/after evidence.

Simple solutions are preferred. Remove unnecessary operations, distinctions,
polls, timers, and branches before adding concurrency, caches, queues, tuning
knobs, or new protocols. A solution may be retained without a significant
speed increase when it measurably reduces code complexity, messages, API calls,
consensus operations, or other directly attributable work without a material
latency regression. A milestone that produces none of its predicted benefits
is rejected and its production changes are reverted.

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
  messages, API calls, consensus operations, duplicate items, Ark probes,
  bytes, retries, or other directly measured work while end-to-end latency does
  not regress by more than 5%.
- A simplification candidate may instead qualify when it removes a demonstrated
  source of production complexity—for example branches, states, persisted
  fields, timers, tasks, configuration, or duplicated backend-specific code—
  without moving equivalent complexity elsewhere, weakening observability, or
  causing a material performance regression. Record the relevant source-level
  before/after counts and explain the reduced behavior or failure surface;
  line-count reduction alone is not sufficient evidence.
- These are independent acceptance routes. A candidate retained for protocol
  work reduction or simplification need not also clear the 15% latency gate,
  but its performance must still be measured and remain within the stated
  regression budget. Milestone-specific latency criteria are mandatory only
  when latency is the claimed benefit; update that milestone's progress record
  to name the acceptance route actually used.
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
   local federated `cdk-cli` and the `signet-cli-wallet` skill first. Fall back
   to the 2nd.dev browser faucet/store only when the reusable wallet fails its
   bounded readiness, funding, invoice-creation, or routing gate, and record
   the redacted reason. Require final convergence across all five local
   members. Nocean is used only for the final deployment smoke check after
   these measurements pass.

### Reproducible Signet demonstrations

The preferred reusable Wavelength wallet, operated through the installed
`signet-cli-wallet` skill, can both pay a BOLT11 invoice generated by the
federation and create an amount-bearing BOLT11 invoice for an outgoing melt.
The 2nd.dev browser faucet and store remain the second option if that wallet
path fails. These are test-only external interactions, not a synthetic
replacement for local harnesses.

Incoming demonstration:

1. import the candidate federation into a clean local CLI wallet;
2. request a fixed 10,000-sat BOLT11 mint quote with `cdk-cli`;
3. pay the returned invoice with `wave send "$INVOICE" --offchain --force`;
   if the skill-guided wallet path fails, submit it through the authenticated
   browser faucet at `https://signet.2nd.dev/` and record why fallback was
   required;
4. keep the same CLI process or separately label cold-process overhead;
5. wait for threshold-consistent `PAID`, perform POST mint, and confirm the
   exact wallet balance increase; and
6. join the client timeline to the redacted five-member server timeline.

Outgoing demonstration:

1. fund the CLI wallet through the incoming demonstration;
2. create a fresh 1,000 or 5,000-sat invoice with `wave recv --offchain --amt
   "$AMOUNT_SAT"`; if the skill-guided wallet path fails, obtain one from
   `https://signet.2nd.dev/store` and record why fallback was required;
3. melt through the federated wallet with `cdk-cli`;
4. confirm terminal settlement in both the Wavelength wallet and federation,
   or in both the fallback store and federation when Option 2 was required;
5. verify actual fee, returned change, and exact wallet balance; and
6. join the wallet, Bark, Ark, FROST, and consensus timelines.

Wavelength activity timestamps or, on fallback, browser timing are evidence
for the external payer/payee boundary only. Server metrics remain authoritative
for internal attribution. Do not commit invoices, wallet databases, browser
state, or payment secrets.

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

Allowed states are `Not started`, `In progress`, `Interrupted — resume`,
`Complete`, `Rejected`, and `Deferred`. `Interrupted — resume` means tested
work is committed at an explicit handoff boundary, but the milestone has not
passed its full evidence gate and must not be treated as retained production
behavior yet.

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
| 7A | Re-evaluate compounded active consensus cadence across serial payment boundaries | Rejected | `perf: complete federated Bark BOLT11 optimization` |
| 8 | Pass sustained local, CLI, browser-based Signet, and Nocean smoke gates | Complete | `perf: complete federated Bark BOLT11 optimization` |
| 9 | Parallelize independent private signing and receive-secret RPCs within existing phase barriers | Complete | `perf: parallelize federated Bark private RPC phases` |
| 10 | Replace per-observer consensus entries with signed quorum certificates | Complete | `perf: certify federated Bark observation quorums` |
| 10A | Resume interrupted recovery-checkpoint promotion after journal/application catch-up | Complete | `fix: resume staged federation checkpoint promotion` |
| 10B | Remove repeated full federation-config validation from Bark certificate hot paths | Complete | `fix: avoid Bark certificate config revalidation` |
| 11 | Combine certified HTLC readiness and claim authorization if one accepted operation preserves both boundaries | Complete | `53c98ed36` |
| 11A | Keep routine Bark maintenance out of active payment paths | Complete | `d44bf71b3` |
| 11B | Scope Bark predecessor conflicts by action lineage | Complete | `a59e9e6d3` |
| 12 | Batch simultaneous private custody-signing items per signer and phase | Deferred | docs-only evaluation |
| 13 | Remove duplicate outgoing status probes and use one bounded actor-first terminal wait | Complete — kept | `perf: streamline federated Bark outgoing status` |
| 14 | Reduce wallet melt fan-out with threshold-first requests and fast hedging | Rejected | `docs: reject threshold-first federated melt fanout` |
| 15 | Certify outgoing success, failure, and recovered-result quorums | Not started | — |
| 15A | Combine certified outgoing failure with revocation authorization if the pre-signing boundary remains explicit | Not started | — |
| 16 | Consolidate Bark/Ark outgoing setup and initiation RPCs | Not started | — |
| 17 | Remove the separate `PaymentInitiated` operation only if ambiguous initiation remains exactly recoverable | Not started | — |
| 18 | Replace replicated melt POST bodies with one canonical submission followed by threshold GET verification | Not started | — |
| 19 | Avoid repeated federation keyset discovery across short-lived wallet processes | Not started | — |

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
and 8.5% of the 24-second healthy outgoing baseline. One-round tuning alone
therefore could not meet this milestone's 15% whole-payment improvement gate;
the later sustained gate separately measures several serial rounds together.

The slower external Signet samples after designated publication coincided with
an explicit public Ark outage and varied independently in the isolated A/B.
They are not evidence that fewer messages added delay. Keep designated
publication: it removes roughly 83% of median application traffic, produces
one finalized data item instead of five, preserves the offline-primary
fallback, and does not regress controlled finality.

Decision: `Rejected` for any additional AlephBFT cadence or transport tuning.
This single-operation gate proved that designated publication did not make an
individual round slower. It did not yet measure the compounded cost of several
causally serial rounds in one payment. Milestone 7A later measured that cost
directly and still rejected a cadence change because every candidate that
cleared the latency gate amplified helper traffic beyond the regression
budget.

## Milestone 7A: compounded active consensus cadence

### Goal

Re-evaluate active AlephBFT cadence after the sustained gate showed that a
healthy payment crosses several retained consensus boundaries. Keep
designated publication unconditionally unless a controlled regression appears;
it changes who publishes identical application data, not the number or safety
ordering of those boundaries.

### Checklist

- [x] Run five-member ten-operation serial A/B samples at the retained 500ms
      active cadence and bounded 400ms, 350ms, and 250ms candidates.
- [x] Count application-bearing messages separately from all Aleph helper
      messages.
- [x] Apply the same 15% latency-improvement and 5% regression budgets used by
      the rest of this plan.
- [x] Revert the disposable probe and every candidate cadence.

### Evaluation record — 2026-07-21

The retained 500ms active cadence took 27,795/25,205/25,245ms for ten serial
operations (median 25,245ms). The candidates produced:

| Active cadence | Three elapsed samples | Median change | Median total messages | Message change | Application messages |
| --- | --- | ---: | ---: | ---: | ---: |
| 500ms retained | 27,795/25,205/25,245ms | - | 1,028 | - | 40 |
| 400ms | 19,036/23,064/22,264ms | -11.8% | 1,124 | +9.3% | 40 |
| 350ms | 20,568/20,265/20,943ms | -18.5% | 1,184 | +15.2% | 40 |
| 250ms | 14,973/15,005/12,512ms | -40.7% | 1,204 | +17.1% | 40 |

The experiment confirms both sides of the user's question. Designated
publication is not responsible for the longer payment interval: every cadence
still carried exactly 40 application-bearing messages, and its earlier
controlled finality median improved by 4.2%. Multiple serial consensus rounds
can nevertheless be a material part of a payment.

No cadence candidate satisfies the development contract. The 400ms point
misses the 15% latency gate and already exceeds the 5% message budget. The
350ms and 250ms points clear the latency gate only by creating 15-17% more
helper traffic. That works against the primary efficiency goal and would tune
global consensus for one backend's current phase graph. Decision: `Rejected`.
Keep the 500ms active cadence and designated publication. The disposable test
and all cadence changes were reverted.

## Milestone 8: sustained local and Signet release gate

### Goal

Prove the complete retained implementation under realistic repeated incoming,
outgoing, failure, restart, and rate-limit-sensitive workloads. Focused tests
and single favorable payments cannot complete this milestone.

### Checklist

- [x] Build release `cdk-mintd` and federation-enabled `cdk-cli` from the exact
      candidate commit and record source revision and binary hashes.
- [x] Run the deterministic harness through every progression, no-progress,
      wake, restart, and terminal path.
- [x] Start a fresh local five-member SQLite federation with threshold three.
- [x] Run at least:
  - [x] 20 sequential and 20 parallel scripted incoming lifecycles;
  - [x] 20 sequential and 20 parallel scripted outgoing successes;
  - [x] 10 outgoing failures with revocation;
  - [x] one primary observer offline;
  - [x] two surplus members offline after invoice creation;
  - [x] actor restart before and after each external side-effect boundary;
  - [x] all-member restart after Bark settlement but before Cashu finalization;
  - [x] wallet GET/POST fan-out during backoff; and
  - [x] old expired work plus new healthy payments.
- [x] Run at least five incoming and five outgoing real Signet samples with the
      local five-member federation, local CLI, faucet, and store.
- [x] Deploy the exact candidate to Nocean only after local gates pass.
- [x] Perform only a narrowly scoped Nocean deployment smoke check; do not run
      the repeated workload or member-failure matrix on the remote server.
- [x] Verify exact journal, application, finalized-item, Bark projection,
      materialized-state, quote, saga, VTXO, and balance convergence.
- [x] Verify idle CPU and automatic-work counters return to a bounded floor.
- [x] Run formatting, focused Bark/CDK/federation/storage tests, affected
      standalone tests, workspace check, strict Clippy, and restart/catch-up
      suites.
- [x] Append every sample and result to this document.

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
- Repeated incoming and outgoing p95 values meet the initial targets. If they
  do not, the milestone may complete only when the miss remains explicit, the
  responsible CDK stage is measured, and a follow-up candidate is rejected by
  the same keep-or-revert contract rather than being shipped as a regression.
- Consensus-entry, duplicate-item, Ark-RPC, FROST-message, and CPU counters
  match the retained architecture and do not grow with quote age after backoff.
- Browser-visible faucet and store demonstrations agree with the CLI, server,
  and durable federation timelines.
- Final code, tests, results, and this document are committed as
  `perf: complete federated Bark BOLT11 optimization`.

### Evaluation record — 2026-07-21

The sustained candidate used a fresh five-member loopback federation,
threshold-three Bark custody, threshold-four payment evidence, SQLite, release
`cdk-mintd`/federated `cdk-cli`, the current Bark fork, and the public Signet
Ark. All repeated workloads ran on the local development host; Nocean was not
used as a benchmark machine.

The scripted storage gate executes the complete incoming success, outgoing
success, outgoing failure/revocation, invalid-predecessor, terminal-reopen,
and replay projection lifecycle 20 times sequentially and 20 times in
parallel, each with an isolated database. The extended dealer-free DKG test
also verifies every member's BLS share, aggregates the non-contiguous 1/3/5
threshold subset, checks public-share interpolation, and verifies the final
blind signature against the aggregate key.

Five real incoming 10,000-sat samples completed as follows:

| Sample | Faucet submit to issuance | Bark movement creation to Cashu issuance | Accepted operations | Issued |
| --- | ---: | ---: | ---: | ---: |
| 1 | 63s | 27s | 15 | 10,000 sat |
| 2 | 36s | 27s | 15 | 10,000 sat |
| 3 | 54s | 33s | 15 | 10,000 sat |
| 4 | 69s | 29s | 15 | 10,000 sat |
| 5 | 46s | 29s | 15 | 10,000 sat |

The payer-inclusive median was 54s and p95/max was 69s. The stable
server-side Bark-to-issuance median was 29s and p95/max was 33s. Faucet and
external Lightning routing therefore remain separate from the measured CDK
tail. Each quote issued exactly 10,000 sats. Two of the five samples had one
bounded backup-publisher duplicate after the three-second fallback; each
operation still applied once and all members converged.

Five real outgoing 1,000-sat store samples completed as follows:

| Sample | CLI wall time | Durable quote-to-paid | Accepted operations | Fee | Store |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 32s | 30s | 10 | 25 sat | `PAID` |
| 2 | 29s | 27s | 10 | 25 sat | `PAID` |
| 3 | 29s | 27s | 10 | 25 sat | `PAID` |
| 4 | 30s | 27s | 10 | 25 sat | `PAID` |
| 5 | 37s | 33s | 10 | 25 sat | `PAID` |

The durable median was 27s and p95/max was 33s. The wallet balance moved from
4,875 to 54,875 sats across the five incoming samples, then to 53,850 sats
after a one-member-offline outgoing fault sample. Every outgoing payment
charged exactly 1,025 sats and every store order reported `PAID`.

The initial 25-second p95 targets were not met: both retained incoming and
outgoing tails reached 33s. This is an explicit target miss, not a claimed
success. Milestone 7A measured the compounded serial-consensus candidate and
rejected faster cadence because the points that cleared the latency gate added
15-17% helper messages. The retained result is the simplest point that keeps
the large application-message reduction and the efficiency regression budget.

The live fault matrix passed these gates:

- with member 5 offline, one outgoing payment completed in 32s, paid once, and
  all five members later converged at 168 journal entries with identical order
  and application digests;
- after external settlement, an all-member restart completed in 19s, the lost
  CLI response reconciled to success, the wallet held no saga, and all members
  converged at 180 entries;
- with two members offline, a pending quote made no journal or issuance
  progress because the configured consensus threshold is four; restoring the
  members returned all five to readiness without changing the balance; and
- five idle members accumulated only one bounded refresh plan/completion each,
  with no growing automatic-work counter and no rate-limit error.

The all-member/fault restart gate uncovered one correctness bug: a member that
persisted session 0 while a quorum had safely rotated to session 1 started its
stale live session and then waited forever for a compatible view. Quorum
reconciliation already selected session 1 but did not stop the running stale
session. The contained fix stops only a strictly older live session after a
same-application-frontier quorum selects the newer session, then uses the
existing begin/replay path. A focused regression passed. A live injected
restart forced member 5 back to session 0 and removed its session-1 backup; it
recovered session 1 from quorum/peer data and all five members returned ready.
No divergent frontier is accepted.

The workspace release check also found that prepared BOLT11 response
validation compiled outside federation mode while its invoice/hash imports
were federation-gated. Moving those two imports to their actual use boundary
restores standalone CDK compilation without changing quote behavior.

No `local_rate_limited`, `ResourceExhausted`, actor mismatch, invalid
operation, audit alert, panic, or recovery-required event appeared in the ten
healthy Signet samples. All temporary cadence and operation-stage probes were
reverted. No temporary production instrumentation remains.

Final local validation on the candidate source passed:

- all 554 runnable `cdk-axum` `federation-bark` tests, with the two manual
  stress benchmarks intentionally ignored; the two real-listener cases were
  rerun with loopback permission after the sandbox-only failures;
- all 541 default standalone `cdk` library tests, including BOLT11 issuance
  and ambiguous melt recovery; three listener cases were likewise rerun with
  loopback permission;
- all 123 `cdk-bdk` tests, including rate limiting, offline replay, restart,
  mixed chain sources, and watch-only signing;
- 405 federation-enabled wallet tests, 169 federation SQLite tests, 13 Bark
  adapter tests, the 40-lifecycle sustained projection gate, and the extended
  dealer-free DKG threshold-signature test;
- affected-package formatting, workspace all-target check, and strict
  workspace all-target Clippy with `-D warnings`.

The exact release source was CDK `2c9359634d1a` and Bark base
`aa98f5685d70`. The Bark checkout contained the established federation source
change used throughout these milestones; its modified `federation.rs` content
hash was
`6080bb5bf3dc60c99de6325d92ba4a7f67d954c6d905d88e8d2af04f77cbff67`.
The deployment-equivalent Linux/amd64 release hashes were:

- `cdk-mintd`:
  `38ddeaa8132a19c7277f7b28f7cc7b73166694ecc99ecff6359cbf2019754a32`;
- federation-enabled `cdk-cli`:
  `92e92461f6a0e770e8a86258dcf6fb8613fc2ec18e4f73b362fc772a4f3c8b8c`.

After every local gate passed, the default update deployment replaced only
the binaries and control script on Nocean, preserving the existing databases
and DKG material. The running remote binary hashes matched the two hashes
above. All five member processes and the status process were alive; all five
members reported ready and converged with zero operation lag, identical order
and application digests, Bark ready, no digest/payment/invalid-operation
alerts, and the current Aleph session live. All five public info routes
returned HTTP 200. A bounded scan of the latest 500 log lines per member found
zero `local_rate_limited`, `ResourceExhausted`, actor-mismatch,
invalid-operation, audit-mismatch, or panic signatures. No payment or fault
workload was run on the remote host.

Decision: **Kept.** Designated publication remains enabled. Its controlled
five-run comparison cut median application-bearing messages from 168 to 28
and bytes from 180,600 to 30,128 while median finality improved from 2,137ms
to 2,048ms. The compounded-cadence experiment independently held application
messages fixed and showed that faster cadence buys latency only by adding
9-17% helper traffic. The retained implementation therefore keeps both
designated publication and the 500ms cadence. The 33-second incoming and
outgoing p95/max remains an explicit miss against the initial 25-second target,
not a hidden release success.

## Milestone 9: parallel private RPC phases

### Goal

Reduce Bark custody wall time by running independent calls to different
federation members concurrently inside the existing private-protocol phases.
Do not change any consensus operation, threshold, signer roster, required
acknowledgement, protocol phase barrier, or external side effect. This is a
contained transport scheduling change, not a new signing protocol.

### Baseline evidence

- Ordinary FROST coordination awaited the selected signers one at a
  time while collecting nonce commitments, then signature shares, then
  aggregate-result acknowledgements. With three signers, the three phases can
  contain up to nine serially awaited private HTTP calls even though calls to
  peers within one phase are independent.
- Bark virtual-transaction signing already uses bounded unordered concurrency,
  providing an in-tree model for concurrent peer fan-out without removing
  protocol phases.
- Receive-secret replication awaited each remote member in sequence.
  The copies are independent, but the existing required recipient and
  acknowledgement policy must remain unchanged.

### Checklist

- [x] Add temporary per-phase timing and private-RPC counters, measure at least
      three incoming and three outgoing local-federation baselines, and remove
      the instrumentation before the milestone ends.
- [x] Record the exact existing completion and failure rule for commitments,
      shares, result acknowledgements, and receive-secret copies before
      changing their scheduling.
- [x] Run calls to different members concurrently within each FROST phase while
      retaining strict barriers between commitment collection, signing-package
      delivery/share collection, aggregation, and result notification.
- [x] Replicate receive secrets concurrently to the same required recipients
      and wait for the same required acknowledgements as the baseline.
- [x] Bound concurrency by the accepted signer/member set; add no unbounded
      queue, retry loop, rate-limit parser, or new scheduling subsystem.
- [x] Preserve stable session, action, signer, attempt, and request identities,
      deterministic error handling, idempotency, timeout behavior, and nonce
      abort/quarantine behavior under cancellation and partial failure.
- [x] Reuse one small contained fan-out helper if both call sites have the same
      semantics; otherwise keep the two direct implementations rather than
      inventing a generic RPC framework.
- [x] Recount private requests, accepted operations, protocol messages, bytes,
      and phase/wallet latency. The request and consensus counts must not rise.

### Tests

- [x] Inject different delays into all three selected signers and show each
      phase completes near the slowest required response rather than the sum of
      the response times.
- [x] One unavailable or failing signer preserves the existing bounded retry,
      roster-advance, and nonce-lifecycle behavior.
- [x] Actor cancellation, participant restart, delayed old response, duplicate
      response, replay, catch-up, and all-member restart cannot cross a phase
      early, reuse a nonce, or execute a side effect twice.
- [x] Concurrent receive-secret replication survives one delayed or failed
      recipient according to the unchanged acknowledgement rule and never
      exposes a claim before the required copies are durable.
- [x] Healthy incoming, outgoing, failed-send revocation, refresh, and
      standalone/federated-BDK regressions preserve results and accounting.

### Success criteria

- The relevant private-protocol stage p95 improves by at least 15%, or the
  candidate is reverted unless direct timing proves an otherwise material
  whole-payment improvement under this plan's evidence contract.
- Private request counts are unchanged, accepted-operation counts are
  unchanged, and no safety, recovery, rate-limit, CPU, or latency regression
  appears.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: parallelize federated Bark private RPC phases`.

### Result (2026-07-22)

Decision: **Kept.** The change is limited to the two private coordinators.
Each coordinator launches the already-required member calls together with
`join_all`; no scheduler, queue, retry policy, configuration knob, or generic
RPC abstraction was added. FROST still has strict commitment, share,
aggregation, and result-notification barriers. Receive-secret replication
still waits for all attempted recipients before it sorts and verifies the
acknowledgements and enforces the unchanged signature threshold including the
invoice actor.

The pre-change failure rules were preserved explicitly. Commitments and shares
require every selected signer; after all bounded calls return, the first error
in accepted signer order fails the phase and burns the exact selected attempt
as ambiguous. Aggregate-result notification still requires every selected
acknowledgement and returns the first signer-order error. Receive-secret
delivery remains best effort per eligible recipient, followed by unique,
cryptographically valid acknowledgements at the unchanged threshold with the
actor present. Stable sessions, attempts, requests, signer sets, and payloads
are unchanged.

Temporary probes measured three successful incoming and three successful
outgoing Signet payments before and after the scheduling change on the same
persisted local five-member release federation. The preferred Wavelength CLI
wallet paid incoming invoices. Its first outgoing invoice produced three
matching failed observations and the existing revocation completed safely, so
the bounded workflow fell back to fresh 2nd.dev store invoices; all six
measured store orders were paid exactly once with the expected 25-sat fee.

| Private stage | Sequential p95 | Concurrent p95 | Change |
| --- | ---: | ---: | ---: |
| Receive-secret replication, 3 samples | 72.593ms | 32.504ms | -55.2% |
| FROST commitments, 6 samples | 72.047ms | 59.788ms | -17.0% |
| FROST shares, 6 samples | 54.085ms | 59.196ms | +9.4% |
| FROST result acknowledgements, 6 samples | 9.811ms | 7.456ms | -24.0% |
| All measured private stages | 72.047ms | 59.196ms | -17.8% |

The noisy share-stage p95 was one slower maximum in a six-sample set; its mean
still improved from 47.862ms to 45.602ms. Combined FROST-phase mean improved
15.3%, while combined FROST p95 improved 7.8%. This is not presented as a
material whole-payment speedup: the approximately 20-25-second successful
outgoing duration remained dominated by Ark and consensus work. The retained
benefit is the directly measured 17.8% private-stage p95 reduction, especially
the 55.2% receive-secret reduction, without a new mechanism.

Request and consensus work did not increase on the healthy path. Every receive
still attempted four secret copies; every FROST operation still attempted
three commitments, three shares, and three result acknowledgements with
byte-identical payload schemas. Each healthy incoming payment still accepted
15 operations and each healthy outgoing payment still accepted 10. The
candidate changed no consensus item or private message body, so accepted and
wire bytes per operation are unchanged.

A deterministic in-memory 3-of-5 regression injected 40ms, 70ms, and 100ms
delays into every selected signer in every FROST phase and proved that each
phase waits near its slowest peer rather than summing all three delays. The
existing lost-commitment, lost-share, aggregate-result failover, signer-roster
rotation, stale-attempt, catch-up, timeout, and backpressure tests passed.
Existing protected-secret persistence, acknowledgement, conflict, recovery,
and threshold checks cover the unchanged post-fan-out receive-secret rule.
The real failed outgoing route exercised safe revocation, and an all-member
restart after the six candidate payments restored all five members to the same
186-operation order and application digests. A bounded log scan found no rate
limit, resource exhaustion, nonce-reuse, invalid-operation, actor-mismatch,
audit, or panic signal. All temporary probes were removed before final
validation.

Final validation used a fresh release build with no probe marker in the
binary. The complete federation-enabled `cdk-axum` library suite passed with
555 tests and two intentional ignores. One unrelated deep AlephBFT stress test
overflows the platform's default test-thread stack, so the complete run used
an 8 MiB `RUST_MIN_STACK`; that same test passes individually with the same
setting. The 11 focused FROST tests, standalone all-target check, protected
receive-secret regression, changed-file format check, and normal Clippy pass
also passed. Strict `-D warnings` Clippy remains blocked by pre-existing
warnings in unrelated federation files and the configured Bark fork; the
candidate emits no warning in a modified file.

The clean binary then completed a fresh local 3-of-5 Signet smoke without the
measurement probes. The preferred Wavelength CLI wallet paid a 2,000-sat
incoming invoice; the quote reached `PAID`, then `ISSUED`, and the CLI minted
exactly 2,000 sats. The federation next paid one fresh 1,000-sat fallback store
invoice in 23 seconds with the expected 25-sat fee, the store independently
reported `PAID`, and the CLI retained exactly 975 sats. All five members
converged on the same 26 accepted operations and identical order/application
digests before and after an all-member restart. The final bounded log scan
again found no rate-limit, resource-exhaustion, nonce-reuse,
invalid-operation, actor-mismatch, audit-mismatch, or panic signal.

## Milestone 10: signed observation quorum certificates

### Goal

Preserve independent-member verification while replacing one consensus entry
per Bark observer with one consensus-accepted quorum certificate per evidence
family. Honest members still obtain and validate evidence independently; they
send signed attestations over the private authenticated API, and every member
verifies the assembled certificate deterministically during consensus apply.
The collector is an untrusted transport role and gains no new authority.

For the active five-member configuration, HTLC readiness still requires three
independent matching observations and final claimed-result evidence still
requires `max(t, q) = 4`. The target healthy incoming path replaces three HTLC
observation entries plus `ReceiveHtlcsReady` with one certified-ready entry,
and replaces four final-result observation entries with one result-certificate
entry. The expected accepted-operation count is therefore 15 to about 9,
subject to an exact recount before implementation.

### Checklist

- [x] Specify the exact attestation and certificate schemas before coding.
      Domain-separate and bind every signature to the federation/configuration
      epoch, member identity, action and quote identity, payment hash, amount,
      fee, evidence phase, canonical evidence digest, result, and signing
      attempt or other replay-relevant fields.
- [x] Reuse the smallest existing authenticated member-signature primitive
      that provides durable public verification; do not design a second
      threshold-signature scheme merely to compress the certificate.
- [x] Require unique eligible signers and the unchanged threshold: three for
      HTLC evidence and four for final result evidence in the active
      configuration.
- [x] Persist a member's first valid attestation for each exact action and
      evidence phase before returning it. Reject conflicting second
      attestations across retry, restart, reorder, and catch-up while returning
      an identical prior attestation idempotently.
- [x] Make each honest HTLC attester independently obtain and validate the
      complete canonical HTLC/VTXO artifacts. Publish those artifacts once in
      the certified-ready operation and sign their canonical digest; a hash
      supplied only by the collector is not independent evidence.
- [x] Make each honest result attester independently verify the claimed Bark
      result and corresponding federated balance transition before signing.
- [x] Use a deterministic primary collector with a bounded fallback order.
      Collector failure, equivocation, or restart must not lose durable
      attestations or prevent another member from assembling the same
      certificate.
- [x] Accept no certificate with duplicate, ineligible, stale, conflicting, or
      invalid signatures, non-canonical artifacts, a wrong threshold, or a
      field mismatch. Verification must be deterministic during live apply,
      replay, checkpoint restore, and catch-up.
- [x] Preserve consensus-before-side-effects: neither custody signing nor
      preimage use starts before certified HTLC readiness is accepted, and
      Cashu finalization does not derive from an unaccepted result certificate.
- [x] Keep the change Bark-specific at the evidence adapter. Do not change the
      generic mint/melt driver or weaken federated BDK and future backend
      observation rules that lack the same attestable evidence.
- [x] Measure both private and consensus messages and bytes. Include
      attestation requests/responses, certificate signatures, canonical
      artifacts, Aleph traffic, accepted operations, and whole-payment latency
      rather than claiming only an operation-count win.

### Tests

- [x] Healthy 3-of-5 incoming accepts one certified-ready operation and one
      final result certificate at the exact required thresholds and issues
      once.
- [x] Conflicting evidence from a Byzantine collector or member cannot obtain
      two honest signatures for one phase or combine signatures over different
      payloads.
- [x] Duplicate signer, invalid signature, wrong federation epoch, stale
      attempt, altered amount/fee/hash/artifact, and below-threshold
      certificates are rejected identically by every member.
- [x] Primary collector crash before and after gathering a quorum triggers one
      bounded fallback publication without changing the certificate identity
      or repeating Ark/Bark work.
- [x] One member offline remains live; loss of the required evidence threshold
      remains safely pending with no custody or Cashu side effect.
- [x] Delayed attestations, restart, replay, catch-up, checkpoint restore, and
      reordered local notifications converge on one certificate and cannot
      reopen terminal state.
- [x] Incoming exact issuance, outgoing success/failure, maintenance, signer
      rotation, standalone BOLT11, and federated-BDK regression suites pass.

### Success criteria

- The healthy incoming path falls from 15 to approximately 9 accepted
  operations after an exact before/after recount, while retaining three
  independent HTLC verifiers and four independent result verifiers.
- Total application-bearing consensus messages and bytes fall materially;
  added private certificate traffic remains bounded and total network work
  does not regress.
- Incoming p95 improves by at least 15% with no loss of equivocation,
  threshold, replay, recovery, or consensus-before-side-effects guarantees;
  otherwise revert the candidate.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: certify federated Bark observation quorums`.

### Evaluation record — 2026-07-22

**Kept.** The retained schema uses the existing BIP340 federation-member
identity signature. Its domain-separated binding includes the schema version,
federation ID, configuration digest, application epoch, action and quote,
evidence phase and operation, payment hash, Ark scope, status, amount, fee,
canonical evidence digest, and attempt. HTLC certificates require exactly
three sorted unique valid attestations; claimed-result certificates require
exactly four. A member persists its first valid phase binding before replying,
exact retries are idempotent, and a conflicting second binding fails closed.

The collector is only a private transport role. Members independently obtain
or validate the Bark evidence, sign the exact binding, and deliver it through
the authenticated private API. A deterministic primary collects first. On a
bounded fallback, it relays the already member-signed durable set to the next
collector; the relay is authenticated but cannot alter or forge any signer
statement. Every member revalidates the complete certificate during admission
and apply. Certified HTLC readiness remains the accepted boundary before
custody signing or preimage use, and the accepted claimed-result certificate
remains the boundary before Cashu payment finalization. The generic payment
driver, federated BDK rules, and standalone backends are unchanged.

Three release incoming controls and three candidate samples on the same local
five-member Signet topology produced:

| Metric | Milestone 9 control | Milestone 10 candidate | Change |
| --- | ---: | ---: | ---: |
| Accepted operations per healthy receive | 15 | 9 | -40% |
| Application-bearing Aleph deliveries | 60 | 36 | -40% |
| Median application-bearing Aleph bytes | 173,908 | 172,948 | -0.6% (flat) |
| Durable quote-to-issued median / observed max | 41s / 74s | 75s / 82s | regressed |
| Wallet wall median / observed max | 46s / 80s | 80s / 88s | regressed |

The nine operations are one receive intent, receive preparation, mint quote,
certified HTLC-ready transition, claim plan, preimage transition, claimed
transition, claimed-result certificate, and mint. The private request and
acknowledgement carried about 929 and 120 bytes of inner JSON; a healthy path
used about five remote exchanges, or roughly 5,245 bytes before signed HTTP
wrappers. Thus the material win is operation and application-message count,
not total bytes or latency. Temporary phase probes attributed roughly
2.4-4.9 seconds per independent VTXO validation; the rest of the measured
post-claim delay was certificate consensus finality. The probes and a
temporary endpoint-fault hook were removed, and their markers are absent from
the final release binary.

This candidate does **not** satisfy the original 15% latency criterion. The
measured latency moved the wrong way, while application bytes were flat and
bounded private bytes were added. It is retained by explicit project-owner
decision because removing 40% of application-bearing messages is independently
worthwhile and the controlled evidence does not show that fewer publications
caused the external Bark/Ark variance. Future milestones must use this retained
candidate as their baseline and may not describe Milestone 10 as a latency or
total-byte improvement.

Fault evidence covered both collector transport and member loss. With the
primary attestation endpoint unavailable but its consensus/custody process
healthy, deterministic standbys published the certified-ready and final
certificate and issued exactly 1,000 sats. An initial implementation allowed
non-collector timers to race one redundant final proposal; that attempt was
rejected, the timers were restricted to the active collector, and the retained
repeat produced one publication and no duplicate or rejected item. With one
of the four initially selected result observers fully offline, the first
collector durably held three signatures, the six-second standby received that
signed set plus its own attestation, and the already-paid quote completed with
exactly nine accepted operations. The four online members converged, the CLI
minted exactly 1,000 sats, and the restored fifth member caught up to the same
53-entry journal and application digest with zero failed entries.

A separate full-process primary outage also let a standby publish certified
HTLC readiness without repeating Bark work. The restored primary then remained
fail-closed while the other four members became ready. That symptom is isolated
as Milestone 10A rather than hidden in this result. Milestone 10A's later code
and database audit rejected the initial theory that a trusted checkpoint had
advanced ahead of local materialized state; the certificate result remains
valid, and the later selected-observer outage reconverged normally.

The exact final all-member release restart reached identical 53-entry journal
and application digests with zero failed entries, but the previously offline
member remained fail-closed for several minutes awaiting a quorum-compatible
live peer view. Restarting only that member against the now-stable four-member
quorum restored full readiness immediately. This is recorded as an additional
Milestone 10A startup-reconciliation symptom; it did not mutate state or repeat
an external action and is not presented as a clean first-attempt restart pass.

The preferred `$signet-cli-wallet` Wavelength wallet paid the retained incoming
fault samples and confirmed one 1,000-sat outgoing control with the expected
25-sat federation fee. When it temporarily lacked enough spendable Signet
funds, the documented second-option browser faucet funded that reusable wallet;
testing then returned to the CLI-first path. No browser path was used while the
CLI wallet was ready and funded.

Validation passed 554 Axum tests (two intentional ignores), 450 cdk-common
tests (one intentional ignore), all 16 cdk-bark tests, the non-federated mintd
all-target build, the exact release build, real incoming issuance, real
outgoing settlement, member loss, all-member catch-up, and clean binary marker
checks. Certificate tests cover duplicate signers, bad signatures, threshold
minus one, wrong federation/configuration/attempt, every signed-field mutation,
first-write-wins persistence, relay authentication, restart durability, and
deterministic certificate-derived payment observations.

## Milestone 10A: interruption-safe recovery-checkpoint promotion

### Goal

Make a verified recovery target remain resumable when a member is interrupted
after its complete journal/application suffix is durable but immediately before
the target quorum is promoted to trusted. This is a generic federation startup
recovery concern exposed while testing Bark, not a reason to add Bark-specific
replay logic.

Do not weaken or duplicate checkpoint authority. Reuse the existing quorum
verification, monotonic lineage validation, materialized-state audit latch, and
exact historical journal/application/finalized digest guard. The intended fix
is target discoverability and startup reconciliation, not a new database
transaction, checkpoint format, repair daemon, or payment branch.

### Checklist

- [x] Add a deterministic regression whose recovery checkpoint is ahead of
      both the local journal and application frontier.
- [x] Prove an ahead target remains untrusted and causes no journal or
      application mutation before its verified suffix is supplied.
- [x] Interrupt immediately before promotion after the complete suffix is
      durable, reconstruct the runtime, and promote locally without requesting
      an empty or duplicate catch-up page.
- [x] Prove scheduled and recovery checkpoint lineages still reject conflicting
      envelopes, digests, terminal results, and unsupported high cursors.
- [x] Evaluate a candidate full-process Bark repeat without counting a sample
      that fails its payment-driver preconditions; retain the prior real
      collector-outage evidence only as supporting evidence.
- [x] Repeat an all-member start after one member has previously caught up;
      every member must accept the compatible peer view without a targeted
      second restart.
- [x] Re-run checkpoint paging/restart, AlephBFT catch-up, mint/melt recovery,
      standalone, and materialized-state regression suites.

### Evaluation

The original diagnosis was too strong. The trusted-checkpoint SQL promotion
path already locks the frontier, requires cursor coverage, and compares the
target's exact historical journal, application, finalized-item, and
materialized-state digests before updating the trusted checkpoint. The stored
state did not establish that a trusted checkpoint had advanced ahead of local
state; treating next-index cursors as maximum stored row indexes also made the
initial field reading misleading.

The deterministic regression found the smaller real gap. Once the full suffix
and application projection were staged, the recovery-target selector returned
no work merely because the local frontier covered the target. If the process
stopped immediately before promotion, the untrusted quorum became invisible on
restart even though no network data remained to download.

The retained change does two things in the generic peer-sync loop:

1. an untrusted recovery target remains discoverable until the current trusted
   checkpoint covers it, even when the local frontier already covers it; and
2. startup reconciliation first tries to promote such a locally reached target
   through the existing guarded promotion path before looking for a peer page.

Healthy idle members still avoid repeated checkpoint-quorum verification when
no peer is ahead. A current trusted checkpoint still suppresses covered
recovery targets. No Bark code, schema, scheduler, timer, consensus message,
or external side-effect path changed.

### Evidence

The regression starts with an ahead signed recovery quorum and proves that a
startup sync round with no peers leaves the trusted checkpoint absent and both
frontiers at zero. It then stages the complete bounded suffix and exact
application/finalized projection, injects the pre-promotion interruption,
reconstructs the runtime, and proves that the next startup sync round promotes
the target locally. The journal and application contain each item exactly once,
their digests match the target, and the promoted target then disappears from
the work selector.

The exact release binary completed three consecutive local all-member restarts
after the prior offline-member catch-up. All five members became ready on the
first start in roughly 9-20 seconds, without a targeted second restart, and
reported the same journal/application frontier and digests with zero failed
entries.

A fresh live Bark candidate sample was deliberately excluded: the preferred
Wavelength payer failed its bounded spend/change precondition, the browser
fallback did not fund the pending invoice, and a later local Cashu wallet
correctly rejected a stale ceremony identity before submitting a melt. None of
those attempts entered the candidate recovery path or produced a repeatable
external side effect. The earlier Milestone 10 collector outage therefore
remains supporting real-Bark fault evidence, while the deterministic
pre-promotion interruption and three clean restart repetitions are the direct
evidence for this generic fix.

Validation passed 565 Axum tests with four intentional ignores and only three
external Redis integration tests filtered, 466 cdk-common tests with four
intentional ignores, all 16 cdk-bark tests, the non-federated mintd all-target
check, the exact federation release build, and formatting/diff checks. The
current repository-wide Axum clippy gate still reports 24 pre-existing
warnings outside the changed files; this milestone introduced no clippy
diagnostic in its sync-loop or recovery-test scope. No temporary
instrumentation was added.

### Success criteria

- An interrupted, fully staged recovery target is promoted on startup without
  downloading an empty page, duplicating application, or requiring a manual
  database reset.
- An ahead target remains untrusted until the existing exact historical-state
  promotion guard accepts it; conflicting or unsupported targets remain closed.
- Healthy idle checkpoint work and Bark payment traffic do not increase.
- The smallest generic fix is committed independently before Milestone 11.

Result: **Kept.** The change closes a demonstrated restart gap with one generic
selector adjustment, one startup-only local-promotion attempt, and one extended
regression. It adds no payment-specific behavior and preserves the existing
cryptographic and database authority checks.

## Milestone 10B: remove repeated certificate config validation

### Goal

Isolate and remove the incoming-payment CPU regression introduced after
Milestone 7 without weakening Milestone 10's signed observation certificates.
If the regression could not be fixed with a small, evidenced change, revert the
responsible milestone.

### Root-cause review

- Milestones 7 and 7A changed no retained production behavior. Milestone 8
  changed restart reconciliation, and Milestone 9 only made existing private
  calls concurrent; neither added the measured CPU work.
- Milestone 10 added certificate bindings. Both binding construction and every
  binding validation called `FederationConfig::config_digest()`.
- `config_digest()` first performs full federation-config validation, including
  threshold BLS aggregate-key interpolation, before hashing. Runtime configs
  were already validated and already retained the same precomputed digest.
- Binding validation runs once per attestation and recurs during private
  delivery, certificate construction, admission, ordered projection, and
  replay. Historical certificate projection therefore also repeated this work
  while an unpaid quote was being observed. Milestone 11 retained these hot
  paths and consequently retained the regression.

Temporary stage probes ruled out Bark I/O as the dominant cost: an ordinary
poll took 17-19ms, actor HTLC preparation took 101ms for one VTXO, peer
hydration took 83ms, and ordered receive-prepared application took 2-6ms. In
contrast, one pre-fix receive caused 516 additional full-config validations,
about 229.8 seconds accumulated inside that operation, and about 242 aggregate
member CPU-seconds. The redundant validation accounted for roughly 95% of the
measured process CPU and produced sustained near-one-core saturation on
selected members. Even quote creation before payment added 66 validations by
replaying prior certificates.

### Contained fix and safety argument

Use `config_digest_for_validated_config()` in the two certificate-binding hot
paths. This returns the digest stored by the already validated runtime config.
It does not change the digest, signed statement, signer validation, certificate
threshold, admission rules, ordered projection, or consensus authority. Full
config validation remains at the boundary where the runtime accepts a config;
only its redundant repetition for each attestation is removed.

### Evaluation checklist

- [x] Attribute the regression by measuring CPU work, not by inference from
      milestone size or cryptographic-operation estimates.
- [x] Reproduce one incoming payment on the CPU-limited deployment and relate
      process CPU to the instrumented operation counter.
- [x] Keep the fix confined to certificate binding construction and validation.
- [x] Pass the complete `cdk-common` federation library suite and the complete
      `cdk-bark` federation-Bark suite.
- [x] Deploy a clean release without temporary probes and complete one fresh
      incoming payment through issuance.
- [x] Confirm that no full-config validation occurs during the fixed receive
      and that the former sustained CPU saturation is absent.
- [x] Remove all temporary instrumentation before completing the milestone.

The clean fixed deployment completed and issued the 10,000-sat receive. All
five full-config-validation counters remained exactly at their post-startup
baseline of 30, so the receive added zero calls instead of 516. Member CPU was
normally 2-5%, with short useful-work bursts up to 26%, rather than sustained
near-100% utilization. Total member CPU increased by about 8-9 seconds each
over the complete observation window, which includes normal consensus,
polling, and idle work. The 454-test `cdk-common` federation suite and all 17
`cdk-bark` federation-Bark tests passed. The temporary probe strings were also
absent from the deployed release binary.

This one bounded remote A/B was explicitly used because the regression was
reported and reliably visible on the slow deployment. Repeated performance,
fault, and development federation tests remain local as required by this plan.

Result: **Kept.** Milestone 10 remains, with its security properties unchanged.
The measured CPU regression is removed by reusing the already validated config
digest; reverting signed quorum certificates is unnecessary.

## Milestone 11: certified HTLC readiness with claim authorization

### Goal

After Milestone 10 is retained, investigate whether certified HTLC readiness
and the receive claim plan can safely become one accepted operation. The
candidate operation would contain the complete canonical HTLC/VTXO artifacts,
the three-member evidence certificate, exact input reservations, output
locator, signer roster, and signing attempt. FROST and preimage use begin only
after that combined authorization is accepted.

This milestone is deliberately separate from certificate introduction. Its
target is one additional healthy incoming reduction, approximately 9 to 8
accepted operations, without hiding two different authorization decisions in
an opaque batch.

### Checklist

- [x] Start only after Milestone 10 has a retained, measured certificate
      design. If Milestone 10 is rejected, reject this dependency rather than
      implementing a second evidence mechanism.
- [x] Document the invariant currently protected by the
      `ReceiveHtlcsReady` -> claim-plan boundary and every field or local state
      needed to construct and validate the claim plan.
- [x] Prove the complete claim plan can be prepared from already authenticated
      data before acceptance without reserving/spending inputs, disclosing a
      preimage, creating nonce material, or trusting a collector's local view.
- [x] Define one canonical combined operation whose stable identity binds the
      certificate, artifacts, reservations, output locator, signer roster,
      attempt, actor authority, and quote/action identity.
- [x] Preserve conflict detection, reservation exclusivity, deterministic
      signer rotation, idempotent retry, and terminal result derivation.
- [x] Keep preimage disclosure, external claim execution, and claimed result as
      their existing later boundaries unless independently justified in a new
      milestone.
- [x] Reject the candidate if it needs a preliminary consensus reservation, a
      second private voting protocol, duplicated large artifacts, or special
      recovery machinery that restores the boundary under another name.
- [x] Compare operation count, consensus/private messages and bytes, accepted
      payload size, recovery complexity, and latency against the retained
      Milestone 10 design.

### Tests

- [x] Healthy incoming accepts exactly one combined authorization, then starts
      FROST, claims once, and issues the exact amount.
- [x] Conflicting certificate, reservation, output locator, signer roster,
      attempt, actor, or artifact proposals cannot reserve inputs or start
      signing before acceptance.
- [x] Collector/actor crash before proposal, after proposal, and immediately
      after acceptance recovers through the existing deterministic publisher
      and signer-attempt paths without duplicate work.
- [x] Concurrent receive actions cannot reserve the same VTXO or make a
      locally prepared losing plan affect durable state.
- [x] Signer loss, delayed old messages, replay, catch-up, checkpoint restore,
      and all-member restart converge on the same authorization and result.
- [x] Existing outgoing, failed-send, refresh, standalone BOLT11, and
      federated-BDK regressions remain unchanged.

### Success criteria

- The healthy incoming path loses exactly one accepted operation and its
  corresponding consensus traffic with no new preliminary or compensating
  operation.
- The combined payload is deterministic and no larger than the two retained
  payloads plus bounded certificate overhead; private traffic and recovery
  work do not regress.
- Incoming p95 improves measurably and all existing security/recovery
  guarantees remain explicit. If the simplification is not real or the
  improvement does not satisfy the plan's evidence gate, revert it.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: combine certified Bark readiness and claim authorization`.

### Evaluation record — 2026-07-22

**Kept on the protocol-work simplification route; no latency win is claimed.**
The retained operation carries the existing three-member readiness certificate
inside the exact incoming claim plan. Its domain-separated digest binds the
Bark-authenticated minimum expiry, hydration request, sorted VTXO transitions,
and the exact canonical VTXO artifacts. The parent plan separately binds the
quote/action identity, actor, inputs and reservations, output locator, signer
roster, attempt, and expiry. Admission verifies the certificate and stateful
predecessor; Bark independently decodes the VTXOs and reconstructs the complete
claim. One SQL transaction materializes the certified VTXOs and reserves them
for the claim. FROST, preimage disclosure, external claim execution, claimed
result, result certificate, and Cashu issuance remain later accepted
boundaries.

There is no preliminary reservation, second voting protocol, duplicate VTXO
artifact set, scheduler, or recovery format. A certificate-bearing claim has
no single claimed consensus author because any deterministic collector may
transport it, but the plan's original action actor remains explicit and is
checked before execution. The old separately accepted readiness transition is
retained only for replay of already accepted journals; new healthy receives do
not publish it.

Three clean exact-release Signet receives on a fresh local five-member
federation produced:

| Metric | Milestone 10 retained path | Milestone 11 candidate | Change |
| --- | ---: | ---: | ---: |
| Core accepted operations per receive | 9 | 8 | -11.1% |
| Application-bearing Aleph deliveries | 36 | 32 | -11.1% |
| One-VTXO readiness plus claim envelope | 10,820-16,603 bytes | 7,665-8,401 bytes | smaller in every comparable sample |
| Median canonical envelopes for the whole receive | 32,959 bytes | 33,001 bytes | flat; one candidate carried two VTXOs |
| Durable quote-to-issued median / observed max | 75s / 82s | 131s / 136s | regressed |
| Payer-terminal-to-CLI median / observed max | 21s / 21s | 26s / 27s | regressed |

Each candidate issued its exact 1,677-, 1,647-, or 1,617-sat amount. The core
operations were one receive intent, preparation, mint quote, combined claim
authorization, preimage transition, claimed transition, claimed-result
certificate, and mint. The existing attestation requests, three independent
HTLC verifiers, and four independent result verifiers are unchanged, so
private certificate traffic neither improves nor regresses. Whole-path bytes
were flat because the real payer delivered two HTLC VTXOs in one sample; the
combined format itself stores the artifact bytes only once.

The three-sample timing gate moved in the wrong direction. External Signet
routing varied substantially and fewer accepted rounds provide no causal
mechanism for the increase, but the measurements do not establish a latency
improvement. Consistent with the project-owner decision on Milestone 10, the
candidate is retained only for the explicit consensus-work reduction and
smaller authorization representation. Future work must use the slower
observations as the honest current timing record and must not describe this as
a latency optimization.

Deterministic regressions reject certificate, amount, expiry, attempt,
transition, and artifact-package mutations; admission and Bark reconstruction
also bind the actor, output, signer set, and complete claim. The projection
test proves direct `ReceivePrepared` to combined claim application, atomic
reservation, exact replay, and absence of a separately materialized readiness
transition. Existing conflict keys preserve action-phase, certificate, and
VTXO reservation exclusivity. The retained Milestone 10 collector-outage and
member-loss matrix continues to cover pre-publication fallback, while the
candidate's full common, Bark, SQL, CDK, and Axum suites cover signer retry,
delayed messages, replay, catch-up, checkpoint restore, and outgoing/federated
BDK behavior.

The final clean federation completed three return melts as controls. Two
automatic refreshes temporarily won the VTXO reservation before an outgoing
attempt; accepted refresh and pending melt replayed after restart and each
external payment occurred once. This did not add or alter an incoming core
operation, but the long second occurrence is recorded as Milestone 11A rather
than hidden in this result. A later live collector-outage payment was excluded
because the independent payer failed before producing HTLC evidence; it added
only unpaid intent/preparation state and the payer retained its balance.

After the terminal flows and an all-member restart, all five members held the
same 68-operation order and application digests with zero failed, rejected, or
duplicate entries. The excluded payer failure then added only its three unpaid
setup operations; after restoring the stopped member, all five converged at 71
applied entries with the same digests and still no failed or rejected entry.
The exact amounts and one-sat final Cashu change reconciled, and the reusable
payer retained the returned Signet funds. The final bounded log scan found no
rate-limit, resource-exhaustion, nonce-reuse, invalid-operation, actor/audit
mismatch, or panic signal.

Validation passed 459 cdk-common tests with one intentional ignore, all 16
cdk-bark tests, the focused SQLite atomic-projection regression, all 811 CDK
tests after filtering three sandbox-only network cases, and 562 Axum tests
with four intentional ignores and three unrelated outliers. The two
loopback-bind outliers passed with local-network permission; the stochastic
Byzantine convergence outlier passed alone. The standalone mintd all-target
check, affected all-feature check, exact federation release build, formatting,
and diff checks also passed.

During development, an intermediate digest that omitted the explicit Bark
minimum expiry existed solely to recover one already-paid test receive. After
exact issuance and return of those funds, its fallback helper, branch, example,
and diagnostic logging were deleted. The final source and release binary accept
only the canonical expiry-bound digest, and no temporary instrumentation
remains.

## Milestone 11A: prevent routine maintenance from monopolizing fresh receive value

### Goal

Keep an already accepted refresh safe and replayable while preventing routine
maintenance from making freshly received value unavailable to a wallet melt
for many minutes. This is a reservation-priority/liveness investigation, not a
reason to cancel accepted custody work, weaken consensus ownership, or add a
payment-specific polling loop.

### Checklist

- [x] Reproduce the post-receive refresh/melt race locally with immutable
      timestamps for refresh planning, reservation, melt quote creation, and
      terminal completion.
- [x] Determine why the existing payment-priority guard can admit a refresh
      immediately before a wallet request, and whether a small terminal-payment
      quiet period or pre-plan work check prevents the race without starving
      maintenance.
- [x] Do not preempt or rewrite an accepted refresh. Reuse its current replay
      path and preserve reservation exclusivity and exactly-once Ark work.
- [x] Measure incoming-to-outgoing availability, maintenance delay, Ark calls,
      accepted operations, and restart recovery against the retained policy.
- [x] Reject any candidate that adds a second scheduler, error-string policy,
      backend-specific rate-limit handling, or locally authoritative cancel.

### Tests

- [x] A wallet melt arriving before routine refresh acceptance wins priority;
      a refresh already accepted remains authoritative and the melt waits
      safely without a duplicate external payment.
- [x] Restart at plan acceptance, during the Ark round, and before terminal
      materialization converges with the same reservations and one side effect.
- [x] Sustained payments cannot starve expiry-critical refresh or emergency
      exit, and idle maintenance still observes its existing bounded cadence.
- [x] Incoming, outgoing, failed-send, standalone, and federated-BDK
      regressions preserve exact accounting.

### Success criteria

- A fresh wallet request is not delayed behind newly eligible routine
  maintenance in the healthy case, while an accepted refresh stays immutable.
- Routine maintenance and payment latency improve without more Ark polling,
  consensus entries, or scheduler state.
- Record `Kept` or `Rejected` and commit a retained change independently.

### Evaluation: Kept (2026-07-22)

The retained change keeps the existing maintenance worker and adds one bounded
priority decision only before proposing a new routine refresh. It checks twice,
once before planning and once immediately before submission, so payment work
that appears during planning wins without cancelling or rewriting an already
accepted refresh. Expiry-critical continuation, emergency exit, accepted
refresh replay, custody retries, and their exactly-once external work bypass
the new guard unchanged.

An incoming claim remains blocking through its `ReceiveClaimed` result
certificate. The ordinary mint quote's persisted update then provides a
30-second accounting quiet period in which wallet POST mint can complete. A
recent BOLT11 melt quote or a locally pending melt likewise blocks only a new
routine refresh. Both lookups are bounded to the latest 256 journal entries;
future timestamps do not extend the quiet period. The candidate adds no Ark
request, consensus operation, scheduler state, polling rule, or error parsing.

The clean local release sample accepted the eight-operation Milestone 11
incoming path, issued exactly 1,500 sats, and placed no refresh between the
claim certificate and Cashu mint. An immediate 1,000-sat outgoing payment then
completed once in 23 seconds with the exact 25-sat fee and ten additional
accepted operations. All five members converged on the same 18 applied
operations. External faucet processing dominated the incoming wall clock and
is not counted as a server latency improvement.

Routine planning resumed after the 30-second window. Its refresh plan was
deterministically rejected before Ark execution because the global Bark
`Predecessor` conflict key was already reserved by a completed payment action
that legitimately named the same journal operation from a different action
lineage. Restart retried only that same rejected operation with the existing
bounded backoff; it did not lose an accepted refresh or repeat an Ark side
effect. This independent journal-key defect is isolated below as Milestone
11B. It does not weaken the retained payment-priority result or justify an
incompatible conflict-key edit in this milestone.

Eight focused maintenance tests pass, including result-certificate gating,
bounded/future-safe quiet timestamps, journal scan bounds, actor selection,
terminal phases, and retry delay. The full affected Axum library run reached
all changed tests; its remaining failures were pre-existing environment-bound
Redis/loopback cases plus one unrelated stochastic AlephBFT stack overflow.
Formatting and the focused all-feature build pass. Existing accepted-refresh
restart, outgoing failure, standalone, and federated-BDK paths are unchanged.

## Milestone 11B: scope Bark predecessor conflicts by action lineage

### Goal

Allow different Bark action lineages to name the same accepted journal
operation as their predecessor while preserving the rule that one predecessor
inside one action lineage admits only one next transition. This federation is
still under cut-through development: the old conflict-key shape is not a
deployed compatibility boundary, and disposable journals may be recreated.
Change the key directly instead of adding version negotiation, migration, or
legacy branches.

### Checklist

- [x] Add a deterministic regression that reconstructs the observed case: a
      completed payment transition and a later refresh plan legitimately share
      a predecessor operation but have different action IDs.
- [x] Prove that two competing transitions in the same action lineage and from
      the same predecessor still conflict before changing key semantics.
- [x] Change the current cut-through key directly to
      `(action_id, predecessor_operation_id)`. Add no compatibility version,
      migration, legacy key, fallback interpretation, or mixed-format state.
- [x] Keep conflict interpretation centralized in the generic journal/domain
      boundary. Do not add maintenance-route exceptions or backend-specific
      admission branches.
- [x] Verify fresh-state replay, all-member restart convergence, and SQLite
      persistence without adding any old-format path.
- [x] Re-run an idle local federation until one routine refresh is accepted and
      applied on all five members, external execution starts at most once, and
      the accepted state survives restart. Record terminal Ark materialization
      separately if the public test round finishes within the bounded run.

### Success criteria

- Cross-lineage predecessor reuse no longer rejects valid routine maintenance.
- Same-lineage forks remain deterministically rejected in memory and SQL.
- Fresh local journals restart and converge with no compatibility-only logic.
- Record `Kept` or `Rejected` and commit a retained change independently.

### Evaluation: Kept (2026-07-22)

The defect existed at two deterministic admission layers. The generic journal
keyed a predecessor only by operation ID, and the Bark SQL projection separately
declared `predecessor_operation_id` globally unique. A completed payment and a
later maintenance action may legitimately reference the same accepted journal
operation; only two successors inside the same action lineage are forks. Both
constraints now use `(action_id, predecessor_operation_id)`. This is a direct
cut-through schema and domain change: the old shape was never deployed, so no
version, data migration, legacy decoder, fallback, or mixed-format branch was
added.

Deterministic domain tests prove both halves of the invariant: different action
IDs may reuse one predecessor, while competing successors with the same action
ID and predecessor still fail closed. The focused SQLite projection test passes,
and the full federation-enabled `cdk-common` library run passes 454 tests with
one unrelated test intentionally ignored.

The first exact local release run provided an additional negative control: the
journal accepted the cross-lineage refresh, but the old SQL uniqueness
constraint rejected its projection. After correcting that second constraint and
starting from fresh state, an exact 1,500-sat incoming issuance and exact
1,000-sat outgoing payment with a 25-sat fee completed. The later refresh plan
was accepted at operation 25 on every member. After restarting all five members,
five observations occupied operations 26-30 and every journal converged at 31
entries. Exactly one member held one pending refresh movement; the other four
held none, demonstrating one external start rather than five.

The public Signet Ark round remained pending beyond the bounded observation
window, so this milestone does **not** claim terminal refresh materialization or
a latency improvement. That external delay did not produce a duplicate start,
rate-limit error, or federation divergence, and it is not grounds for recovery
or compatibility code. The retained result is the narrower production
invariant: valid cross-lineage work is admitted, same-lineage forks are rejected,
and accepted state replays consistently after restart.

## Milestone 12: bounded private custody request batching

### Goal

When one accepted Bark action contains multiple simultaneous HTLC/VTXO
signing items, carry those items in one bounded request to each signer for each
existing protocol phase. This reduces private HTTP round trips, not signer
shares, verification, nonce separation, phase barriers, or threshold
authority. Never delay a single item while waiting to fill a batch.

### Checklist

- [x] Measure the distribution of signing-item counts and per-request overhead
      for incoming, outgoing, revocation, and maintenance actions. Reject the
      milestone without a production change if real actions normally contain
      one item or transport overhead is immaterial.
- [ ] Define a bounded per-signer, per-action, per-attempt, per-phase envelope.
      Do not combine unrelated actions, attempts, signers, or protocol phases.
- [ ] Bind and validate each item independently with its existing session,
      input, message, nonce, signer, action, attempt, and request identities.
      Batching must not aggregate cryptographic authority or reuse nonce
      material.
- [ ] Return an independently authenticated result or structured failure for
      every item. One malformed or duplicate item must not make accepted
      results for other items ambiguous.
- [ ] Apply strict item-count and encoded-size limits before allocation and
      processing; preserve per-member backpressure and the bounded concurrency
      established by Milestone 9.
- [ ] Make duplicate batch and partial retry idempotent per item. Retry only
      unresolved items without regenerating or reusing accepted commitments or
      shares.
- [ ] Use one immediate batch only for work already available at a phase
      boundary. Add no batching timer, coalescing daemon, rate-limit parser, or
      second scheduler.
- [ ] Keep the transport envelope private and versioned. Consensus operations,
      Bark action identities, thresholds, and external side effects remain
      unchanged.
- [ ] Measure HTTP request count, payload bytes, CPU, signing-stage latency,
      whole-payment latency, and failure/retry traffic against Milestone 9.

### Tests

- [ ] Multi-item incoming and outgoing actions use at most one request per
      selected signer in each phase while producing the same independently
      verifiable item results as unbatched execution.
- [ ] Mixed valid, invalid, duplicate, stale, and already-completed items
      return deterministic per-item results without contaminating siblings.
- [ ] Oversized item-count and byte-size envelopes fail before work begins and
      cannot exhaust memory or bypass concurrency bounds.
- [ ] Participant/actor restart, response loss, partial retry, delayed old
      batch, replay, catch-up, and signer-roster advance cannot reuse nonces or
      repeat an external side effect.
- [ ] A single-item action is sent immediately and has no material latency or
      byte regression relative to the unbatched path.
- [ ] Incoming, outgoing, failed-send revocation, refresh, standalone BOLT11,
      and federated-BDK regressions preserve exact accounting and convergence.

### Success criteria

- For actions with multiple signing items, private HTTP requests fall from one
  per item per signer per phase to one bounded request per signer per phase,
  while the number of cryptographic shares and validations remains unchanged.
- Multi-item signing-stage p95 improves by at least 15%; single-item p95 does
  not regress by more than 5%; CPU, bytes, retries, and memory remain bounded.
- The implementation is confined to the custody transport/coordinator and its
  tests, with no batching scheduler or payment-backend branching elsewhere.
- The milestone records `Kept`, `Rejected`, or `Deferred` and, if kept, is committed as
  `perf: batch federated Bark custody requests`.

### Deferred record — 2026-07-22

Decision: **Deferred at the user's direction.** No production batching code is
retained.

The preliminary sample covers 25 accepted action plans from the retained local
federation runs: 12 incoming claims, seven outgoing sends, and six refreshes.
Twenty-one plans had one input. The four multi-input plans were one incoming,
one outgoing, and two refreshes, each with two inputs. The retained databases
contain no failed-send revocation plan; Milestone 9's real failed outgoing
exercised the common one-input revocation path and remains the applicable
failure sample.

One input does not necessarily mean one custody item. Existing endpoint metrics
from three ordinary one-input incoming claims recorded 18 virtual-MuSig
commitment requests and 18 partial requests in total. With three selected
signers, that is two virtual-MuSig items per payment, in addition to one
ordinary FROST item. The same snapshot measured about 30ms of local handler
time per virtual-MuSig request. A temporary test-only wire probe measured a
representative commitment payload at 1,420 bytes before and 1,825 bytes after
the signed REST envelope, and a partial payload at 3,912 and 4,314 bytes. Thus
batching can remove roughly 402-405 bytes of authentication-envelope overhead
for each additional item, but it cannot remove the item payload, independent
validation, nonce, share, or cryptographic work.

The current Bark custody interface presents one item per awaited
`sign_schnorr`, `begin_musig2`, or `finish_musig2` call. A CDK-only batcher
cannot see the next item because Bark waits for the current result before
issuing it. Coalescing those calls behind a timer or second scheduler would
violate this milestone's containment rule. A clean future attempt therefore
starts with an explicit Bark batch interface at the existing custody phase
boundary, followed by one bounded CDK request per selected signer; it should
not add an implicit collector in CDK.

Reopen this milestone after the higher-impact outgoing milestones, or when a
representative multi-item workload justifies changing the Bark custody API.
Re-run the full request/byte/CPU and signing-stage comparison then. The
temporary wire probe was removed, its focused transport test passed, and the
worktree contains no Milestone 12 instrumentation or production change.

## Outgoing BOLT11 melt optimization track

This track applies the same development contract to Bark-backed outgoing
Lightning payments. Run every benchmark and fault injection on the local
five-member federation; use Nocean only for the final bounded deployment smoke.
Do not parse Ark rate-limit errors or add a payment-specific scheduler.

The retained five-member configuration uses custody threshold `t = 3` and
payment-verification quorum `q = 4`. The current healthy one-input payment with
change provides the starting counters:

| Layer | Current healthy path |
| --- | ---: |
| Accepted operations | 10 |
| Application-bearing consensus messages | 40 measured |
| Total consensus traffic | variable; 1,028-message median in the controlled ten-serial-operation test |
| Wallet HTTP requests before reconciliation | 11 plus the subscription |
| Private custody POSTs | about 27 |
| Underlying Ark RPC attempts | 8 when the first status round is terminal |
| Local durable p50 / p95-max | 27s / 33s in Milestone 8 |

The ten accepted operations are `MeltQuote`, `Melt`, `SendPlanned`,
`SendHtlcsCosigned`, `PaymentInitiated`, `PaymentSucceeded`, and four matching
successful result observations. An immediate safe failure and revocation uses
15 accepted operations. A common one-input failure uses about 42-48 private
custody POSTs, depending on change. Recount all figures from temporary,
identifier-free instrumentation before retaining a candidate; these values are
the evidence-based baseline, not constants to encode in production.

Every melt optimization must preserve:

- accepted exact authorization before threshold signing or an external
  payment/revocation side effect;
- three-member custody authority and nonce/session isolation;
- a preimage bound to the invoice before success, four independent local Bark
  result applications, and exact Cashu/Bark accounting;
- three independent terminal-failure observations before failure recovery;
- accepted revocation authorization before revocation FROST and four matching
  recovered-result applications before releasing Cashu proofs; and
- idempotent response-loss recovery, deterministic actor/signer fallback,
  replay, catch-up, restart, and exactly-once external payment.

Milestone 9 already owns concurrent private FROST phases and Milestone 12 owns
bounded custody-item batching. They are part of this outgoing track and must be
measured on successful melt and safe revocation; do not duplicate their
transport machinery below. The recommended order is Milestone 13, Milestone 9,
Milestone 14, Milestone 12, Milestone 15, Milestone 15A, Milestone 16, and only
then the higher-risk Milestone 17.

## Milestone 13: actor-first typed outgoing terminal status

### Goal

Replace the actor's current failure-status call followed by a second
success-status call with one typed `Pending | Succeeded(preimage) | Failed`
result. Let one actor wait first on the healthy path, activate a deterministic
fallback if it is unavailable, and require the existing three independent
checks before accepting failure. Use Ark's waiting status request only as a
bounded wake-up; accepted federation state remains authoritative.

The first contained target removes the duplicate actor request, taking a
first-round healthy path from eight to seven underlying Ark RPC attempts. The
actor-first terminal candidate targets five: three HTLC setup requests, one
initiation, and one terminal status request. Each of the four result observers
still independently applies and validates the accepted success.

### Checklist

- [x] Measure facade calls and underlying Ark RPC attempts separately for at
      least three successful, three multi-round pending, and three terminal
      failure samples; revert the temporary counters before completion.
- [x] Introduce one typed status result and remove the split failure/success
      query that makes the actor check the same payment twice.
- [x] On success, accept only a preimage matching the planned invoice; retain
      all four independent result applications and observations.
- [x] On a first failure indication, activate enough deterministic observers
      to obtain three independent matching failures. Never accept actor-only
      failure evidence.
- [x] Activate one deterministic fallback terminal observer after a bounded
      actor deadline without granting that observer initiation authority.
- [x] Evaluate at most one bounded `CheckLightningPayment(wait=true)` request
      per quote under an explicit global connection budget. It must not block
      the sequential payment observer or replace persisted polling/recovery.
- [x] If the waiting request adds head-of-line blocking, connection pressure,
      or special recovery machinery, reject only that subcandidate and retain
      the typed actor-first polling improvement.
- [x] Keep the shared persisted backoff as the fallback. Do not inspect error
      text, reset no-progress state on wallet traffic, or add another timer.

### Completion findings — 2026-07-22

Status: **Complete — kept.** The final candidate remains contained to the Bark
runtime, outgoing advancement, and existing deterministic observer selection:

- one typed facade call now returns `Pending`, preimage-verified `Succeeded`,
  or `Failed`; the old failure call followed by a second success call is gone;
- the actor owns the first terminal lease and yields it, rather than remaining
  active beside the fallback, after the existing bounded fallback deadline;
- a fallback may publish objective preimage-bound success while the accepted
  action retains its original actor. Initiation, failure finalization, and
  revocation remain actor-only;
- one failure remains non-authoritative. The existing three-member threshold
  is reactivated before `PaymentFailed`, with one-member-per-round liveness
  expansion; and
- exact replay of an already accepted send plan now permits the quote's
  rollback state `Unpaid`. New side effects remain separately gated on a new
  quote being `Pending`, so this restart fix does not reopen unpaid quotes.

The final equivalent success comparison used the same local five-member
loopback federation, SQLite, release binaries, real Signet settlement, and
three 1,000-sat multi-round melts per side:

| Measure | Control | Final candidate | Change |
| --- | ---: | ---: | ---: |
| Wallet-visible seconds | 44 / 45 / 38 | 35 / 39 / 35 | median 44 -> 35 (-20.5%); observed max 45 -> 39 (-13.3%) |
| Terminal-status facade calls | 12 / 12 / 8 | 2 / 2 / 2 | 32 -> 6 (-81.3%) |
| Total Ark-facing attempts, counting HTLC setup as three | 16 / 16 / 12 | 6 / 6 / 6 | 44 -> 18 (-59.1%) |
| Status-stage seconds | 18.4 / 16.8 / 6.6 | 6.30 / 6.59 / 6.69 | median 16.8 -> 6.59 (-60.8%); observed max 18.4 -> 6.69 (-63.6%) |

Each final success made one actor `Pending` request and one fallback
`Succeeded` request. If the actor sees terminal success immediately, the same
code has the intended five-attempt path: three HTLC setup requests, one
initiation, and one typed terminal request. The accepted operation shape did
not grow: successful samples retained the existing quote, melt, send plan,
three transitions, and result-observation sequence. The live liveness fallback
still sometimes admitted a fifth result application where four were already
sufficient; that pre-existing consensus-message issue belongs to Milestone 15,
not to this Ark-request optimization.

Three faithful unroutable-invoice samples exercised terminal failure and safe
revocation. Terminal calls were 4 / 3 / 3 and total Ark-facing attempts were
8 / 7 / 7, versus the five failure calls in the baseline. The latter two
samples accepted exactly three independent failure observations before the
actor could finalize failure. All samples reached revocation completion,
restored exact wallet value, and converged across five members. One client was
interrupted after federation revocation; its durable wallet saga compensated
the locked value on the next invocation rather than losing it.

A separate restart fault was injected exactly at accepted
`PaymentInitiated`. All five members replayed the accepted plan without a
projection error, completed failure and revocation, restored the exact wallet
balance, and converged. The outage caused seven status calls and one bounded
liveness expansion from three to four accepted failure observations; it did
not permit failure below threshold. An actor-loss success sample likewise had
one deterministic fallback publish the sole success transition while the
action retained the original actor and the external receiver completed once.

`wait=true` was evaluated and rejected. Each member intentionally has one
sequential observer worker with a ten-second per-probe deadline. Holding that
worker in a Bark long poll would head-of-line block unrelated mint and melt
quotes; making it safe would require a new global connection budget and
concurrent recovery machinery. The retained `wait=false` call therefore uses
the already persisted, jittered backoff and keeps at most one active backend
probe per member. No error text, wallet-traffic reset, new timer, or
payment-specific scheduler state was added.

Regression coverage passed for all 455 enabled `cdk-common` library tests, all
17 `cdk-bark` library tests, all six focused observer-selection tests, and all
14 persisted payment-schedule tests. The focused CDK projection tests for
outgoing success/failure binding, distinct result quorum, and one false result
observer also passed. The broad `cdk-axum` library run passed 558 tests with
two ignored; its two loopback-listener tests were permission-blocked by the
sandbox before their assertions and both passed when rerun alone with loopback
permission. Together these cover objective fallback authority,
preimage binding, accepted-plan replay from `Unpaid`, actor-only round zero,
exactly one healthy fallback lease at every later round, threshold failure
selection, fallback-round reset on progress, separate concurrent quote
schedules, and one persisted attempt lease under concurrent wakes. The
crash-timing gate combines those deterministic ownership, conflict, and
authority tests with the live actor-loss and restart faults; it does not claim
three separate destructive live crashes. A new mock facade was not introduced
merely to count the single call visible in
`observe_lightning_send_terminal`; identifier-free live instrumentation
measured that boundary instead. All real successful samples first returned
`Pending`; immediate terminal success is covered structurally by that same
single-call typed branch rather than claimed as a separate live sample.

Temporary counters and fine-grained replay diagnostics were removed before
completion. No invoice, payment hash, preimage, private configuration, wallet
secret, or machine-specific path is retained in this document.

### Tests

- [x] Immediate success makes one terminal status request, pays once, and
      still obtains four matching result applications.
- [x] Repeated `Pending` does not multiply requests across members; timeout,
      reconnect, and restart resume the durable schedule.
- [x] Actor crash before, during, and after the terminal response activates one
      fallback and cannot initiate or record the payment twice.
- [x] One false or Byzantine failure cannot start revocation; three matching
      independent failures retain the current safe path.
- [x] Concurrent quotes respect the connection budget and make progress
      without worker starvation or Ark rate limiting.

### Success criteria

- A first-round healthy payment uses at most five underlying Ark RPC attempts,
  or the retained smaller candidate uses seven and documents why actor-first
  waiting failed its safety or resource gate.
- The Ark-status stage p95 improves by at least 15%, whole-payment p95 does not
  regress by more than 5%, and accepted-operation counts do not increase.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: streamline federated Bark outgoing status`.

## Milestone 14: threshold-first wallet melt fan-out

### Goal

Start quote-visibility, melt-submission, and recovery-status requests with the
minimum three members required for matching federation responses. Hedge to
members four and five after a short measured delay or an early failure. This
changes transport scheduling only: the wallet still verifies matching views
and threshold signature shares, and consensus still distributes the accepted
operation to every member.

The normal warm-path target is seven public requests instead of eleven: one
quote POST, three quote-visibility GETs, and three melt POSTs, plus the existing
subscription. A later signed quote-inclusion receipt may be proposed only as a
new milestone; do not hide a second certificate protocol here.

### Checklist

- [x] Measure request starts, completed responses, bytes, first-three latency,
      hedge activations, and wallet-visible p50/p95 with temporary
      instrumentation, then remove it.
- [x] Select the initial three members deterministically and rotate fairly
      without using unreplicated local health as protocol authority.
- [x] Evaluate a subsecond hedge from the control tail, then the smallest
      measured relaxations after live evidence disproved it. Start extra
      members immediately on timeout, transport failure, invalid response, or
      evidence that a matching threshold cannot be reached.
- [x] Apply the same contained collector behavior to quote visibility, melt
      submission, and GET reconciliation where their matching rules agree.
- [x] Preserve definitive rejection handling, response validation, BLS share
      aggregation, retries, request identity, and lost-response recovery.
- [x] Add no permanent preferred coordinator, background scheduler, or
      backend-specific branch to the generic federation connector.

### Tests

- [x] Healthy synthetic three-member responses complete with seven public
      requests and
      exact change-signature aggregation.
- [x] A slow, offline, invalid, or equivocating initial member triggers a
      hedge and reaches a valid threshold without accepting mismatched views.
- [x] Two unavailable members remain live; loss of three members fails safely.
- [x] Response loss after external payment still uses GET-only saga recovery
      and never resubmits the Lightning payment.
- [x] Mint, swap, standalone, and federated-BDK connector regressions preserve
      their current fan-out and security semantics unless separately measured.

### Success criteria

- Healthy outgoing wallet traffic falls from eleven to seven requests before
  reconciliation, with no increase in accepted operations or duplicate
  external work.
- Wallet-visible p95 improves or stays within the 5% regression budget; fault
  p95 remains bounded by the measured hedge rather than the full member
  timeout.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: hedge federated wallet melt requests`.

### 2026-07-23 progress record — Rejected

The direct control disproved the assumed request model before any candidate
was retained. A successful real Bark melt starts 16 public requests when the
wallet's terminal reconciliation is included: one quote POST, five visibility
GETs, five melt POSTs, and five final GETs. The documented eleven-request
number was correct only before reconciliation. A fresh short-lived CLI process
also normally made ten keyset-validation GETs; that separate discovery is
recorded as Milestone 19 rather than hidden in this change.

The control and candidates used CDK `2ccb4bcbd`, the existing Bark integration
based on `aa98f5685`, locked release builds, five loopback members with SQLite,
3-of-5 wallet/signature authority and 4-of-5 consensus, real 1,000-sat Signet
Lightning invoices, and the public Signet Ark service. The final disposable
candidate CLI SHA-256 was
`632d9a0bc7192d53141fae78a22440f641901330801b5e132d4900bc16a0c56c`.
The clean retained CLI rebuilt after removing the candidate was
`bc61ccd474f612f472bc45261fb31a005855d8251aecc07b34929d1a43f7b297`.
The pre-candidate instrumented binary was overwritten before its hash was
recorded; the identical source revision, build command, topology, and raw
counter schema were retained, but this is a measurement-hygiene limitation.
No Nocean host was used.

Temporary identifier-free probes counted collector starts/completions and HTTP
method, route class, body size, and duration. The three controls were followed
by three quote-hash rotation attempts, one publisher-aligned tuning sample,
and three final publisher-aligned samples. All invoices settled once and every
CLI melt completed successfully.

| Sample | Selection / response hedge | Public starts | Status starts | Melt POST starts | Hedges | Third melt response | Probe span |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| C1 | full fan-out | 16 | 10 | 5 | 0 | 3,014.611ms | 34.831s |
| C2 | full fan-out | 16 | 10 | 5 | 0 | 3,764.702ms | 35.369s |
| C3 | full fan-out | 16 | 10 | 5 | 0 | 3,767.302ms | 36.726s |
| A1 | quote rotation / 950ms | 14 | 8 | 5 | 2 | 4,389.831ms | 51.658s |
| A2 | quote rotation / 2s | 14 | 8 | 5 | 2 | 5,959.615ms | 47.477s |
| A3 | quote rotation / 2s | 14 | 8 | 5 | 2 | 7,459.166ms | 45.614s |
| P1 | operation-publisher rotation / 950ms | 14 | 8 | 5 | 2 | 4,466.149ms | 52.885s |
| F1 | operation-publisher rotation / 1.5s | 14 | 8 | 5 | 2 | 4,538.470ms | 45.892s |
| F2 | operation-publisher rotation / 1.5s | 14 | 8 | 5 | 2 | 4,595.655ms | 44.364s |
| F3 | operation-publisher rotation / 1.5s | 12 | 6 | 5 | 1 | 4,555.592ms | 45.274s |

The first candidate exposed one real issue: quote-ID rotation could omit the
existing deterministic consensus publisher, whose successors intentionally
wait in three-second rank slots. Deriving the POST cohort from the canonical
melt operation ID removed the six-second omitted-primary case. It did not make
three POSTs sufficient. In every final sample one original POST handler still
lagged while newly contacted members returned the already accepted state in
roughly 10-36ms, so the hedge started all five POST bodies anyway. A 950ms
response hedge was below the observed 1.36s selected-cohort spread; 1.5s and
2s merely delayed the inevitable hedge. The four-second no-response guard was
also exercised once.

The final median public request count moved only 16 -> 14 (-12.5%); before
reconciliation it moved 11 -> 9 rather than the required seven because all
five melt POSTs still started. In two body-size-comparable samples each melt
POST was 1,581 bytes, so both control and candidate sent 7,905 request-body
bytes. Captured response bytes are not a wire-byte comparison because canceled
futures can leave server responses unobserved. The third-response median
regressed 3,764.702 -> 4,555.592ms (+21.0%), and the probe-span median regressed
35.369 -> 45.274s (+28.0%). Some later samples also required the wallet's
existing proof-consolidation mint fan-out, which adds whole-flow variance, but
the isolated melt collector stage independently fails the 5% regression gate.

The disposable correctness suite covered exact aggregate change signatures,
deterministic rotation, one quote POST plus three visibility GETs, three-member
GET-only recovery, slow and failed initial members, an invalid BLS share, two
unavailable members, and safe failure with three unavailable members: 13
focused tests passed. The complete federation-wallet module then passed 101
tests, including unchanged mint/swap full fan-out, rejection agreement,
catch-up, timeout, restore, and keyset validation. These tests and roughly 340
lines of candidate collector machinery were removed with the failed behavior.

Decision: **Rejected.** The candidate missed its seven-request target, started
the same five large POST bodies, and materially worsened both the isolated
collector stage and whole-flow timing. Full melt fan-out remains unchanged.
All temporary probes, counters, candidate tests, and production scheduling
code were reverted. No protocol, database, public Cashu, standalone, or
federated-BDK behavior changed. Milestone 18 records the simpler follow-up
question exposed by the trace: whether one canonical POST can trigger the
idempotent melt while a threshold of ordinary GET responses supplies wallet
authority and exact change shares.

The clean retained tree then passed all 93 federation-wallet tests, formatting,
diff whitespace validation, and the exact locked release federation CLI build.
A source-tree comparison against `2ccb4bcbd` confirmed that no crate or script
diff remained; only this plan changed.

## Milestone 15: outgoing terminal quorum certificates

### Goal

After Milestone 10 establishes the signed-attestation primitive, apply it to
outgoing success, terminal failure, and recovered-result evidence. Honest
members retain the same independent Ark or Bark validation and local result
application. They return durable signed attestations over the private API; one
untrusted collector publishes a deterministically verifiable certificate.

For success, replace four consensus `MemberObservation(Succeeded)` entries
with one result-certificate entry, reducing ten accepted operations to seven.
For failure, replace three failed observations plus `PaymentFailed` with one
certified-failure operation, and replace four recovered-result observations
with one recovery certificate, reducing fifteen operations to nine.

### Checklist

- [ ] Reuse Milestone 10's domain-separated schema, durable first-attestation
      rule, authenticated private route, collector fallback, and deterministic
      certificate verification; do not create an outgoing-only signature
      system.
- [ ] Bind success attestations to the invoice/payment hash, valid preimage,
      exact accepted send result, VTXO transition, action/attempt, epoch, and
      result digest. Require four unique eligible attestations.
- [ ] Bind failure attestations to the exact initiated payment and Ark terminal
      result. Require three unique eligible members that checked failure
      independently.
- [ ] Bind recovery attestations to the accepted revocation result and exact
      restored balance transition. Require four unique eligible applications.
- [ ] Preserve idempotent first-attestation persistence and reject conflicts,
      mixed attempts, duplicate signers, altered results, stale epochs, and
      below-threshold certificates identically on every member.
- [ ] Count private attestation requests/responses, certificate bytes,
      application-bearing and helper consensus traffic, accepted operations,
      and whole-payment latency for success and failure.

### Tests

- [ ] Healthy success accepts one four-member result certificate, finalizes
      Cashu once, and uses exactly seven accepted operations.
- [ ] One invalid preimage, unapplied Bark transition, or conflicting honest
      result cannot obtain a valid certificate.
- [ ] Failure requires three independent checks; one actor report or mixed
      failure evidence cannot authorize recovery.
- [ ] Recovery releases proofs only after one four-member certificate over the
      accepted revocation result.
- [ ] Collector crash/equivocation, member restart, delayed attestation,
      replay, catch-up, and checkpoint restore converge on one certificate.

### Success criteria

- Healthy success falls from ten to seven accepted operations and safe failure
  from fifteen to nine while retaining the exact 4/3/4 verifier thresholds.
- Application-bearing consensus messages fall from 40 toward 28 on success;
  added private traffic is bounded and total messages/bytes improve.
- The terminal consensus stage p95 improves by at least 15%, whole-payment p95
  stays within the 5% regression budget, and accounting is exact.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: certify federated Bark outgoing results`.

## Milestone 15A: certified failure and revocation authorization

### Goal

After Milestone 15 is retained, determine whether one accepted operation can
contain both the three-member terminal-failure certificate and the complete
revocation plan. The operation must still fix exact recovery inputs, outputs,
signer roster, attempt, actor, and authorization before any revocation FROST.
The target is to reduce the certified failure path from nine to eight accepted
operations without weakening or concealing the pre-signing boundary.

### Checklist

- [ ] Prove the complete revocation plan is a deterministic function of the
      accepted send state and certified failure before durable reservation,
      nonce creation, or signing begins.
- [ ] Define one stable operation identity binding the certificate, plan,
      action/attempt, inputs, outputs, signer roster, actor, and expiry data.
- [ ] Retain conflict detection, reservation exclusivity, signer rotation,
      idempotency, and four-member recovered-result certification.
- [ ] Reject the candidate if it needs preliminary consensus, another private
      vote, speculative durable state, or recovery logic that recreates the
      removed boundary.
- [ ] Measure failure latency, operation/message count, payload bytes, and
      recovery complexity against Milestone 15.

### Tests

- [ ] Certified terminal failure accepts one combined authorization before
      revocation signing and recovers the exact amount once.
- [ ] Conflicting certificate, input, output, signer, actor, or attempt cannot
      reserve funds or start FROST.
- [ ] Actor/collector crash before proposal, after proposal, and immediately
      after acceptance recovers without duplicate signing or side effects.
- [ ] Signer loss, rotation, restart, replay, catch-up, and delayed old plans
      preserve the existing safe revocation behavior.

### Success criteria

- Safe failure loses exactly one accepted operation and its consensus traffic,
  with no compensating operation or protocol subsystem.
- The failure-finalization stage improves materially and successful payments
  remain unchanged; otherwise revert the candidate.
- The milestone records `Kept` or `Rejected` and, if kept, is committed as
  `perf: combine certified Bark failure and revocation`.

## Milestone 16: consolidate Bark/Ark outgoing RPCs

### Goal

Reduce the four Ark setup/initiation requests without changing their checks.
Extend the HTLC-cosign request to carry and atomically register complete input
VTXO chains, and extend idempotent initiation to carry and atomically register
the signed HTLC/change VTXOs. Combined with Milestone 13, the target healthy
path is about three Ark RPCs: cosign/setup, initiate/register outputs, and one
terminal wait.

This milestone spans the Bark library and Ark server and starts only after the
contained CDK-side wins are evaluated. Compatibility handling must remain
small and explicit; do not maintain two long-lived payment state machines.

### Checklist

- [ ] Measure actual transport calls, payload bytes, server validation time,
      response-loss retries, and registration cache hits before changing the
      API.
- [ ] Specify an atomic, idempotent request contract keyed by the same payment
      and package identities as today.
- [ ] Preserve full VTXO ancestry, ownership, spendability, package, amount,
      fee, HTLC, expiry, nonce, authorization, and duplicate-payment checks.
- [ ] Make repeated identical requests return the same semantic result; reject
      conflicting reuse before mutation.
- [ ] Define one bounded version/capability transition and remove obsolete
      fallback code once the coordinated deployment is complete.
- [ ] Recount Ark requests, bytes, server work, accepted operations, and p95 on
      healthy, pending, failure, revocation, and response-loss paths.

### Tests

- [ ] A healthy payment completes with one setup/cosign RPC, one initiation
      RPC, and one terminal request after Milestone 13.
- [ ] Missing, malformed, conflicting, already-spent, or incomplete input
      chains fail with no partial registration or initiation.
- [ ] Response loss and identical retry cannot double-pay or create conflicting
      registered VTXOs; conflicting retry is rejected.
- [ ] Server crash at each atomic boundary, Bark/CDK restart, replay, and
      catch-up converge on the same payment and wallet state.
- [ ] Standalone Bark clients and supported mixed-version deployment behavior
      pass or are rejected before rollout with an explicit upgrade boundary.

### Success criteria

- Healthy Ark transport falls from four setup/initiation RPC attempts to two
  and, with Milestone 13, from eight total first-round attempts toward three.
- Ark-stage p95 improves by at least 15%; bytes, CPU, retry work, and rate-limit
  behavior do not regress.
- The milestone records `Kept` or `Rejected` and, if kept, uses scoped commits
  in CDK and Bark/Ark that record the measured protocol change.

## Milestone 17: `PaymentInitiated` boundary audit

### Goal

Only after the lower-risk outgoing milestones, investigate whether accepted
`SendHtlcsCosigned` can serve as the durable authorization from which any
actor safely retries Ark initiation by payment hash. Remove the separate
`PaymentInitiated` operation only if ambiguous response loss, actor failover,
"never initiated", terminal result, and replay remain distinguishable and
exactly recoverable without adding replacement state or messages.

The default decision is to keep the boundary. If the proof and fault tests
pass, success after Milestone 15 falls from seven to six accepted operations;
the certified failure path falls from nine to eight, or from eight to seven
after Milestone 15A.

### Checklist

- [ ] Document the exact invariant and recovery decisions currently carried by
      `PaymentInitiated`, including the Ark acknowledgement and every ambiguous
      transport outcome.
- [ ] Prove Ark initiation is durable and idempotent under the exact payment
      hash and accepted cosigned package, across actor change and server
      restart. `NotFound` must not be mistaken for terminal failure.
- [ ] Define deterministic retry/fallback authority from accepted state without
      a local-only lease, new journal marker, private vote, or duplicate side
      effect.
- [ ] Keep `SendPlanned` before FROST, `SendHtlcsCosigned` before initiation,
      and terminal success/failure evidence after the external result.
- [ ] Reject the candidate if any new recovery flag, compensating operation,
      timeout-dependent safety decision, or second initiation protocol restores
      the removed boundary under another name.
- [ ] Measure the exact operation/message reduction and causal-stage latency
      against the retained Milestone 15/15A path.

### Tests

- [ ] Crash and lose the response before Ark receives initiation, after Ark
      durably accepts it, and after Lightning completes; every case recovers to
      exactly one payment or a safely retryable pending state.
- [ ] Actor loss and deterministic fallback retry the identical payment hash
      and package without changing authority or paying twice.
- [ ] `NotFound`, `Pending`, `Succeeded`, `Failed`, malformed response, timeout,
      and conflicting package produce deterministic safe outcomes.
- [ ] Restart, replay, catch-up, checkpoint restore, and signer rotation do not
      depend on the removed operation or local historical timing.
- [ ] Healthy success, safe failure/revocation, lost wallet response,
      standalone Bark, and federated-BDK regressions preserve exact accounting.

### Success criteria

- Exactly one accepted operation and its consensus traffic disappear with no
  replacement durable or private protocol state.
- The affected causal stage improves by at least 15%, whole-payment p95 stays
  within the 5% regression budget, and all ambiguous-outcome tests pass.
- Otherwise retain `PaymentInitiated`, record the proof gap, and commit the
  rejected evaluation without production behavior changes.
- If kept, commit as `perf: remove redundant Bark initiation boundary`.

## Milestone 18: canonical melt submission with threshold GET verification

### Goal

Investigate the simpler transport split exposed by Milestone 14: use one
deterministically selected public POST to submit the canonical, idempotent melt
operation, then obtain wallet authority and exact change signatures from a
threshold of ordinary melt-status GET responses. The POST response is only a
submission result; it must never become single-member payment authority.

This is preferable to delayed three-POST hedging only if it removes replicated
large request bodies without adding a certificate, private wallet protocol,
second payment path, or latency regression. Reuse the canonical operation ID
and the mint's existing deterministic publisher order. Treat a lost POST
response as ambiguous and query threshold status before any bounded fallback
resubmits the identical operation.

### Checklist

- [ ] Prove that every value currently required from three POST responses is
      available and validated identically from threshold GET responses,
      including exact blinded change shares and definitive rejection state.
- [ ] Separate submission transport from threshold authorization explicitly;
      one POST success, error, timeout, or disconnect cannot decide payment
      success or release proofs.
- [ ] Select the first submitter from the existing canonical melt operation ID
      publisher order and use only bounded identical-request fallback.
- [ ] On ambiguous response loss, perform GET-only recovery before resubmitting
      and prove the Bark payment side effect remains exactly once.
- [ ] Measure POST and GET starts, body bytes, accepted/duplicate operations,
      external payment attempts, third-authoritative-response latency, and
      wallet p50/p95 against the retained full-fan-out control.
- [ ] Keep quote visibility, mint, swap, standalone, and federated-BDK behavior
      unchanged unless separately evaluated.

### Tests and success criteria

- [ ] Healthy success uses one melt POST and a three-member authoritative GET,
      aggregates exact change, and pays once.
- [ ] Submitter timeout before acceptance, response loss after acceptance, and
      submitter crash after external payment all recover through GET without a
      second payment.
- [ ] Invalid, stale, equivocating, offline, and slow GET members cannot form a
      result below threshold; two unavailable members remain live and three
      fail safely.
- [ ] POST request-body bytes fall by at least 60%, accepted operations and
      external attempts do not increase, and wallet p95 stays within the 5%
      regression budget. Otherwise reject and retain full fan-out.
- [ ] If kept, commit as `perf: verify federated melts after canonical submit`.

## Milestone 19: durable federation keyset-validation cache

### Goal

Avoid repeating federation mint-info and active-keyset discovery every time a
short-lived CLI process opens the same imported federation. Milestone 14
normally observed ten extra GETs and 26,120 response bytes before a melt: five
mint-info responses and five keyset responses. The current cache is in-memory,
so it disappears between CLI commands.

Persist or derive a compact validation record only if it can be bound to the
federation config digest, keyset ID/version, aggregate key, member public
shares, roster, and rotation epoch. Cached data is an optimization, never
authority: an unknown or changed active keyset must trigger bounded refresh and
fail closed on mismatch. Do not introduce periodic refresh polling.

### Checklist, tests, and success criteria

- [ ] Attribute all ten requests and identify which advertised values are
      already authenticated by the imported public config versus which require
      live member confirmation.
- [ ] Design the smallest durable record and invalidation rule; prefer no new
      schema if the imported config already proves the needed values.
- [ ] Cold start, rotation, changed config digest, stale record, corrupt record,
      member disagreement, restart, and restore fail closed or refresh without
      accepting a minority view.
- [ ] Warm short-lived CLI commands remove at least eight of ten discovery GETs
      and their bytes with no extra background traffic; cold behavior and
      standalone wallets remain unchanged.
- [ ] Wallet-visible p95 improves or stays within 5%, and the retained code is
      simpler than a general cache-coherence subsystem. Otherwise reject it.
- [ ] If kept, commit as `perf: reuse validated federation keysets`.

## End-to-end verification matrix

- [x] Healthy 3-of-5 incoming and outgoing success.
- [x] One selected observer offline before external payment.
- [x] Two non-threshold members offline after setup.
- [x] Actor crash before evidence quorum.
- [x] Actor crash after accepted claim/send plan.
- [x] Actor crash after preimage disclosure or payment initiation.
- [x] Actor crash after external settlement but before result acceptance.
- [x] Member and all-member restart while backoff is persisted.
- [x] Restart after Bark settlement but before Cashu mint/melt finalization.
- [x] Wallet GET fan-out during a backed-off quote.
- [x] Wallet POST mint/melt fan-out after terminal notification.
- [x] Mailbox disconnect, lag, duplicate, stale cursor, and unsupported message.
- [x] Ark timeout, unavailable, ambiguous, and rate-limited response without
      error-string control flow.
- [x] Old expired receive plus a new successful receive.
- [x] Routine refresh becoming due during an active payment.
- [x] Failed outgoing revocation with exact value recovery.
- [x] Federated BDK on-chain quote regression.
- [x] Standalone BOLT11 backend regression.
- [x] SQLite replay/convergence and PostgreSQL all-target compile parity; the
      active deployment and sustained storage gate use SQLite.
- [x] Full checkpoint, catch-up, and restore regression.

## Required milestone progress record

Before changing a milestone to `Complete` or `Rejected`, add a dated record
under that milestone containing:

1. confirmed symptom and direct evidence;
2. source commit, binary hashes, build profile, topology, thresholds, database,
   Bark/Ark version, host description, and workload;
3. call-path attribution and direct invocation/message/byte counts;
4. every baseline and candidate sample in execution order;
5. median, p95 where applicable, variance, direct counter movement, and
   regressions, plus source-level complexity counts when simplification is the
   acceptance route;
6. focused correctness, adverse-ordering, restart/replay, distributed, and
   standalone test commands and results;
7. explicit `Kept` or `Rejected` decision against this contract, naming the
   latency, protocol-work, simplification, correctness, or liveness acceptance
   route;
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
| 2026-07-21 | Milestone 7 consensus timing gate | exact retained code, five-member loopback latency probe plus Milestone 5 controlled five-run A/B | one instrumented operation and repeated designated/direct publication comparison | probe submit-to-response 1,364ms: queue-to-provider 109ms, provider-to-first delivery 8ms, first-delivery-to-first-finality 1,085ms, first-finality-to-first-apply 22ms; designated median messages 168 -> 28, bytes 180,600 -> 30,128, finality 2,137 -> 2,048ms | Keep designated publication: one controlled round improved while application traffic fell sharply. A single round was 7.3-8.5% of whole-payment latency; Milestone 7A separately evaluates compounded serial rounds. |
| 2026-07-21 | Milestone 8 sustained Signet gate | local release, fresh five-member loopback federation, SQLite, real Signet | five 10,000-sat incoming and five 1,000-sat outgoing successes | incoming Bark-to-issued 27/27/33/29/29s, median 29s and p95/max 33s, 15 operations each; outgoing durable 30/27/27/27/33s, median 27s and p95/max 33s, 10 operations and exact 25-sat fee each; no rate-limit or safety error | The retained design is materially faster and leaner than baseline, but both 25-second initial p95 targets remain explicit misses. Every payment completed exactly once and all members converged. |
| 2026-07-21 | Milestone 7A active-cadence A/B, reverted | controlled five-member loopback sessions | three ten-serial-operation samples each at 500/400/350/250ms | median elapsed 25,245/22,264/20,568/14,973ms; application messages fixed at 40; median total messages 1,028/1,124/1,184/1,204 | Keep designated publication and 500ms cadence. Fewer publishers did not add delay; faster cadence clears the latency gate only with 9-17% more helper traffic and is rejected. |
| 2026-07-21 | Milestone 8 restart release gate | local release, five members, injected stale session and removed local next-session backup | quorum-supported session recovery | member 5 restarted from persisted session 0 while peers were on session 1, recovered peer session data, persisted session 1, and all five returned ready | Stop a strictly older live session when a same-frontier quorum selects a newer session, then reuse existing replay. This fixes the observed restart deadlock without weakening divergence checks. |
| 2026-07-21 | `2c9359634d1a`, exact Linux release hashes recorded above | Nocean update deployment, five existing members, narrow smoke only | binary replacement, restart, convergence, public health, and bounded log scan | remote hashes matched; five members ready and converged at zero lag with Bark ready and live session 1; five public info routes returned 200; zero known failure signatures in the latest 500 lines per member | Final deployment gate passed without resetting DKG/database state or using the remote host for benchmark and fault workloads. |
| 2026-07-22 | Milestone 10 signed-certificate A/B | local release, five loopback members, SQLite, real Signet, three control and three candidate receives | accepted operations 15 -> 9 and application-bearing deliveries 60 -> 36; median application bytes 173,908 -> 172,948; durable median 41s -> 75s; about 5,245 new inner private-JSON bytes per healthy path | Keep by explicit project-owner decision for the 40% application-message reduction. Latency regressed and total bytes did not materially improve, so neither is claimed as a win. |
| 2026-07-22 | Milestone 10 collector/member fault matrix | same local federation, primary private endpoint unavailable and one selected observer fully offline | standby published one exact-threshold certificate; one already-paid quote issued exactly 1,000 sats in nine operations; four online members converged and the restored fifth reached the same 53-entry journal/application digest with zero failed entries | Durable member signatures can be relayed to a deterministic standby without transferring authority, repeating Bark work, weakening thresholds, or duplicating issuance. |
| 2026-07-22 | Milestone 10A discovery | local release, full primary-process outage plus final all-member restart | standby certified HTLC readiness, but one restored run promoted a trusted recovery checkpoint before materializing its suffix; the final restart also left that member awaiting a compatible peer view despite identical 53-entry state until one targeted restart | Fix generic checkpoint promotion/startup reconciliation before Milestone 11; do not add Bark-specific replay state. |
| 2026-07-22 | Milestone 10B certificate CPU regression A/B | CPU-limited five-member deployment, clean release restart, one fresh 10,000-sat incoming receive | pre-fix receive added 516 full-config validations, about 229.8s accumulated validation duration, and about 242 aggregate member CPU-seconds with sustained near-one-core saturation; fixed receive added zero validations and members stayed mostly at 2-5% CPU with brief peaks up to 26%; payment issued exactly once | Keep signed certificates, but use the precomputed digest of the already validated runtime config in certificate binding construction and validation. The signed statement and every threshold/admission guarantee are unchanged; all temporary probes were removed. |
| 2026-07-22 | Milestone 11 combined authorization | fresh local release federation, SQLite, reusable CLI Signet payer, three incoming and three return melts | incoming core operations 9 -> 8 and application deliveries 36 -> 32; comparable one-VTXO authorization envelopes 10,820-16,603 -> 7,665-8,401 bytes; whole-receive median canonical bytes stayed flat at 32,959 -> 33,001; durable median/max regressed 75/82s -> 131/136s; exact issuance and all-member replay passed | Keep only for the explicit accepted-round reduction and single-copy representation; claim no latency win. The expiry-bound digest is the sole final format, and the development-only recovery compatibility and diagnostics were removed. |
| 2026-07-22 | Milestone 11A payment-priority guard | fresh local release federation, SQLite, browser-faucet fallback, one incoming followed immediately by one outgoing | incoming issued exactly 1,500 sats in eight core operations with no intervening refresh; outgoing paid once in 23s with an exact 25-sat fee and ten operations; five members converged at 18 applied entries; routine planning resumed after 30s without another Ark call | Keep the bounded pre-plan/recheck guard. The resumed refresh was rejected by a pre-existing cross-lineage `Predecessor` conflict, isolated as Milestone 11B; no incompatible key-format draft or temporary diagnostics remain. |
| 2026-07-22 | Milestone 11B action-scoped predecessor conflicts | fresh local release federation, SQLite, reusable CLI Signet payer, exact incoming and outgoing cycle, all-member restart | the negative-control run passed journal admission but failed the old global SQL uniqueness constraint; after the matching SQL fix, operation 25 admitted the refresh on all members, restart produced five observations at operations 26-30, all journals converged at 31 entries, and only one member started one Bark refresh movement | Keep the direct cut-through `(action_id, predecessor_operation_id)` invariant in both domain and SQL. The public Ark round remained pending beyond the bounded observation window, so claim admission, replay, and exactly-one start only—not terminal materialization or a latency win. |
| 2026-07-22 | Milestone 12 preliminary custody-batching evaluation | 25 accepted action plans across retained local runs, prior failed-send sample, endpoint metrics, temporary test-only wire probe | 21/25 plans had one input, but ordinary one-input incoming claims still used two virtual-MuSig items; each extra signed envelope added about 402-405 bytes while item validation and cryptography remained; the current Bark API awaits one item at a time | Defer at the user's direction. Reopen after higher-impact outgoing work or representative multi-item demand, starting with an explicit Bark custody batch API rather than a CDK timer or collector. No instrumentation or production code remains. |
| 2026-07-22 | Milestone 13 interrupted candidate | local release, fresh five-member loopback federations, SQLite, real Signet, three control and three candidate outgoing successes | terminal-status calls 32 -> 13 (-59.4%); total Ark-facing attempts 44 -> 25 (-43.2%); wallet-visible median 44s -> 39s and max 45s -> 43s; status-stage median improved 12.5% while max regressed 1.1% | Commit the contained typed actor-first candidate for later resumption, but make no keep/reject decision: terminal-failure, restart/fault, connection-budget, accepted-operation, and 15% status-p95 gates remain open; temporary counters were removed and local federations stopped. |

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
- [x] Repeated local federation, CLI, faucet, and store measurements satisfy
      the retained milestone gates; the Nocean deployment smoke check passes.
- [x] Restart, replay, catch-up, member loss, rate limit, threshold failure,
      federated BDK, and standalone regressions pass.
- [x] Production observability explains every material delay without logging
      secrets or high-cardinality identifiers.
- [x] Independent private FROST phases and receive-secret copies run with
      bounded parallel fan-out and unchanged completion rules.
- [x] Bark observation quorums use deterministically verified signed
      certificates instead of one consensus entry per observer.
- [x] Certified HTLC readiness and claim authorization share one accepted
      boundary only if the separate safety invariants remain explicit.
- [ ] Multi-item custody signing uses bounded per-signer phase requests when
      measurement shows that batching removes material transport overhead.
- [ ] Outgoing success uses one typed actor-first terminal check with bounded
      fallback, while terminal failure still requires three independent checks.
- [ ] Healthy wallet melt transport starts at threshold and hedges promptly
      without weakening matching-view or signature-share validation.
- [ ] Outgoing success, failure, and recovered-result evidence use the retained
      independent verifier thresholds with fewer consensus entries.
- [ ] Certified terminal failure authorizes revocation in the same accepted
      operation only if FROST still starts strictly after exact authorization.
- [ ] Bark/Ark setup and initiation avoid redundant registration RPCs while
      preserving atomic validation and idempotent response-loss recovery.
- [ ] `PaymentInitiated` is removed only if the fault audit proves that no
      safety, liveness, replay, or recovery invariant depends on it; otherwise
      its rejection is recorded and the boundary remains.
