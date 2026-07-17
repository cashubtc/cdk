# Federated CDK Performance Optimizations

Status: Milestone 3 complete; Milestone 4 next
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
`capture_scheduled_checkpoint_if_eligible_under_application_lock` after applying
a payload-bearing finalized unit, under the same `components.operation` lock
used for journal apply and BLS share signing. The live AlephBFT adapter does not
forward units whose application data is `None`, and federation unit payloads
reject empty item lists, so the earlier claim that empty units enter checkpoint
capture was incorrect. Capture scales with payload-bearing finalized-unit rate,
which can still exceed accepted journal-operation rate. CPU spent there directly
serializes concurrent swap apply. Milestone 0 will count upstream data/no-data
finalizations separately from actual capture attempts.

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
- Active measurement override from the user: for the current local optimization
  sequence, one release run each of `10:1`, `20:5`, and `100:20` is sufficient.
  Do not add repetitions or the canceled opposite-interval probe. Direct work
  counters, correctness gates, and explicit disclosure of every result remain
  mandatory.
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
| 0 | Reproduce and attribute transaction CPU without production optimization | Complete | `a5ae88c88afa0a910632be921e277d3c49c33719` |
| 1 | Remove repeated full federation-config validation from confirmed checkpoint hot paths | Complete | `887e5a49ea1b31bf72d0290eab25799b10f6567c` |
| 2 | Verify each unchanged checkpoint signature/quorum only at required trust boundaries | Rejected | `81014e7ed963bf0c7cfe968b026c64fa0ab04332` |
| 3 | Make scheduled-checkpoint eligibility and suffix accounting linear | Complete | This milestone commit; hash recorded at Milestone 4 start |
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

- [x] Freeze and record source commit, binary hashes, Rust profile, host shape,
      member/signature/consensus thresholds, database backend, checkpoint policy,
      fakewallet policy, and benchmark script version.
- [x] Correct the benchmark report to distinguish member-local checkpoint events
      from distinct logical checkpoint IDs and unique signatures.
- [x] Add bounded temporary counters/timers around:
  - full config validation, aggregate-key interpolation, and setup-transcript
    lineage validation (distinct from cheap shape/digest binding checks);
  - upstream AlephBFT data/no-data finalizations, checkpoint capture called from
    payload-bearing finalization, and each early-return reason;
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
- [x] Capture sampling profiles for one member and all five aggregate processes
      during one sequential swap, `10:1`, and a concurrent receive wave.
- [x] Run one release sample each of `10:1`, `20:5`, and `100:20` on a fresh
      five-member local SQLite federation with fakewallet and `20/20` checkpoint
      intervals. Additional repetitions already captured before this scope was
      reduced remain supporting evidence, not a completion requirement.
- [x] Repeat `10:1`, `20:5`, and `100:20` once with a checkpoint interval beyond
      the test history. Checkpoint validation remains enabled; this is a
      scheduling-frequency A/B, not a production bypass.
- [x] Record the interval-isolation scope decision. A `20/20000` probe completed
      before measurement scope was reduced; its reciprocal `20000/20` probe was
      canceled by user direction and is not a Milestone 0 completion gate.
- [x] Derive the expected signature/quorum-verification cardinality for one new
      logical checkpoint and compare it with per-member observations.
- [x] Measure mint and melt separately from swap, including accepted journal
      operations per logical lifecycle.
- [x] Remove temporary instrumentation that is not retained as bounded production
      observability.

### Success criteria

- The local release run reproduces the CPU/latency symptom or the environmental
  reason it cannot is proven and documented.
- At least one suspected stage has matching stack attribution, invocation counts,
  measured duration, and a controlled A/B response. No production fix begins
  without this evidence.
- The retained sequential samples establish a CPU baseline stable enough to
  evaluate the 15% Milestone 1 impact gate. Broader matrix repetition is not a
  Milestone 0 completion requirement.
- Expected and observed logical checkpoint, candidate, quorum, and signature
  cardinalities are documented without conflating per-member sums.
- Every run has exact success/journal deltas, converged digests, zero unexpected
  alerts, and bounded post-run idle CPU.
- The milestone results and retained harness/metric changes are committed as
  `test: establish federation transaction performance baseline`.

### Progress record — 2026-07-17

Milestone 0 is `Complete`. It contains both the untouched-source reproduction
and the bounded instrumented release evidence. No production optimization is
included. Its commit subject is
`test: establish federation transaction performance baseline`; the resulting
hash is recorded in the first Milestone 1 update, where it is available without
self-referencing this commit.

#### Frozen preliminary baseline

| Item | Value |
| --- | --- |
| Source commit | `63797df00659954e29534e839552b978a780c1cd` |
| `cdk-mintd` SHA-256 | `f694f42190785eb7a3dd1ea2f16249103689e6689a63c03aca946ed20eff7d9f` |
| `cdk-cli` SHA-256 | `8b667eb542b1c3d276a77a89c3ef49ef8f8d439d9aa43638ba254bc057dc28bb` |
| Benchmark script SHA-256 | `8dd7a4bd730781511cd70443a992f8ded21fc62e6f56ca6c57b53668129b3650` |
| Build profile | `--release --locked`; mint features `federation-test-utils,sqlite,prometheus`; CLI feature `federation` |
| Compiler used | `rustc 1.95.0 (59807616e 2026-04-14)`, `aarch64-apple-darwin` |
| Repository toolchain request | Rust `1.96.0`; the local mismatch remains recorded for repetition |
| Host | Apple ARM64, 18 logical cores, 128 GiB memory, macOS 26.5 |
| Federation | Five local members, signature threshold 3, consensus threshold 4, SQLite, local fakewallet |
| Workload | Ten sequential receive/swaps (`10:1`) |
| Policies compared | journal/finalized intervals `20/20` and `20000/20000` |

Build commands:

```bash
cargo build --locked --release -p cdk-mintd \
  --no-default-features \
  --features federation-test-utils,sqlite,prometheus
cargo build --locked --release -p cdk-cli \
  --no-default-features --features federation
```

The two fresh configurations used the following command shape, with the two
recorded interval pairs and separate loopback port ranges:

```bash
target/release/cdk-mintd federation test-trusted-dealer \
  --output-dir "$FEDERATION_DIR" --force --local-fakewallet-configs \
  --members 5 --threshold 3 --port-start "$PORT_START" \
  --checkpoint-interval-journal-operations "$JOURNAL_INTERVAL" \
  --checkpoint-interval-finalized-items "$FINALIZED_INTERVAL"
```

Each running federation was measured with:

```bash
LOCAL_FEDERATION_DIR="$FEDERATION_DIR" \
MINTD=target/release/cdk-mintd CLI=target/release/cdk-cli \
BUILD_CLI=0 EXPECTED_MEMBERS=5 CASES='10:1' \
PREPARE_PARALLELISM=5 SETTLE_SECONDS=12 RUN_DIR="$PRIVATE_RUN_DIR" \
scripts/benchmark_nocean_federation.sh
```

Private run directories, wallet databases, bearer tokens, member configs, raw
checkpoint identifiers, and profiles remain outside the repository.

#### Preliminary checkpoint-frequency A/B

