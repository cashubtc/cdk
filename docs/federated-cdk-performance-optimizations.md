# Federated CDK Performance Optimizations

Status: Implementation plan ready; milestone 0 not started
First recorded: 2026-07-17

## Purpose

This document is the milestone tracker for improving end-to-end federated CDK
transaction throughput, latency, and CPU efficiency. It records measurements,
confirmed causes, rejected experiments, retained optimizations, and final
realistic verification. Its immediate goal is to explain and fix the
transaction-latency and CPU regression first observed after checkpoint support
was enabled on the Nocean test deployment, without weakening checkpoint
validation, consensus, cryptographic verification, or federation safety.

The optimization scope includes checkpoint processing, proof and threshold-
signature cryptography, AlephBFT scheduling and dissemination, consensus
batching, private federation transport, storage, and the wallet/server lifecycle.
Checkpoint evidence is the starting point, not an assumption that checkpoints
are the only or largest remaining bottleneck.

The checkpoint project's primary success criterion remains bounded startup
work: a member restarting from a recent trusted checkpoint should not replay
the federation's entire history. Transaction processing must also remain fast
and must not perform work proportional to retained history.

Related documents:

- `docs/federated-cashu-implementation-plan.md`
- `docs/federated-cdk-checkpoint-implementation-plan.md`
- `docs/federated-cdk-checkpoint-performance-benchmark.md`
- `docs/federated-cashu-checkpoint-consolidation-plan.md`
- `docs/federated-cashu-nocean-stress-test.md`
- `docs/federated-cashu-minting-performance-benchmark.md`
- `docs/federated-cashu-materialized-state-benchmark.md`
- `docs/federated-cashu-performance-and-issues.md`

## Initial checkpoint-regression finding

The checkpoint-enabled revision is materially slower than the earlier Nocean
fakewallet baseline for the comparable sequential receive/swap workload. Two
independent `10:1` samples completed without client failures but achieved only
0.057-0.063 confirmed transactions per second, compared with 0.119 TPS in the
earlier baseline.

The retained current sample had 15.218-second median latency, 18.475-second p95
latency, and sustained high aggregate member CPU. The larger concurrency matrix
was stopped because preparing the next 20 test tokens was already taking
minutes. Waiting longer would not have added useful evidence.

This establishes a regression. Static call-path review below ranks the most
likely amplification sources and exact hot symbols; controlled A/B and sampling
profiles in Milestone 0 remain required before a cause is treated as confirmed.

## Source and binary traceability

### Current checkpoint-enabled sample

| Item | Value |
| --- | --- |
| Git source commit | `a21bf79d37a6807981504741cf65e597ee401b6e` |
| Native benchmark CLI SHA-256 | `8b667eb542b1c3d276a77a89c3ef49ef8f8d439d9aa43638ba254bc057dc28bb` |
| Deployed `cdk-mintd` SHA-256 | `301fa0d7708510a8ca42a5103626a120f14f45811176f618b3b1800131a19ee5` |
| Deployed server `cdk-cli` SHA-256 | `d6015d25cd07fe9c8f45ef2b14820b9e58a6ca78cc49ce9f9a1e8e7d4f5f1a69` |
| Benchmark script | `scripts/benchmark_nocean_federation.sh` |

The Rust binaries were built from the recorded commit. The worktree also had
shell-only deployment changes in `scripts/deploy_nocean_federation.sh` and
`scripts/nocean_federation_control.sh` to select the fakewallet configuration.
Those changes did not alter the compiled binaries but did determine the runtime
configuration described below.

### Historical comparison sample

| Item | Value |
| --- | --- |
| Git source commit | `81fc96374405183fe012bfd71e431ee5f7147823` |
| Results document | `docs/federated-cashu-nocean-stress-test.md` |
| Comparable case | `10:1` |

## Deployment configuration

- Fresh five-member federation.
- BLS signature threshold: 3.
- AlephBFT consensus threshold: 4.
- All five members co-located on the same four-logical-CPU Nocean test host.
- SQLite member databases.
- Public HTTPS entry through member 1.
- Fakewallet enabled for Bolt11, Bolt12, and on-chain mint and melt methods.
- Checkpoint interval: 20 journal operations.
- Checkpoint finalized-item interval: 20 finalized items.
- No active FROST/BDK on-chain application in the public federation config.
- All five members reported healthy and ready before the run.

Each fakewallet instance has independent process-local payment state. The test
therefore used the repository's explicit development single-observation policy,
the same policy used by the local fakewallet federation harness. This is only a
test configuration. Production payment backends retain member-quorum payment
observation.

The public NUT-4 and NUT-5 method lists were checked before benchmarking. Both
advertised Bolt11, Bolt12, and on-chain support.

## Workload

One benchmark transaction is receipt of an independently prepared 1-sat Cashu
token into an isolated wallet. Receipt performs one federated swap. The timed
interval includes CLI startup, wallet database access, public network and TLS,
federation consensus, threshold signing, response collection, and client-side
persistence.

Token minting, token preparation, wallet setup, the pre-run convergence gate,
and the 12-second settle delay are outside the measured transaction interval.

The intended matrix matched the earlier benchmark:

```text
10:1 20:5 40:10 20:20
```

Only `10:1` was allowed to finish. The remaining cases were stopped during
preparation after the regression had been reproduced twice.

## Results

### Completed checkpoint-enabled samples

| Sample | N | Parallelism | Success | Failure | Batch wall | Confirmed TPS | Journal TPS | p50 | p95 | Total member CPU | Scheduled checkpoints |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| confirmation sample | 10 | 1 | 10 | 0 | 176.694s | 0.057 | 0.057 | 17.965s | 19.552s | 477.00s | 10 |
| retained primary sample | 10 | 1 | 10 | 0 | 158.300s | 0.063 | 0.063 | 15.218s | 18.475s | 466.00s | 10 |

The first sample's private artifact directory disappeared during the following
case's token preparation. Its printed summary is retained above, but the second
sample is the primary result because its detailed artifacts survived.

### Retained sample runtime observations

