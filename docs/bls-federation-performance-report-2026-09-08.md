# Federation performance optimization report

Status: in progress. Two DKG arithmetic changes are measured; final campaign
acceptance and payment-performance comparisons remain pending.

Baseline: CDK `8e507cdf8`, Bark `8d1eda643dd11e9eb600ec44e36622c8f77b6322`.
Method and acceptance rules: [approved plan](bls-federation-performance-plan-2026-09-08.md).
Evidence and decisions: [experiment journal](bls-federation-performance-journal-2026-09-08.md).

## Results

| Operation | Release baseline | Final candidate | Disposition |
|---|---|---|---|
| Full BLS/FROST DKG and readiness | Pending | Pending | Unmeasured |
| Bark Lightning receive | Pending | Pending | Unmeasured |
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

## Initial inspection

The baseline already caches validated DKG state/plans, retains verified wallet
shares per request, uses focused Bark action queries, and notifies payment
observers after accepted operations. These are existing optimizations, not gains
from this campaign. Inspection identified whole-ceremony-history loading in the
DKG driver as a candidate to measure, not an established dominant bottleneck.

## Completion gates

All campaign matrix, regression, adversarial, real-payment, restart, conservation,
combined-comparison and two-final-profile gates remain pending. No final
performance or correctness claim is made by this report yet.