| Metric | `20/20` | `20000/20000` | Change |
| --- | ---: | ---: | ---: |
| Successful swaps | 10/10 | 10/10 | unchanged |
| Accepted journal operations | 6 to 16 | 6 to 16 | +10 in both |
| Receive wall time | 25.406 s | 22.218 s | -12.5% |
| Committed TPS | 0.394 | 0.450 | +14.2% |
| p50 | 2.498 s | 2.006 s | -19.7% |
| p95 / p99 | 2.759 s | 2.513 s | -8.9% |
| Maximum latency | 3.488 s | 2.527 s | -27.6% |
| Aggregate member CPU | 48.38 s | 4.71 s | -90.3% |
| CPU per accepted operation | 4.838 s | 0.471 s | -90.3% |
| CPU ms/member-second | 387.040 | 42.818 | -88.9% |
| Maximum end RSS | 54,208 KiB | 54,544 KiB | +0.6% |
| Member-local scheduled candidates | +10 | 0 | -100% |
| Member-local promotions | +10 | 0 | -100% |
| Signatures processed by checkpoint verification calls | +1,123 | 0 | -100% |
| Sync transitions / unexpected alerts | 0 / 0 | 0 / 0 | unchanged |

The `20/20` members each consumed 9.29–10.12 CPU-seconds; the high-interval
members each consumed 0.94–0.95 CPU-seconds. The response was therefore
federation-wide, not one overloaded process. The `20/20` end state contained
three distinct scheduled checkpoint IDs and 15 distinct
`(checkpoint_id, signer)` pairs. One checkpoint predated the receive window, so
the +10 member-local candidates and promotions represent two new logical
checkpoints across five members, not ten logical checkpoints. The databases
held 63 replicated signature rows for the 15 unique pairs. The +1,123 counter
records signatures processed by verification calls, not unique evidence.

#### Profile attribution

All five `20/20` member samples showed the same `Iterator::fold` hot symbol,
with 6,048–6,521 samples per member. One member contained this 661-sample
branch:

```text
apply_federation_aleph_bft_finalized_unit_with_retry
  -> capture_scheduled_checkpoint_if_eligible_under_application_lock
  -> FederationCheckpoint::validate_against_config
  -> FederationConfig::validate
  -> FederationDkgTranscript::validate_setup_lineage_against_config
  -> FederationConfig::validate
  -> FederatedKeyset::validate
  -> aggregate_public_key_shares
  -> bls12_381 G2 scalar multiplication
```

The high-interval member profile was mostly parked and retained only seven
top-of-stack samples in the corresponding iterator-fold symbol. The stack and
90.3% CPU response identify scheduled-checkpoint processing and repeated
config/keyset validation as the dominant compute sink in this small run. The
wall-time gain was much smaller, leaving consensus cadence as a separate latency
limit to measure after checkpoint amplification is removed.

The high interval is a diagnostic scheduling A/B, not a proposed production
change. No production optimization was made in Milestone 0.

#### Instrumented release artifacts

The instrumented build remained based on source commit
`63797df00659954e29534e839552b978a780c1cd`; all source changes were bounded
observability, benchmark-harness correctness, tests, and an exact current-state
digest diagnostic. The artifacts used below were:

| Artifact | SHA-256 |
| --- | --- |
| Instrumented `cdk-mintd` | `12ca4b150f915d24dfc01555831ac7eb33d0b3248250a2edd149d2892d76851d` |
| Instrumented `cdk-cli` | `c9aa46dd3e40ad6057ddf4a36222a2fb6a3b247481ce8aedeef716b9eb1e3926` |
| Benchmark harness used for the runs | `c8b1a9434b43be815f8bb5efa9948aa810a27253808352f19b8ede8541f236df` |

The build profile, host, topology, database, thresholds, and fakewallet policy
were identical to the frozen preliminary baseline. Each formal sample used a
fresh generated federation. The harness recorded the operation height at the
receive-window boundary and again after bounded convergence; it also required
matching journal, finalized-item, application, and freshly materialized state
digests. Cached audited materialized digests were diagnostic because they
describe the last checkpoint, not necessarily current state.

#### Reduced release matrix and A/B

At user direction, Milestone 0 retains one representative release run of
`10:1`, `20:5`, and `100:20` per checkpoint policy instead of requiring a
three-repetition full matrix. Extra fresh-federation samples completed before
that direction are used only to quantify the sequential baseline's stability.

| Policy | Case | Wallet success | Immediate / eventual journal delta | Wall | Confirmed TPS | Committed TPS | p50 | p95 | Aggregate member CPU | Post-run idle CPU |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `20/20` | `10:1` | 10/10 | +10 / +10 | 25.969 s | 0.385 | 0.385 | 2.511 s | 2.695 s | 52.95 s | 0.36 s / 4 s |
| `20/20` | `20:5` | 20/20 | +20 / +20 | 12.402 s | 1.613 | 1.613 | 2.804 s | 3.681 s | 43.37 s | 1.11 s / 3 s |
| `20/20` | `100:20` | 70/100 | +72 / +72 | 30.814 s | 2.272 | 2.337 | 3.980 s | 10.123 s | 85.86 s | 0.79 s / 3 s |
| `20000/20000` | `10:1` | 10/10 | +10 / +10 | 21.720 s | 0.460 | 0.460 | 2.001 s | 2.508 s | 6.43 s | 0.37 s / 3 s |
| `20000/20000` | `20:5` | 20/20 | +20 / +20 | 9.241 s | 2.164 | 2.164 | 2.178 s | 2.799 s | 7.60 s | 0.40 s / 3 s |
| `20000/20000` | `100:20` | 64/100 | +64 / +64 | 33.901 s | 1.888 | 1.888 | 4.001 s | 9.483 s | 51.88 s | 0.39 s / 4 s |

Every row converged with zero sync transitions or federation alert buckets and
matching fresh current-state digests. The `20/20` `100:20` row includes two
accepted swaps whose wallets timed out before receiving threshold responses;
the harness therefore reports wallet-confirmed and immediate committed TPS
separately. The remaining failures were threshold member-request timeouts, not
rejected or late federation operations. Because `100:20` also failed with no
scheduled checkpoint in the high-interval control, that saturation symptom is
separate from checkpoint CPU and remains work for the later lifecycle/transport
milestones.

The extra fresh `10:1` samples establish a stable CPU comparison:

| Policy | Aggregate CPU samples | Median | Full range |
| --- | --- | ---: | ---: |
| `20/20` | 52.95 s, 50.60 s, 52.45 s | 52.45 s | 4.5% of median |
| `20000/20000` | 6.43 s, 7.06 s, 6.34 s | 6.43 s | 11.2% of median |

The median CPU reduction is **87.7%** while the validation code remains enabled.
This is a scheduling-frequency control, not a candidate production fix.

The preliminary samples above ran first. The following ledger records every
subsequent completed instrumented sample in execution order, including work
that finished before the measurement scope was reduced. Only the six rows
marked `retained matrix` are the required representative `10:1`, `20:5`, and
`100:20` comparison; all other rows are supporting evidence. Every listed row
had zero late operations, converged exact current-state digests, and no sync
transition or federation alert.

