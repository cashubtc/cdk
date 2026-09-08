# Federation performance optimization report

Status: in progress. Two DKG arithmetic changes, dispatched/queued recovery-barrier
cleanup, and the disclosed-claim recovery correction are retained. The incoming
collector was reverted after ten matched pairs because its CPU/receipt guard
remained inconclusive. Checkpoint-summary digest reuse was also reverted after
ten idle pairs: CPU and instruction savings were clear, but memory intervals
still permitted regressions above 5%. These are inconclusive guard results,
not confirmed regressions. Final campaign and combined-candidate
payment/performance acceptance remain pending.

Baseline: CDK `8e507cdf8`, Bark `8d1eda643dd11e9eb600ec44e36622c8f77b6322`.
Method and acceptance rules: [approved plan](bls-federation-performance-plan-2026-09-08.md).
Evidence and decisions: [experiment journal](bls-federation-performance-journal-2026-09-08.md).

## Current dispositions

| Change | Implementation status | Evidence scope |
| --- | --- | --- |
| DKG polynomial evaluation and public signer IDs | Retained: `ceaf02216`, `fffa9d688` | Matched in-memory BLS measurements and scaling correctness; full service DKG remains unmeasured |
| Dispatched and queued recovery barriers | Retained: `d880c9703`, `1d1669db4` | Separate reproduced orderings, component comparisons and recovery tests; percentages cannot be added |
| Disclosed Bark claim recovery | Retained: `bacc0f971` | Authorization work reduction, original-payment recovery and further restart; no payment TPS claim |
| Broadcaster notifications | Provisional | Two payment series failed matched-input/state requirements; no accepted speedup |
| Session-scoped replay cursor | Provisional | Quorum/recovery tests; live performance and payment gates outstanding |
| Stable incoming evidence collector | Reverted | Ten-pair CPU/receipt guard remained inconclusive despite more window completions |
| Cached checkpoint digest in summaries | Reverted | Component work reduction and ten-pair idle CPU benefit; memory guard remained inconclusive |

These retained changes have their documented local gates; they do not constitute
final campaign acceptance. The full workload matrix, combined comparison against
the original baseline, final payment/recovery checks and two concluding profiles
remain outstanding.

## Results

| Operation | Release baseline | Measured candidate | Disposition |
|---|---|---|---|
| Full BLS/FROST DKG and readiness | Pending | Pending | Unmeasured |
| Bark Lightning receive | 60/80 window receipts; 69 including drain | 78/80 window receipts | Collector reverted; CPU/receipt guard inconclusive |
| Bark Lightning send | Pending | Pending | Unmeasured |
| BDK on-chain receive | Pending | Pending | Unmeasured |
| BDK on-chain send | Pending | Pending | Unmeasured |
| Ecash send/receive | Pending | Pending | Unmeasured |

Final operation latency breakdowns, sustainable-throughput curves, active/idle
resource comparisons, history scaling and combined gains remain pending.
Component and provisional idle results below have narrower scopes. Existing historical and debug-build results are
not release baselines for this campaign.

### First candidate: DKG public-polynomial arithmetic

Ten alternating release A/B pairs at five members/eight denominations show:

- Full in-memory BLS ceremony median: **18.1% lower**, from 3.881 s to 3.178 s
  (geometric means of run medians; ratio 95% interval 0.7720–0.8685).
- Whole-harness CPU: **18.8% lower**; retired instructions: **21.4% fewer**.
  These process totals include fixture preparation.
- Temporary counters confirmed 4,800 → 3,200 public-polynomial scalar
  multiplications per full BLS ceremony. All temporary probes were removed.
- Full-ceremony p95 remains inconclusive. RSS ratio interval is 0.9918–1.0254.

The change uses Horner polynomial evaluation with the same group library and
validation. Independent arithmetic/identity checks, 641 Cashu tests and 40 DKG
tests passed. These measurements exclude FROST, transport, storage and service
activation; they do not establish payment throughput or full-service DKG gains.
Scaling correctness passed at five/full-denomination, eleven and twenty members.
Final runtime/payment gates are still pending. See the journal and
[all numeric samples](benchmarks/2026-09-08-dkg-horner.csv), including outliers.

### Combined DKG arithmetic result

Five additional matched pairs against the original baseline measured the combined
Horner and public-signer-ID multiplication changes:

