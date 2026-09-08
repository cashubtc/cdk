# Federation performance optimization journal

Status: in progress. See the [plan](bls-federation-performance-plan-2026-09-08.md)
and [report](bls-federation-performance-report-2026-09-08.md).

Only anonymized aggregates and repository-relative commands belong here. Raw
artifacts are private and ignored. Never record payment secrets, authentication,
local machine identifiers, addresses, or absolute environment paths in Git.

## Campaign baseline

- CDK: `8e507cdf8`; clean working tree at initial inspection.
- Bark: `8d1eda643dd11e9eb600ec44e36622c8f77b6322` (unchanged dependency pin).
- Toolchain: pinned `nix develop .#stable`; release profile, locked dependencies.
- Measurement order: DKG phases, serial consensus/ecash, then disposable
  five-member process/payment fixtures and the remaining matrix dimensions.
- Existing local databases and services are preserved. No baseline/candidate
  fixture shares live signing state.
- Five alternating matched pairs are required for candidate decisions; the
  historical one-run and 15% policies do not apply to this campaign.

## E00 — Measurement infrastructure and immutable baseline

Hypothesis: existing harnesses can supply reproducible release measurements
without production changes. This is infrastructure, not an optimization claim.

Inspection:

- `crates/cdk-common/examples/dkg_performance.rs` provides 20 phase measurements,
  five/eleven/twenty-member profiles, and optional all-member BLS ceremony checks.
  It currently supports one/eight denominations. It does not measure HTTP/Iroh
  transport or the full service BLS/FROST activation path.
- `scripts/benchmark_local_aleph_bft_swaps.sh` provides serial release swaps and
  network/queue counters; its in-process adapter is not an HTTP/Iroh benchmark.
- `scripts/benchmark_nocean_federation.sh` has explicit local mode, independent
  token/wallet fixtures, late-work accounting and CPU/RSS sampling. CLI startup
  belongs in its results; persistent SDK results need separate identification.
- `scripts/run_local_bark_federation.sh` is signet-only. Regtest needs a separate
  disposable fixture; invoking its destructive fresh mode on old state is invalid.

Commands (run inside the pinned Nix shell; build output kept privately):

```sh
rustc -Vv
cargo build --locked --release -p cdk-common --features federation,bench --example dkg_performance
```

Outcome: the unchanged DKG release binary was preserved. A five-member,
one-denomination pilot (five iterations, all phases, full in-memory BLS ceremony)
passed all reported correctness checks. The entire process took 8.82 seconds;
extended OS resource counters were unavailable in the sandbox. This is a pilot,
not a matched baseline/candidate comparison. The consensus release build is
pending. No production edits.

Pilot phase medians (microseconds; distinct benchmark operations, not additive
wall-time spans): validated participant generation 21,596; one secret-share
verification 2,028; aggregate-polynomial public keyset derivation 10,017; one
local keyset derivation 20,662; complete in-memory BLS ceremony 503,479. These
do not measure HTTP/Iroh, FROST, persistence, or service readiness.

Reusable benchmark support: denomination counts now accept 1–64, enabling the
full 64-denomination configuration without changing production configuration.
The analyzer normalizes existing DKG/Aleph reports and compares run-level paired
log ratios with two-sided 95% Student t intervals (four or nine degrees of
freedom). Within-run transactions are not independent run samples. Confidence
intervals describe run variability; they do not establish causation or replace
correctness/work-counter gates. Seven analyzer tests pass, covering incomplete or
duplicate pairs, mismatched workloads, execution-order bias, tail sample size,
noise, metric direction, client queueing, window boundaries and late completions.
The release harness also rejects zero, negative, above-64 and nonnumeric
denomination counts before generating any fixture.

```sh
python3 -m unittest discover -s scripts -p test_analyze_federation_performance.py -v
```

Instrumentation disposition: no production probes added yet.
Decision: pending infrastructure validation.

### Common result format

`scripts/analyze_federation_performance.py normalize dkg|aleph INPUT METADATA`
emits JSONL. `compare INPUT_JSONL` emits all paired samples, ratios, intervals
and statistical dispositions. Neither command runs workloads or changes state.