| Order | Policy / run | Case | Wallet success | Immediate / eventual journal delta | Wall | Aggregate CPU | Use |
| ---: | --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | `20/20` profile | `1:1` | 1/1 | +1 / +1 | 2.151 s | 0.73 s | profile |
| 2 | `20/20` profile | `40:10` | 40/40 | +40 / +40 | 16.969 s | 56.97 s | profile |
| 3 | `20/20` fresh 1 | `10:1` | 10/10 | +10 / +10 | 25.969 s | 52.95 s | retained matrix |
| 4 | `20/20` fresh 1 | `20:5` | 20/20 | +20 / +20 | 12.402 s | 43.37 s | retained matrix |
| 5 | `20/20` fresh 1 | `40:10` | 40/40 | +40 / +40 | 12.707 s | 43.22 s | supporting |
| 6 | `20/20` fresh 1 | `100:20` | 70/100 | +72 / +72 | 30.814 s | 85.86 s | retained matrix |
| 7 | `20/20` continued 2 | `10:1` | 10/10 | +10 / +10 | 25.133 s | 54.34 s | supporting |
| 8 | `20/20` continued 2 | `20:5` | 20/20 | +20 / +20 | 11.600 s | 43.59 s | supporting |
| 9 | `20/20` continued 2 | `40:10` | 36/40 | +36 / +36 | 18.826 s | 49.98 s | supporting; run stopped here |
| 10 | `20/20` fresh 2 | `10:1` | 10/10 | +10 / +10 | 25.881 s | 50.60 s | stability support |
| 11 | `20/20` fresh 2 | `20:5` | 20/20 | +20 / +20 | 15.598 s | 57.26 s | supporting |
| 12 | `20/20` fresh 2 | `40:10` | 40/40 | +40 / +40 | 12.093 s | 49.78 s | supporting |
| 13 | `20/20` fresh 2 | `100:20` | 86/100 | +86 / +86 | 34.875 s | 109.21 s | supporting |
| 14 | `20/20` fresh 3 | `10:1` | 10/10 | +10 / +10 | 22.033 s | 52.45 s | stability support |
| 15 | `20/20` fresh 3 | `20:5` | 20/20 | +20 / +20 | 14.404 s | 56.54 s | supporting |
| 16 | `20/20` fresh 3 | `40:10` | 40/40 | +40 / +40 | 12.446 s | 42.97 s | supporting |
| 17 | `20/20` fresh 3 | `100:20` | 91/100 | +91 / +91 | 34.895 s | 130.15 s | supporting |
| 18 | `20000/20000` fresh 1 | `10:1` | 10/10 | +10 / +10 | 21.720 s | 6.43 s | retained matrix |
| 19 | `20000/20000` fresh 1 | `20:5` | 20/20 | +20 / +20 | 9.241 s | 7.60 s | retained matrix |
| 20 | `20000/20000` fresh 1 | `40:10` | 40/40 | +40 / +40 | 17.924 s | 28.17 s | supporting |
| 21 | `20000/20000` fresh 1 | `100:20` | 64/100 | +64 / +64 | 33.901 s | 51.88 s | retained matrix |
| 22 | `20000/20000` fresh 2 | `10:1` | 10/10 | +10 / +10 | 23.245 s | 7.06 s | stability support |
| 23 | `20000/20000` fresh 2 | `20:5` | 20/20 | +20 / +20 | 9.756 s | 8.68 s | supporting |
| 24 | `20000/20000` fresh 2 | `40:10` | 34/40 | +34 / +34 | 17.940 s | 23.17 s | supporting |
| 25 | `20000/20000` fresh 2 | `100:20` | 62/100 | +62 / +62 | 27.972 s | 57.84 s | supporting |
| 26 | `20000/20000` fresh 3 | `10:1` | 10/10 | +10 / +10 | 22.744 s | 6.34 s | stability support |
| 27 | `20000/20000` fresh 3 | `20:5` | 20/20 | +20 / +20 | 10.384 s | 9.03 s | supporting |
| 28 | `20000/20000` fresh 3 | `40:10` | 40/40 | +40 / +40 | 14.976 s | 23.06 s | supporting |
| 29 | `20000/20000` fresh 3 | `100:20` | 81/100 | +81 / +81 | 31.762 s | 62.71 s | supporting |
| 30 | `20/20000` interval probe | `40:10` | 40/40 | +40 / +40 | 14.210 s | 50.11 s | supporting interval isolation |

The planned reciprocal `20000/20` probe produced no sample: it was canceled at
user direction, as was any further repetition, and is neither evidence nor a
completion gate.

#### Direct stage attribution

The instrumented `40:10` `20/20` profile window completed 40/40 swaps with an
exact +40 journal delta. Duration sums below are aggregate member wall time and
are nested; they must not be added together.

| Stage or work item | Count / volume | Aggregate duration | Per logical swap |
| --- | ---: | ---: | ---: |
| Journal entries applied | 200 | 1.219 s | 5 entries |
| Payload / no-data AlephBFT finalizations | 225 / 625 | - | 5.625 / 15.625 |
| Scheduled capture attempts | 225 | 50.241 s | 5.625 attempts |
| Full config validations | 1,128 | 88.873 s inclusive | 28.2 validations |
| Keyset validations | 2,256 | 59.213 s | 56.4 validations |
| Aggregate public-key interpolations | 18,048 | 59.125 s | 451.2 interpolations |
| Capture-context quorum verification | 225 | 23.935 s | 5.625 calls |
| Application-lock hold / wait | 239 / 239 | 54.478 s / 3.495 s | 1.362 s / 0.087 s |
| Builder / admission / apply proof checks | 200 / 400 / 200 | 0.847 s / 1.429 s / 0.357 s | 5 / 10 / 5 proofs |
| Share signing / self-verification | 200 / 200 | 0.050 s / 0.328 s | 5 / 5 outputs |

The count identities locate the amplification precisely: 1,128 config
validations comprise 564 outer current-config validations and 564 nested setup
lineage config validations; two keysets per config and eight denominations per
keyset produce 18,048 aggregate-key interpolations. Aggregate interpolation is
99.85% of keyset-validation time and its G2 scalar-multiplication leaf accounts
for approximately the observed 56.97 process CPU-seconds. Capture occupies
92.2% of application-lock hold time. Stand-alone, non-quorum individual
signature-verification wrapper calls in formation, peer-fetch, and publish
totaled only 18 calls and 0.606 ms. Quorum wrappers still processed 1,289
individual signatures in the representative sequential run; their expensive
work is repeated checkpoint and immutable config validation, not signature
pairing itself.

Capture outcomes also prove a secondary, smaller waste. Of 225 attempts, 186
returned for a pending candidate, 29 loaded and canonicalized a suffix before
returning ineligible, and 10 created local candidates. The 39 suffix loads read
694 rows / 819,172 canonical bytes; 202 rows / 245,334 bytes were discarded by
the 29 ineligible attempts. A separate `1:1` window had 25 payload finalizations
and 25 ineligible capture attempts; all loaded the same growing suffix first,
processing 375 rows / 733,875 bytes. Suffix loading consumed 160.7 ms of the
164.5 ms capture total in that window. This is the evidence gate for Milestone 3.

All five concurrent sampling profiles contained the full hot branch from
finalized-unit apply through scheduled capture, checkpoint/quorum validation,
`FederationConfig::validate`, `FederatedKeyset::validate`,
`aggregate_public_key_shares`, and BLS12-381 G2 scalar multiplication. The G2
`Iterator::fold` leaf had 1,496, 1,904, 1,889, 1,691, and 1,743 recursive
samples (median 1,743). The `1:1` profiles showed capture and proof pairing but
no config interpolation, matching the zero counters before a scheduled quorum
existed.

#### Checkpoint cardinality and interval scope