| Metric | Original baseline | Combined candidate | Change |
|---|---:|---:|---:|
| Full in-memory BLS ceremony p50 | 3.672 s | 1.406 s | 61.7% lower |
| Full in-memory BLS ceremony p95 | 3.738 s | 1.429 s | 61.8% lower |
| Whole-harness CPU, including fixtures | 23.641 s | 8.819 s | 62.7% lower |
| Retired instructions, including fixtures | 680.304 billion | 245.536 billion | 63.9% fewer |

The journal contains paired confidence intervals and the
[complete combined samples](benchmarks/2026-09-08-dkg-combined.csv).
An independent five-pair ablation establishes the second change's incremental
benefit. No memory reduction or p99 claim is supported. These results cover five
members/eight denominations and exclude FROST, transport, database work and service
readiness. Private-scalar arithmetic and all validation requirements are unchanged.

### Payment fixture validation

The original release server passed fresh five-member HTTP Bark Lightning incoming
and outgoing, BDK on-chain incoming and outgoing, and independent/shared-wallet
ecash receives. Outgoing recipients independently confirmed 1,000 sat each.
The fixed-schedule miner ran separately from on-chain calls. The SDK open-loop
pilot correctly rejected overload before initiation. Exact value conservation:
54,096 deposited − 2,000 external payments − 315 recorded fees = 51,781 sat,
comprising 51,269 sender balance, 288 recipient balances and 224 in unsubmitted
tokens. These are functional pilots, not a final candidate payment gate or a
baseline/final latency or throughput comparison.

### Outgoing on-chain investigation

Temporary probes measured 4.11 and 4.24 seconds of waiting for the broadcaster
after successive accepted signing/authorization stages. A small event-driven
wakeup candidate preserves the existing periodic repair and exact-transaction
checks. It remains provisional.

All 50 payments in the initial comparison series completed with independent
receipts and exact wallet debits. However, fee-estimator drift changed payment
shapes in two pairs, and one candidate post-payment CPU sample was anomalous.
The series therefore supports **no accepted combined performance claim**.
[All numeric samples](benchmarks/2026-09-08-bdk-broadcast-uncontrolled.csv) are retained.
A replacement series used controlled fee inputs and pre-initiation reserve
checks, but subsequently failed the runtime-state audit described below. The analyzer now rejects mismatched
observed inputs rather than relying only on matching configuration.

### Controlled-fee series: starting-state mismatch found

All 50 payments completed with matched fees/proof selections, independent
recipient receipts and audited Bitcoin fees. A later runtime-state audit found
one pending consensus item **before payments** in two candidate runs, while their
baselines had none. The resulting latency/CPU intervals are invalid for acceptance;
the earlier provisional 31% median comparison must not be treated as a gain.
The analyzer now rejects this series with the observed runtime state included.

The same pending work explains why some post-payment CPU samples were not
quiescent. Four restart measurement pairs preserve diagnostic evidence, but they
also contain pending finalization work and support no idle improvement claim.
The recovery-barrier corrections below address the subsequently reproduced
startup orderings. They do not retroactively validate these samples. All 50
controlled-fee requests, phase observations, fees, proof shapes and starting/pending
counts remain in the [numeric ledger](benchmarks/2026-09-08-bdk-broadcast-controlled.csv).
The broadcaster candidate remains unaccepted.

## Initial inspection

The baseline already caches validated DKG state/plans, retains verified wallet
shares per request, uses focused Bark action queries, and notifies payment
observers after accepted operations. These are existing optimizations, not gains
from this campaign. Inspection identified whole-ceremony-history loading in the
DKG driver as a candidate to measure, not an established dominant bottleneck.

## Completion gates

Earlier incoming Lightning diagnostics recovered original pending invoices after
worker-lease routing corrections. Those late recoveries remain documented in the
journal and do not enter the original measurement windows. The subsequent stable
collector was reverted after its ten-pair comparison. The separately retained
E09 correction below recovers disclosed claims without repeated full scans;
neither result establishes final incoming Lightning performance.

The old-session replay-cursor candidate passes 18 quorum/recovery tests but still
requires live performance and payment validation. Checkpoint-summary digest reuse
was removed after its ten-pair resource decision; its prepared payment helpers
were not run against the rejected candidate.

Final campaign-level matrix, feature/FFI, adversarial, SQL/replay, real-payment,
restart, full value-conservation, original-baseline combined comparison and
two-profile gates remain outstanding. Individual completed checks below must not
be treated as that final combined validation.