| Metric | Value |
| --- | ---: |
| Journal operations before/after | 28 / 38 |
| Journal delta | 10 |
| Metrics observation window | 169s |
| Aggregate member CPU | 466.00 CPU-seconds |
| CPU per member-second | 551.479ms |
| Approximate average CPU per member | 55.1% of one logical CPU |
| Maximum ending member RSS | 66,244 KiB |
| Log lines emitted | 84 |
| Sync-state transitions | 0 |
| Scheduled checkpoints | 10 |
| Recovery checkpoints | 0 |
| Checkpoint quorums transferred | 1 |
| Checkpoints promoted | 10 |
| Catch-up batches applied | 0 |
| Checkpoint signatures verified | 1,222 |

The members did not enter catch-up and clients did not fail. The regression is
therefore present in the normal healthy transaction path, not only during
recovery.

### Comparison with the earlier Nocean baseline

| Metric | Earlier baseline | Current retained sample | Change |
| --- | ---: | ---: | ---: |
| Batch wall time | 83.709s | 158.300s | 1.89x longer |
| Confirmed TPS | 0.119 | 0.063 | 47.1% lower |
| Journal TPS | 0.119 | 0.063 | 47.1% lower |
| p50 latency | 8.388s | 15.218s | 1.81x longer |
| p95 latency | 8.639s | 18.475s | 2.14x longer |
| Failures | 0 | 0 | unchanged |

The current and historical samples used the same Nocean host shape, public
network path, fakewallet backend class, isolated-wallet receive workload, and
`10:1` case shape. They used different source revisions. The historical run did
not collect equivalent synchronous CPU and checkpoint counters, so CPU cannot
yet be compared directly.

## Additional observations

### Idle CPU and the earlier runaway

Before this benchmark, the repeated checkpoint-validation CPU bug was fixed and
redeployed. Five idle members then used approximately 1.4-1.8% CPU each, and a
host-level sample showed roughly 93-98% idle CPU. That confirms the original
constant idle 100% CPU condition was removed.

The transaction benchmark nevertheless consumed an average of about 55% of one
logical CPU per member. The idle fix therefore did not eliminate expensive work
on the accepted-operation path.

### Checkpoint cadence looks unexpectedly dense

The retained sample accepted 10 swaps and reported 10 scheduled and 10 promoted
checkpoint events despite configuration intervals of 20 journal operations and
20 finalized items. The benchmark aggregates these counters across all five
members. Ten scheduled-candidate creations and ten promotions therefore most
likely represent two logical federation checkpoint boundaries, with one local
candidate and promotion per member at each boundary, rather than one logical
checkpoint per swap. Milestone 0 must record both member-local events and
deduplicated logical checkpoint IDs so the expected cadence is established
without inference.

### Signature verification volume is suspicious

The five members collectively reported 1,222 checkpoint-signature
verifications over a 10-operation measured window. This is about 122 aggregate
verifications per accepted swap. The counter records signatures processed by
verification *calls*, not unique signatures; with consensus threshold 4 that
implies on the order of hundreds of full-quorum re-verifications. The lower
bound for two logical checkpoint boundaries formed once per member is far
smaller (on the order of `S × members × signatures_per_quorum` once each). The
excess matches re-inspecting already-durable local evidence on the healthy path.

### Capture runs on every unit finalization, not only journal ops

`operation_pipeline::apply_finalized_unit` always calls
`capture_scheduled_checkpoint_if_eligible_under_application_lock` after apply,
under the same `components.operation` lock used for journal apply and BLS share
signing. Empty and payload-bearing units both enter this path. Capture therefore
scales with finalized-unit rate (historically several items per accepted op), not
only with accepted journal operations. CPU spends there directly serialize
concurrent swap apply.

### Token preparation is also affected

Preparing independent tokens is intentionally outside the timed receive
interval, but preparation for the next 20-token case took minutes. Preparation
can perform real swaps when the funding wallet lacks exact denominations, so
this is additional evidence that the slowdown affects ordinary federated swap
work rather than only the measurement harness.

## Static call-path ranking (2026-07-17 review)

Code review of the healthy finalize/apply path. Ordered by likely pure-CPU
contribution before Milestone 0 measurement. Not yet A/B-confirmed.

| Rank | Path | Hot symbols | Why it amplifies |
| ---: | --- | --- | --- |
| 1 | Every capture attempt re-verifies the latest stored scheduled quorum | `checkpoint_manager::capture_scheduled_checkpoint_if_eligible_under_application_lock` → `verify_checkpoint_quorum_with_metrics` → `FederationCheckpointQuorum::verify` → `FederationConfig::validate` | Full `validate()` re-runs threshold BLS aggregate-key interpolation for every keyset×denomination and, when present, full setup-transcript lineage validation. Already rejected as too expensive on the wallet runtime path (`validate_wallet_runtime_metadata`). No verified-quorum cache. |
| 2 | Same capture attempt may re-serialize and re-hash the checkpoint payload per signature | `FederationCheckpointSignature::verify` / `FederationCheckpoint::id` | Canonical JSON + SHA-256 for each signature inside each quorum re-verify. |
| 3 | Capture loads the full finalized suffix and folds canonical bytes **before** eligibility short-circuit | `get_federation_aleph_bft_finalized_items_range` + `federation_checkpoint_batch_canonical_bytes` then `is_scheduled_eligible` | O(items since last scheduled) work even when the unit is not a candidate. |
| 4 | Eligible scheduled capture builds a full materialized-state digest | `materialized_state_digest_with_metrics` → `Mint::federation_materialized_state_snapshot` | O(retained materialized keys); holds the application lock while snapshotting and signing. |
| 5 | Quorum formation / promote / publish re-verify the same evidence | `persist_checkpoint_signature`, `persist_checkpoint_quorum`, `local_checkpoint_signature`, peer `publish_checkpoint_quorum` / fetch | Additional full-quorum and trusted-current re-verifies beyond one-time trust-boundary work. |
| 6 | Member swap/mint BLS share production | `apply_federation_swap` → `MemberConfigPrivate::sign_blinded_messages` (sign + per-share self-verify pairing) | Dominant *necessary* mint-side crypto once checkpoint amplification is removed; historically ~0.4–0.9s class for signing ops. |
| 7 | Admit + apply input proof verify; wallet share/aggregate/final verify | federation admit and apply validators; wallet batch verify/aggregate | Real but small for 1-sat tokens; grows with inputs/outputs. |
| 8 | AlephBFT finality wait | active round schedule ~500 ms; idle tail 2 s | Dominates sequential wall time, not cumulative CPU. Local parallel once reached ~25 committed swaps/s. |