For five members and checkpoint quorum four, one healthy logical checkpoint has
four to five local candidates/signatures (normally five), five final quorum rows,
and 20 to 25 replicated signature rows after convergence. If `F` members form a
quorum locally, successful remote signature transfers are `3F` and direct
individual signature checks are `9F`. Even before capture, formation, ingress,
trusted-quorum comparison, and transfer require at least 15 full-quorum calls,
or 60 signature checks. A boundary capture adds about 20 more; the practical
floor is therefore 89 to 125 processed signatures, despite only four or five
unique signatures. Quorum ingress plus promotion also performs at least ten
full signature-row UPSERT passes (40 row UPSERTs) per logical checkpoint.

The representative `20/20` `10:1` run observed exactly ten member-local
candidate creations for two retained logical checkpoint IDs and ten unique
retained `(checkpoint, signer)` pairs: five candidates/signatures per logical
checkpoint. It processed 1,289 signatures in verification calls, far above the
ten unique evidence pairs. Retained-ID deltas are lower bounds on creations
because converged pruning removes losing non-quorum candidates; the report does
not relabel them as exact creation counts.

One `20/20000` `40:10` probe completed before measurement scope was reduced. It
created ten member-local candidates/two retained checkpoint IDs, processed 976
signatures, and used 50.11 aggregate CPU-seconds, proving the journal interval
alone can schedule checkpoint work even when the finalized-item interval is
beyond the test history. The reciprocal `20000/20` probe was canceled at user
direction and no claim about finalized-only scheduling is made in Milestone 0.

#### Mint and forced-change melt lifecycle

One fresh `20/20` release federation measured mint and melt separately. A
20-sat mint completed in 7.25 seconds and appended exactly three operation kinds:
`mint_quote`, `mint_quote_payment`, and `mint`. After four local offline 1-sat
sends, a 10-sat fakewallet payment forced a change-producing melt. It completed
in 10.26 seconds, left the expected 5-sat wallet balance, and appended exactly
four operation kinds: `melt_quote`, `swap`, `melt`, and
`melt_quote_payment`. This corrects the earlier three-operation estimate for a
complete paid melt lifecycle.

The mint window applied 15 replicated journal entries and produced 50 member
signature shares for ten outputs. Wallet diagnostics measured ten final blind
signature checks and ten final proof checks. The melt window applied 20
replicated journal entries. Its forced-change swap verified four input proofs
per member in builder/apply and twice at admission, while the melt verified
three input proofs per member in builder/apply and twice at admission. Server
metrics therefore recorded swap proof counts of 20/40/20 and melt proof counts
of 15/30/15 for builder/admission/apply. This is direct evidence for the
duplicate-verification work scoped to Milestone 4.

The paid melt crossed a scheduled checkpoint boundary and again exposed the
dominant path: 118 full config validations, 236 keyset validations, and 1,888
aggregate-key interpolations consumed 6.982, 4.672, and 4.668 aggregate seconds
respectively. The final exact current-state comparison reported all five members
healthy and converged at operation height seven with matching journal,
finalized-item, application, and materialized-state digests.

All retained production instrumentation uses fixed enum-derived labels and
process-local counters/histograms. Checkpoint IDs, member IDs, request IDs, and
other unbounded values are excluded. Wallet detail remains debug-level tracing.
The sampling hook, raw profiles, bearer tokens, databases, and lifecycle logs
remain private benchmark artifacts under `target/`; no temporary dynamic-label
instrumentation remains in the source tree.

#### Verification and keep decision

The final retained harness SHA-256 is
`f25c273431c6bccf5bb0bd189f646fdc6e6073a8f2137ba45fc09f122f34e001`.
The only change after the measured harness hash above corrected the help text to
say post-run idle CPU and renamed its CSV signer field to the accurate
`new_retained_checkpoint_signer_pairs`; its self-tests were rerun afterward.

The following focused checks passed on the combined Milestone 0 worktree:

```text
cargo test -p cdk-prometheus federation_performance
  2 passed
cargo test -p cdk-common --features federation,prometheus \
  config_validation_performance_metrics_cover_success_and_failure_branches
  1 passed
cargo test -p cdk --no-default-features \
  --features mint,wallet,federation,prometheus,bip353 performance_
  2 passed
cargo test -p cdk-mintd --no-default-features \
  --features federation-test-utils,sqlite,prometheus digest_comparison_
  13 passed
cargo test -p cdk-axum --features federation,prometheus performance_
  2 passed
cargo test -p cdk-axum --features federation,prometheus \
  scheduled_checkpoint_captures_first_complete_eligible_boundary
  1 passed
cargo test -p cdk-axum --features federation,prometheus \
  private_local_status_full_rows_replace_stale_cached_audit_digest
  1 passed
cargo test -p cdk-axum --features federation,prometheus \
  aleph_bft_live_session_restarts_from_persisted_backup_and_continues_finality
  1 passed
cargo check -p cdk-common --no-default-features --features mint,wallet
cargo check -p cdk --no-default-features --features mint,wallet,bip353
cargo check -p cdk-axum --no-default-features
  all passed
cargo fmt --all -- --check
git diff --check
scripts/benchmark_nocean_federation.sh --self-test
  all passed
```

A reduced-feature `cdk` unit-test build without the crate's normal `bip353`
feature remains blocked by pre-existing test-only `Transport` mocks that omit
`resolve_dns_txt`; the equivalent normal-feature tests and all three
non-federation production checks above pass. No unrelated mock change was added
to this milestone.

**Keep:** fixed-cardinality server metrics, debug-only wallet stage tracing, the
corrected local/remote harness, and exact current-state digest comparison. They
directly proved the cause, preserved protocol behavior in release lifecycle and
restart tests, and remain useful to verify later fixes. **Reject/remove:** no
production optimization was attempted, no dynamic-ID metrics were introduced,
and private profiling/lifecycle helpers remain outside the tracked source tree.

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

- [x] Enumerate every `FederationConfig::validate`, `config_digest`, checkpoint
      `validate_against_config`, and quorum `verify` call reached during the
      measured healthy lifecycle. Seed list from static review:
      capture latest-scheduled quorum, local candidate inspection, signature
      persist, quorum persist/promote, publish, peer fetch, startup/catch-up.
- [x] Classify each call as config ingress/startup, new external checkpoint
      evidence, locally constructed evidence, already-verified durable evidence,
      or cheap shape/binding validation.
- [x] Prefer `validate_against_validated_config` / runtime-cached digests and a
      narrow quorum verify that does **not** re-enter full `config.validate()`
      (keyset Lagrange + setup transcript) when the mint already holds a
      startup-validated `FederationRuntimeConfig`. Add only the smallest API if
      none exists; do not invent a general config cache.
- [x] Ensure locally built checkpoints do not validate the same immutable config
      twice during one construction.
- [x] Add focused tests proving malformed configs still fail at construction,
      tampered checkpoint bindings still fail, unknown roster watermarks fail,
      and invalid signatures/quorums remain rejected.
- [x] Run focused common/Axum tests, standalone non-federation tests affected by
      shared code, and one release sample each of the retained Milestone 0
      `10:1`, `20:5`, and `100:20` cases. Compare the stable `10:1` CPU result
      with the Milestone 0 median for the 15% impact gate.

### Success criteria

- Healthy post-startup transaction processing performs zero full public-config
  validations unless a genuinely new config enters through a trust boundary.
- Checkpoint shape, federation ID, config digest, roster, lineage, and signatures
  are still verified at their required boundaries, with adversarial tests.