### Incoming Lightning collector investigation

The stable-collector candidate keeps evidence collection attached to the original
intent actor while allowing the current leased worker to prepare incoming HTLCs.
A concurrent eight-request pilot completed all eight inside 60 seconds. Its
baseline completed four inside that window, three during drain, and left one
pending. Independent receipts verify 16,384 sat for the candidate and 14,336 sat
for the baseline by drain. The pending baseline invoice is preserved for recovery.
[All 16 pilot outcomes](benchmarks/2026-09-08-bark-incoming-isolation-pilot.csv) retain
late/pending results and external payer intervals.

This is **not an accepted performance comparison**: the baseline pilot ran with
an inactive comparison federation still present. The replacement isolated series
stopped at its pre-payment gate when a fresh baseline retained a startup consensus
item. No payment was initiated in that fixture. A subsequent idle sample found
similar CPU use across members, including the member with that item; a bounded
probe is investigating whether it is stale recovery-barrier bookkeeping.
That pilot did not meet the five-pair acceptance requirement. The subsequent
ten-pair result and reversion are recorded below.

One replacement isolated pair subsequently completed with matching funding, a
single warm restart in each arm, and empty pre-payment queues. Candidate: eight
window completions. Baseline: six window completions, one drain completion and
one pending attempt. The [complete partial ledger](benchmarks/2026-09-08-bark-incoming-isolated-partial.csv)
contains all 16 outcomes; one pair cannot establish the required confidence or
acceptance. The next baseline again retained a startup item after its warm restart,
so preparation is held before invoices. A normal warm restart is not a reliable
remedy. Fresh-start attribution subsequently confirmed an identical already accepted
recovery barrier remained pending for more than two minutes.

Local process control resumed after explicit continuation. Earlier approval-review
timeouts prevented two controller-stop attempts; those controllers later exited.
These partial results established no accepted performance improvement. Temporary
probes are absent from source. The collector was subsequently reverted; broadcaster
and replay-cursor candidates remain provisional. Recovery-barrier cleanup is
accepted below.


### Recovery-barrier cleanup

A local recovery barrier remained pending after a peer finalized the identical
generation. That stale entry blocked empty-session rotation and was unnecessarily
requeued during session recovery. The candidate releases the satisfied local
entry while preserving unrelated facts, different generations, rejected barriers,
and independent validation of late units.

Five alternating release component pairs replayed the same public peer prefix,
then recovered the session and drained its resulting payloads:

| Metric | Baseline | Candidate | Change |
|---|---:|---:|---:|
| Mean recovery cycle | 43.843 µs | 23.826 µs | 45.7% lower |
| Whole-harness CPU, including preparation | 1.769 s | 1.378 s | 22.1% lower |
| Redundant recovery payloads per cycle | 1 | 0 | Removed |

The component harness excludes network, SQL and payments. Its held batch of
20,000 runtimes is not representative of live-service RSS. The journal contains
paired confidence intervals and [all ten samples](benchmarks/2026-09-08-recovery-barrier-cleanup.csv).
A fresh five-member HTTP runtime reached healthy, converged state with empty
queues without a second startup. Its 60-second idle sample used 2.207 aggregate
CPU seconds versus 2.223 in an earlier baseline diagnostic; those unpaired samples
support no idle reduction claim. All 55 consensus, 46 runtime recovery, 10 session
and 20 Bark tests passed, as did formatting and scoped strict common-crate lint.
The manual ten-minute checkpoint benchmark was ignored by the recovery test run.
**Keep:** the cleanup eliminates reproduced redundant recovery work and improves
the measured component without changing validation or consensus policy. Full
campaign payment and recovery gates remain pending.

A subsequent fresh baseline containing this cleanup still retained one pending
startup item; its matching candidate completed eight incoming Lightning payments
inside 60 seconds. The baseline stopped at the pre-payment gate. This remains an
[unpaired diagnostic](benchmarks/2026-09-08-bark-incoming-e06-partial.csv), with no
new payment performance claim. The remaining startup case was diagnosed as the
queued ordering below; E06's result above is limited to its dispatched-barrier
recovery trace.

### Queued recovery-barrier cleanup

The next ordering case occurs when a peer accepts the barrier before the local
provider dispatches its queued copy. A fresh bounded probe reproduced this on
E06; a regression failed on that implementation and passes with the queued copy
removed. Unrelated queue contents, ordering and operation metadata are preserved.