Expected Milestone 0 discrimination: a large checkpoint-interval A/B (no new
candidates, validation still enabled) collapses ranks 3–5 only; residual per-
finalize cost of ranks 1–2 remains if capture still re-verifies the latest
quorum. Profilers and full-config-validation counters should show rank 1 first.

## Optimization hypotheses

These are investigation leads, not conclusions. Milestone numbers map to the
table above.

1. **M1.** Checkpoint quorum verification on the unit-finalization capture path
   repeatedly performs full public-config validation, including threshold BLS
   aggregate-key interpolation and setup-transcript lineage checks when the
   transcript is present, even though `FederationRuntimeConfig` already
   validated the immutable config at startup. Local candidate inspection can
   also call `validate_against_config` on every capture.
2. **M2.** Unchanged checkpoint signatures and quorum evidence are verified
   repeatedly after their trust-boundary verification, instead of carrying the
   existing verified-quorum authority through persistence and promotion. Capture
   itself re-loads and fully re-verifies the latest *already durable* scheduled
   quorum on every finalized unit.
3. **M3.** Scheduled-checkpoint capture reloads and canonicalizes the full
   finalized suffix after every finalized unit before proving that an interval
   was crossed, creating quadratic work within a checkpoint interval and scaling
   with unit finalization rate rather than candidate count.
4. **Capture + apply lock.** Capture, full-state digest construction, and BLS
   share signing share `components.operation`. Expensive checkpoint work on the
   finalize path therefore serializes ordinary transaction apply and can dominate
   p50 even when gateway and wallet work are healthy.
5. **When a schedule fires.** Materialized-state full-row snapshot and chained
   digest on candidate creation is O(retained state) and multiplies across all
   five co-located members. The bulk quote projection work was already reduced
   (see `federated-cashu-materialized-state-benchmark.md`); the remaining cost is
   still full recomputation at candidate time.
6. Swap and melt input proofs are verified more than once on the same request
   path before consensus and are verified again during authoritative apply on
   every member.
7. Scalar input-proof verification and per-output self-verification of newly
   produced member signature shares become material at larger input/output
   counts.
8. SQLite, AlephBFT cadence, wallet CLI/setup, or another current-revision path
   is responsible for more wall latency or CPU than the checkpoint and BLS paths
   above after amplification is removed. Controlled A/B measurements must remain
   able to prove this.
9. Sparse AlephBFT payloads, serial dissemination, retry amplification, or an
   unsuitable healthy-path delay can cap throughput after compute amplification
   is removed. These constants are not presumed defective: each is eligible for
   tuning only after queue, batch, timing, and traffic measurements identify it
   as a material limit.

## Implementation contract

This performance work inherits all safety, cut-through, testing, and
observability requirements from `docs/federated-cashu-implementation-plan.md`.
The following rules are mandatory for every milestone.

### Evidence before changes

- Identify each symptom and its exact call path, invocation count, and measured
  cost before changing production behavior. A plausible code review finding is
  not sufficient on its own.
- Change one independently measurable cause at a time. Establish a before/after
  pair from the same source base, release profile, federation topology, database
  backend, checkpoint policy, workload, and host conditions.
- Temporary instrumentation is allowed for timing, counts, row volume, and stack
  attribution. It must not change protocol scheduling or validation outcomes,
  expose secrets or payloads, or create unbounded labels. Remove it before the
  milestone commit unless the resulting metric is bounded and useful for normal
  operations.
- Performance conclusions require release binaries. Debug runs remain useful for
  correctness and diagnostics but must not be compared with release throughput.
- Do not call a cause confirmed until a direct counter or timing identifies the
  work and a controlled experiment that removes or bypasses only that work moves
  CPU or latency in the predicted direction.

### Keep-or-revert rule

- Run at least three equivalent samples and compare medians. Record every sample,
  not only the best result.
- A candidate is meaningful when it reduces its primary end-to-end CPU or latency
  metric by at least 15% without regressing another primary metric by more than
  5%. An asymptotic fix may instead qualify by proving a lower growth class over
  at least three input sizes, provided the realistic end-to-end case does not
  regress by more than 5%.
- The direct work counter must also fall by the amount predicted by the change.
  End-to-end movement without the expected counter movement is not attribution.
- If a candidate has no meaningful repeatable impact, revert its production-code
  changes. Record the rejected candidate and evidence in this document; do not
  keep speculative cleanup under a performance justification.
- Never obtain a faster result by weakening checkpoint, quorum, config, proof,
  signature-share, payment, finality, replay, readiness, or convergence checks;
  increasing timeouts; disabling checkpoints in production; or adding a
  benchmark-only fast path.

### Simplicity and cut-through

- Keep the implementation in the current owning layers and choose the smallest
  fix that removes the measured work. Reuse existing validated-config APIs,
  cached digests, verified-quorum types, batch primitives, and runtime state
  before adding a new abstraction.
- Do not add a general cache, wrapper, parallel implementation, compatibility
  adapter, or migration unless the measured cause cannot be removed more simply.
- Federation formats are undeployed. Change federation schemas, members, clients,
  tests, generated fixtures, and documentation together. Regenerate experimental
  databases and configs; do not retain old federation behavior or compatibility
  code.
- Preserve standalone CDK behavior and public Cashu protocol compatibility.

### Milestone and commit discipline

- A milestone is complete only when its success criteria pass, this document
  contains the exact commands and results, and the coherent code, tests, and
  documentation are committed together with a conventional commit.
- Update the milestone table and append the result log before creating that
  commit. Record the resulting commit hash in the following milestone's first
  documentation update; do not amend historical benchmark results.
- Rejected milestones still end with production code reverted and a documentation
  commit recording why the candidate was not kept.
- Do not combine milestones to hide which change produced an improvement. A
  later milestone starts from the committed result of the previous one.

### Newly discovered performance and stability issues

- If work on any milestone exposes another performance or stability symptom,
  add it to the milestone table and give it a separately numbered investigation
  milestone before attempting a production fix. Do not bury it in a result log,
  fold it into an unrelated fix, or defer it to the final release gate.
- Record the observed symptom and impact separately from the suspected cause.
  The new milestone's first task is to reproduce and identify the cause without
  doubt using bounded instrumentation, profiles, counters, or controlled fault
  injection. Code inspection or temporal correlation alone is not confirmation.