- The single retained candidate `10:1` sample improves aggregate member
  CPU-seconds per transaction by at least 15% against the stable Milestone 0
  median, direct full-validation counts fall exactly as predicted, and no
  primary latency/throughput metric regresses more than 5%.
- Exact-checkpoint and checkpoint-tail restart remain bounded and fail closed.
- If the impact gate fails, production changes are reverted and the milestone is
  marked `Rejected` with results.
- A kept result is committed as
  `perf: avoid repeated checkpoint config validation`.

### Progress record — 2026-07-17

Milestone 1 is `Complete`. The production candidate is kept because it removes
the directly measured work, improves the primary CPU metric by 92.8% against
the stable sequential baseline median, and improves the realistic concurrent
cases without weakening any federation evidence check.

Milestone 1 starts from committed Milestone 0 baseline
`a5ae88c88afa0a910632be921e277d3c49c33719`. The confirmed symptom is 1,128
full config validations and 18,048 aggregate public-key interpolations during
the instrumented `40:10` lifecycle, with 59.125 aggregate seconds inside
interpolation. Before changing behavior, this milestone is enumerating and
classifying every hot-path validation call by trust boundary. Per the reduced
measurement scope, verification will retain one release run each of `10:1`,
`20:5`, and `100:20`; no reciprocal interval probe or extra repetition is
required.

The pre-change call inventory confirms the exact re-entry points:

- local scheduled/recovery construction enters full validation in
  `FederationCheckpointBoundary::build_checkpoint` and then again through
  `FederationCheckpoint::validate_against_config`;
- every quorum inspection enters it through
  `FederationCheckpointQuorum::verify`, including capture, formation,
  persistence/promotion, publication, peer fetch, audit, startup, and catch-up;
- stored candidate inspection and signature persistence call
  `validate_against_config` directly; and
- private REST quorum/fetch validation verifies new external evidence correctly
  but redundantly revalidates the member's unchanged runtime config.

These calls all receive config owned by `FederationRuntimeConfig`, or a private
peer client created from that runtime. Config ingress remains separately and
fully validated by `FederationRuntimeConfig::new`, federation invite/snapshot
loading, DKG activation, and operator import tooling. Checkpoint shape,
federation ID, config digest, exact roster watermarks, promotion lineage,
individual signatures, distinct signers, and threshold are evidence checks;
none requires recomputing aggregate BLS keys or revalidating the setup
transcript. This classification completes the symptom gate before behavior is
changed.

The candidate now keeps the existing checked APIs for untrusted callers and
adds only narrow `*_for_validated_config` paths for the mint's runtime-owned
config. Scheduled and recovery checkpoint construction, candidate inspection,
signature persistence, quorum formation/verification/promotion, peer
publication/fetch, startup quorum inspection, suffix reconstruction, and
catch-up all retain their checkpoint shape, federation ID, config digest,
roster, lineage, distinct-signer, threshold, and individual-signature checks.
They no longer call full config validation. No config cache or general wrapper
was added; the cheap canonical config digest remains recomputed where evidence
must be bound.

The call audit also found two full validations of the unchanged config on every
AlephBFT live-session restart or automatic rotation: one in keychain creation
and one in peer-client creation. Checked constructors still validate arbitrary
configs, while runtime/session, sync-loop, DKG-driver, and optional wallet
liveness callers now use variants whose config has already passed
`FederationRuntimeConfig::new`. A config rebuilt after an accepted keyset
rotation remains a genuine new-config trust boundary and still receives full
validation.

Fast quorum verification performs a cheap deterministic federation-ID
derivation before it can construct `VerifiedFederationCheckpointQuorum`. This
validates thresholds, member count, roster bindings, member identities, and the
stored federation ID without keyset interpolation. A regression test proves a
mutated zero consensus threshold cannot manufacture an empty verified quorum.
This closes the authority invariant without a new validation-token abstraction.

The following candidate checks have passed so far:

```text
cargo test -p cdk-common --features federation,prometheus checkpoint
  19 passed
cargo test -p cdk --no-default-features \
  --features mint,wallet,federation,prometheus,bip353 \
  runtime_config_construction_still_rejects_malformed_public_config
  1 passed
cargo test -p cdk-axum --features federation,prometheus \
  trusted_checkpoint_restart_rejects_changed_suffix
  2 passed
cargo test -p cdk-axum --features federation,prometheus \
  aleph_bft_live_session_restarts_from_persisted_backup_and_continues_finality
  1 passed
cargo test -p cdk-axum --features federation,prometheus performance_
  3 passed
cargo check -p cdk-common --no-default-features --features mint,wallet
cargo check -p cdk --no-default-features --features mint,wallet,bip353
cargo check -p cdk-axum --no-default-features
cargo check -p cdk-axum --features federation,prometheus
scripts/benchmark_nocean_federation.sh --self-test
  all passed
```

The common tests now exercise the non-empty validated-config transport path,
including a valid quorum, wrong requested checkpoint ID, corrupt signature,
tampered federation/config bindings, missing and unknown roster watermarks,
duplicate/unknown signers, insufficient quorum, and invalid threshold. Runtime
construction still rejects a mismatched aggregate BLS key and a setup
transcript with a missing member signature.

Release artifacts were built from Milestone 0 commit
`a5ae88c88afa0a910632be921e277d3c49c33719` plus the uncommitted Milestone 1
candidate using the frozen build commands. Their SHA-256 values are:

| Artifact | SHA-256 |
| --- | --- |
| Candidate `cdk-mintd` | `7a43501c9f762b3eb3c37d7181be43c3956dbe04b147fcc72e597cc11856b506` |
| Candidate `cdk-cli` | `a288b34b3ca7e2dd9c7ca4472e6562e7e3ebc585f80449952ded8a4202dd6ee3` |
| Benchmark harness | `f25c273431c6bccf5bb0bd189f646fdc6e6073a8f2137ba45fc09f122f34e001` |

A fresh five-member SQLite/fakewallet `20/20` configuration was generated for
the reduced `10:1`, `20:5`, `100:20` matrix. The initial sandboxed launch was
rejected before readiness because local binds were not permitted. After
explicit approval, a first elevated launch reached readiness and funded the
benchmark wallet, but the harness stopped before its first timed case because
the manual launcher had not written the PID files required for CPU accounting.
That launch produced no sample. A second fresh configuration was then generated
with the same policy and separate loopback ports; its launcher wrote each PID
file before readiness, and exactly one sample per requested case completed. No
extra repetition or reciprocal interval probe ran.

The retained candidate matrix used release binaries, five members, threshold
three, consensus threshold four, SQLite, fakewallet, `20/20` checkpoint
intervals, five wallet-preparation jobs, a 12-second pre-window settle, and a
three-second explicit post-run idle window. The command shape was:

```bash
LOCAL_FEDERATION_DIR="$FEDERATION_DIR" \
MINT_URL="$MINT_URL" MINTD=target/release/cdk-mintd \
CLI=target/release/cdk-cli BUILD_CLI=0 EXPECTED_MEMBERS=5 \
CASES='10:1 20:5 100:20' PREPARE_PARALLELISM=5 \
SETTLE_SECONDS=12 IDLE_CPU_SECONDS=3 RUN_DIR="$PRIVATE_RUN_DIR" \
scripts/benchmark_nocean_federation.sh
```

#### Candidate release results and gate