Each row contains `schema_version: 1`, `pair` (integer), `arm` (baseline or
candidate), `order` (1 or 2 within pair), `revision`, `artifact_sha256`, `workload`,
`sample_count`, `metrics`, and `higher_is_better` (metric names). Metadata supplies
the run identity and workload manifest; normalization adds measured harness
dimensions. Include toolchain/profile/features, holdings, history, storage,
connection state, resource limits, policy and fixture seed class in `workload`.
The manifest must match across all pairs. Keep actual secrets outside it.

Metric names include their units. Null means unavailable, not zero. DKG phases
are not payments; Aleph adapter swaps report consensus operations/s. Process CPU
including fixture setup must not be labeled transaction CPU. A p99 needs at
least 1,000 completed samples in each run. Zero-valued counters require absolute
analysis; the analyzer does not invent ratios for them. Statistical improvement
alone is never a keep decision. Inspect all regression/idle/correctness gates.

`payment-window INPUT_JSON` analyzes a bounded SDK arrival window on one
monotonic clock. Attempts use numeric IDs and scheduled/start/finish offsets in
nanoseconds, with explicit completed/failed/timeout/rejected/pending outcomes.
It includes client queueing, separates every late outcome, and never counts
late completions in window throughput. It does not infer sustainable throughput
without queue-stability evidence.

## E02a — Public polynomial evaluation (before production editing)

Owner: federation-only BLS public commitment arithmetic in `cashu`.

Hypothesis: Horner evaluation removes one full G2 scalar multiplication per
nonempty polynomial and avoids maintaining scalar powers while producing the
same public point. Existing code multiplies the constant coefficient by one.
Expected work: for a polynomial with `t` commitments, G2 multiplications change
from `t` to `t - 1`. No validation, group library, wire format or cryptographic
construction changes. Empty/identity results must retain existing rejection.

Attribution so far: a ten-second local sampling profile of the release
five-member/eight-denomination all-phase harness attributed 4,254 of 7,688 worker
stack samples (55.3%) to public-polynomial evaluation. This early capture includes
fixture and checked participant generation, so it cannot establish the share of
full service activation time. A focused local-keyset profile remains required.
The profiled process completed successfully: 163.01 s wall, 142.27 s user CPU,
1.22 s system CPU, 7,585,792 bytes maximum RSS. These totals include preparation
and profiler overhead; they are attribution evidence, not A/B acceptance samples.

Primary comparison: five-member/eight-denomination local-keyset derivation and
full in-memory BLS ceremony, five alternating independent pairs, same release
harness and features. Validate one denomination, full configured denominations,
eleven members, and twenty-member scaling after the primary comparison. Measure
CPU and memory independently from phase latencies; process totals include setup.

Correctness obligations: direct comparison with the existing power-sum formula
and trusted-dealer shares, boundary signer IDs, empty/identity polynomials,
all-member ceremony agreement and existing malformed/share/restart tests.

Decision: pending focused attribution, implementation and paired measurements.

Focused attribution completed: 100 single-local-keyset derivations, five members
and eight denominations. During a ten-second capture after fixture preparation,
public-polynomial evaluation appeared in 5,510 of 7,817 worker samples (70.5%).
The profiled phase median was 171,599 microseconds. This establishes a hot path;
the profiled timing is not used as an uninstrumented acceptance baseline.

Temporary probe design: bounded atomic counts of public-polynomial evaluations
and G2 scalar multiplications, reset/read at benchmark phase boundaries. Build
one baseline and one candidate probe binary, compare exact work on identical
shapes, then remove the statics, exports and phase fields before timing pairs.
Probe binaries and diffs stay private. The final code must have no counter branch
or exported probe API.

Probe results (one iteration per phase; five members, eight denominations):

| Phase | Evaluations, both revisions | Baseline G2 multiplications | Candidate G2 multiplications |
|---|---:|---:|---:|
| One secret-share request verification | 8 | 24 | 16 |
| Public keyset derivation | 40 | 120 | 80 |
| One local keyset derivation | 80 | 240 | 160 |
| Complete in-memory BLS ceremony | 1,600 | 4,800 | 3,200 |

