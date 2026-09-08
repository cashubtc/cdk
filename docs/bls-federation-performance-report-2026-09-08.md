# Federation performance optimization report

Status: in progress. Two DKG arithmetic changes are measured. One isolated incoming
Lightning pair is recorded; its optimization is not accepted. Recovery-barrier
cleanup is accepted after five component comparisons, fresh-runtime validation
and scoped regression/lint checks. Final campaign acceptance remains pending.

Baseline: CDK `8e507cdf8`, Bark `8d1eda643dd11e9eb600ec44e36622c8f77b6322`.
Method and acceptance rules: [approved plan](bls-federation-performance-plan-2026-09-08.md).
Evidence and decisions: [experiment journal](bls-federation-performance-journal-2026-09-08.md).

## Results

| Operation | Release baseline | Final candidate | Disposition |
|---|---|---|---|
| Full BLS/FROST DKG and readiness | Pending | Pending | Unmeasured |
| Bark Lightning receive | One isolated pair | One isolated pair | Provisional; five pairs incomplete |
| Bark Lightning send | Pending | Pending | Unmeasured |
| BDK on-chain receive | Pending | Pending | Unmeasured |
| BDK on-chain send | Pending | Pending | Unmeasured |
| Ecash send/receive | Pending | Pending | Unmeasured |

Latency breakdowns, throughput curves, active/idle CPU, history scaling and
combined gains remain pending. Existing historical and debug-build results are
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
Controlled fee inputs, pre-initiation reserve checks and settled idle measurements
are being used for the replacement series. The analyzer now rejects mismatched
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
Investigation is focused on that work before more acceptance runs. All 50
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

The four-concurrent incoming Lightning diagnostic completed two payments and left
two pending after drain. An isolated correction to evidence routing after worker
lease rotation recovered both original payments: independent payer settlement,
matching member states, and 2,048 sat in each original SDK wallet. These late
recoveries do not count toward the earlier window. Fresh concurrent validation
and matched performance acceptance remain pending.

A separate restart reproducer exposed old-session replay cursors being retained
when joining a later session. Its provisional fix passes 18 quorum/recovery
tests. Neither correction has yet passed real-payment or performance acceptance.

All campaign matrix, regression, adversarial, real-payment, restart, conservation,
combined-comparison and two-final-profile gates remain pending. No final
performance or correctness claim is made by this report yet.

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
The collector optimization remains provisional; the required five matched pairs
have not been completed.

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
No new performance improvement is accepted from these partial results. Temporary
probes are absent from source; the production collector, broadcaster and replay
cursor candidates remain provisional. Recovery-barrier cleanup is accepted below.


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