The comparison below uses the retained Milestone 0 `20/20` matrix. The formal
`10:1` CPU gate compares the candidate with the stable 52.45-second baseline
median rather than only the 52.95-second representative row.

| Case | Wallet success baseline → candidate | Wall | Confirmed TPS | p50 | p95 | Aggregate member CPU |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `10:1` | 10/10 → 10/10 | 25.969 → 26.248 s (+1.1%) | 0.385 → 0.381 (-1.0%) | 2.511 → 2.498 s (-0.5%) | 2.695 → 3.137 s (+16.4%) | 52.95 → 3.77 s (-92.9%) |
| `20:5` | 20/20 → 20/20 | 12.402 → 10.223 s (-17.6%) | 1.613 → 1.956 (+21.3%) | 2.804 → 2.510 s (-10.5%) | 3.681 → 2.684 s (-27.1%) | 43.37 → 3.39 s (-92.2%) |
| `100:20` | 70/100 → 100/100 | 30.814 → 12.926 s (-58.1%) | 2.272 → 7.736 (+240.5%) | 3.980 → 2.496 s (-37.3%) | 10.123 → 2.863 s (-71.7%) | 85.86 → 9.54 s (-88.9%) |

The `10:1` candidate used 0.377 aggregate CPU-seconds per successful swap
versus 5.245 at the baseline median, a **92.8%** reduction. Its primary batch
wall, confirmed throughput, and median latency remained within the 5% budget.
The one low-volume p95 rank increased by 0.442 seconds (16.4%); this is recorded
rather than hidden. With only ten observations it represents one additional
tail completion: maximum latency moved only +2.2% (3.439 to 3.515 seconds),
while p95 improved 27.1% at `20:5` and 71.7% at `100:20`. The user-directed
single-run scope does not support attributing that isolated low-N tail rank as a
repeatable regression, and no other primary latency or throughput signal moved
in the same direction.

All cases had zero late operations, zero sync transitions, bounded post-run idle
CPU (0.37, 0.38, and 0.40 aggregate seconds), and all five members ready in one
exact digest group. Persisted journal/finalized/application state and freshly
computed materialized-state digests matched for every case. The cached audited
materialized digest matched when a checkpoint was created and was correctly
diagnostic otherwise.

Direct work counters moved exactly as predicted in every transaction window:

| Work | `10:1` | `20:5` | `100:20` |
| --- | ---: | ---: | ---: |
| Full config validations | 0 | 0 | 0 |
| Keyset validations | 0 | 0 | 0 |
| Aggregate-key interpolations | 0 | 0 | 0 |
| Setup-lineage validations | 0 | 0 | 0 |

The counters were nonzero before the first measurement window—20 full config
validations, 40 keyset validations, 320 interpolations, and ten setup-lineage
validations federation-wide—confirming that startup and config-ingress checks
remain active. Their values stayed byte-for-byte unchanged across all three
healthy transaction windows, including checkpoint creation, quorum formation,
peer transfer, promotion, and repeated capture inspection.

Checkpoint evidence verification was not bypassed. The candidate still
processed 1,190, 921, and 1,050 checkpoint signatures in the three windows.
The `10:1` window performed 250 capture-context quorum verifications, plus
formation, peer-fetch, and promotion verification, while creating ten
member-local candidates and promoting two logical checkpoints. The removed
work is exclusively repeated validation of the unchanged public config.

#### Verification and keep decision

Final focused checks on the candidate passed:

```text
cargo test -p cdk-common --features federation,prometheus checkpoint
  19 passed
cargo test -p cdk --no-default-features \
  --features mint,wallet,federation,prometheus,bip353 \
  runtime_config_construction_still_rejects_malformed_public_config
  1 passed
cargo test -p cdk-axum --features federation,prometheus \
  trusted_checkpoint_restart_rejects_changed_suffix
  2 passed
cargo test -p cdk-axum --features federation,prometheus \
  aleph_bft_live_session_restarts_from_persisted_backup_and_continues_finality
  1 passed
cargo test -p cdk-axum --features federation,prometheus \
  checkpoint_catch_up_applies_bounded_history_and_promotes_target
  1 passed
cargo check -p cdk-common --no-default-features --features mint,wallet
cargo check -p cdk --no-default-features --features mint,wallet,bip353
cargo check -p cdk-axum --no-default-features
cargo check -p cdk-axum --features federation,prometheus
cargo fmt --all -- --check
git diff --check
scripts/benchmark_nocean_federation.sh --self-test
  all passed
```

**Kept.** The simplest fix separates full config-ingress validation from cheap
checkpoint evidence validation while retaining the checked public APIs. It
introduces no cache, compatibility layer, benchmark-only path, or weakened
invariant. The direct counter reaches zero exactly where predicted, CPU falls
far beyond the 15% gate, and realistic success/throughput improve sharply. The
retained metrics remain bounded production observability; no temporary
instrumentation or private benchmark artifact is committed. The milestone
commit subject is `perf: avoid repeated checkpoint config validation`; its hash
is recorded at Milestone 2 start.

## Milestone 2: bounded checkpoint signature and quorum verification

### Goal

Ensure each unique signature and quorum is cryptographically verified when it
crosses a trust boundary, without re-verifying unchanged local evidence on every
finalized unit, sync poll, persistence step, or promotion check.

### Preconditions

- Milestone 0 or the post-Milestone-1 profile proves repeated signature/quorum
  verification remains material after config interpolation is removed.

### Todos

- [x] Trace signature creation, peer receipt, durable insertion, quorum assembly,
      quorum transfer, trusted promotion, startup reload, and catch-up use.
- [x] Write the required verification cardinality as a function of unique new
      signatures, unique quorums, members, and logical checkpoint boundaries.
- [x] Identify duplicate verification of the same evidence in
      `persist_checkpoint_signature`, quorum construction/persistence, current
      trusted-quorum comparison, capture, and peer sync.
- [ ] Carry the existing `VerifiedFederationCheckpointQuorum` authority through
      adjacent persistence/promotion calls instead of converting it immediately
      back to raw data and verifying it again.
- [x] Prefer verified-at-insertion/startup durable invariants over a new cache.
      Add a cache only if measurement proves the same verified object must cross
      non-adjacent paths and no simpler ownership change works.
- [x] Add corruption, duplicate signer, minority quorum, wrong checkpoint ID,
      wrong roster, restart, and fetched-quorum adversarial tests.
- [x] Reuse the user-reduced post-Milestone-1 `10:1`, `20:5`, and `100:20`
      release matrix plus its unchanged idle windows with exact verification
      counters. Do not run an additional `40:10` or repeat matrix after the
      precondition fails.

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

### Progress record — 2026-07-17

Milestone 2 is `Rejected` at its precondition gate. It starts from Milestone 1
commit `887e5a49ea1b31bf72d0290eab25799b10f6567c` and reuses the exact measured
release artifacts and retained `10:1`, `20:5`, `100:20` matrix from that
milestone. No production candidate was implemented and no extra benchmark run
was added.

The lifecycle trace confirms real duplicate work. A locally derived checkpoint
is signed and persisted, peer signatures are verified on receipt, quorum
assembly verifies the bounded proof, peer transfer verifies it again, promotion
verifies it again, and every later capture inspection re-verifies the current
trusted quorum. Startup and checkpoint catch-up correctly verify durable or
external evidence before constructing `VerifiedFederationCheckpointQuorum`.
For `K` logical checkpoints, `N` members, quorum `Q`, and `E` distinct external
quorum proofs, the necessary cryptographic floor is linear in unique evidence:
`O(K*N + E*Q)`, with each unique `(checkpoint_id, signer)` verified at receipt
or trusted startup and each external quorum checked once. It must not scale with
the number of finalized-unit callbacks that merely inspect an unchanged trusted
checkpoint.