- Every added milestone must define a focused goal, existing evidence,
  preconditions, explicit todos, and measurable success criteria. It inherits
  this entire implementation contract, required progress record, keep-or-revert
  threshold, simplicity rule, safety invariants, realistic local five-member
  fakewallet workloads, adverse-member coverage where relevant, and milestone
  commit discipline.
- Attempt the smallest fix only after attribution. Verify it with an equivalent
  before/after release comparison and the direct work or failure counter that
  established the cause. Revert production changes that do not produce a
  meaningful repeatable improvement or stability correction.
- Insert discovery milestones before the sustained local and Nocean release
  gate, renumber that gate and all affected references, and keep the release gate
  last. A critical safety or liveness issue may be prioritized immediately, but
  it still receives its own milestone, evidence, verification, documentation,
  and commit.
- If investigation disproves the suspected cause, cannot reproduce the symptom
  under the recorded conditions, or finds no material safe improvement, mark the
  milestone `Rejected`, leave production code unchanged, and commit the evidence
  so the same speculation is not repeated later.

## Measurement definitions

- `scheduled_checkpoints_created` and `checkpoints_promoted` are member-local
  counters. The benchmark must report both their federation-wide sum and the
  number of distinct logical checkpoint IDs.
- `signatures_verified` counts signatures processed by verification calls, not
  unique signatures. Report unique `(checkpoint_id, signer)` pairs separately
  while diagnosing duplicate work.
- Report aggregate member CPU-seconds, CPU-seconds per successful transaction,
  normalized CPU per member-second, confirmed TPS, committed journal TPS, p50,
  p95, p99, failures, finalized units/items, checkpoint candidates, distinct
  checkpoint quorums, rows/bytes scanned, and cryptographic verification calls.
- For consensus experiments, also report payload-bearing and empty units, items
  and canonical bytes per payload-bearing unit, proposal and outbound queue wait,
  private REST messages/bytes, retry deliveries, and each value normalized per
  committed operation.
- Separate wallet preparation and token creation from timed receipt, but measure
  them as their own workload because preparation can itself perform swaps.
- Report cold CLI lifecycle, warm persistent-wallet lifecycle, and server-only
  operation latency separately. Never use one as an unlabeled proxy for another.
- Every distributed run must finish with all members ready and converged on the
  exact journal, application, finalized-item, and materialized-state digests.

## Required milestone progress record

Before changing a milestone to `Complete` or `Rejected`, add a dated progress
record under that milestone containing:

1. the exact symptom and the evidence that made it confirmed;
2. source commit, binary hashes, configuration, workload, and host description;
3. instrumentation or profiler attribution, including invocation and volume
   counts;
4. every baseline and candidate sample in execution order;
5. focused correctness, adversarial, restart/replay, standalone, and distributed
   verification commands with results;
6. the before/after median, variance, direct work-counter movement, and any
   regressions;
7. the explicit `Kept` or `Rejected` decision and why it satisfies the contract;
8. temporary instrumentation removed or the bounded production metrics retained;
9. the milestone commit subject and, once available, commit hash.

Raw bearer tokens, wallet databases, member private configs, credentials,
machine-specific paths, and other sensitive artifacts remain outside the
repository. Only payload-free aggregates and reproducible commands belong here.

## Milestone status

Allowed states are `Not started`, `In progress`, `Complete`, and `Rejected`.

| Milestone | Goal | Status | Commit |
| --- | --- | --- | --- |
| 0 | Reproduce and attribute transaction CPU without production optimization | Not started | - |
| 1 | Remove repeated full federation-config validation from confirmed checkpoint hot paths | Not started | - |
| 2 | Verify each unchanged checkpoint signature/quorum only at required trust boundaries | Not started | - |
| 3 | Make scheduled-checkpoint eligibility and suffix accounting linear | Not started | - |
| 4 | Remove duplicate same-request swap/melt proof verification | Not started | - |
| 5 | Batch federated input-proof verification where volume proves value | Not started | - |
| 6 | Batch or remove redundant member signature-share self-verification where proven safe | Not started | - |
| 7 | Re-profile the remaining consensus and transaction lifecycle | Not started | - |
| 8 | Tune only measured AlephBFT batching, scheduling, or federation transport limits | Not started | - |
| 9 | Pass sustained local fakewallet and final Nocean verification | Not started | - |

## Milestone 0: causal baseline and bounded instrumentation

### Goal

Reproduce the regression locally and establish, without relying on inference,
which call paths consume CPU during healthy swap, mint, and melt lifecycles. No
production optimization is allowed in this milestone.

### Todos

- [ ] Freeze and record source commit, binary hashes, Rust profile, host shape,
      member/signature/consensus thresholds, database backend, checkpoint policy,
      fakewallet policy, and benchmark script version.
- [ ] Correct the benchmark report to distinguish member-local checkpoint events
      from distinct logical checkpoint IDs and unique signatures.
- [ ] Add bounded temporary counters/timers around:
  - full config validation, aggregate-key interpolation, and setup-transcript
    lineage validation (distinct from cheap shape/digest binding checks);
  - checkpoint capture called from unit finalization, including empty units, and
    each early-return reason;
  - individual signature and quorum verification, keyed only by bounded operation
    class rather than checkpoint ID in production metrics; split own calls into
    capture-of-latest-stored, formation, promotion, publish, peer-fetch, and
    startup/catch-up;
  - finalized-suffix row loading and canonical-byte computation, including
    whether loading happened for an ineligible early-out;
  - materialized-state digest construction and checkpoint persistence;
  - time held on `components.operation` by capture versus journal apply / share
    signing;
  - swap/melt proof verification, member-share signing/self-verification, and
    wallet share/aggregate/final-proof verification.
- [ ] Capture sampling profiles for one member and all five aggregate processes
      during one sequential swap, `10:1`, and a concurrent receive wave.
- [ ] Run at least three release samples of `10:1`, `20:5`, `40:10`, and `100:20`
      on a fresh five-member local SQLite federation with fakewallet and `20/20`
      checkpoint intervals.
- [ ] Repeat the same matrix with a checkpoint interval beyond the test history.
      Checkpoint validation remains enabled; this is a scheduling-frequency A/B,
      not a production bypass.
- [ ] Vary journal and finalized-item intervals independently to identify which
      progress dimension triggers candidate creation and suffix work.
- [ ] Derive the expected signature/quorum-verification cardinality for one new
      logical checkpoint and compare it with per-member observations.
- [ ] Measure mint and melt separately from swap, including accepted journal
      operations per logical lifecycle.