Both probe runs passed all-member completion, transcript/keyset/public-share
agreement and local secret/public correspondence. The predicted one-third
reduction in point multiplications is observed; polynomial evaluations are
unchanged. All temporary statics, exports, and phase fields were removed before
building the acceptance candidate.

Arithmetic checks: two release tests pass. The existing trusted-dealer test now
covers thresholds 1/3/8/14, nonconsecutive signer IDs and the maximum signer ID,
and compares against the original power-sum formula. A boundary test preserves
empty/final-identity rejection while permitting a zero intermediate sum.

```sh
cargo test --locked --release -p cashu --features federation public_polynomial -- --test-threads=1
cargo build --locked --release -p cdk-common --features federation,bench --example dkg_performance
dkg_performance --profile 5-of-3 --denominations eight --iterations 5 \
  --phases single_secret_share_verification,single_local_keyset_derivation,public_keyset_derivation_aggregated \
  --full-ceremony --timeout-secs 300
```

Acceptance workload: five alternating pairs using the preserved baseline with
the same version-8 harness and a probe-free candidate. Each pair uses separate
fresh in-memory fixtures. Five iterations per phase; p99 is unavailable. Locked
Nix Rust 1.98.0, default release profile, `federation,bench`, one CPU worker, no
transport or database. Resource totals include fixture preparation; phase
latencies exclude it. No change is accepted until the pairs and required gates
are recorded.

### Ten-pair result

The first five pairs showed a repeatable CPU benefit but inconclusive latency
intervals. The experiment was extended to ten pairs without dropping any run.
Every raw phase-duration sample and whole-process resource observation is
retained in [the numeric sample ledger](benchmarks/2026-09-08-dkg-horner.csv).
There are 20 runs, 400 phase observations and 20 process resource observations.
All 20 runs passed the all-member ceremony consistency checks. Resource rows
explicitly include fixture work; they are not CPU/payment measurements.

| Metric | Baseline geometric mean | Candidate geometric mean | Candidate/baseline 95% interval | Interpretation |
|---|---:|---:|---|---|
| Full in-memory BLS ceremony run p50 | 3,880.70 ms | 3,177.58 ms | 0.7720–0.8685 | 18.1% lower |
| Full in-memory BLS ceremony run p95 | 4,110.15 ms | 3,669.81 ms | 0.7439–1.0717 | Inconclusive |
| Whole-harness CPU | 25.147 s | 20.417 s | 0.7734–0.8524 | 18.8% lower |
| Whole-harness retired instructions | 680.363 billion | 535.087 billion | 0.7864–0.7866 | 21.4% fewer |
| Whole-harness maximum RSS | 7.018 MB | 7.077 MB | 0.9918–1.0254 | No confirmed increase; interval below 5% regression |

Other phase ratios: secret-request p50 0.7482 (0.6362–0.8799), p95 0.7644
(0.6409–0.9116); local-keyset p50 0.8450 (0.7653–0.9330), p95 0.9837
(0.6376–1.5176); public-keyset p50 0.9330 (0.5914–1.4719), p95 0.9834
(0.6173–1.5665). The latter tail/public-keyset comparisons remain inconclusive.
In particular, pair four's candidate contains a large short-phase outlier; its
cause is not established and the sample is retained. No p99 conclusion is possible.

Correctness: `cashu` release federation suite: 641 passed, two existing ignored;
`cdk-common` DKG-filtered release suite: 40 passed; strict federation Cashu library
Clippy: passed. Existing independent formula/secret-share checks remain enabled.

Disposition: keep the narrow arithmetic change for its repeatable primary
full-ceremony median and CPU savings. No measured metric establishes a regression
above the rejection threshold; several tail comparisons remain inconclusive and
are not treated as passing evidence for a tail improvement. Scaling correctness
and ordinary downstream isolation subsequently passed. Final full-service,
runtime/recovery and payment gates remain pending. This component decision is not
final campaign acceptance or evidence for a Lightning/ecash throughput gain.