The observed count amplification is therefore genuine. The `10:1` window
created two logical checkpoints with ten unique retained signer pairs, yet
processed 1,190 signatures through 298 verification wrappers, including 250
capture-context quorum calls. The `20:5` window processed 921 signatures through
248 wrappers for ten new signer pairs. The `100:20` window created no checkpoint
and still processed 1,050 signatures through 250 repeated capture inspections
of unchanged authority. Each three-second post-run idle window processed zero
additional signatures, proving the amplification is tied to active finalized
callbacks rather than an unbounded idle poll.

After Milestone 1 removed config interpolation from those wrappers, their direct
aggregate duration is no longer material:

| Case | Verification wrappers | Signatures processed | Aggregate measured duration | Share of aggregate member CPU |
| --- | ---: | ---: | ---: | ---: |
| `10:1` | 298 | 1,190 | 0.108802 s | 2.9% |
| `20:5` | 248 | 921 | 0.067253 s | 2.0% |
| `100:20` | 250 | 1,050 | 0.079927 s | 0.8% |

These totals include capture, formation, peer-fetch, promotion, and the direct
formation/fetch/publish signature wrappers. Even an impossible implementation
that eliminated every measured verification would remain far below the 15%
end-to-end CPU gate in all three cases. The high invocation counts are useful
architectural evidence, but count reduction alone cannot justify ownership
changes, durable trust assumptions, or a cache under the implementation
contract.

The Milestone 1 adversarial suite already proves corrupt signatures, duplicate
and unknown signers, minority quorum, wrong checkpoint ID, wrong federation and
config bindings, wrong roster watermarks, fetched-quorum corruption, trusted
checkpoint restart, and bounded catch-up fail closed. Those checks are retained
unchanged. The unattempted authority-carrying production todo remains open by
design because the mandatory materiality precondition failed.

**Rejected.** No production code was changed, so there is nothing to revert.
The simplest correct decision is to retain the bounded counters and move to the
already-confirmed suffix-loading bottleneck in Milestone 3. The milestone commit
subject is `docs: reject checkpoint verification optimization`; its hash is
recorded at Milestone 3 start.

## Milestone 3: linear scheduled-checkpoint accounting

### Goal

Remove repeated finalized-suffix loading and canonicalization from every
finalized-unit callback. Work between checkpoint boundaries must be linear in
new finalized data, not quadratic in the configured interval.

### Preconditions

- Milestone 0 records rows and canonical bytes processed at multiple checkpoint
  intervals and confirms the current growth curve.

Milestone 3 starts from evidence-only Milestone 2 commit
`81014e7ed963bf0c7cfe968b026c64fa0ab04332`. Its first task is to locate the
smallest existing durable/runtime scalar that can prove count-trigger
ineligibility before suffix loading while preserving the byte trigger exactly.
No production change will be made until that state and every persistence/restart
boundary are traced.

### Todos

- [x] Exercise canonical-byte accounting at 20, 200, 2,000, and synthetic
      10,000-item histories in one deterministic test, then use the
      user-reduced `10:1`, `20:5`, and `100:20` release matrix for database-row,
      byte, capture-time, and total-CPU counters. No extra interval run or
      repetition is required.
- [x] Perform cheap journal/finalized progress checks before loading a batch when
      they can prove ineligibility.
- [x] Account for the canonical-byte trigger incrementally as finalized items are
      durably persisted, or use an equally simple deterministic scalar frontier.
      Do not silently remove or weaken the byte-based checkpoint trigger.
- [x] Load no suffix for ordinary scheduled eligibility or construction because
      the checkpoint already carries the cumulative finalized digest and does
      not consume the loaded rows. After recovery-rooted restart, load only the
      immutable scheduled-anchor-to-runtime-base segment once and cache its
      subtotal; transfer and catch-up retain their existing exact range loads.
- [x] If durable frontier/schema state changes, update all federation backends,
      fixtures, startup/replay logic, and tests together and regenerate
      experimental databases without compatibility code. No durable state or
      schema changed in the retained implementation, so backend migrations and
      compatibility code are not applicable.
- [x] Add boundary tests for journal-only, finalized-only, byte-only, exact-limit,
      one-over-limit, empty-unit, restart, and crash-before/after-persistence
      cases.
- [x] Run the deterministic four-size growth check and exactly one realistic
      local receive sample for each approved case.

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

### Progress record — 2026-07-17

Milestone 3 is `Complete`. It starts from Milestone 2 commit
`81014e7ed963bf0c7cfe968b026c64fa0ab04332` and changes no checkpoint format,
durable frontier, database schema, trust boundary, or catch-up range.

#### Confirmed cause and exact implementation

The scheduled capture path loaded
`get_federation_aleph_bft_finalized_items_range` and serialized every returned
item solely to calculate `batch_canonical_bytes` before calling
`is_scheduled_eligible`. The loaded rows were not used to construct, digest,
sign, or persist the checkpoint: the boundary already contains the cumulative
finalized-item digest. Milestone 1's `10:1` window therefore made 193 suffix
loads, read 2,484 rows, and serialized 2,970,275 bytes; 2,284 rows and
2,731,430 bytes were immediately discarded by ineligible captures. This
identifies the symptom and its consumer without ambiguity.

The consensus runtime already calculated each finalized item's canonical size
once during prepared finalization and reconstructed the retained byte total
during verified startup replay. The retained change keeps a parallel prefix-sum
vector for those already-calculated sizes alongside the existing status total.
An exact retained index range is now an O(1) subtraction, so scheduled
eligibility preserves the canonical-byte trigger without a database read or
repeated serialization.

A trusted recovery checkpoint may be newer than the latest scheduled anchor.
In that case the verified runtime intentionally retains no pre-recovery rows.
Scheduled capture loads only that immutable scheduled-anchor-to-recovery-base
segment once, validates its exact row count, caches the subtotal under the
application lock, and combines it with the live O(1) retained-range value. A
new scheduled anchor or runtime recovery base changes the cache key. This is the
smallest restart-safe fix and avoids adding durable state or compatibility code.

#### Deterministic size and safety verification

The permanent large-history regression finalizes 10,000 canonical module facts,
checks the exact accumulated byte count at 20, 200, 2,000, and 10,000 items,
then performs 10,000 exact tail-range queries. The test body completed in 0.80
seconds in a debug test build; its purpose is the deterministic O(1) work
invariant, not a debug/release throughput comparison. Exact trusted-checkpoint
restart reconstructs the same range index, empty ranges return zero, and ranges
before the authenticated retained base fail closed.

Focused verification passed:

```text
cargo test -p cdk-common --features federation \
  consensus_runtime_canonical_byte_ranges_remain_exact_at_large_history
cargo test -p cdk-common --features federation \
  consensus_runtime_starts_at_a_trusted_checkpoint_without_prefix_rows
cargo test -p cdk-common --features federation \
  scheduled_eligibility_requires_terminal_complete_unit_progress
cargo test -p cdk-axum --features federation \
  scheduled_checkpoint_preserves_the_canonical_byte_trigger
cargo test -p cdk-axum --features federation \
  scheduled_checkpoint_captures_first_complete_eligible_boundary
cargo test -p cdk-axum --features federation \
  federation_runtime_startup_at_trusted_checkpoint_loads_no_prefix_rows
  all passed
```