- [ ] Remove temporary instrumentation that is not retained as bounded production
      observability.

### Success criteria

- The local release run reproduces the CPU/latency symptom or the environmental
  reason it cannot is proven and documented.
- At least one suspected stage has matching stack attribution, invocation counts,
  measured duration, and a controlled A/B response. No production fix begins
  without this evidence.
- Run-to-run median CPU-seconds per transaction is stable enough to evaluate a
  15% change, or additional samples establish a confidence interval that is.
- Expected and observed logical checkpoint, candidate, quorum, and signature
  cardinalities are documented without conflating per-member sums.
- Every run has exact success/journal deltas, converged digests, zero unexpected
  alerts, and bounded post-run idle CPU.
- The milestone results and retained harness/metric changes are committed as
  `test: establish federation transaction performance baseline`.

## Milestone 1: stop repeated full config validation

### Goal

Remove full public-config validation and threshold BLS aggregate-key
interpolation from checkpoint paths that already hold a startup-validated
`FederationRuntimeConfig`, while retaining full validation at every external
config ingress, startup, DKG activation, and untrusted import boundary.

### Preconditions

- Milestone 0 directly confirms repeated full config validation is a material
  transaction-path cost.

### Todos

- [ ] Enumerate every `FederationConfig::validate`, `config_digest`, checkpoint
      `validate_against_config`, and quorum `verify` call reached during the
      measured healthy lifecycle. Seed list from static review:
      capture latest-scheduled quorum, local candidate inspection, signature
      persist, quorum persist/promote, publish, peer fetch, startup/catch-up.
- [ ] Classify each call as config ingress/startup, new external checkpoint
      evidence, locally constructed evidence, already-verified durable evidence,
      or cheap shape/binding validation.
- [ ] Prefer `validate_against_validated_config` / runtime-cached digests and a
      narrow quorum verify that does **not** re-enter full `config.validate()`
      (keyset Lagrange + setup transcript) when the mint already holds a
      startup-validated `FederationRuntimeConfig`. Add only the smallest API if
      none exists; do not invent a general config cache.
- [ ] Ensure locally built checkpoints do not validate the same immutable config
      twice during one construction.
- [ ] Add focused tests proving malformed configs still fail at construction,
      tampered checkpoint bindings still fail, unknown roster watermarks fail,
      and invalid signatures/quorums remain rejected.
- [ ] Run focused common/Axum tests, standalone non-federation tests affected by
      shared code, and the exact Milestone 0 release matrix three times.

### Success criteria

- Healthy post-startup transaction processing performs zero full public-config
  validations unless a genuinely new config enters through a trust boundary.
- Checkpoint shape, federation ID, config digest, roster, lineage, and signatures
  are still verified at their required boundaries, with adversarial tests.
- Median aggregate member CPU-seconds per transaction improves by at least 15%,
  direct full-validation counts fall exactly as predicted, and no primary
  latency/throughput metric regresses more than 5%.
- Exact-checkpoint and checkpoint-tail restart remain bounded and fail closed.
- If the impact gate fails, production changes are reverted and the milestone is
  marked `Rejected` with results.
- A kept result is committed as
  `perf: avoid repeated checkpoint config validation`.

## Milestone 2: bounded checkpoint signature and quorum verification

### Goal

Ensure each unique signature and quorum is cryptographically verified when it
crosses a trust boundary, without re-verifying unchanged local evidence on every
finalized unit, sync poll, persistence step, or promotion check.

### Preconditions

- Milestone 0 or the post-Milestone-1 profile proves repeated signature/quorum
  verification remains material after config interpolation is removed.

### Todos

- [ ] Trace signature creation, peer receipt, durable insertion, quorum assembly,
      quorum transfer, trusted promotion, startup reload, and catch-up use.
- [ ] Write the required verification cardinality as a function of unique new
      signatures, unique quorums, members, and logical checkpoint boundaries.
- [ ] Identify duplicate verification of the same evidence in
      `persist_checkpoint_signature`, quorum construction/persistence, current
      trusted-quorum comparison, capture, and peer sync.
- [ ] Carry the existing `VerifiedFederationCheckpointQuorum` authority through
      adjacent persistence/promotion calls instead of converting it immediately
      back to raw data and verifying it again.
- [ ] Prefer verified-at-insertion/startup durable invariants over a new cache.
      Add a cache only if measurement proves the same verified object must cross
      non-adjacent paths and no simpler ownership change works.
- [ ] Add corruption, duplicate signer, minority quorum, wrong checkpoint ID,
      wrong roster, restart, and fetched-quorum adversarial tests.
- [ ] Repeat unchanged idle sync rounds and active `10:1`, `40:10`, and `100:20`
      cases with exact verification counters.

### Success criteria

- Verification counts are a documented bounded function of newly observed
  evidence and do not grow during unchanged idle polls or repeated inspection of
  one locally trusted checkpoint.
- Two logical checkpoint boundaries cannot produce unexplained hundreds of
  repeated signature-verification units.
- Median CPU-seconds per transaction improves by at least 15% after Milestone 1,
  with the expected verification-count reduction and no primary regression over
  5%; otherwise the optimization is reverted and recorded as `Rejected`.
- Invalid or corrupted evidence still fails at external receipt and startup, and
  only verified authority can reach trusted promotion or catch-up.
- A kept result is committed as
  `perf: avoid repeated checkpoint quorum verification`.

## Milestone 3: linear scheduled-checkpoint accounting

### Goal

Remove repeated finalized-suffix loading and canonicalization from every
finalized-unit callback. Work between checkpoint boundaries must be linear in
new finalized data, not quadratic in the configured interval.

### Preconditions

- Milestone 0 records rows and canonical bytes processed at multiple checkpoint
  intervals and confirms the current growth curve.

### Todos

- [ ] Benchmark checkpoint intervals of 20, 200, 2,000, and a synthetic 10,000
      finalized items while recording capture calls, suffix rows loaded, bytes
      canonicalized, SQLite time, and total CPU.
- [ ] Perform cheap journal/finalized progress checks before loading a batch when
      they can prove ineligibility.
- [ ] Account for the canonical-byte trigger incrementally as finalized items are
      durably persisted, or use an equally simple deterministic scalar frontier.
      Do not silently remove or weaken the byte-based checkpoint trigger.
