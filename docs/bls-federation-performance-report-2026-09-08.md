# Federation performance optimization report

Status: in progress. No campaign performance improvement accepted yet.

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