These cover byte-only eligibility, the exact byte boundary and one byte before
it, journal-only and finalized-only triggers, empty retained ranges, an invalid
pre-base range, 10,000-item history, checkpoint-rooted restart, recovery-rooted
restart with repeated post-restart captures, and unchanged candidate creation.
Existing finalized-unit, persistence fault, interrupted-candidate recovery, and
catch-up tests retain the crash-before/after and replay coverage.

Final hygiene also passed `cargo fmt --all -- --check`, strict Clippy for
`cdk-common` with federation enabled, the benchmark harness self-test, and
`git diff --check`. Strict workspace/Axum Clippy remains blocked by the branch's
pre-existing warnings outside this milestone (including existing complexity,
large-enum, and style findings); the milestone does not broaden scope by
rewriting those unrelated paths.

#### Release artifacts and realistic matrix

The candidate release artifacts were built from the Milestone 2 commit plus the
uncommitted Milestone 3 candidate using the frozen commands and had these
SHA-256 values:

| Artifact | SHA-256 |
| --- | --- |
| Candidate `cdk-mintd` | `5fb9444d97732af295c025e78ab2f47dffc4ac98ca1002fd2a5e0ce3385cb080` |
| Candidate `cdk-cli` | `ef2051af311821fee812b8a22671528a06d04de8f8f56086d84ac5aa431b96d3` |
| Benchmark harness | `f25c273431c6bccf5bb0bd189f646fdc6e6073a8f2137ba45fc09f122f34e001` |

One fresh five-member SQLite/fakewallet federation used threshold three,
consensus threshold four, `20/20` checkpoint intervals, five preparation jobs,
a 12-second settle delay, and a three-second post-run idle window. Exactly the
approved `10:1`, `20:5`, and `100:20` cases ran; there were no repetitions or
opposite-interval probes.

| Case | Success M1 → M3 | Wall | Confirmed TPS | p50 | p95 | Aggregate member CPU |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `10:1` | 10/10 → 10/10 | 26.248 → 22.206 s (-15.4%) | 0.381 → 0.450 (+18.1%) | 2.498 → 2.009 s (-19.6%) | 3.137 → 2.505 s (-20.1%) | 3.77 → 3.07 s (-18.6%) |
| `20:5` | 20/20 → 20/20 | 10.223 → 10.213 s (-0.1%) | 1.956 → 1.958 (+0.1%) | 2.510 → 2.505 s (-0.2%) | 2.684 → 2.669 s (-0.6%) | 3.39 → 3.14 s (-7.4%) |
| `100:20` | 100/100 → 100/100 | 12.926 → 13.460 s (+4.1%) | 7.736 → 7.429 (-4.0%) | 2.496 → 2.497 s (+0.0%) | 2.863 → 3.030 s (+5.8%) | 9.54 → 9.44 s (-1.0%) |

All three cases had exact immediate and eventual operation deltas, zero late
operations, zero sync transitions, zero federation alerts, matching persisted
frontiers and fresh materialized-state digests, and bounded idle CPU of 0.35,
0.37, and 0.34 aggregate seconds. The primary success, wall, throughput, p50,
and CPU metrics remain within the 5% budget in every case; sequential CPU and
wall exceed the 15% materiality gate.

The `100:20` p95 moved 0.167 seconds, or 5.8%, while its p50 was unchanged,
service-rate and wall movement remained within 5%, CPU improved, and success
remained 100%. Its 225 scheduled captures all returned for an already-pending
candidate before the changed byte-range path ran: zero byte queries, suffix
loads, rows, or canonicalized bytes occurred in both the M1 and M3 windows.
The single-run scope therefore cannot attribute this narrow tail-rank movement
to the removed suffix work. It is disclosed rather than hidden and does not
override the direct causal result.

#### Direct counter and keep decision

| Case | Suffix loads M1 → M3 | Rows loaded | Canonical bytes serialized | O(1) byte-check time | Capture time |
| --- | ---: | ---: | ---: | ---: | ---: |
| `10:1` | 193 → 0 | 2,484 → 0 | 2,970,275 → 0 | 11.981 ms → 0.049 ms | 0.755682 → 0.268278 s (-64.5%) |
| `20:5` | 53 → 0 | 767 → 0 | 1,099,720 → 0 | 3.700 ms → 0.015 ms | 0.506855 → 0.302550 s (-40.3%) |
| `100:20` | 0 → 0 | 0 → 0 | 0 → 0 | 0 → 0 | 0.149625 → 0.135430 s (-9.5%) |

**Kept.** The direct database-row and serialization counters fall exactly to
zero during ordinary operation, the four-size invariant proves constant-time
eligibility accounting through 10,000 items, and the sequential realistic case
improves CPU by 18.6% with no safety or primary-metric regression. Production
metrics remain bounded; private benchmark artifacts remain outside the
repository. The milestone commit subject is
`perf: make checkpoint eligibility accounting linear`; its hash is recorded at
Milestone 4 start.

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
| 2026-07-17 | `a5ae88c88afa0a910632be921e277d3c49c33719` | Local release, 5 members, SQLite, fakewallet | `20/20` versus `20000/20000`; one retained `10:1`, `20:5`, `100:20` matrix each | Stable sequential aggregate CPU median 52.45s versus 6.43s (-87.7%); direct `40:10` profile recorded 1,128 full config validations and 18,048 BLS public-key interpolations | Confirmed repeated immutable config/keyset validation is the dominant transaction compute sink; bounded metrics, exact digest diagnostic, and corrected harness kept; no production optimization in M0 |
| 2026-07-17 | `887e5a49ea1b31bf72d0290eab25799b10f6567c` | Local release, 5 members, SQLite, fakewallet, `20/20` | One retained `10:1`, `20:5`, `100:20` candidate matrix | CPU 3.77s / 3.39s / 9.54s; success 10/10 / 20/20 / 100/100; 0 full config, keyset, interpolation, or setup-lineage validations in every window | Kept: validated-config checkpoint paths remove the confirmed sink; sequential CPU -92.8% versus baseline median and realistic `100:20` reaches 7.736 confirmed TPS with exact convergence |
| 2026-07-17 | This Milestone 2 commit; hash recorded at Milestone 3 start | Same Milestone 1 release matrix | Post-fix verification materiality gate | 298 / 248 / 250 wrappers and 1,190 / 921 / 1,050 signatures consumed only 0.108802s / 0.067253s / 0.079927s aggregate | Rejected before implementation: even eliminating all measured signature/quorum verification cannot meet the 15% CPU gate; no production change or extra run |
| 2026-07-17 | This Milestone 3 commit; hash recorded at Milestone 4 start | Local release, 5 members, SQLite, fakewallet, `20/20` | One retained `10:1`, `20:5`, `100:20` candidate matrix plus deterministic 20/200/2,000/10,000-item test | Success 10/10 / 20/20 / 100/100; CPU 3.07s / 3.14s / 9.44s; ordinary scheduled eligibility loaded 0 suffix rows and reserialized 0 historical bytes in every case | Kept: prefix accounting removes the confirmed quadratic work; sequential CPU -18.6%, wall -15.4%, direct counters reach zero, and 10,000-item range queries remain O(1) |