- [ ] Load and validate the exact suffix only when checkpoint construction,
      transfer, or catch-up actually requires it.
- [ ] If durable frontier/schema state changes, update all federation backends,
      fixtures, startup/replay logic, and tests together and regenerate
      experimental databases without compatibility code.
- [ ] Add boundary tests for journal-only, finalized-only, byte-only, exact-limit,
      one-over-limit, empty-unit, restart, and crash-before/after-persistence
      cases.
- [ ] Repeat the interval growth curve and realistic local receive matrix.

### Success criteria

- Total rows/bytes processed across an interval grows linearly and no finalized
  unit reloads the complete accumulated suffix merely to prove ineligibility.
- All three scheduling triggers select the same first eligible complete-unit
  boundary as before.
- The 10,000-item synthetic case remains bounded in CPU, memory, and database
  reads, and exact-checkpoint startup remains independent of retained history.
- The asymptotic improvement is repeatable at three or more sizes and the
  realistic end-to-end matrix does not regress any primary metric by more than
  5%; otherwise the candidate is reverted.
- A kept result is committed as
  `perf: make checkpoint eligibility accounting linear`.

## Milestone 4: eliminate duplicate same-request proof verification

### Goal

Verify untrusted swap/melt proofs once per public request admission on each
contacted member and once during authoritative accepted apply on each member,
without repeating the same pure admission verification in nested local calls.

### Preconditions

- Milestone 0 records proof-verification invocations by operation, member, and
  lifecycle layer and confirms duplicate same-request work.

### Todos

- [ ] Trace operation builders, public submission, direct consensus submission,
      peer submission, duplicate request handling, and accepted apply for swap and
      melt.
- [ ] Define which entry points accept untrusted envelopes and must validate them
      independently.
- [ ] Remove the nested local validation duplication with the smallest ownership
      change. Do not add an unchecked public entry point or allow peer traffic to
      bypass admission.
- [ ] Retain authoritative proof, spending-condition, state, balance, and
      double-spend validation during deterministic apply.
- [ ] Add valid, invalid proof, invalid witness, duplicate request, direct peer,
      retry, conflicting operation, and replay tests.
- [ ] Benchmark one-input and multi-input swaps and melts with wallet fanout to
      three, four, and five responding members.

### Success criteria

- One public member request does not invoke the same pure proof verifier more than
  once before consensus, and accepted apply still verifies once per applying
  member.
- Invalid requests fail before consensus and malicious or corrupted accepted data
  still fails closed during apply/replay.
- Median CPU-seconds per transaction improves by at least 15% for the confirmed
  multi-input workload, with no one-input or latency regression over 5%; otherwise
  the change is reverted and recorded as `Rejected`.
- A kept result is committed as
  `perf: remove duplicate federation proof verification`.

## Milestone 5: batch federated input-proof verification

### Goal

Replace scalar pairing verification of valid multi-input swap/melt proofs with
the existing deterministic BLS batch primitive when realistic proof counts show
material value.

### Preconditions

- Milestone 4 leaves scalar verification as a measured material cost at realistic
  input counts.

### Todos

- [ ] Add scalar-versus-batch microbenchmarks for 1, 4, 14, and 64 proofs using
      mixed denomination/keyset fixtures and release binaries.
- [ ] Preserve all cheap metadata, keyset, DLEQ, content-bound, uniqueness, and
      spending-condition checks before cryptographic batching.
- [ ] Use deterministic domain-separated batch weights through existing common
      primitives. Fall back to scalar verification only if error attribution is a
      required existing behavior.
- [ ] Test corrupted first/middle/last proofs, mixed keys, malformed points,
      duplicate proofs, maximum valid input count, and one-over-limit rejection.
- [ ] Run multi-input swap and melt process benchmarks through the five-member
      local fakewallet federation.

### Success criteria

- Batch and scalar verification accept and reject the same fixtures.
- Median cryptographic verification time improves by at least 20% at 14 and 64
  proofs, one- and four-proof performance does not regress more than 5%, and the
  process benchmark shows at least a 15% CPU improvement at the retained realistic
  shape.
- If realistic wallets rarely reach a profitable batch size or end-to-end impact
  is not meaningful, production batching is not kept and the milestone is marked
  `Rejected`.
- A kept result is committed as
  `perf: batch federation input proof verification`.

## Milestone 6: member signature-share self-verification

### Goal

Determine whether pairing-verifying every locally generated blinded-signature
share is material, then retain the simplest safe batch or invariant-based change
only if it produces meaningful end-to-end improvement.

### Preconditions

- Milestone 0 or the post-Milestone-5 profile identifies local share
  self-verification as a material output-count-dependent cost.

### Todos

- [ ] Measure signing and self-verification separately for 1, 4, 14, and 64
      outputs across all five members.
- [ ] Document the invariant supplied by startup validation between each local
      secret share and configured public share.
- [ ] Evaluate independently: batched self-verification; verification once per
      output batch; or removal only if the already-validated secret/public binding
      makes the check redundant and fault tests prove no safety loss.
- [ ] Prefer batching over removing a defense when both meet the performance
      target with similar simplicity.
- [ ] Add wrong local secret share, wrong public share, corrupted generated share,
      mixed keyset, restart, and rotation tests.
- [ ] Run mint, swap, melt-change, and restore output-volume process benchmarks.

### Success criteria

- The retained behavior detects every fault that the current scalar checks detect
  at a trust boundary where that fault remains possible.
- Median signing/self-verification time improves by at least 20% at 14 and 64
  outputs, small batches do not regress more than 5%, and the realistic process
  case improves aggregate CPU by at least 15%.
- A candidate without meaningful end-to-end impact is reverted even if its
  microbenchmark improves.
- A kept result is committed as
  `perf: reduce federation share self-verification cost`.

## Milestone 7: re-profile remaining lifecycle and consensus cost

### Goal

After removing confirmed compute amplification, establish whether AlephBFT
cadence, empty units, private REST, wallet fanout, SQLite, or serialized lifecycle
rounds are the remaining throughput or latency limit. This milestone diagnoses
and ranks the remaining limits; AlephBFT and federation-transport tuning belongs
to Milestone 8 and must not start without this evidence.

### Todos

- [ ] Repeat the Milestone 0 full profiles with all retained fixes.
- [ ] Attribute wall and CPU time across wallet startup, public HTTP/TLS, request
      admission, consensus proposal/finality, apply, checkpoint work, share
      collection, wallet verification, and persistence.