Five alternating release component pairs against `d880c9703` measured a mean
recovery cycle of 61.916 → 29.839 microseconds (51.8% lower; paired ratio interval
[0.44678, 0.51982]) and whole-harness CPU of 2.244 → 1.560 seconds (30.5% lower;
interval [0.62595, 0.77240]). The initial redundant payload and its recovery requeue
both disappear. [All ten samples](benchmarks/2026-09-08-queued-recovery-barrier-cleanup.csv)
are retained, along with the journal's invalid harness attempt. These component
percentages cover a different ordering from E06 and cannot be added together.
There is no payment, sustainable-throughput, live-service RSS or p95/p99 claim.
**Keep:** all 56 consensus, 46 recovery, 10 session and 20 Bark tests passed,
along with formatting and strict common-crate lint. The manual checkpoint idle
benchmark remained ignored. A fresh isolated five-member runtime reached healthy,
empty-queue readiness and retained stable state through a 60-second idle sample.
That unpaired sample used 1.805 aggregate CPU seconds; it establishes no idle gain.
Temporary probes/examples are removed. Full combined campaign gates remain pending.

### Current incoming-Lightning comparison and recovery finding

Five matched pairs, each offering eight independent 2,048-sat payments at
concurrency four, produced 30/40 baseline window receipts versus 38/40 candidate
receipts. Including drain, receipts were 35/40 versus 38/40. Baseline retained five
pending requests; candidate retained one pending request and one client failure
requiring reconciliation. All states and identities are preserved.

The geometric mean window-completion ratio is 1.2588 (paired 95% interval
1.0729–1.4768). CPU per window receipt is inconclusive: ratio 1.0033, interval
0.7700–1.3074. Aggregate fixed-window CPU was 97.482459 versus 122.897347 seconds
across the five runs; more payments completed in those windows, so the raw CPU
increase is not itself a CPU-per-payment regression. Peak aggregate RSS ratio was
1.0115, interval 1.0051–1.0180. Whole-observation backend waits increased, while
the predicted reduction in pending evidence collections is not yet conclusive;
these counters span differing drain lengths. Unconditional p50/p95 comparisons
are censored by unfinished/failed requests. There is no p99 or sustainable
throughput conclusion. See [every run](benchmarks/2026-09-08-bark-incoming-with-recovery-runs.csv)
and [every request](benchmarks/2026-09-08-bark-incoming-with-recovery-requests.csv).

**First-block decision: extend to ten matched pairs.** This historical decision
is superseded by the ten-pair reversion below. Neither frozen comparison binary
was changed during the intervening recovery investigation.

### Pending incoming claim recovery

The preserved claim exposed three connected recovery defects: custody rejected
its exact reserved input after accepted disclosure changed it to `PendingSpend`;
post-disclosure handling discarded signer unavailability; and terminal claim
application could not follow an accepted signing retry. The accepted correction
preserves exact input ownership, carries the custody failure to the existing
consensus-authorized retry, and binds finalization to the latest accepted attempt
while retaining original disclosure evidence. Generic ambiguous outcomes remain
pending; nonce safety, external payment identity and value checks remain enforced.

**Original claim recovery now passes.** The existing quote issued 2,048 sat with
unchanged payer records and no new payment. All five members converged on the
accepted disclosure → attempt-two retry → claim sequence. The seven paid invoices
reconcile to 14,336 spendable sat across all eight SDK wallets and equal paid/issued
totals on every member. This checks incoming principal, not complete custody/fee
conservation. The fixture was stopped with all state preserved.

Three projection tests, 24 Axum Bark tests, 19 Bark runtime tests and strict scoped
lint pass for the complete correction. The new regression covers stale attempts,
mixed signers, old-predecessor forks and the first-attempt path. Earlier incomplete
candidates and failed recovery attempts remain documented in the journal.

A diagnostic probe reduced projection reads from 244 to three across three
checks (18,056 versus 222 decoded SQL rows), restoring successful authorization.
Five alternating uninstrumented pairs then reduced mean process CPU from 0.370
to 0.042 seconds per three-check run (88.7% paired reduction; ratio interval
0.0960–0.1337), and instructions by 93.1%. Peak RSS was unchanged within measurement
uncertainty (ratio interval 0.9918–1.0082). See [all measured runs and durations](benchmarks/2026-09-08-pending-claim-authorization.csv).