## E00 follow-up — Payment measurement support

The existing parallel ecash shell harness now uses nearest-rank percentiles and
reports N/A for p99 below 1,000 successful receives and for all latencies when
there are no successes. Its self-test passes, including new empty/small/1,000
sample cases. This fixes evidence reporting; it is not a production speedup.

The persistent SDK receiver is being added as a federation-gated example using
the same independent token files that the existing harness prepares. It shares
one long-lived federation connector, creates fresh durable recipient wallets,
and measures warm/cold metadata preparation separately. Independent-wallet and
shared-wallet cases remain explicit. Closed-loop execution is bounded by the
requested concurrency; open-loop arrivals reject excess work with zero queued
requests. Arrival, scheduler-start and finish offsets share one monotonic clock.
Late completions and drain-time pending operations remain separate. Client
errors never authorize a payment retry; private seeds/databases remain for
reconciliation. Driver build and real-payment validation are pending.

## Candidate queue after inspection

| ID | Hypothesis | Required attribution | Status |
|---|---|---|---|
| E01 | DKG driver loads and decodes unrelated ceremony records | SQL rows/time and driver phase cost at matched history | Not yet measured |
| E02 | Remaining DKG planning/crypto/delivery work dominates activation | Release phases plus complete-process profile | Not yet measured |
| E03 | Payment delays include residual notification/RPC/lock waits | Correlated CDK and independent backend phase evidence | Not yet measured |
| E04 | Ecash throughput has a remaining shared submission/application cost | SDK latency, queue counters and CPU stacks | Not yet measured |

Every production candidate will record its owning layer, expected work reduction,
primary metric, exact workload, revisions, all paired samples and confidence
interval, correctness evidence, probe removal and keep/revert disposition before
being accepted. Unmeasured entries are hypotheses only.

## E00 follow-up — Isolated payment fixture and scaling validation

The preserved release baseline completed a fresh five-member, 64-denomination
HTTP setup and activation. Signed status reported every member healthy and ready,
matching order/application/materialized-state digests, no audit mismatches, and
Bark ready. Existing services and databases were preserved. Campaign-owned
Bitcoin, PostgreSQL, CLN payer/receiver and Ark services use fresh storage and
identities. Fixture construction and liquidity preparation are outside timings.

Two environment issues were corrected before payment measurement: the pinned
Ark server required a Bitcoin RPC response field absent from the initially chosen
Bitcoin 28 image, so the isolated node was upgraded to Bitcoin 31.1 while preserving
its chain; the isolated Docker network also needed the configured hold-service
hostname. An invoice-preparation pilot failed before payment while that hostname
was missing. Its logs and wallet state remain private; it is not a speed sample.

After correction, a fresh Bark incoming Lightning pilot paid and issued 4,096 sat.
CLI wall time was 7,980.13 ms: 2,338.63 ms to invoice availability, 1,762.84 ms for
the independent payer command, and 3,878.66 ms from payer completion to CLI exit.
These are single-run functional observations including CLI startup, not an
acceptance baseline or a throughput claim. The payer receipt and wallet state are
retained privately. Sixteen distinct 32-sat offline tokens were then prepared for
driver validation. No failed or ambiguous payment was automatically reinitiated.

The probe-free Horner candidate passed full in-memory BLS ceremony consistency
at five members/64 denominations, eleven members/one denomination, and twenty
members/one denomination. Each case used one iteration and verified all-member
completion, transcript/keyset/public-share agreement and independent secret/public
correspondence. Scaling timings are exploratory, not matched performance results.

Commands (inside the pinned stable Nix shell):

```sh
cargo build --locked --release -p cdk --no-default-features \
  --features wallet,federation,iroh --example federation_receive_performance
python3 scripts/check_standalone_api.py
cargo fmt --all -- --check
```

The SDK driver build, isolated ordinary API/serialization/dependency checks, FFI
default-feature assertion and formatting check passed. Full FFI compilation and
the remaining campaign gates are still pending.