- [ ] Record units and finalized items per accepted operation, empty-unit ratio,
      broadcast queue depth, private REST rate/bytes, SQLite lock time, runtime
      task latency, and consensus batch size.
- [ ] Measure swap as one consensus operation and mint/melt as complete logical
      lifecycles with their accepted-operation counts reported explicitly.
- [ ] For each material AlephBFT, batching, or federation-transport stage, name
      the exact controlling constant or code boundary and hand the measured
      baseline to Milestone 8. Do not change those controls in this milestone.
- [ ] If a remaining database or wallet stage is material, run one controlled
      candidate under the same keep-or-revert rule. Do not combine database and
      wallet changes in one experiment.
- [ ] Record a no-change result when remaining costs are inherent, already below
      budget, or not improved by a simple safe candidate.

### Success criteria

- At least 90% of measured active CPU and end-to-end wall time is attributed to
  named stages, with waiting distinguished from compute.
- Every retained database or wallet candidate meets the 15% impact gate
  independently. Otherwise all production changes from this milestone are
  reverted.
- The result is committed as `test: attribute remaining federation lifecycle cost`,
  `perf: reduce measured federation lifecycle cost` when a non-consensus fix is
  retained, or a no-change/rejected result as
  `docs: record federation lifecycle performance limits`.

## Milestone 8: measured AlephBFT and federation throughput tuning

### Goal

Experiment with the small set of AlephBFT scheduling, proposal batching,
network-pump, and private federation transport controls that Milestone 7 proves
are limiting healthy-path throughput. Retain only the simplest individually
measured change. This milestone must not weaken finality or trade bounded CPU and
network work for a superficially lower single-request latency.

### Current controls and experiment eligibility

The current values are intentional baselines, not defaults to replace wholesale:

- AlephBFT 0.45.4 recommends a 20 ms task-queue tick, a 5-second round-zero unit
  delay, and a 500 ms unit delay for subsequent production rounds. CDK uses that
  round-aware schedule while work is active and changes only inactive tail rounds
  after round 64 to two seconds. The former fixed 100 ms active cadence created
  roughly five times the supported steady unit rate, amplified empty units and
  CPU, and was removed. It must not be reintroduced.
- The data provider drains immediately, in FIFO order, up to 64 items and up to
  four maximum-sized consensus items' worth of canonical body bytes per unit. A
  short coalescing window is eligible only when concurrent load leaves most units
  sparse. Larger item or byte caps are eligible only when the corresponding cap
  is frequently hit while proposal backlog remains.
- The network task drains one outbound message per pump call, but the outer task
  is event-driven and can call it again immediately; this value is not a
  five-second per-message throttle. Deliveries to a message's remote peers are
  already concurrent. After enough remote members accept a message, the pump
  waits up to 25 ms for surplus peers and queues unfinished retryable deliveries.
  Pump batching or bounded cross-message concurrency is eligible only when
  outbound queue age proves serial pump service is material. The 25 ms grace is
  eligible when measured grace waits or duplicate retries are material.
- Private REST currently permits 128 inbound and 64 outbound requests per member.
  These limits are eligible only when semaphore wait is measured. The five-second
  peer-sync interval, ten-second peer-request timeout, catch-up page size,
  reconnect backoff, payment-observer interval, and pending/waiter/cache capacities
  are not normal healthy-path throughput knobs. Do not change them unless the
  matching recovery path, semaphore, or bounded queue is demonstrably saturated.
- AlephBFT coordinate, parent, newest-unit request, and 15-20 second rebroadcast
  delays are repair controls. They are eligible only when missing-unit/request
  telemetry attributes a healthy-path stall to them. Session round caps and
  rotation limits are lifecycle controls, not transaction-rate controls.

### Preconditions

- Milestone 7 attributes a material share of CPU or wall time to sparse proposal
  batches, AlephBFT scheduling, outbound dissemination/retry, or private REST
  admission and identifies the corresponding control without doubt.
- A release baseline exists for normal five-member operation, one slow surplus
  member, one offline surplus member, and a 30-second idle cool-down. It includes
  unit, message, retry, queue, and batch cardinalities per committed operation.

### Todos

- [ ] Add or enable bounded instrumentation for proposal queue depth and item
      arrival spacing; items and canonical bytes per unit; item-count and byte-cap
      hits; empty and payload-bearing units; proposal-to-finality duration and
      Aleph round; outbound queue age/depth; per-message quorum-completion time;
      time spent in the 25 ms surplus-peer grace; per-peer delivery latency;
      retry enqueue/drain/drop counts; private REST semaphore wait; and messages,
      bytes, and finalized units per committed operation.
- [ ] From those measurements, mark every control above `Eligible` or `Excluded`
      with the exact symptom and threshold. An excluded control is not swept just
      because it is configurable.
- [ ] If units are sparse under sustained concurrent load, test bounded proposal
      coalescing windows of 0, 1, 5, 10, and 25 ms. Measure the gain in items per
      unit against added low-load p95 latency. Do not wait when the queue has
      already reached an item or byte cap. Test a larger item or body-byte cap
      separately, and only after cap-hit plus backlog evidence.
- [ ] If dissemination is material, sweep the surplus-peer completion grace at
      0, 5, 25, and 50 ms under normal and deliberately slow-surplus-peer shapes.
      Count added retry deliveries and total private REST bytes. Test pump drain
      widths or bounded cross-message concurrency of 1, 2, 4, and 8 only if
      outbound queue age remains material; preserve per-peer ordering and bound
      all in-flight work.
- [ ] If consensus scheduling remains material after batching and dissemination
      experiments, keep the upstream round-aware schedule as the baseline and
      test one field at a time. For steady active rounds, bracket the current
      500 ms value with conservative 375 ms and 250 ms candidates while retaining
      the round-zero behavior, idle-tail slowdown, and bounded round semantics.
      Test the 20 ms Aleph task tick at 10, 20, and 40 ms only when task-queue wait
      is visible. Treat round zero separately as cold-session latency; changing
      its five-second delay cannot establish a sustained-throughput win.
- [ ] Tune coordinate/parent/newest request or rebroadcast delays only for a
      measured missing-unit repair stall. Tune private REST inbound/outbound
      concurrency only for measured permit wait. Do not increase queue capacities
      or timeouts to hide backpressure.