Every baseline check hit the known timeout; every candidate check authorized the
input. These are component work savings, not CPU/payment, successful-payment
latency or throughput claims. A further restart preserved all receipts and
convergence, issued no additional value, and retained principal reconciliation.
**Keep E09 as one complete recovery correction.** The remaining campaign matrix
and combined performance/idle-resource acceptance are still outstanding.

### Ten-pair incoming collector result: reverted

The complete comparison retained all twenty runs and 160 requests. Baseline
received 60/80 payments within the fixed windows and 69/80 including drain;
candidate received 78/80 within the windows. Eleven baseline requests remained
pending; candidate retained one pending request and one client failure. Later
recovery does not alter these measurement outcomes.

The paired window-completion ratio was 1.2955 (95% interval 1.2139–1.3826), but
CPU per in-window receipt remained inconclusive: 0.9801 (0.8704–1.1037). That
interval permits a regression above the plan's 5% guard. Peak RSS stayed within
the guard, with ratio interval 1.0040–1.0173. Unconditional p50/p95 are censored;
there is no p99 or sustainable-throughput result.

Pending evidence collections fell from 634 to 541 over the observations, while
backend waits rose from 1,679 to 2,868 and projection readiness checks from 3,493
to 4,487. Different drain lengths and completed-work counts prevent treating
these totals as matched-load work savings. The collector change and its added
test were reverted to accepted HEAD; E09's recovery correction remains intact.
All data and the rejected patch are preserved. A narrower redundant-observation
hypothesis requires new profiling before another candidate. The journal records
confidence intervals, the preparation-only network repair, and
[all runs](benchmarks/2026-09-08-bark-incoming-with-recovery-runs.csv) and
[requests](benchmarks/2026-09-08-bark-incoming-with-recovery-requests.csv).

### Checkpoint-summary digest reuse: measured savings, reverted

The owning runtime already caches the digest of its immutable validated public
configuration. Reusing it for stored checkpoint summary validation removes
repeated full-configuration hashing while preserving shape, federation binding,
digest equality and roster checks. Full proof, admission and startup validation
retain their existing paths.

A bounded probe confirmed 100 → 0 configuration-digest calls with identical
summary output. Five alternating uninstrumented component pairs then measured:

| Metric | Baseline | Candidate |
| --- | ---: | ---: |
| Mean validation and summary construction | 1,107.688 μs | 4.071 μs |
| Process CPU per 2,000-check run, including setup | 2.926 s | 0.754 s |
| Retired instructions, including setup | 65.499 billion | 20.093 billion |
| Maximum process RSS | 13.094 MB | 11.833 MB |

Paired CPU reduction is 74.23% (ratio interval 0.2498–0.2658). This demonstrates
component work savings, not service idle CPU or payment throughput. Scoped
checkpoint/recovery, minimal-feature, standalone-common and strict lint checks
pass; temporary counters and harness code are removed from source. See the
[ten component runs](benchmarks/2026-09-08-checkpoint-summary-component.csv) and
journal for all intervals and the harness-only failed build. The subsequent
service comparison did not establish its memory guard, as detailed below.

Ten fresh, independently prepared service idle pairs show **44.08% less aggregate
member CPU** (paired ratio 0.55917, 95% interval 0.53524–0.58417) and **77.93% fewer
instructions** (0.22072, 0.21890–0.22256). Mean CPU per 60-second observation is
2.34085 seconds baseline versus 1.30626 candidate across five members. All arms
kept accepted history and queues unchanged.

The memory guard remains inconclusive: peak sampled RSS ratio interval
0.96587–1.05709 and physical footprint 0.99712–1.05460 still permit increases
above 5%. Package wakeups are also inconclusive (0.74365–1.22072); interrupt
wakeups have interval 0.98444–1.01037. No sample was excluded, including the
lower-RSS baseline in pair 3. See [all twenty idle runs](benchmarks/2026-09-08-checkpoint-fresh-idle.csv).

**Revert E05 under the ten-pair inconclusive rule.** This does not prove a memory
regression, and the measured CPU savings remain recorded. The three production
files were restored to retained HEAD; all binaries, rejected patch and raw
observations remain private. Prepared active-payment helpers were not run against
this rejected candidate. No payment-latency or throughput gain is claimed.
