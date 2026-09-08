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