- [ ] Change one control per experiment, restore the baseline between candidates,
      and run at least three interleaved baseline/candidate release samples. Only
      after independent candidates pass may their simplest compatible combination
      be tested as a new candidate.
- [ ] For every eligible candidate, run a warm five-member local SQLite
      federation with fakewallet through 100 sequential swaps, at least 1,000
      swaps at parallelism 20, one saturation step above 20, 100 complete mint
      lifecycles, and 100 complete melt lifecycles. Repeat the retained candidate
      with one slow surplus member, one offline surplus member, restart/replay,
      and a 30-second idle cool-down.
- [ ] Remove experiment-only configuration and instrumentation, keep only bounded
      generally useful metrics, and record every rejected and retained candidate
      in this document before the milestone commit.

### Success criteria

- Every tested control has direct evidence connecting it to proposal wait,
  finality wait, CPU, network work, retry work, or admission wait. Candidate
  movement matches the predicted direct counter; coincidental end-to-end movement
  is not enough.
- Each retained change independently satisfies the global 15% CPU, throughput,
  or latency impact gate without regressing another primary metric by more than
  5%. Proposal coalescing also keeps low-load p95 within the same 5% budget.
- Messages, bytes, empty units, finalized units, and retries per committed
  operation remain bounded and explained; the retry queue drains to zero without
  drops; private routes remain responsive; and idle CPU returns below 2% of one
  logical CPU per member.
- All normal, slow-member, offline-member, restart, replay, finality, ordering,
  deduplication, and convergence checks pass. No result relies on a higher queue
  limit, longer timeout, disabled validation, or benchmark-only fast path.
- A winning configuration is demonstrated at realistic volume and advances the
  project toward the 10 TPS release target. If no candidate passes, all production
  changes are reverted and the milestone is `Rejected`; that is preferable to
  retaining an unproven tuning knob.
- A kept result is committed as
  `perf: tune measured federation consensus throughput`; an all-rejected sweep is
  committed as `docs: record rejected federation throughput tuning`.

## Milestone 9: sustained local and Nocean release gate

### Goal

Prove the complete retained implementation under realistic volume using the
real five-process local federation with fakewallet, then repeat the controlled
Nocean workload. This is the ultimate verification gate; microbenchmarks and
synthetic tests cannot complete it.

### Todos

- [ ] Build fresh release `cdk-mintd` and federation-enabled `cdk-cli` binaries
      from the exact candidate commit and record their hashes.
- [ ] Start a fresh five-member SQLite federation through the normal local
      fakewallet harness with BLS threshold 3 and consensus threshold 4.
- [ ] Run three warm release repetitions of:
  - 100 sequential isolated-wallet receives;
  - at least 1,000 receives at parallelism 20 and a saturation step above 20;
  - at least 100 complete mint lifecycles and 100 complete melt lifecycles,
    including forced-change melts;
  - multi-input and multi-output shapes retained by Milestones 5 and 6;
  - one surplus member slow, one surplus member offline, and one member restarted
    during sustained load.
- [ ] Run a low checkpoint-interval campaign crossing at least ten logical
      scheduled boundaries and a production-policy campaign long enough to cross
      at least two logical scheduled boundaries. Choose operation count from the
      measured finalized-item amplification rather than assuming one item per
      transaction.
- [ ] Measure a full idle cool-down, intact restart, exact-checkpoint restart,
      checkpoint-tail restart, and bounded peer catch-up after the load.
- [ ] Require exact wallet-success/journal deltas and identical journal,
      application, finalized-item, and materialized-state digests on all members.
- [ ] Only after the local gate passes, deploy those exact binaries to a fresh
      Nocean fakewallet federation. Run `10:1` first, then `20:5`, `40:10`,
      `100:20`, and the sustained case supported by the host.
- [ ] Append every run to the result log, including failures and stopped runs.

### Success criteria

- All local operations succeed with exact committed deltas, no safety/readiness
  alerts, no unexplained checkpoint work, and no lost convergence under the
  defined slow/offline/restart cases.
- Warm local swaps sustain at least 10 confirmed and committed transactions per
  second at parallelism 20, with p95 at or below 5 seconds and aggregate member
  CPU at or below 0.40 CPU-seconds per successful swap.
- Concurrent mint and melt campaigns sustain at least 10 accepted journal
  operations per second. Logical lifecycle TPS and the number of serialized
  consensus operations per lifecycle are reported separately rather than hidden
  behind journal TPS.
- After every campaign, member CPU returns below 2% of one logical CPU per member
  during a 30-second idle window, with zero growth in work counters that should be
  event-driven.
- Checkpoint creation, promotion, restart, and catch-up remain bounded and exact
  after histories large enough to cross the required boundaries.
- The fresh Nocean `10:1` case is no worse than the historical 0.119 TPS and
  8.388-second p50 baseline, and the higher-concurrency result demonstrates the
  best achievable service rate without CPU saturation or queue amplification.
- Formatting, focused federation/common/storage tests, standalone affected tests,
  workspace check, strict Clippy, and relevant local integration suites pass.
- Final code, tests, benchmark results, and this document are committed as
  `perf: complete federation performance optimization work`.

## Release target

Before running the full Nocean matrix again:

- warm local swaps sustain at least 10 confirmed and committed transactions per
  second at parallelism 20 under the CPU and latency budgets in Milestone 9;
- all 10 sequential receives succeed;
- p50 and p95 are no worse than the earlier Nocean range without a documented
  environmental reason;
- member CPU returns near idle between operations and does not remain saturated;
- checkpoint and signature-verification counts match an explained bounded
  protocol cardinality;
- all five members remain ready and converged with identical journal,
  application, and materialized-state digests;
- no checkpoint, payment-observation, invalid-operation, or audit alerts occur;
- restart from a recent checkpoint still avoids replay proportional to total
  retained history.

## Result log

Add future measurements here rather than rewriting earlier results.

| Date | Git commit | Environment | Case | Result | Interpretation |
| --- | --- | --- | --- | --- | --- |
| 2026-07-17 | `a21bf79d37a6807981504741cf65e597ee401b6e` | Nocean, 5 members, fakewallet | `10:1` | 0.063 TPS, p50 15.218s, p95 18.475s, 55.1% average CPU/member | Regression reproduced; static review ranks repeated full `config.validate()` + quorum re-verify on every unit-finalize capture as the top CPU lead; A/B confirmation still required (M0) |
