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
reconciliation. Driver build and serial/shared/open-loop real-payment validation passed;
acceptance workload and instrumentation-overhead measurements remain pending.

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

## E02b — Public signer-ID multiplication

Registered before production editing. Owning layer: federation-only Cashu BLS
public-polynomial evaluation, after E02a (`ceaf02216`). An eight-second sampling
profile of 150 local-keyset derivations (five members/eight denominations) placed
3,886 of 6,279 worker stack samples, 61.9%, inside public-polynomial evaluation.
The run median was 131,094 µs; this attribution run is not acceptance evidence.
The pinned `bls12_381` 0.8.0 source implements general scalar multiplication with
255 doubling/addition iterations even when the multiplier is a small public
signer ID. IDs are explicitly nonzero `u16` values; no secret scalar is involved
in this operation.

Hypothesis: retain Horner evaluation but use the existing group addition/doubling
operations over the significant bits of the public ID. This removes leading-zero
work and additions for unset public bits. The helper must accept `BlsSignerId`,
remain private, and explicitly document variable-time behavior on public data.
All private-scalar operations retain the existing constant-time implementation.
No cryptographic construction, group, encoding, membership rule or validation
changes. Empty/final identity rejection and intermediate identity handling remain.

Expected work: at IDs 1–5, each multiplication requires 0–2 doublings and 0–1
additions instead of 255 of each. Confirm actual invocation/operation counts with
bounded temporary counters, then remove probes. Primary metrics are full in-memory
BLS ceremony p50 and process CPU, with p95/RSS guardrails. Compare five alternating
pairs against E02a using the same eight-denomination/five-iteration workload;
extend to ten if inconclusive. Then compare the combined change to the original
baseline and use E02a as the ablation. Arithmetic boundary/reference tests precede
measurement. No keep decision has been made.

E02b work counters confirmed 3,200 public-ID multiplications per full BLS
ceremony. Those calls used 3,840 group doublings and 1,280 group additions, versus
816,000 of each in the pinned general-scalar algorithm for the same 3,200 calls.
The single-local-keyset phase used 160 calls, 96 doublings and 32 additions.
Temporary per-call diagnostic records and phase markers were removed before the
paired run; the acceptance binary was built without them. Nine arithmetic/public
key tests passed, including power-of-two boundaries through the maximum `u16`
ID, identity/negative points, and independent secret/power-sum references.

The SDK pilot also passed serial independent-wallet, concurrent shared-wallet
and bounded open-loop behavior against the original release server:

| Case | Offered | Completed | Rejected before initiation | Recipient balances |
|---|---:|---:|---:|---|
| Serial, four independent wallets | 4 | 4 | 0 | Four × 32 sat |
| Concurrency four, shared wallet | 4 | 4 | 0 | 128 sat |
| Concurrency one, 10 ms arrivals | 8 | 1 | 7 | 32 sat; seven empty wallets |

Every offered token contained one proof. There were no failed, late or unresolved
receives. Serial client latency was p50 901.34 ms / p95 1,034.26 ms, with 1,090.65 ms
of separate preparation. These small, deliberately exhausted fixtures validate
accounting and scheduling only; their window throughput is not sustainable TPS.
The rejected tokens were never submitted and remain private for later use.

E02b's five alternating ablation pairs completed with all-member consistency in
all ten runs. [All numeric samples](benchmarks/2026-09-08-dkg-public-id-ablation.csv)
are retained. Against E02a, candidate/baseline paired 95% intervals were:

| Metric | Ratio | 95% interval |
|---|---:|---|
| Full in-memory BLS ceremony p50 | 0.4729 | 0.4685–0.4774 |
| Full in-memory BLS ceremony p95 | 0.5005 | 0.4237–0.5914 |
| Whole-harness CPU | 0.4673 | 0.4580–0.4767 |
| Whole-harness instructions | 0.4591 | 0.4591–0.4592 |
| Whole-harness maximum RSS | 0.9909 | 0.9667–1.0156 |

Secret-request, public-keyset and local-keyset p50/p95 intervals also showed
improvement. p99 remains unavailable. The RSS interval does not establish a memory
reduction, but its upper bound remains below the 5% regression threshold. The
combined comparison against the original baseline and broader correctness gates
are pending; these numbers are incremental DKG microbenchmark results only.

### E02 combined comparison against the original baseline

Five new alternating matched pairs used the preserved original production code
with the identical version-8 harness versus probe-free E02a+E02b. Every run passed
all-member BLS consistency. [All samples](benchmarks/2026-09-08-dkg-combined.csv)
include every iteration and process observation. Same primary workload, compiler,
features and release profile as the ablation; no compilation overlapped timings.

| Metric | Baseline geometric mean | Combined geometric mean | Ratio 95% interval |
|---|---:|---:|---|
| Full in-memory BLS ceremony p50 | 3,671.88 ms | 1,406.38 ms | 0.3723–0.3940 |
| Full in-memory BLS ceremony p95 | 3,738.41 ms | 1,429.50 ms | 0.3640–0.4017 |
| Whole-harness CPU | 23.6410 s | 8.8187 s | 0.3628–0.3836 |
| Whole-harness instructions | 680.304 billion | 245.536 billion | 0.3608–0.3610 |
| Whole-harness maximum RSS | 6.848 MB | 6.746 MB | 0.9657–1.0047 |

This supports 61.7% lower BLS ceremony median, 61.8% lower p95, 62.7% lower harness
CPU and 63.9% fewer instructions. It does not establish a memory improvement.
Secret-request/public-keyset/local-keyset p50 ratios are 0.2747/0.2749/0.3092,
respectively, with all p50 and p95 confidence intervals below one. p99 is unavailable.
Process resource totals include fixture preparation. Transport, FROST, durable
storage, readiness, idle behavior and payment throughput are outside this result.
The original E02a runs and their outlier remain retained; they were not replaced
by this separately declared combined comparison.

E02b correctness follow-up: 642 Cashu release tests passed (two existing ignored),
40 DKG-filtered common release tests passed, strict Cashu federation library Clippy
passed, federation-enabled FFI compilation passed, SDK receiver release build and
workspace formatting passed. Five/full-denomination, eleven/one-denomination and
twenty/one-denomination full in-memory BLS consistency cases also passed. Commands
match E02a, with `cargo check --locked -p cdk-ffi --features federation` added.

Disposition: keep E02b for its measured work reduction and repeatable incremental
and combined latency/CPU gains. No confirmed metric exceeds the regression bound;
RSS intervals include no change. Neither arithmetic edit runs on the ordinary
non-federation path. Final full-service DKG, transport, runtime/recovery and
candidate payment gates remain required before campaign completion.

### Payment directions and baseline restart preparation

Fresh original-release pilots independently confirmed outgoing Bark Lightning
(1,000 sat; recorded fee 30 sat), incoming BDK on-chain (50,000 sat issued), and
outgoing BDK on-chain (1,000 sat received with the required confirmation; recorded
fee 285 sat). The miner used a fixed five-second schedule, independently of the
blocking CLI calls; its observations and transaction/receipt identities are private.
Pilot CLI durations were 6,101.51 / 12,393.75 / 36,918.10 ms respectively. These
single samples include process costs and block waiting and are not acceptance
comparisons. Signed status subsequently showed all five members ready and converged
at operation 68 with no alerts.

Conservation: 4,096 + 50,000 − 1,000 − 30 − 1,000 − 285 = 51,781 sat.
Observed balances were 51,269 sat in the sender, 288 sat in SDK recipients, and
224 sat in seven never-submitted token fixtures. No failed or uncertain external
payment was retried. [All ecash pilot attempts](benchmarks/2026-09-08-ecash-driver-pilots.csv)
are retained without token data.

Existing Prometheus phase totals suggest waiting dominates the BDK outgoing pilot:
fee estimation and PSBT construction took about 2 ms and 1 ms; FROST commitment
and signature rounds about 56 ms and 37 ms. These nested aggregate spans must not
be summed as wall time. Existing phase traces were hidden by the default file
writer level despite the target filter. Five campaign members shut down gracefully
in 210 ms to enable diagnostic file tracing. An initial config document used the
wrong logging section and was rejected before storage mutation; the corrected
`info.logging` document was accepted and the same member state restarted. The
first status request was premature; readiness is being rechecked. No original
fixture data or unrelated service was removed.

## E03a — Measure broadcaster scheduling waits

Registered before the next production experiment. Owning layer: federation BDK
transaction broadcaster and terminal application publication. A fresh traced
outgoing payment after restart took 37,926 ms and was independently received.
The selected signer spent 63 ms in the commitment round and 43 ms in the signature
round; exact transaction broadcast RPCs took 0.8–7.2 ms across members. Proposal
preparation was 1,491 ms and transaction finalization 1,189 ms, including their
nested consensus waits. These observations do not justify changing cryptography,
confirmations or durability. The sampling profile contains many parked runtime
workers; parked-thread samples are not summed as CPU utilization.

Code inspection shows the broadcaster checks at a five-second interval, including
between accepted signed bytes and accepted broadcast authorization. Hypothesis:
notify its existing worker only after applied `TransactionSigned` and
`TransactionBroadcastIntent` entries. Coalesce notifications, preserve periodic
repair, exact-byte checks, independent validation and every retry deadline. Do not
wake it for unrelated ecash or Lightning operations. No timeout/queue/consensus
policy changes are proposed. First use one bounded member-local monotonic probe
from terminal application publication to the next broadcaster round. This will
measure the suspected queue wait before implementing a wakeup. The probe and its
logs remain temporary and outside acceptance runs.

The temporary monotonic probe measured **4,111.06 ms** from signed-state
publication to the next broadcaster round and **4,235.53 ms** from authorization
publication to the following round on the instrumented member. Each sample is a
single process's `Instant` duration; no cross-machine or wall-clock subtraction
was used. They cover successive causal stages, not overlapping work. The diagnostic
member differed from the original release only by the retained DKG arithmetic
(which is outside the warm payment path) and this probe. No performance acceptance
claim uses the instrumented run.

This supports testing one focused change: an `Arc<Notify>` owned by the existing
broadcaster, signaled by terminal publication only for applied signed/authorization
entries. One stored permit coalesces bursts and survives publication during an
in-flight round. The five-second periodic repair timer and all local retry gates
remain. Primary metric: SDK end-to-end outgoing on-chain latency, with broadcast
and post-confirmation phases separated from an independent fixed block schedule.
Predicted counter change: publication-to-round waiting falls without increasing
initiations, accepted transaction identities, external broadcasts per required
observation, or idle wakeups. Matched latency/CPU/memory and adversarial/recovery
checks must pass before keeping the change. No keep decision yet.

The diagnostic payment completed in 33,259.65 ms with an independent receipt.
The diagnostic-only executable was removed from the live fixture, all original
baseline binaries and logging settings were restored, and signed readiness/
convergence passed again. Temporary probe fields and callbacks are absent from
production source. The current unaccepted candidate adds only the coalesced
broadcaster notification and its terminal-publication call.

A separate serial persistent SDK melt example now reads an existing prepared
private wallet, performs recovery/connection preparation outside its window,
records quote and saga identities before confirmation, and stops initiating work
after any uncertain result. Its on-chain pilot confirmed an independent 1,000-sat
receipt and exact 1,285-sat wallet debit. The measured operation used ten proofs,
no preparatory swap, 916 ms to quote readiness and 35,644 ms to completion, with
779 ms of separately recorded setup. This is driver validation, not acceptance
performance; independent fixture construction overlapped the pilot.

For E03a's real comparison, both arms will use fresh independent federations and
matched funding/holdings rather than the earlier diagnostic fixture's history.
The new candidate completed five-member, full-denomination HTTP DKG. The fresh
comparison baseline is starting. Funding and all builds remain outside measured
payment windows. No nonce or custody state is copied between arms.

### E03a matched-run manifest

Both fresh five-member HTTP federations completed full-denomination setup and
started with 13 accepted operations per member. Each received 131,072 sat from the
same two-output Bitcoin transaction, issued as 2,048-sat target proofs. The driver
binary is identical across arms and retains a client/wallet across five sequential
1,000-sat on-chain payments per run. Five pairs alternate baseline/candidate and
candidate/baseline order. Actual proof selection, fee, history and receipt records
are retained for every run. The window is a bounded fixed-work sequence: it ends
when work completes, with a five-minute maximum and separately classified drain.
Observed fixed-work throughput is not automatically sustainable throughput.

Compilation and funding finished before the pairs. Temporary diagnostic traces
and binaries are absent. Existing Prometheus snapshots bracket each run; process
CPU/RSS is sampled once per second. A separate miner follows a five-second fixed
schedule. An external observer records mempool, confirmation and private completion
file observations on its own monotonic clock (500 ms mempool / 100 ms file polling).
Those observation bounds must accompany external phase conclusions; they are not
subtracted from unrelated SDK timestamps. A 30-second post-run idle sample follows
receipt, metrics and convergence checks. Every run checks independent 1,000-sat receipts,
exact wallet debit, and within-run signed member digest agreement. The harness
stops before further initiation on any failure, ambiguous outcome, monitor failure
or history mismatch. Raw manifests and source/binary fingerprints are private.
An automatic permission-review timeout occurred before execution; the explicitly
permitted single retry succeeded and did not duplicate any payment.

### E03a first pair and fixture correction

The first pair completed all ten payments with independent receipts, exact wallet
debits and within-run member agreement. Per-payment milliseconds, in request order:

- Baseline: 37,330.43; 34,243.47; 37,082.62; 36,557.75; 34,341.38.
- Candidate: 21,041.18; 19,181.92; 37,067.23; 18,352.53; 19,679.12.

Aggregate member CPU/payment was 5.384 versus 4.192 seconds; post-run 30-second
idle CPU was 1.48 versus 1.44 seconds. These are one pair, not an acceptance result.
The candidate's long observation remains included. Selected proof counts matched
exactly: 0, 9, 9, 9, 11, with three proofs requiring a preparatory swap on the first
request and no preparatory swap on subsequent requests.

The harness stopped before pair two because histories ended at 169 versus 159
entries. A read-only journal audit found identical counts for every operation type
except chain observations of wallet change. Both arms recorded all five members'
confirmed observations for every change output. Baseline additionally recorded 25
unconfirmed observations; candidate recorded 15, because some observations first
saw an already-confirmed output. No confirmation or payment evidence was omitted.

Subsequent pairs therefore use fresh independent federations, matched funding and
13 accepted entries before each arm. The first pair already meets that policy and
is retained. Completed comparison fixtures are stopped with bounded graceful
shutdown and their complete databases/keys are preserved. Each new pair generates
its own custody and nonce state. Only the active comparison pair and the same
diagnostic federation remain running, keeping background service count matched.
No production/build change is made during comparisons. The initial result schema's
missing `higher_is_better: []` field was corrected without changing numeric samples.

External polling missed mempool visibility for two fast candidate transactions;
their broadcast-to-confirmation durations are unavailable, not zero. All five
confirmations and completions were observed. Preliminary external observations
also show substantial post-confirmation waiting, so the broadcaster notification
does not explain the complete outgoing path. Further attribution remains required.

The second pair reversed execution order and again confirmed all ten payments and
exact balances. Candidate milliseconds: 40,361.46; 35,013.60; 18,761.41; 19,589.72;
20,489.95. Baseline: 36,941.98; 35,681.14; 38,679.11; 34,111.72; 33,302.31.
Member CPU/payment was 4.524 versus 5.298 seconds; idle CPU was 1.59 versus 1.49
seconds. No latency or idle acceptance conclusion is drawn from two pairs.

Resource-accounting limitation: process samples bracket the complete SDK process,
including its separately timed wallet opening/recovery/connection preparation.
Thus CPU/payment is whole-driver member CPU, not a measurement restricted exactly
to the steady-state SDK window. DKG, fixture creation, funding and compilation are
outside both CPU and latency measurements. The same frozen SDK executable serves
both arms; later benchmark-source improvements do not change that executable.

### E03a fee-estimator drift invalidates the initial acceptance series

The actual-shape audit found an uncontrolled fixture input. Bitcoin's estimate
changed during pair three: baseline paid four 285-sat fees and one 428-sat fee;
candidate paid five 428-sat fees. The reserve changed from 3,478 to 4,967 sat,
causing additional preparatory swaps and different proof selections. Matching
configuration and initial funding alone did not keep fee-dependent work matched.
Pair five also crossed a fee-estimate boundary (428 to 569 sat). Pair four matched
the higher fee in both arms, but its candidate post-run CPU
sample was also anomalous: 3.97 aggregate CPU seconds in 30 seconds versus 1.38
for baseline, distributed across all five members. Thermal telemetry was
unavailable, so no environmental explanation is established.

The initial five-pair series is retained as investigative evidence, not accepted
performance evidence. In particular, pair three cannot enter a matched-workload
confidence interval. No sample is deleted or relabeled as successful acceptance.
The candidate remains uncommitted. Before repeating the series:

- Use a private regtest RPC fixture that supplies a fixed 2 sat/vB smart-fee
  estimate and forwards all other Bitcoin RPCs unchanged. This controls an
  external test input; it changes no production fee calculation, signer check,
  confirmation policy, durability or transaction construction.
- Persist and check the expected 3,478-sat quote reserve before initiating each
  payment. Verify actual fees, proof counts and preparatory swaps across arms.
- Measure settled idle CPU after a full minute of quiescence, separately from the
  initial series' immediate post-payment CPU. Retain both kinds of evidence.
- Keep fresh independent state per pair, fixed mining cadence, alternating order,
  release server artifacts and independent receipt/value checks.

Fee-estimation service latency through this controlled fixture is not a measurement
of real external fee-estimator performance. The experiment targets waiting after
accepted signing/authorization; fee-estimation optimization is not claimed.

All 50 initial payments have independent receipts, matching wallet debits and
within-run member agreement. The
[complete numeric ledger](benchmarks/2026-09-08-bdk-broadcast-uncontrolled.csv)
includes every request, actual fees/reserves, proof shape, CPU, sampled RSS and
history. The raw unchecked interval output is preserved privately and explicitly
marked invalid for acceptance. The analyzer now rejects this actual series because
observed inputs differ within pairs three and five. Its two new guard tests plus
the existing seven evidence tests pass.

### Persistent driver validation and controlled-series start

Pinned Nix release commands completed successfully:

```sh
cargo fmt --all
cargo build --locked --release -p cdk --no-default-features \
  --features wallet,federation,iroh \
  --example federation_receive_performance --example federation_melt_performance
cargo clippy --locked --release -p cdk --no-default-features \
  --features wallet,federation,iroh \
  --example federation_receive_performance --example federation_melt_performance \
  -- -D warnings
python3 -m unittest discover -s scripts -p test_analyze_federation_performance.py
```

The expanded incoming SDK driver completed two serial Lightning receives and two
serial on-chain receives against the original five-member HTTP runtime. Each
direction issued 4,096 sat into independent fresh wallets, matching independent
payer/deposit receipts exactly. Functional-pilot client milliseconds were
7,705.83 / 7,330.06 for Lightning and 11,785.26 / 10,600.98 for on-chain. These
validate the driver, not a baseline/candidate improvement. The on-chain miner ran
independently every five seconds. No payment identity was retried.

The private controlled-fee fixture was checked against real regtest chain data and
two fee-estimate targets. The replacement five-pair series has started with the
same frozen server artifacts, a newly frozen common SDK driver and fresh custody
state for every arm. It records matching observed inputs, checks quote reserves
before confirmation, and waits 60 seconds before a separate 30-second idle sample.

## E03b — Prospective Bark incoming contention investigation

No production change. Inspection found a member-wide incoming lock held across
`advance_federated_bark_mint_quote_inner`, invoice preparation/recovery and backend
HTLC waits. Existing projection timers start after lock acquisition, so they do
not attribute time waiting for this lock. The same paths also load a complete
Bark projection to find one quote/action. A parallel incoming workload must first
measure lock acquisition/hold durations and decoded rows at matched history.

If measured, consider the existing CDK per-quote weak-lock-map pattern and existing
action-scoped storage interfaces. Before narrowing ownership, audit whether the
backend requires cross-action serialization for derivation, custody state,
evidence binding or recovery. Per-quote observer claims alone do not prove that
RPC evidence-delivery and quote-creation callers share the same exclusion boundary.
No lock removal, cache or concurrent backend mutation is authorized by this
inspection alone; acceptance requires the campaign's measurements and safety gates.

Further ownership inspection found that `fresh_intent` and incoming claim planning
select the next custody locator from consensus derivation allocations under that
same lock. A blanket per-quote-lock replacement is therefore not an acceptable
first experiment. Any narrowed critical section must preserve allocation and
accepted-plan serialization while measuring whether backend waits can safely sit
outside it. The existing backend wait is bounded at four seconds; that bound alone
is not evidence that a particular request waited four seconds for the lock.

The existing `federation_receive_performance` driver now accepts
either token fixtures or `mint_requests` (method `bolt11`/`onchain`, `amount_sat`).
It reuses the same persistent connectors, independent/shared wallets, scheduler,
bounded drain and completion accounting. Mint quotes are durably stored before
their requests are exposed in private files to an independent payer. It records
quote-readiness offsets and actual issued proof counts. No funding is performed
inside the SDK driver. A separate private payer records its own causal timestamps
and initiates each invoice/deposit only once. Compilation, strict lint and both serial incoming pilots passed before the
controlled paired measurements started, as recorded above. An optional tracing
flag is being added for diagnostic runs; those traced runs cannot replace
probe-free acceptance samples. This support change is not a production optimization.

## E03c — Prospective post-confirmation notification investigation

No production change. The first two E03a pairs place the long candidate observations
after independent Bitcoin confirmation: about 21–26 seconds versus roughly 5–7
seconds on their faster requests. These are external observation durations, not
an attribution to a specific internal worker.

Inspection found that `payment_wakes_for_entry` does not map transaction broadcast
observations to their melt quotes. The payment scheduler retains exponential
external-evidence backoff, while the broadcaster independently publishes confirmed
observations. Hypothesis: accepted confirmation evidence can arrive between probes
without waking the associated pending quote. Next diagnostic: correlate the
required accepted confirmation state with the next quote probe and its durable
schedule on one member clock. Keep the existing timeouts, leases and repair timers.
Only if that gap is measured should a narrowly scoped notification be tested.

A transaction observation contains a proposal ID, not quote IDs. Resolving affected
quotes must preserve existing ownership, avoid database I/O in terminal publication
and avoid an unrelated-history read for each notification. An extra generic event
framework or speculative cache would not be justified. Delayed/lost notification,
duplicate request and restart tests remain mandatory for any candidate here.


### E03a precision and restart follow-up prepared

The first four controlled pairs completed without input-shape mismatches; all
payments have independent recipient receipts and exact SDK wallet debits. The
fifth pair is still running. One candidate again showed elevated settled CPU,
so no on-chain candidate is accepted yet.

A private native process sampler was validated against the installed SDK ABI,
Python process CPU time and the existing process sampler. Initial validation
caught CPU counters expressed in platform clock ticks; conversion with the
reported clock timebase now passes. Raw native counters must not be mislabeled
as nanoseconds. This profiler is external to production code.

After the payment series, restart the preserved completed fixtures one pair at
a time, allow 60 seconds to settle, and sample each arm for 60 seconds with
alternating order. This covers a full repair interval. Record precise CPU,
retired instructions, cycles, hardware interrupt/package-idle wakeups, disk
bytes and sampled memory, together with collector overhead. Hardware wakeup
counts are not a count of all async task reschedules. Capture existing phase
metrics around the window, separately from its CPU bracket, and require
unchanged canonical state plus within-run member agreement. Preserve every
sample, including anomalies. These checks remain pending.

The next diagnostic build is prepared, but not applied during acceptance runs:
256 bounded incoming lock/row probes and 512 bounded confirmation/scheduling
probes per diagnostic member. Confirmation publication and quote probes share
one monotonic clock; a proposal-to-quote mapping comes from the durable fixture
state. Scheduling probes record the already chosen retry delay without changing
it. The builder restores all temporary source edits after either success or
failure. No lock, lease, retry, evidence or authorization policy is changed.


### E03a controlled five-pair preview — later invalidated by runtime audit

All five alternating pairs completed with the frozen original/candidate servers
and common SDK artifact. Each arm started at 13 accepted operations with 64
proofs of 2,048 sat and 131,072 sat total. Every run selected the same proof
counts `[0, 9, 9, 9, 11]`, swap counts `[3, 0, 0, 0, 0]`, quote reserves of
3,478 sat and fees of 285 sat. No request failed, timed out or remained
unfinished. The independent chain audit verified 50 recipient outputs totaling
50,000 sat and actual Bitcoin fees totaling 14,250 sat. Exact wallet debits and
within-run member digests agree. The initial uncontrolled 50-payment series also
passed an independent chain fee audit: 19,096 sat actual fees; this does not
repair its input mismatches or make it acceptance evidence.

The following unchecked preview is preserved for the investigation. The later
runtime audit below invalidates it for acceptance. Geometric means of run metrics
and paired 95% intervals:

| Metric | Baseline | Candidate | Candidate/baseline interval |
|---|---:|---:|---:|
| Client p50 | 36.850 s | 25.307 s | 0.4775–0.9877 |
| Client p95 | 38.596 s | 37.408 s | 0.8913–1.0540 |
| Member CPU/payment, including SDK preparation | 5.302 s | 4.900 s | 0.7582–1.1265 |
| Sampled peak aggregate RSS | 492.917 MB | 507.636 MB | 0.9603–1.1044 |
| Post-payment 30-second aggregate member CPU | 1.499 s | 2.217 s | 0.7976–2.7433 |

These are run-level intervals; the five transactions within each run are not
independent replicates. Five observations do not support p99, and each run's
nearest-rank p95 is its maximum. Candidate idle CPU was elevated in pairs three
and five (3.69 / 3.95 s), and RSS was also higher in those runs. Preserve these
observations. Median latency improved, but the guard metrics remain inconclusive;
the candidate is still provisional. Diagnose idle work, then expand comparisons
if no confirmed rejection is found. The
[complete controlled numeric ledger](benchmarks/2026-09-08-bdk-broadcast-controlled.csv)
includes all 50 samples. Missing external observation boundaries remain blank;
their absence is not zero latency. External phase differences use one controller
clock; SDK latency uses its own monotonic clock.


## E05 — Repeated checkpoint-summary configuration digest hypothesis

The idle stack profiles did not capture the earlier CPU spike. They did capture
`FederationCheckpoint::validate_against_validated_config` calling
`FederationConfig::config_digest_for_validated_config` beneath checkpoint-summary
responses in both revisions. Inspection shows that this serializes the entire
public configuration and hashes it for each summary. The immutable
`MintFederationRuntimeConfig` already stores the same validated configuration's
digest. No new cache is needed if these callers can safely reuse it.

Hypothesis: reusing that existing digest in checkpoint-summary validation removes
repeated serialization, hashing and allocation without skipping shape, federation
binding, digest equality or roster checks. Owning layer: federation checkpoint
validation and Axum summary construction. Predicted counter change: zero full
configuration reserializations caused by summary validation after construction.
Primary metric: settled member CPU at matched history; guard metrics: active
payment latency/CPU, memory and recovery. First add bounded digest-duration and
byte-count probes to the diagnostic build and measure the existing path. Do not
change production behavior before those measurements. Existing ingress/startup
verification and full-proof verification remain required.


E03c ownership follow-up: the mint already has a federation-only payment-event
channel carrying the established `payment::Event` type. The observer treats a
successful-payment event as a prompt to independently probe accepted state.
Transaction-observation application already loads the transaction projection and
knows when its required confirmation settlement becomes complete. If the timing
probe establishes the gap, consider emitting the existing successful-payment
event for affected quotes **after the transaction commits**, using the already
loaded authoritative receipts. This avoids another event channel, lookup cache
or unrelated-history read in terminal response publication. Emit only on the
transition into confirmed settlement; retain periodic recovery for lost events.
This remains a hypothesis, with rollback, duplicate, lost-event and restart tests
required before acceptance.


### Starting-runtime audit invalidates E03a controlled acceptance

The complete status snapshots expose a confound missed by the original fixture
readiness check. Before the controlled payment window, both arms had empty
queues in pairs one, two and four. In pairs three and five, all candidate members
already had one item pending finalization; all baseline members had zero. The
same condition remained in the corresponding elevated post-payment CPU samples.
The starting proof/fee shapes matched, but starting consensus work did not.

Added the observed pending-item counts, pending-finalization counts, live-session
flags and session numbers to the existing matched-input dictionary. The analyzer
now rejects the controlled series as well. Its earlier unchecked interval output
and records are preserved privately. The numeric ledger exposes these starting
conditions explicitly. **No E03a latency, CPU or memory gain is accepted from
either payment series.** All independent receipt, fee and wallet-value audits
remain valid functional evidence.

The native restart follow-up completed four pairs and restarted pair five before
its controller was stopped. No fixture database or service was removed. Most
restarted fixtures had one pending finalization item; candidate pair three had
none and used 82 aggregate active network pump cycles versus the baseline's
1,525 during the metric bracket. Their native CPU rates were 0.0689 versus
0.1409 aggregate member CPU seconds/second. This is diagnostic evidence of
different pending work, not an idle performance comparison. Both reported live
sessions and matching within-run digests. Readiness alone is insufficient for a
quiescent benchmark. New acceptance windows must verify empty queues and pending
finalization, plus matched session/process conditions, before initiation.

## E06 — Leftover consensus finalization work

The pending-work condition appears in the original revision after restart as
well as in candidate fixtures. SQLite has no pending payment-operation rows in
the inspected affected fixture, so the item may be an internal recovery barrier.
Inspection found that pending-decision cleanup recognizes operation identities
and conflict keys, while recovery barriers use separate coverage semantics.
This is a hypothesis, not yet a diagnosed cleanup bug.

A bounded diagnostic probe will report pending operation/barrier/module-fact
counts and whether an already accepted retained recovery barrier covers a pending
barrier under the existing coverage predicate. It runs only every 50 calls for
the first 5,120 calls and logs scalar counts/cursors. No item is removed or
requeued by the probe. Owning layer: federation consensus pending bookkeeping.
Primary metric if a bug is established: CPU/wakeups with no user work, with exact
pending-item counts and independent replay/finalization tests. Do not clear a
pending item merely because a timer expired or the log advanced; authorization,
barrier coverage and actual terminal decisions must justify cleanup.


### E06 narrow reproducer and provisional cleanup

The focused reproducer fails on the current implementation: after a peer
finalizes an identical recovery barrier, the local pending-finalization count
remains one instead of zero. A provisional change extends the existing pending
operation-decision cleanup to accepted **identical barrier generations**. It
releases only local bookkeeping; it does not remove journal records, mutate
unit payloads, accept rejected barriers or infer completion from a timer.

The complete consensus module passes: 55 tests, including the new peer-decision
reproducer, late local-unit validation, preservation of unrelated module facts
and different generations, and rejection of premature peer barriers. The
reproducer's original failing output is retained. Commands in the pinned shell:

```sh
cargo test --locked -p cdk-common --no-default-features --features federation   --lib peer_finalization_releases_equivalent_local_recovery_barrier
cargo fmt --all
cargo test --locked -p cdk-common --no-default-features --features federation   --lib federation::consensus::tests
```

This proves a bookkeeping defect, but not yet that it accounts for the live
fixture's leftover item. Restarting only the diagnostic member cleared that
member's pending count while four untouched members retained one; that first
probe therefore did not capture the offending item. A simultaneous five-member
restart with the frozen **pre-fix** probe is now running to identify it.
No E06 performance acceptance or commit has occurred.

### E05 digest cost measured

The first diagnostic member captured 112 full configuration digest calls across
startup/readiness and the following 20-second observation. Serialized public
configurations were 366,856–372,706 bytes. Median method duration was 1,147.021 μs,
maximum 4,012.917 μs, and summed non-overlapping method work was 191.293 ms.
These are bounded method timings, not end-to-end phase latency or an idle
comparison. Together with the checkpoint-summary stack attribution, they justify
testing reuse of the already stored immutable configuration digest. No E05
production change has been made yet.

The pre-fix simultaneous diagnostic restart did not reproduce pending work. Four
members became ready; one remained awaiting quorum after the bounded readiness
attempt, even though operation, application, materialized-state and persisted
finalized-item digests agreed across all members. All reported the same session.
No payments were initiated in this state. Logs and databases are preserved, and
all five diagnostic members are being restored to the original baseline binary.
This is a separate recovery investigation; it neither accepts nor disproves the
narrow equivalent-barrier cleanup. All temporary probes are removed from source.

The restoration completed successfully: all five original baseline members
became ready and converged. The diagnostic restart failure remains recorded.

### E03b four concurrent Bark receives: incomplete workload

Workload: five original-revision HTTP members, with only the receive actor using
the frozen pre-fix bounded lock probe; four independent persistent SDK wallets,
four 2,048-sat invoices, concurrency four, 60-second measurement window and
120-second drain. Preparation and external payer initiation use the existing
incoming SDK driver and private one-shot payer helper. No compilation overlapped
the payment window. This was attribution/driver validation, not a matched pair.

Two requests completed with independent payer receipts and 2,048 sat in each SDK
wallet. Two remained pending after drain; their payer subprocess timeouts did not
cancel the Lightning payments. The original invoice, quote, wallet and payer
identities are retained for reconciliation. No replacement payment was initiated.
All five members agree: two actions are `receive_claimed`, two `receive_prepared`.

Read-only independent checks found the two pending HTLCs accepted at the receiving
Lightning node. Ark's structured log records preparation of HTLC packages for all
four requests, but claims only for the two successful requests. Other members
have stored ready HTLC packages for both pending actions; the actor retains one
ready package and one awaiting-payment checkpoint. This narrows the investigation
to local evidence/recovery progress, rather than absent Lightning delivery.
Invoice hashes match the durable action hashes. No preimage or invoice is
included in this journal; raw evidence stays private.

The bounded actor probe observed a maximum lock wait of 3,292.789 ms and maximum
hold of 1,590.823 ms. These demonstrate contention, but the failed workload cannot
support an optimization acceptance or throughput claim. No lock-scope production
change has been made. The actor is being restored to the original binary with
targeted debug logging to inspect the existing recovery decisions. The SDK's two
pending attempts must be reconciled, not restarted as new payments.

## E07 — Session-scoped replay targets

The actor's first original-binary restart remained awaiting quorum for the full
180-second readiness bound. Its live session had advanced, while its application
frontier agreed with the four healthy peers. Inspection found that
`quorum_supported_replay_target` retains the old local session's cursor as both a
replay floor and a quorum vote when selecting a later session. Unit rounds reset
at the session boundary, so these old rounds have no meaning in the new session.
A subsequent same-session restart with the original binary restored readiness
and convergence. This supports the diagnosis but is not candidate acceptance.

The new focused test fails on the original helper. It checks both the obsolete
replay floor and the prohibition on counting an old local cursor toward a new
session's supported watermark. A provisional fix uses zero local watermarks
when the target session differs; same-session durable floors remain unchanged.
The existing `quorum_` tests are running. No production binary with this fix has
been used and no performance claim is made.

The initial minimal `federation` test command failed to compile existing
BDK-specific tests without their feature. The reproducer was rerun with
`federation-bdk,federation-bark,prometheus,iroh` and failed at its intended
assertion. This feature-matrix failure remains an outstanding gate. E06's scoped
strict `cdk-common` Clippy check passed.

## E08 — Receive evidence routing after worker-lease rotation

Hypothesis recorded before editing: incoming HTLC evidence uses inconsistent
collector roots after the replaceable worker lease changes. The incoming path
selects its collector and transition builder from the current receive worker,
but passes the original intent actor to certificate collection. The evidence
delivery wake also derives its fallback offset from the original actor.

In the incomplete four-payment workload, pending intents originated at journal
indices 203 and 206, with original actor 1 and current operation count 225.
The existing eight-item lease policy therefore selects member 3 for both.
Evidence collection still routes to member 1. This explains how every member can
retain valid HTLC evidence while the selected transition builder lacks the
matching certificate. The two later requests completed before this became a
permanent wait in their workflows.

Owning layer: Axum's incoming evidence routing. Candidate: use the current receive
worker consistently for `ReceiveHtlcsReady` collection and its delivery wake;
leave immutable action identity, signing actor, claim plan, evidence bindings,
thresholds and external initiation unchanged. Expected work reduction: eliminate
repeated unsuccessful collection/wake attempts after a lease transition. Primary
outcome: the existing pending identities complete through normal authorized
recovery, followed by fresh serial/concurrent payment comparisons. No evidence,
preimage, nonce, quote or backend payment state may be edited to force completion.

### E08 original payments recovered without reinitiation

The isolated release candidate contains only E08 on top of committed work; E03a,
E06 and E07 were excluded during compilation, then their workspace changes were
restored. It contains no temporary probes. Existing Bark tests pass: 19. All five
original members were stopped within the shutdown bound and restarted against
their existing databases with this candidate. All became ready and converged.

Normal workers advanced both pending actions to `receive_claimed` on every member.
Independent payer `listsendpays` records show exactly one completed 2,048,000-msat
part per original invoice. A temporary SDK recovery helper reopened the original
wallet databases and 64-byte seeds, validated the saved quote/invoice identities,
and used the ordinary proof stream for those existing funded quotes. Each wallet
issued 2,048 sat and ended with a 2,048-sat spendable balance. Its temporary source
was removed from the workspace after compilation and retained privately with the
binary and receipts. It cannot create quotes or initiate backend payments.

Together with the two original successes, all four original requests now have
independent settlement and 8,192 sat of SDK receipts. The two recoveries occurred
after the original drain; they are **late reconciliations**, not completions in
that measurement window. No matched E08 performance acceptance has occurred.
A fresh four-concurrent candidate receive run is now testing normal completion.

Pinned-shell commands for the focused checks and candidate build:

```sh
cargo test --locked -p cdk-axum --no-default-features \
  --features federation-bdk,federation-bark,prometheus,iroh --lib quorum_
cargo test --locked -p cdk-axum --no-default-features \
  --features federation-bdk,federation-bark,prometheus,iroh --lib bark_
cargo build --locked --release -p cdk-mintd --no-default-features \
  --features fakewallet,sqlite,info-page,federation-bdk,federation-bark,prometheus,iroh
```

Results: 18 quorum tests and 19 Bark tests pass; isolated release build passes.
Raw commands with local fixture arguments, frozen source snapshots, fingerprints,
receipts and restart logs remain private. E07 has not been deployed in this run.

The fresh E08 concurrent batch completed all four receives with independent payer
receipts, 8,192 sat in four SDK wallets, and converged member state. Each issuance
contained 30 proofs. End-to-end durations by request were 70,180.628, 12,731.816,
13,955.634 and 21,914.253 ms; external payer completion intervals were 64,337.408,
4,182.666, 4,514.568 and 12,385.458 ms after the payer observed each invoice.
The first request completed during drain, outside the 60-second window. These
intervals use their own monotonic clocks; payer intervals are not subtracted from
unrelated SDK timestamps. They do not establish a paired speedup or sustainable
throughput. The remaining approximately one-minute delay warrants targeted
collector/scheduling attribution before accepting E08 performance.

Scoped strict Axum lint passed with the payment/transport feature combination:
`cargo clippy --locked -p cdk-axum --no-default-features --features
federation-bdk,federation-bark,prometheus,iroh --lib --tests -- -D warnings`.

The next isolated E08 diagnostic adds at most 512 routing/schedule events per
member, recording local monotonic elapsed durations, action correlations, worker
and collector member numbers, fallback rounds, attestation counts and scheduled
delays. It records no payment secrets or invoices. The first build-helper attempt
stopped at an insertion assertion before compilation and restored source; the
corrected build passed and also restored source. No probe is retained in source.
The two completed on-chain comparison federations were stopped without removing
databases before this profile. Its four-receive workload is diagnostic only.

### E08b — Stable evidence collection across preparation-worker leases

The routing probe reproduced two long receives: 69,294.707 and 67,258.415 ms;
the other two completed in 13,465.573 and 10,187.869 ms. All four independently
settled and delivered 8,192 sat. The long requests completed after the window.
On one request, members 2 and 3 retained attestations and scheduled federation
repair for 60,000 ms; meanwhile the preparation lease moved from member 1 to
member 2. Member 1 then became unselected despite already holding evidence.
On another request the lease moved again while the previous collector waited for
a third attestation. Quote-specific notifications do not wake every unrelated
pending quote when other journal operations move its worker lease. Merely routing
to the current worker fixes the permanent mismatch but leaves this waiting cost.

Next hypothesis, before editing: keep the evidence collector and observer ordering
anchored to the immutable intent actor and the existing explicit fallback round.
Continue deriving the sole preparation worker from its replaceable lease, and
always allow that worker to observe/prepare even if it falls outside the selected
observer prefix. This separates external preparation ownership from evidence
collection using the existing mechanisms. No new cache, wake-all scan, channel,
timeout, queue or protocol field is required. The original actor may be unavailable;
the existing collector fallback must retain its recovery coverage.

Predicted counter change: retained attestation delivery remains useful after lease
changes, so collection no longer waits for unrelated members' 60-second repair
cycles. There may be one additional worker observer outside the threshold prefix;
measure that cost and reject a confirmed CPU regression. Independent validation,
thresholds, action identity and claim authorization remain unchanged. E08's first
routing candidate is superseded provisionally, with its samples retained; neither
version is accepted without the remaining comparative/correctness gates.

The revised observer-selection test covers every worker lease in the five-member
roster: the original threshold prefix remains selected, the current worker is
included even outside it, and existing fallback expands to the complete roster.
All 20 Bark tests pass. An isolated E08b release build is in progress. The original,
E08 and instrumented E08 request-level outcomes are retained in the
[diagnostic ledger](benchmarks/2026-09-08-bark-incoming-routing-diagnostics.csv).
Empty completion fields denote pending attempts, not zero latency.

The isolated E08b release build passed. All five members restarted ready and
converged, with diagnostic binaries replaced and custody state preserved. Its
first fresh concurrent batch completed all four requests inside the 60-second
window: 18,144.912, 27,913.224, 15,901.946 and 17,130.339 ms. Independent payer
intervals were 7,276.49, 18,843.79, 2,623.89 and 7,602.00 ms. Every wallet received
2,048 sat, and the post-run member state converged. No one-minute delay appeared
in this single run. Different starting histories and the lack of paired CPU
measurements preclude an accepted speedup claim. E08b remains provisional.

## Verification support — Minimal federation test feature combination

The earlier minimal-feature compilation failure had two test-only causes: two
BDK chain-anchor tests were not gated with `federation-bdk`, and generic FROST
signing fixtures imported the BIP32 registration crate without declaring it as a
development dependency. The two tests now use the implementation's feature gate,
and the existing fixture dependency is declared under `dev-dependencies`.
No test was deleted, no production feature was enabled, and the full-feature tests
retain their existing coverage. The minimal-feature quorum test command is being
rerun; this is verification support, not a performance optimization or gain claim.

The minimal-feature quorum command passes all 16 applicable tests. Strict lint
then found a Bark-only latency wrapper compiled without any caller in the minimal
configuration; its method now uses its sole caller's `federation-bark` gate.
Strict Clippy passes for minimal federation, the full BDK/Bark/HTTP/Iroh feature
combination, and both persistent SDK benchmark examples. The receiver's optional
phase-tracing switch defaults to disabled and affects only benchmark support.

### E08b matched ablation protocol

Before measurement: compare the stable-collector candidate against the committed
DKG improvements without any other provisional production changes. Each arm uses
an independently created Bitcoin/Lightning/Ark backend and a fresh five-member
HTTP federation. Initial balances, channel capacity, chain height, Ark policy,
zero accepted operations, session, and empty pending queues must match. Fresh
keys and transaction identifiers intentionally differ. No custody state is cloned.

The first pair offers eight independent 2,048-sat incoming Lightning requests at
closed-loop concurrency four, with a 60-second window and 120-second drain.
This finite burst measures completion latency and resource cost; it cannot
establish sustainable throughput. Independent payer records and SDK balances
verify receipts. Uncertain payments retain their original identities and are
never initiated again. Late and unfinished attempts remain separate outcomes.

The persistent SDK's optional start handshake excludes wallet preparation and
connection warming from the native resource bracket. Its bounded 10-ms signal
check runs before the measurement origin; tracing is disabled. Both arms use the
same release SDK binary. Native per-member CPU/RSS samples bracket execution;
builds and fixture funding are excluded. The ablation baseline is based on
`3b968f2f4`; the isolated candidate adds only E08b to the same production lineage.
Binary fingerprints and complete raw samples remain private. This comparison
must not be presented as the final original-baseline campaign comparison.

The first comparison is classified as a protocol pilot, not one of the five
acceptance pairs: the inactive comparison federation was still running. Future
arms will stop inactive federations and their disposable backends before timing.
The baseline completed seven of eight receives by drain, with four inside the
window; one original payment remains pending. Post-run signed status reported a
Bark recovery-required condition on one member despite agreement on accepted
operations. The controller initially treated that failed health gate as a fatal
reporting error. Raw SDK/resource samples were already persisted; a read-only
finalizer retains the failure and independent payer outcomes. Future runs retain
failed post-run health checks as outcomes rather than losing their report. No
payment command was repeated. The pilot's resource bracket is reconstructed from
its first/last native samples and is not acceptance evidence.

The candidate pilot completed eight of eight receives inside the window, with
16,384 sat in independently checked SDK wallets and matching payer settlements.
The baseline delivered 14,336 sat by drain; its one unfinished invoice is preserved
with its payer attempt records. Both fixtures and their backends were gracefully
stopped without deleting state. This asymmetric completion outcome supports the
hypothesis but is not a paired performance acceptance result.

The five acceptance pairs use newly generated fixtures, alternating arm order.
Only one campaign federation and its paired backend run at a time. Each arm starts
from independently funded, equivalent liquidity and a zero-operation/session-zero
federation with empty queues. Economic inputs are checked again immediately before
measurement. Existing operational logging is set to `info` in both arms; temporary
probes and benchmark phase tracing are disabled. Failed post-run readiness is
recorded separately. Native sampling starts only after the SDK preparation signal.
All eight attempts, including drain completions and pending outcomes, are retained.
The optional start handshake passed both pilot arms; scoped example Clippy and its
release build had passed before these runs. It changes benchmark support only.

### E06 follow-up — Classify startup work before payment comparisons

The first isolated baseline retained one pending-finalization item on member 4
with zero accepted operations. No benchmark invoice was created. The preparation
controller was interrupted before payment initiation, with the fixture preserved.
A 60-second native sample after stopping readiness polling recorded stable state:
aggregate CPU 2.223203 seconds; per-member CPU 0.449881, 0.446972, 0.441797,
0.441906 and 0.442646 seconds. The member with the pending item did not use more
CPU. Collector CPU was 0.008826 seconds for the entire sample. This does **not**
establish that the retained item causes excess idle CPU, nor validate earlier
uncontrolled comparisons.

Next attribution probe: count pending operations, module facts and recovery
barriers; separately count barriers with an identical already accepted generation.
Emit at most 32 reports per process, at least 30 seconds apart while pending work
exists. Unlike the earlier probe, empty-state status calls cannot exhaust its
report budget. No artifact contents, secrets or identities are logged. Build an
isolated baseline probe and isolated E06 candidate, removing the probe from source
before further acceptance measurements. Existing E06 tests establish the stale
bookkeeping case; live attribution and any performance effect remain to be shown.

Both isolated E06 artifacts built successfully and temporary probes were removed
from source. The first log-attachment attempt failed before any member launched
because tmux cannot pipe an exited pane. Direct private log capture corrected the
helper. Restarting the preserved unmeasured fixture with the baseline probe
cleared all pending items; signed status was healthy/converged at 35, 65 and 95
seconds with zero operations, session zero and empty queues. No pending item
survived for attribution. Thus the live item type remains unconfirmed. The E06
candidate was not deployed and no E06 gain is claimed.

The isolated incoming comparison will prepare both arms with exactly one normal
warm restart after initial readiness, before SDK setup/timing, then enforce empty
queues. This uses ordinary startup behavior rather than including an unaccepted
fix in either arm. The interrupted second fixture is retained as a diagnostic;
new independent fixtures will supply the five alternating pairs. Initial and
post-restart signed status are both retained. No additional restart is attempted
if this preparation protocol fails its gate.

The first revised isolated pair passed both pre-payment gates after one matched
warm restart. Candidate: eight window completions, no late or pending attempts,
16,384 sat in recipients. Baseline: six window completions, one drain completion,
one pending attempt, 14,336 sat in recipients. Full observation CPU was 25.609199
seconds for the candidate and 28.555962 seconds for the baseline; observation
lengths differ because the baseline exhausted drain. These are individual samples,
not accepted gains. The remaining pairs continue with alternating execution order.

Analysis retains fixed-window native CPU separately from whole-burst/drain CPU,
with the actual native endpoint error recorded. Unconditional burst percentiles
are omitted if any planned request was not offered or did not complete; conditional
completed-only percentiles remain explicitly labelled. Zero-valued count metrics
now retain paired absolute confidence intervals without inventing a percentage
ratio or passing a percentage regression gate. All ten analyzer tests pass.

The next baseline retained one pending-finalization item on member 4 after its
single warm restart, again with zero accepted operations and all other members'
queues empty. No invoice was initiated in this arm. A warm restart is therefore
not a reliable general preparation remedy. The series is held at its gate while
fresh-start attribution is prepared; the completed pair remains recorded, without
acceptance from a one-pair sample. The new probe will observe a fresh ceremony's
runtime directly rather than attempting to reconstruct its pending bookkeeping
by restarting it. Source inspection also confirms that nonzero pending items block
explicit consensus-session rotation; a recovery effect is a hypothesis to verify,
not an inferred CPU gain.

Local automatic approval review timed out on a read-only pane inspection; existing
readiness logs provided that information instead. It also timed out before executing
the controller-stop request. The explicitly allowed single retry is pending. No
permission timeout is classified as an unsafe action or as a completed command.

The allowed controller-stop retry also timed out in automatic approval review;
no process-control command ran. Explicit approval to retry was requested. The
benchmark's pre-payment gate continues to hold the next arm; completed artifacts
and all pending invoice identities remain preserved. All outcomes from the one
completed isolated pair are exported to the partial numeric ledger. The fresh-start
probe and isolated E06 candidate are built; the fresh-start probe has not launched.
The observer-selection test was extended to include a wrapping roster anchored at
member 4; that test-only extension awaits the next scoped Rust test run.

### Continuation after process-control approval

The continuation authorized retrying local process control. Inspection found the
benchmark controllers had already exited; the five members of the unmeasured
startup fixture remained running. They were gracefully stopped with all databases
and backend state preserved. A fresh independent federation is now being created
with the bounded startup probe, sharing only the prepared test backend with the
stopped, unpaid diagnostic fixture. No live custody state is cloned.

Delivery policy: commit each successful optimization immediately after its
measurement/correctness gates pass and the keep decision is recorded. Provisional
production experiments remain uncommitted.

Fresh-start attribution confirmed E06: member 4 retained one recovery barrier with
an identical already accepted generation at finalized cursor 4. Reports at roughly
31, 61, 91 and 122 seconds retained the same classification; operations and module
facts were zero. This is now a reproduced live bookkeeping defect, not an inferred
explanation from a status count.

Before the next experiment: replay the corresponding public consensus trace in an
isolated release component harness. Dispatch the local barrier, finalize matching
barriers from the four peers, then perform the existing session-recovery/requeue
path and drain its resulting local payload. Predicted work reduction: one redundant
barrier, its new-session payload, serialization and finalization disappear per
recovery cycle. Include peer-finalization/cleanup cost in the measured phase.
Measure five alternating independent process pairs, expanding to ten if needed.
Check the identical recorded peer prefix across revisions, and record empty-session
rotation eligibility separately. This measures consensus recovery component CPU
and latency, not payment throughput or full network restart latency. It uses public
configuration only and creates no custody state or external payments. The temporary
harness will be removed after the investigation; no production fast path is added.

E06 component results: five alternating release pairs, 20,000 prepared recovery
cycles per process. Every pair used the identical public peer-prefix digest.
Baseline requeued 20,000 redundant barriers and encoded 10,800,000 extra payload
bytes per process; candidate requeued none. Empty-session rotation was blocked
on the baseline trace and permitted on the candidate trace. All cycles drained
without pending mempool entries; the additional baseline finalized records were
exactly its redundant recovery payloads. This intentionally compares generated
recovery work; the identical recorded peer prefix agrees across revisions.

Geometric means and candidate/baseline 95% paired intervals:

- Measured recovery-cycle mean: 43.842914 → 23.826455 microseconds, 45.7% lower;
  ratio 0.54345, interval [0.51905, 0.56900].
- Whole-process CPU including preparation: 1.769237 → 1.377693 seconds, 22.1%
  lower; ratio 0.77869, interval [0.75807, 0.79987].
- Retired instructions including preparation: 42.340 → 34.048 billion, 19.6%
  fewer; ratio 0.80415, interval [0.80357, 0.80474].
- Batch-harness peak RSS ratio 0.63704, interval [0.63670, 0.63738]. The harness
  retains 20,000 independent in-memory runtimes: this is not a live-service RSS
  reduction claim. No p95/p99 claim follows from aggregated cycle durations.

Commands: build the temporary `cdk-common` recovery example with `cargo build
--locked --release -p cdk-common --no-default-features --features federation,bench
--example e06_recovery_performance`, then run each frozen binary with the same
public configuration and 20,000 cycles under `/usr/bin/time -l`. Pinned Nix/Rust
and release settings match earlier measurements. All ten numeric results are in
[the E06 ledger](benchmarks/2026-09-08-recovery-barrier-cleanup.csv); raw traces,
source snapshots, fingerprints and the temporary example are retained privately.
The temporary example was removed from the workspace after both builds.

A fresh five-member HTTP runtime using the isolated E06 candidate reached healthy,
converged state with zero operations, session zero and empty queues without an
extra restart. No payment was initiated. Its 60-second idle follow-up used
2.206824 aggregate CPU seconds; the earlier baseline diagnostic used 2.223203.
These are unpaired diagnostic samples, not evidence of an idle CPU reduction.

Final scoped checks completed:

```sh
cargo fmt --all
cargo test --locked -p cdk-common --no-default-features --features federation --lib federation::consensus::tests
cargo clippy --locked -p cdk-common --no-default-features --features federation --lib --tests -- -D warnings
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib recovery
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib aleph_bft_session::tests
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib bark_
```

Results: 55 consensus, 46 runtime recovery, 10 session and 20 Bark tests passed;
formatting and strict common-crate lint passed. The recovery filter also selected
`benchmark_unchanged_checkpoint_idle_rounds`, an ignored manual ten-minute
long-history benchmark; it was not executed. The wrapping-roster observer test
extension passed in the Bark suite. Full-feature runtime checks include the
provisional working-tree changes; E06's performance binaries and fresh-runtime
validation isolate its production change from those experiments.

**E06 decision: keep.** Five matched pairs confirm the predicted removal of a
redundant barrier and its serialization/finalization work, with repeatable recovery
component latency and CPU improvements. Live attribution reproduced the defect,
and the isolated candidate reached empty-queue readiness. No end-to-end payment,
sustainable-throughput, service-memory or idle-speedup claim follows. The temporary
probe and component example are removed from source. Commit this focused cleanup
and its evidence now; final combined campaign gates remain pending.

### E08b comparison with the accepted recovery cleanup

E06 was committed as `d880c9703`; analysis support and the first partial isolated
payment ledger were committed separately as `db39b0910`. The next comparison
includes E06 in both arms so obsolete startup work cannot confound collection
performance. Earlier pilot/partial outcomes remain recorded and are not pooled
with this new source baseline.

Predeclared experiment: five alternating pairs of fresh independent five-member
HTTP/SQLite federations, eight incoming 2,048-sat Lightning requests per arm,
closed-loop concurrency four, a 60-second window and 120-second drain. Use the
existing persistent SDK driver with controlled start, production info logging,
and no temporary probes. Each arm gets independent DKG/custody and backend state,
matched height 132, liquidity, resource limits and policy. Require healthy,
converged, empty queues before SDK preparation; do not add an extra startup or
change timeouts to make a failed preparation pass. Stop the measured members and
backend after each arm while preserving their state. Reuse no payment identity
for a new initiation. Baseline is the frozen isolated E06 build; candidate adds
only stable incoming evidence collection, excluding broadcaster and replay-cursor
experiments.

Hypothesis and predicted work: keeping the collector attached to the immutable
intent actor reduces repeated incomplete evidence collections when accepted
operations rotate the preparation worker. The current worker retains preparation
authority. Measure window completions, unconditional latency where the entire
burst completes, pending/late outcomes, CPU per receipt, RSS and phase counts.
The possible additional observer may increase backend waits: retain that cost and
reject a confirmed resource regression. This finite burst does not establish
sustainable throughput. Five paired results are required before a keep decision;
expand to ten if inconclusive. Build and fixture preparation remain outside all
transaction measurement windows.

The first E08b+E06 candidate arm completed all eight requests inside its 60-second
window: 16,384 sat independently verified, no late or unfinished attempts,
23.520179 aggregate member CPU seconds over the observation. All eight outcomes
are retained in the [partial E06-based ledger](benchmarks/2026-09-08-bark-incoming-e06-partial.csv).
The matching baseline reached healthy state but retained one pending-finalization
item on member 4, despite containing E06. No baseline payment was initiated. The
controller and preparation child were stopped at this gate; their databases and
backend state remain intact. There is no matched pair or E08b acceptance from
this series.

This does not invalidate the measured dispatched-barrier E06 trace, but shows it
does not cover every startup interleaving. Inspect the remaining case with a fresh
bounded probe built on E06 before changing production code. Distinguish an already
accepted generation dispatched after its peer decision from a different generation
that still needs a decision. Do not infer either from the pending count alone.
The probe records counts and the requested frontier, creates no payments, and will
be removed from source after building its isolated artifact.

### E06b: peer decision before local queue dispatch

The E06-based probe reproduced an identical accepted generation still pending on
member 4 at roughly 0, 31 and 61 seconds: requested frontier zero, finalized cursor
four, one barrier, no operation or module fact. Recovery proposal construction
reads the cursor and queues its barrier under the consensus lock. Combined with
E06's dispatched-entry cleanup, this points to the peer decision arriving while
the matching local barrier is still queued. Record this as the next ordering case,
not an unrelated pending generation or justification to bypass the startup gate.
The diagnostic fixture was gracefully stopped; its databases and logs are retained.

Predeclared candidate: remove only matching accepted recovery barriers from the
bounded provider queue during decision cleanup. Preserve other generations,
rejected barriers, unrelated item ordering and operation/conflict metadata. Add a
regression covering peer finalization before dispatch; first verify that it fails
on the committed implementation. Measure five alternating release component pairs
against `d880c9703`, with queueing outside the timed phase and peer-prefix
finalization, local dispatch, session recovery and drain inside it. Predicted
savings: no local payload construction, canonical encoding or recovery requeue
for the already satisfied request. Reuse the existing private component harness;
retain identical peer-prefix checks and all samples. No historical scan/cache,
wire change, timeout change or production benchmark branch is proposed.

Regression evidence: the initial assertion attempted direct equality on consensus
items, which do not implement `PartialEq`; that test-only compilation failed. After
changing the assertion to compare serialized contents, the test failed on the
committed implementation because a satisfied recovery barrier was still dispatched
with the unrelated fact. The candidate removes only accepted matching barriers
from pending groups, preserving remaining order/grouping and avoiding any change
to operation/conflict metadata. All 56 focused consensus tests then passed after
formatting. Performance acceptance remains pending the five release pairs and
fresh-runtime validation; no E06b production commit is made yet.

The first component runner stopped after its baseline sample on an incorrect
harness capability assertion: queued, undispatched work can legitimately move to
a new session. The capability check had not performed the local dispatch used by
the measured path. Its raw sample and source are retained as invalid harness
validation, not accepted performance evidence. Correct the separate capability
check to dispatch after the peer prefix before testing rotation, rebuild both
artifacts, and start a new five-pair result directory. The timed phase and predicted
work counts remain unchanged. No production change follows from this harness error.

The corrected component comparison completed all five alternating pairs, 20,000
cycles per process. Every arm finalized the same recorded public peer prefix.
Baseline emitted 20,000 initial local payloads and 20,000 recovery payloads; the
candidate emitted neither. Recovery requeues fell from 20,000 to zero and extra
canonical bytes from 21,600,000 to zero. Every cycle ended with an empty mempool.
The separate rotation check now follows local dispatch: baseline cannot rotate
without recovery; candidate has no outstanding dispatched work.

Geometric means with candidate/baseline paired 95% confidence intervals:

- Recovery-cycle mean: 61.916488 → 29.838577 microseconds, 51.8% lower;
  ratio 0.48192, interval [0.44678, 0.51982].
- Whole-process CPU including preparation: 2.244013 → 1.560330 seconds, 30.5%
  lower; ratio 0.69533, interval [0.62595, 0.77240].
- Whole-process retired instructions: 44.271 → 31.603 billion, 28.6% fewer;
  ratio 0.71385, interval [0.71289, 0.71480].
- Batch-harness peak RSS ratio: 0.57914, interval [0.57808, 0.58021]. This held
  batch of 20,000 runtimes does not establish a live-service memory improvement.

[All ten numeric samples](benchmarks/2026-09-08-queued-recovery-barrier-cleanup.csv)
are retained. Commands: pinned Nix `cargo build --locked --release -p cdk-common
--no-default-features --features federation,bench --example e06b_recovery_performance`,
then each frozen binary with the same public configuration and `20000` under
`/usr/bin/time -l`. Raw traces, fingerprints, both harness versions and the invalid
first attempt remain private. The temporary example was removed after both builds.
These results cover a different ordering case from E06; do not multiply or add
their percentages. End-to-end payment, sustainable throughput, idle and combined
campaign gains remain unestablished. Fresh-runtime validation and final scoped
checks are next before an E06b keep decision.

The isolated uninstrumented E06b candidate reached healthy, converged, empty-queue
readiness on its first startup. Its 60-second idle follow-up retained identical
state and zero pending items on all five members, using 1.804763 aggregate CPU
seconds (collector 0.006678 seconds). This is an unpaired stability diagnostic,
not an idle speedup. The fixture was stopped gracefully before final checks;
all databases and backend state are preserved.

Final commands in the pinned Nix environment:

```sh
cargo fmt --all -- --check
cargo clippy --locked -p cdk-common --no-default-features --features federation --lib --tests -- -D warnings
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib recovery
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib aleph_bft_session::tests
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib bark_
```

Formatting and strict lint passed. Runtime results: 46 recovery, 10 session and
20 Bark tests passed; the recovery filter's manual ten-minute checkpoint benchmark
was ignored. Together with the earlier 56 passing consensus tests, these complete
the scoped checks. As with E06, full-feature tests include other provisional
working-tree changes; the measured binaries and fresh runtime isolate E06b.

**E06b decision: keep.** The reproduced queue-ordering defect and failing-before,
passing-after regression agree with five paired measurements showing removal of
redundant payload construction/encoding and recovery work. Fresh-runtime startup
and idle stability passed. Commit this narrow extension and its evidence now,
without committing the provisional Lightning collector, broadcaster or replay
cursor changes. The temporary diagnostic probe and component example are absent
from production source. Next payment comparisons must include both recovery
cleanups in each arm and use fresh cases; the earlier partial series cannot be
pooled with those new binaries. The full campaign remains incomplete.

### Goal continuation: E08b on both accepted recovery cleanups

Current source is `1d1669db4`. The frozen uninstrumented recovery candidate's
consensus source equals this commit, and its recorded executable fingerprint was
verified. Its original pre-commit build metadata is retained; it is now the
E08b ablation baseline. The new candidate adds only the existing stable-collector
change. Broadcaster and replay-cursor experiments remain excluded. No previous
benchmark controller/member was running at inspection; the unpaid diagnostic
backend was stopped with all volumes retained to release its test ports.

Resume the same predeclared five-pair protocol with new independent cases: five
members, HTTP/SQLite, eight incoming 2,048-sat requests, concurrency four,
60-second window, 120-second drain, persistent SDK, warmed connections and
controlled timing start. Alternate arm order, baseline first in the first pair.
Both arms include E06 and E06b and require healthy empty-queue startup without a
preparation restart. Keep the earlier partial series separate. The older campaign
backend remains unchanged as background infrastructure; only one comparison
backend and federation runs at a time. No build, funding or fixture preparation
runs during a payment window. Preserve every unfinished identity and never
reinitiate an uncertain payment.

Interpret the current federation guide alongside the development requirements,
implementation contract and RFC: historical fakewallet-only/REST-only scope does
not override this approved campaign's real BDK/Bark and HTTP/Iroh requirements.
Their safety, standalone-compatibility and architecture contracts remain binding.
The plan's percentile sentence was clarified to match the approved request:
p50/p95 are reported normally; only p99 requires 1,000 completed samples.

The collector hypothesis and acceptance rules are unchanged: reduce incomplete
collections caused by worker rotation, retain preparation authority and fallback,
and measure the possible extra observer's CPU/backend cost. Do not accept from
completion counts alone or treat finite bursts as sustainable throughput. Expand
to ten pairs if the five-pair comparison remains inconclusive.

The first new baseline arm passed initial health with empty queues. All eight
requests were offered: six completed inside the 60-second window, one completed
during drain, and one remained unfinished. Seven independent receipts total
14,336 sat. Aggregate member CPU over the full observation was 25.210901 seconds;
this includes drain and is not the fixed-window CPU comparison. The unpaid/pending
identity is retained and has not been reinitiated. The baseline members/backend
were stopped with state preserved; the matching candidate's independent fixture
preparation then began. The five-pair controller remains live. These are baseline
outcomes only, not evidence of an accepted collector improvement.

#### New-series observations and custody-recovery investigation

The next continuation revalidated the original controller through its live
process session; no fixture or payment was restarted. The preceding measurement
turn made progress by producing matched samples. The intervening prompt-writing
turn made no campaign progress. Two pairs are now recorded; the third is running.

| Pair | Arm | Window receipts | Late receipts | Not completed | Receipt value (sat) | Whole-observation member CPU (s) |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 16 | Baseline | 6 | 1 | 1 | 14,336 | 25.210901 |
| 16 | Collector candidate | 8 | 0 | 0 | 16,384 | 25.307528 |
| 17 | Collector candidate | 6 | 0 | 2 | 12,288 | 39.246003 |
| 17 | Baseline | 6 | 1 | 1 | 14,336 | 24.973609 |

Every arm offered eight requests. These CPU totals include each arm's drain;
they are not matched fixed-window costs. For pair 16, fixed-window CPU was
18.500413 versus 25.306602 seconds, or 3.083402 versus 3.163325 CPU seconds per
window receipt (candidate approximately 2.6% higher). Peak aggregate RSS was
528,089,088 versus 530,907,136 bytes. Whole-observation pending backend waits
were 190 versus 318 and pending evidence collections 59 versus 58; differing
drain lengths prevent treating these as fixed-window counter comparisons.
Neither the completion improvement nor these costs establish acceptance from
one pair. No p99 or sustainable-throughput claim is made.

The second candidate arm had seven independently settled payer receipts but only
six SDK receipts. One settled payment remained at `receive_preimage_disclosed`;
a separate request failed during quote creation before an invoice was returned.
All five member projections converged and remained healthy with empty consensus
queues. Both failures count against this candidate. Original identities and
state are preserved; there was no repayment or replacement quote.

Read-only SQLite inspection of the stopped candidate members and their saved
post-measurement logs identified a custody-recovery mismatch. Every member has
the same accepted claim plan at operation 14 and disclosure at operation 19,
attempt one. The input ID, locator, amount and reservation action all match the
accepted plan. Its state is `PendingSpend`, as disclosure validation explicitly
requires. `wait_for_bark_custody_projection` recognizes the current disclosed
claim attempt but its input predicate accepts only `Reserved` or emergency-exit
`Exiting`. Consequently the same authorized claim cannot resume custody signing
after the durable disclosure checkpoint. Three selected signers logged the
reservation error repeatedly (17, 34 and 34 string occurrences; these include
propagated errors and are not distinct-request counts). Each rejected wait loops
for up to two seconds and repeatedly loads a complete projection. This is a
separate hypothesis from collector ownership, not evidence that the collector
caused the failure. Private structured inspection records contain the equality
checks without publishing payment material.

**E09 hypothesis, before editing:** the Axum custody projection boundary rejects
the exact pending-spend input of an already accepted, preimage-disclosed incoming
claim. Recognize that durable phase explicitly, retaining current-attempt checks,
exact input/locator/amount/action binding and all cryptographic validation. Do not
permit pending-spend inputs for another action kind, a pre-disclosure phase, a
terminal action, or a stale attempt. This must resume the same claim only; it
must never authorize a new Lightning payment, nonce reuse, or another input.
Expected savings: remove deterministic failed projection waits, redundant full
reads and repeated signing requests on this recovery path. Primary measurements
are authorization latency/read count and time to recover the original settled
receipt, with member CPU and idle costs retained.

Before retaining E09, reproduce the rejection in a regression test, obtain a
bounded profile of the existing wait path after the matched collector series
stops, and compare five alternating baseline/candidate pairs using the same
public projection workload. Verify the original preserved live claim can recover
without payment initiation, then run adversarial state/attempt/reservation and
restart checks. The component workload is not an end-to-end throughput baseline;
a fresh matched payment comparison remains necessary. No E09 production change
or acceptance has occurred. Heavy builds and profiles remain deferred until the
active comparison controller finishes, to avoid contaminating payment samples.

The private offline analyzer now uses the existing `payment_window` classification
for pending, failed, timed-out and rejected requests separately. The driver
summary's `unfinished` field counts every non-completed request, including a
terminal client failure; it must not be relabeled as pending. The table above
therefore says "not completed." Requests requiring reconciliation are counted
separately even when the client operation has failed. Pair 17 candidate has one
pending request and one failed request, both requiring reconciliation. This
analysis correction preserves all raw samples and changes no acceptance rule.

Pair 18 also completed: baseline six window receipts, one late and one pending
(14,336 sat; 28.492993 seconds whole-observation CPU); candidate eight window
receipts (16,384 sat; 23.967487 seconds whole-observation CPU). Both arms offered
eight requests. Their original state was preserved on shutdown. The controller
then began pair 19. These additional samples do not yet meet the five-pair gate.
The report and journal update records progress only; it accepts no production
optimization. Documentation whitespace verification passed.

### Continuation: preparation for E09 and rejected quote attribution

The preceding goal turn made progress: committed comparison evidence and the
custody-recovery hypothesis. This continuation revalidated the same live
controller. Pair 19 candidate completed all eight requests inside the window
(16,384 sat; 23.905522 whole-observation CPU seconds); baseline completed six
inside the window, one late, and retained one pending (14,336 sat; 26.632125 CPU
seconds including drain). The fifth pair is preparing. No measurement was
restarted or excluded.

A test-only reproduction for the pending-spend input predicate is prepared in the
existing custody authorization test module; it has not yet been compiled or run.
Production authorization is unchanged. A private, one-shot profiling script is
also prepared. It refuses to run alongside the comparison controller or another
member fixture, starts only the original stopped backend containers, and resumes
the existing member work directories without copying custody state. It performs
no creation, funding, mining, payment or replacement-quote commands. It records
bounded sampling profiles and preserves state on shutdown. Profiling overhead
is diagnostic only and cannot enter performance acceptance measurements.

Read-only inspection also narrows the candidate's failed quote request. All five
members have eight receive-intent journal entries but only seven mint quotes.
An applied intent at operation 28 (member two) and a rejected intent at operation
30 (member one) selected the same destination locator, index eight, for different
actions, quotes and payment hashes. Each carried five secret acknowledgements.
This explains why a client failure cannot simply be described as an absent
request: a rejected durable proposal exists. Local `bark_incoming_lock` guards
construction on one member; `fresh_intent` chooses the maximum accepted allocation
index plus one, which does not reserve a locator across different members while
both proposals are in flight. The exact timing that admitted the two proposals
still needs a bounded trace/reproduction before an implementation decision.

Source inspection ruled out a separate suspected missing acceptance check:
`submit_federation_envelope_inner` calls `validate_federation_submit_response`,
which calls `ensure_accepted` before returning. The invoice path therefore cannot
mark this rejected intent accepted through that call. No redundant guard was
added. The failed request's secret binding and original proposal remain intact;
no retry or binding replacement is authorized by a client timeout. The measured
locator collision is retained as a follow-up correctness/performance hypothesis,
not attributed to the collector change without comparative evidence.


#### Five-pair E08b result: extend, do not accept yet

The original controller terminated successfully after all ten arms were recorded
and stopped with state preserved. Pair 20 baseline recorded six window/one late/
one pending (14,336 sat; 27.131335 whole-observation CPU seconds); candidate
recorded eight window receipts (16,384 sat; 24.020901 CPU seconds).

Offline commands (private campaign directory, source-independent analysis):

```sh
python3 target/federation-performance-20260908/analyze_e08b_with_recovery_pairs.py
python3 target/federation-performance-20260908/export_e08b_with_recovery_evidence.py
```

Both completed successfully: five pairs, ten run rows and eighty request rows.
The wrapper uses the committed `analyze_federation_performance` module; it checks
matched workloads/inputs, alternating order, binary fingerprints and the shared
SDK fingerprint. All native fixed-window endpoints met the predeclared 100-ms
sampling tolerance. The published numeric CSVs contain explicitly selected
fields only; private artifacts retain manifests, individual profiles, identities,
quotes and original request/outcome records.

| Metric | Candidate / baseline | Run-level 95% interval | Interpretation |
| --- | ---: | --- | --- |
| Completed inside window | 1.258783 | 1.072936–1.476822 | More confirmed window receipts |
| Completed including drain | 1.078957 | 0.919660–1.265847 | Inconclusive |
| CPU per window receipt | 1.003307 | 0.769970–1.307357 | Inconclusive |
| CPU per receipt, whole observation | 0.936720 | 0.583721–1.503191 | Inconclusive; differing drain work |
| Peak aggregate RSS | 1.011514 | 1.005117–1.017952 | Small measured increase, below 5% |
| Pending evidence collections | 0.852474 | 0.704031–1.032215 | Predicted work saving inconclusive |
| Pending backend waits | 1.595985 | 1.169496–2.178004 | More whole-observation waits |

Totals: baseline thirty window receipts, five late and five pending; candidate
thirty-eight window receipts, one pending and one failed client request. The
pending and failed candidate requests both require reconciliation. The absolute
paired pending-count difference is -0.8 per run (95% interval -1.355289 to
-0.244711); the failed-client count difference is +0.2 (-0.355289 to 0.755289).
No client timeout/rejection outcome was recorded, though the failed request
includes a member timeout in its error chain. Full-population p50/p95 are not
reported from selected successes. There are too few samples for p99, and finite
bursts do not measure sustainable throughput.

Fixed-window CPU totals are 97.482459 baseline and 122.897347 candidate seconds;
the raw CPU ratio is 1.262947 (1.109667–1.437399). The generic analyzer labels this
raw cost an increase over 5%; the campaign's efficiency gate is CPU per completed
payment, which remains inconclusive because completed work differs. This does
not make the efficiency gate pass. Likewise, whole-observation wait counters
cannot substitute for matched fixed-window or per-payment work counters.

**Decision: expand to ten pairs, retaining both frozen binaries and all existing
samples.** No E08b optimization is accepted. Before starting the additional five
pairs, profile the preserved claim on the original artifact; any builds/profiles
must finish and the diagnostic fixture must stop before further comparison
payments. This separates diagnostic overhead from measurements and does not
change the ablation. The original fixture resumed from its existing containers
and work directories without creation, funding, mining or payment commands.
Initial read-only backend connection refusals during startup resolved; they did
not trigger any repeated mutation or fixture initialization.

#### E09 baseline profile completed

The one-shot preserved-state profile completed successfully. All five members
became healthy after restart, and all three 20-second sampling commands exited
successfully. The original pending claim remained `receive_preimage_disclosed`
with its pending-spend input on all five members. No payer command ran and no
state was cloned. The diagnostic members and existing backend containers then
stopped with all durable state preserved.

Sampling captured full Bark projection reads and JSON decoding directly beneath
`FederationFrostSigningRuntime::validate_bark` in signing commitment/abort
requests. This supports the attribution from source and rejected-request logs;
sampling frame counts are not SQL query counts or sums of wall time. The three
selected signers logged two, three and three reservation-error string occurrences
during this diagnostic restart/profile. Aggregate member CPU over the sampled
interval was 3.981545 seconds, including profiling overhead; it is not an
acceptance sample or an idle-efficiency claim. Additional projection reads under
application/checkpoint work were also visible and are not attributed to this
custody check.

After the profiler terminated and stopped its fixture, the pinned Nix command
started the failing-before regression:

```sh
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib bark_custody_pending_claim_input_retains_its_reservation -- --nocapture
```

Compilation and its eventual result remain separate from the frozen payment
measurements. No production E09 correction has been applied yet.


#### E09 correction and scoped checks

The new regression compiled and failed at the expected pending-input assertion
on the original validator (one failing test, not a build failure). The correction
then passes the current accepted action's effective phase into the existing input
check. It permits `PendingSpend` only for an incoming claim at
`ReceivePreimageDisclosed`. Current-attempt validation, exact ID/locator/amount/
reservation binding, ordinary reserved inputs and emergency-exit rules remain in
place. No timeout, quorum, wire format, signature check or external identity was
changed. Tests reject unrelated action kinds/phases, missing inputs, spent inputs,
and mismatched IDs, locators, values and owners.

Commands in the pinned Nix shell, after the diagnostic fixture stopped:

```sh
cargo fmt --all
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib frost_signing
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib bark_
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib recovery
cargo clippy --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib --tests -- -D warnings
```

FROST signing: 16 passed, including remote signing, exact retries, lost-response
recovery and the new boundary cases. Bark: 24 passed. Recovery: 46 passed; its existing manual
long-duration checkpoint benchmark remained ignored. Formatting and strict
scoped lint passed. The candidate is still provisional: component comparisons
and recovery of the original settled payment remain required before retention.
These test commands include the other provisional worktree changes; the isolated
component comparison varies only the custody validator.

The component fixture contains only the preserved public Bark projection: seven
actions, thirteen VTXOs, fourteen allocations, seven reservations, twenty-seven
transitions and six observations (74 rows total). The fixture's canonical JSON
hash is `32db203b4c27941d67ab0121be2346da68a2887041d7e602a97a9cd3ff6987a7`.
A fresh test database receives these public records after migrations. It contains
no copied keys, nonces or live wallet state; no federation services start. Three
calls per run invoke the real `wait_for_bark_custody_projection` with the same
accepted plan. Preparation occurs before the stage timer. Whole-process CPU/RSS
includes test initialization and is labeled accordingly.

Build temporary read-count probes first to confirm eliminated projection scans,
then remove that counter and build both final component artifacts without it.
Use five alternating matched pairs of the unprobed artifacts. The baseline is
expected to time out on this valid checkpoint and the candidate to authorize it;
report both outcomes explicitly. Timing that defective check is not the latency
of a successful baseline payment and must not be presented as such. Before/after
live recovery and the broader payment gates remain distinct requirements.


The first temporary component probe exposed a harness feature mismatch: Axum's
SQLite dev dependency enables federation but does not automatically enable its
Bark projection implementation. The unsupported application read returned null,
so the baseline failed immediately (about 65 microseconds, one read per check)
and the candidate assertion failed. These are **invalid harness observations**,
not a performance result. Both artifacts, logs and fixture cases are preserved.
Production source was restored and no temporary probe remains there.

The corrected build explicitly enables `cdk-sqlite/federation-bark` in both arms.
The harness now reads and checks the expected projection before timing and
requires the baseline error to be the specific custody timeout, not just any
error. This prevents the previous false baseline result from passing silently.
New probe directories preserve the original attempt rather than overwriting it.
No dependency manifest or standalone default was changed to fix this test harness.

The corrected probe completed successfully with the expected public projection
and error classification. Across three checks, baseline performed 244 complete
projection reads (18,056 SQL rows decoded) and authorized none; candidate performed
three reads (222 rows) and authorized all three. Mean check duration was
2,003,915 microseconds versus 958 microseconds. Whole-process CPU including test
preparation was 0.67 versus 0.06 seconds; retired instructions were 4,768,731,481
versus 371,818,431. These are **one instrumented diagnostic pair**, not final
performance acceptance. The baseline duration is a failed timeout, not successful
payment latency. The predicted removal of redundant reads is now observed.

The temporary benchmark/counter were removed from production source after both
probe builds. An isolated uninstrumented runtime build has started for recovery
of the original claim. It includes the existing E08b collector used by that
fixture and E09, but excludes the unrelated broadcaster and replay-cursor changes.
Before acceptance, the unprobed five-pair component comparison and the original
claim's recovery still remain required. The frozen E08b comparison artifacts
remain unchanged for the additional five payment pairs.

#### E09 live recovery remains incomplete

The isolated uninstrumented candidate runtime built successfully and replaced
only the stopped fixture's executable link. Original containers, member work
directories, custody identities, existing SDK wallet and original quote were
reused; no state was cloned or initialized. The existing recovery helper creates
no invoices and sends no Lightning payments. Its attempt to obtain proofs for
the original quote timed out after 120.797022 seconds and produced no receipt.
All payer records (identities, statuses and amounts) were unchanged. All five
members were healthy before and after; the action remained
`receive_preimage_disclosed`, attempt one. State was preserved on shutdown.

The corrected runtime emitted no reservation-mismatch error in this attempt.
It emitted one signer-unavailable error while peers were still validating startup
state, then did not recover within the bounded observation. This disproves the
assumption that the projection correction alone suffices for this saved claim.
It is **not an accepted E09 win**. Do not reinterpret the timeout as permission to
repay, replace a quote, reuse a nonce or advance an attempt without consensus.
The SDK recovery output directory and its failed attempt remain intact; do not
rerun that one-shot helper blindly.

Read-only Bark storage inspection still finds a `PreimageRevealed` checkpoint
with a custody session and no new settled receipt. The original signing attempt
may require further recovery, but that cause is not yet established. Before
editing the next recovery boundary, add a temporary local-only bounded probe to
record whether a failed `execute_receive_claim` was after disclosure, whether
custody reported signer unavailability, and the underlying backend error chain.
Today those errors become `RecoveryRequired`, which correctly preserves uncertainty
but hides the failure phase from this investigation. Capture the error without
changing outcomes, authorization, retry decisions or external identities; remove
the probe afterward. Its raw logs remain private and cannot enter acceptance
measurements. A guarded build wrapper has been prepared but not run.

The dependency checkout used for this inspection must match Cargo's pinned
`8d1eda643dd11e9eb600ec44e36622c8f77b6322`. An initially inspected sibling checkout
was a different revision and did not contain that Git object; subsequent source
inspection uses Cargo's existing pinned checkout. No dependency or external
repository was modified, and no conclusion relies on the stale source.

Unprobed E09 paired measurements are deferred until the remaining live recovery
failure is understood. E08b still requires its additional five unchanged-binary
pairs. The full campaign and final correctness/payment matrix remain incomplete.

#### E09 bounded error probe and durable retry deadline

The guarded error-probe build and original-state diagnostic completed. The probe
records at most twelve failures per member and changes no return values. Source
was restored after the build; only the frozen diagnostic executable contains it.
The original five members and backend were resumed without SDK, invoice, payer,
funding or mining commands. All members became healthy and remained healthy;
the sixty-second observation ended normally and all owned processes were stopped
with their databases preserved.

Exactly one claim-execution error was recorded, on the actor during startup:
the preimage was already disclosed and custody reported signer unavailability
while a peer was awaiting quorum validation. No later claim execution was recorded
within that observation. Read-only inspection of the actor's persisted payment
schedule explains the gap: its seventh consecutive no-progress result selected
an existing jittered external-evidence delay of 265,820 milliseconds. The other
members waited for federation evidence. A sixty-second observation cannot test
that actor's next scheduled retry; absence of another error does not establish a
stuck scheduler or terminal signing failure.

The next diagnostic observes the same original claim through the existing retry
deadline, recording bounded error counts, schedule generation, wait category and
remaining deadline. It does not change production timeouts, reset backoff, alter
durable state, initiate another payment or infer permission to reuse a nonce.
Private commands: `python3 "$RUN_ROOT/observe_e09_preserved_claim_errors.py"`,
followed by `python3 "$RUN_ROOT/observe_e09_preserved_claim_deadline.py"` using a
distinct output directory. The latter is in progress; its outcome is not yet
acceptance evidence. No additional production change has been made.

The extended observation reached the original actor's next deadline. Its schedule
advanced from generation 17 to 18, proving the scheduler retried without a manual
wake. The second bounded error identified a virtual MuSig2 session in a
non-retryable state. Custody again classified the failure as signer unavailability
with the preimage already disclosed. The runtime converted it to external-evidence
waiting, leaving the accepted signing attempt unchanged and scheduling another
backoff. The remaining wait is therefore not a missing scheduler wake.

Follow-up hypothesis within E09: retain the custody-unavailable signal in the
existing post-disclosure recovery outcome. The caller must first obtain accepted
preimage disclosure if it is not already accepted, then use its existing
consensus-controlled signing-attempt advancement. Generic ambiguous backend
failures must remain external-evidence waits. This should remove retries against
an unusable nonce session while preserving the original plan, external action,
invoice, input reservation and payment hash. Never reset or reuse that session.
Validation will require the original claim to recover, accepted disclosure and
retry ordering, unchanged payer records, current-attempt rejection tests, and
unprobed matched measurements before acceptance. No performance gain is claimed
from this diagnostic.

The extended diagnostic completed its 360-second window, verified all five
members healthy, and stopped the owned fixture with original state preserved.
There were exactly two bounded errors: startup readiness, then the non-retryable
session at the scheduled deadline. No SDK or payment initiation ran.

The follow-up implementation adds the local custody-unavailable flag to
`FederatedBarkReceiveClaimOutcome::RecoveryRequired`. Incoming orchestration
continues to submit disclosure first when it is not yet accepted. For an already
disclosed claim, only that flag selects the existing accepted signing retry;
generic ambiguous errors continue to wait. The pre-disclosure and post-disclosure
paths share the existing retry submission body in one private helper. No wire
format, public wallet method, standalone behavior, nonce validation or dependency
revision changes. No temporary probe remains in production source.

`cargo fmt --all` and the federation-enabled Axum `--lib bark` filter passed
(24 tests, including custody transport attempt rotation and exact-result retry).
An initial standalone `cdk-bark --lib` invocation selected zero tests because
its federation feature was omitted; it is not runtime coverage. An explicit
`cdk-bark --features federation-bark --lib` run, protocol tests and strict scoped
lint are in progress. Recovery helpers now take a new output directory explicitly
so prior failed receipts and diagnostics remain preserved. They still reuse the
existing wallet, seed, quote and invoice and contain no payment initiation.
The correction remains provisional and uncommitted pending live recovery and
unprobed paired evidence.

The explicit feature run passed 19 Bark runtime tests; the Bark protocol suite
passed 36 tests. Scoped strict Axum library/test lint passed. Exact commands,
all under the pinned stable Nix shell with `RUST_MIN_STACK=16777216`:

```sh
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib bark
cargo test --locked -p cdk-bark --features federation-bark --lib
cargo test --locked -p cdk-federation-bark --lib
cargo clippy --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib --tests -- -D warnings
```

The isolated uninstrumented recovery runtime and explicit-output SDK helper
builds are now in progress. Their wrappers preserve unrelated experiments and
remove private helper source after building. The runtime excludes E03a/E07 and
includes E08b, matching the original preserved fixture except for the E09
corrections. No final payment, performance, ordinary-CDK or campaign acceptance
is inferred from these scoped tests.

Both release builds completed successfully and the private example source was
removed. The new runtime has not been started: source inspection before the live
run found another incomplete post-disclosure retry boundary. Claim finalization
always uses the original disclosure as its predecessor, while deterministic
application requires the current action head and the literal disclosure phase.
An accepted `SigningAttemptAdvanced` changes both the head and literal phase,
preserving disclosure only in `signing_retry_phase`. Consequently the newly
reachable result would be rejected even after successful custody recovery.

Hold the live run until this binding is corrected and tested. Preserve the
original disclosure evidence and exact payment bindings; retain the accepted
custody operation with the private verified claim result, use it as predecessor
when signing advanced after disclosure, and independently require the effective
disclosure phase, latest accepted head, current attempt and exact signer set at
application. Do not bypass application checks, replay disclosure, change value
transitions, or accept a stale result. The frozen incomplete candidate remains
available as an investigative artifact, not an accepted implementation.

The private verified claim result now retains its accepted custody-operation ID.
Claim construction follows that ID only when its attempt is newer than the
original disclosure; otherwise it follows the disclosure directly. The original
disclosure still binds evidence and external-result identity. Older results are
rejected. Deterministic application checks the effective disclosed phase and
requires the current head, attempt, signer set, actor, plan, quote and payment
hash. Its existing input/value validation remains in place. SQL already interprets
the effective phase for allowed transitions, so no SQL/schema change is needed.

The new `receive_claim_retry_requires_the_current_disclosed_lineage` regression
passed. It checks the accepted retry path, rejects a fork from the old disclosure,
stale/future attempts, mixed signers, another payment or actor, and an undisclosed
retry, and retains the ordinary first-attempt claim path. This covers the header
boundary, not full payment validation. The existing projection, Axum Bark and Bark
runtime suites and strict lint are being rerun before building a new isolated
release artifact. No live recovery was run with the known incomplete artifact.

The complete correction passed three projection tests, 24 Axum Bark tests,
19 Bark runtime tests and strict library/test lint across CDK, Axum and Bark.
The new isolated release artifact built successfully and restored unrelated
experiments. The original-state recovery is now running with this artifact and
the explicit-output client. It observes the existing durable retry deadline
before requesting proofs and preserves the prior failed recovery directory.
The payer audit brackets the operation. In addition to the original quote's
receipt, the check reconciles all seven paid incoming invoices (14,336 sat of
principal) with the eight SDK wallets and each member's paid/issued totals.
That is an incoming-principal check, not the campaign's full custody/fee
conservation gate. Its result and the unprobed paired comparisons remain pending.

#### E09 original claim recovered; paired acceptance still pending

The new uninstrumented runtime recovered the original accepted claim. The local
quote was paid at the first five-second observation; the existing-quote SDK
helper then completed in 2.779932 seconds. These are separate unpaired diagnostic
durations, excluding restart/readiness, not an end-to-end performance comparison.
The original wallet issued exactly 2,048 sat and retained its quote/invoice
identity. The already-issued control wallet stayed at 2,048 sat without issuing
again. Payer records were unchanged and no new Lightning payment was initiated.

All five members passed ready/converged and digest-alert checks before
and after. Every member ended with seven claimed receives: six at attempt one
and the recovered claim at attempt two. Read-only journal inspection verifies
the original disclosure, accepted retry, and terminal claim form consecutive
predecessor links with attempts one, two, and two. The original seven completed
invoices total 14,336 sat of principal; all eight SDK wallets together hold
14,336 spendable sat, matching each member's paid and issued totals. The failed
quote-creation request still holds no proofs and was not recreated. This is
principal reconciliation, not a claim of full custody/fee conservation.

The recovery controller exited successfully and stopped only its owned members
and backend, preserving every database. Its private command was
`python3 "$RUN_ROOT/recover_e09_preserved_claim_finalization.py"`; prior failed
recovery outputs remain intact. The instrumented error binary was not used.

Five uninstrumented alternating component pairs are now being built and run with
the previously documented public-only projection fixture. No payment service or
live custody state runs in those cases. Both arms preserve the same surrounding
source; only the projection authorization implementation differs. Production
patches and binary fingerprints are captured privately, and the wrapper removes
the temporary harness after building. These pairs must pass acceptance before
the complete E09 correction is retained and committed. E08b's frozen-binary
extension and the campaign's final gates remain outstanding.

#### E09 accepted component result and restart confirmation

All five alternating uninstrumented pairs completed. Each used three checks on
the same 74-row public SQLite projection, with fixture setup outside check
timing. Whole-process CPU includes setup. Both arms used identical surrounding
source and release features; only the projection validator differed. Frozen
binary/source fingerprints and production patches are retained privately under
base `3d2c08d0a` plus the recorded worktree changes. The temporary harness and
all probes were removed after building.

| Metric, mean per three-check run unless specified | Baseline | Candidate | Paired candidate/baseline, 95% interval |
|---|---:|---:|---:|
| Process CPU seconds | 0.370 | 0.042 | 0.113269 [0.095994, 0.133653] |
| Retired instructions | 5,130,356,197 | 355,017,083 | 0.069175 [0.066697, 0.071745] |
| Peak RSS bytes | 19,988,480 | 19,988,480 | 0.999982 [0.991817, 1.008214] |
| Mean authorization-check microseconds | 2,008,936.561 | 902.722 | 0.000449 [0.000429, 0.000470] |
| Successful authorizations | 0/3 | 3/3 | Absolute increase three in every pair |

Every baseline check reached the specific known custody timeout; every candidate
check authorized the exact input. The roughly 88.7% CPU and 93.1% instruction
reductions are component work savings, **not CPU/payment or payment-throughput
claims**. The timeout-to-authorization duration change is not successful-payment
latency. RSS shows no measurable change and its interval lies within the 5%
regression boundary. The earlier bounded counter probe confirms the predicted
work removal: 244 versus three full projection reads across three checks.
[All ten runs and thirty durations](benchmarks/2026-09-08-pending-claim-authorization.csv)
are retained as numeric evidence. No p99 conclusion applies.

The original recovered fixture then passed a second uninstrumented restart.
Both checked wallets retained 2,048 sat and issued zero additional value; payer
records were unchanged. All five members again passed readiness/convergence and
digest-alert checks. The 14,336-sat incoming-principal reconciliation
still holds across all wallets and all members. The helper completed in
0.852713 seconds, an unpaired recovery diagnostic only. Owned processes were
stopped with state preserved.

Commands: `python3 "$RUN_ROOT/build_e09_projection_bench.py"`,
`python3 "$RUN_ROOT/run_e09_projection_pairs.py"`, and
`python3 "$RUN_ROOT/confirm_e09_preserved_claim_restart.py"`. All builds,
measurements and live diagnostics were serial. **Keep E09** as one complete
disclosed-claim recovery correction with measured removal of wasted projection
work. Its two follow-up fixes are necessary for the original claim to complete;
they do not independently claim a performance percentage. Commit this step
separately from E08b, E03a and E07. Final combined payment throughput, idle CPU,
history scaling, remaining transports/backends and the full campaign gates are
still outstanding; no campaign-wide performance acceptance is claimed.

E09 was committed separately as `bacc0f971` (`fix: recover disclosed federation
Bark claims without repeated scans`). Final formatting and staged whitespace
checks passed; the numeric CSV was normalized to LF before commit. The collector,
broadcaster and replay-cursor experiments remain uncommitted.

#### E08b extension resumed after E09

The additional five pairs (21–25) are running serially using the unchanged frozen
E08b baseline/candidate and persistent SDK. All three executable fingerprints were
verified against pair 16 before starting. The first five pairs' analysis was
preserved in a separate private snapshot. No E09 code enters either comparison
binary. Every extension case is a fresh independent fixture; old databases and
uncertain identities remain preserved. The command is
`python3 "$RUN_ROOT/run_e08b_with_recovery_pairs.py" --first 21 --last 25`.
No build or profiling work will run during these payment measurements. The
extension is in progress; do not infer results or restart an observed-timeout job
without checking its process handle.

### E05 scoped candidate prepared during the frozen E08b extension

The recorded profile and bounded digest probe support a narrow first experiment:
checkpoint-summary construction currently recomputes the full configuration
digest for each stored candidate/quorum slot. The owning immutable runtime already
stores that digest. Pass the runtime object to the existing private summary
helpers so config and digest come from the same trusted owner. Add a focused
checkpoint validation entrypoint accepting this already-computed expected digest;
share the existing shape, federation binding, digest equality and roster checks.
Preserve their error order through lazy digest computation on the original path.
Never take the expected digest from the checkpoint or peer response.

Scope is summary construction only. Full proof verification, admission, startup,
checkpoint capture and all other validation callers retain their existing paths.
No new cache, timer, wire format, dependency or standalone change is planned.
The predicted work reduction is zero configuration reserializations caused by
these summary validations after runtime construction. A public-only fixed-config
component comparison will attribute work; matched settled-member CPU measurements
remain the primary gate. Require tampered-binding/roster and summary-route checks,
five alternating pairs (ten if inconclusive), guarded memory/active-payment checks
and restart coverage before retention. The candidate may be edited while frozen
E08b binaries run, but no build, test or profiling workload will overlap their
payment windows. This is preparation, not a measured E05 win.

#### E08b extension: preparation failure and verified continuation

The controller exited during pair 23 baseline preparation because Docker had
exhausted its predefined network address pools. Pairs 21 and 22 and pair 23's
candidate arm completed and were stopped with state preserved. Authoritative
process inspection confirmed the controller was terminal and no comparison
members or payment driver remained live. The failed baseline directory contained
only the allocation error; no named network, container, volume, federation
fixture or measurement existed. This is a preparation failure, not a payment
sample. Its private error directory was archived without deleting state.

Resume only pair 23 baseline and pairs 24–25. Their independent fixtures use
explicit unused private subnets checked against existing Docker allocations.
No existing networks, services, databases or payment identities are removed or
reused. The bridge topology, images, limits, liquidity checks, frozen binaries,
SDK, payment shape, windows and alternating arm order remain unchanged. Record
this infrastructure change as a comparison limitation; preserve every original
sample, including the pair 17 failure. The command is
`python3 "$RUN_ROOT/run_e08b_with_recovery_pairs.py" --first 23 --last 25 --resume-pair-23-baseline`.
The ten-pair keep/revert decision still awaits completion; no new performance
acceptance is claimed. Builds, tests and profiles remain serialized after this
comparison.

Evidence clarification for E09: the status command confirms readiness and
agreement of accepted operation/order/application state, not necessarily zero
checkpoint lag. The post-recovery restart status recorded a one-operation
checkpoint lag on four members and zero on one, with all members ready and no
audit mismatch. The earlier wording claiming zero lag was too broad and has been
corrected. This does not alter the original receipt, accepted-lineage or
principal-reconciliation results.

E05 measurement preparation now includes a private sequential idle controller.
It restarts baseline and candidate on the same original settled fixture without
cloning or restoring custody state, alternates order, separates a fixed 60-second
warmup from each 60-second observation, and rejects changed accepted history,
principal, checkpoint frontiers or pending queues. It measures per-member native
CPU/instructions/wakeups, aggregate sampled RSS/footprint and collector CPU, with
before/after readiness and payer-identity checks. It creates no invoice and runs
no payment or issuance helper. All records remain private pending numeric export.
The controller is syntax-checked but unexecuted; no E05 runtime or component
benchmark has been built or measured yet. It cannot run while the E08b comparison
or any campaign member is live. Component builders enforce the same exclusion.

The resumed pair 23 baseline completed normally: six of eight receipts in the
60-second window, one during drain, and one pending at drain; 14,336 sat was
received, with 28.236539 member CPU seconds over the whole observation. Its
candidate had completed eight in-window, receiving 16,384 sat. Both arms are
stopped and all state remains preserved. Eight complete pairs (16–23) now exist;
pairs 24–25 are still running. These additional results remain raw/private until
the planned ten-pair analysis and numeric export; there is no interim keep
decision or sample exclusion.

E05 workload refinement before execution: sequential restarts of the original
settled fixture preserve safety, but accumulated Aleph session backups can carry
state between runs. Keep that controller for recovery/history diagnostics only.
Primary idle acceptance will instead use fresh independent baseline/candidate
fixtures through the existing matched Bark backend and dealer-free DKG recipes.
A read-only check of a preserved fresh-start fixture confirms it has zero accepted
operations and a recovery candidate/quorum, so summary validation is exercised
without needing payment traffic. The new one-arm measurement requires those
checkpoints, zero quotes/actions/operations/queues, matching policies and economic
inputs, and unchanged status/SQL state during the fixed warmup/idle protocol.
Five alternating pairs can extend to ten with the same binaries. Fresh-state idle
evidence will not replace later 1,000/10,000-operation history or active-payment
gates. These adapters are private, syntax-checked and still unexecuted.

### E08b ten-pair disposition: revert the collector experiment

All pairs 16–25 completed with the unchanged frozen baseline, candidate and SDK.
The controller exited successfully and authoritative process/container inspection
confirmed the final arms were stopped before analysis or builds. All twenty runs
and 160 requests are retained in the numeric run/request ledgers; no failure,
late completion or pending identity was excluded. The pair 23 preparation-only
network allocation failure and explicit-subnet continuation remain documented.

| Metric | Baseline | Candidate | Paired result |
| --- | ---: | ---: | --- |
| Receipts inside ten 60-second windows | 60/80 | 78/80 | Ratio 1.295522; 95% CI 1.213897–1.382636 |
| Receipts including drain | 69/80 | 78/80 | Ratio 1.127698; CI 1.043945–1.218170 |
| Pending requests at drain | 11 | 1 | Mean difference −1/run; CI −1.337222 to −0.662778 |
| Client failures | 0 | 1 | The original pair 17 quote-creation failure is retained |
| Aggregate fixed-window member CPU seconds | 194.907605 | 247.734711 | More work completed; this total is not CPU/payment |
| CPU per in-window receipt | — | — | Ratio 0.980127; CI 0.870394–1.103694: inconclusive |
| CPU per receipt over the whole observation | — | — | Ratio 0.852655; CI 0.692839–1.049336: inconclusive |
| Peak aggregate RSS | — | — | Ratio 1.010621; CI 1.004011–1.017274, within the 5% guard |
| Pending evidence collections, whole observation | 634 | 541 | Ratio 0.852706; CI 0.779496–0.932791 |
| Pending backend waits, whole observation | 1,679 | 2,868 | Ratio 1.710357; CI 1.419523–2.060778 |
| Projection readiness checks, whole observation | 3,493 | 4,487 | Ratio 1.279793; CI 1.171130–1.398539 |

Ratios are geometric means of paired run ratios, not ratios of the pooled totals.
Unconditional run p50/p95 remain censored by incomplete/failed requests. No p99
or sustainable-throughput conclusion is available. Whole-observation counters
span different drain lengths and completed-work populations: they cannot prove
fixed-load work savings. The analyzer labels the raw fixed-window CPU increase a
regression; interpretation must account for its larger completed population.
The relevant in-window CPU/receipt interval still permits a regression above 5%,
so it does not pass the guard. Lower evidence-collection counts do not resolve
that uncertainty or the increased backend work.

**Revert E08b under the ten-pair inconclusive rule.** Its two production files and
one regression test were restored exactly to the accepted HEAD versions, keeping
the complete E09 recovery correction. The rejected diff, frozen binaries and all
fixture state remain private for investigation. No temporary probe is introduced
or retained by the reversion. A future hypothesis may examine whether allowing
the preparation worker outside the stable observer prefix creates redundant
backend observations, but it requires fresh attribution and a new narrow
comparison against the accepted recovery code. This is not permission to recycle
the same inconclusive series or discard its outlier.

Commands: `python3 "$RUN_ROOT/analyze_e08b_with_recovery_pairs.py" --first 16 --last 25`
and `python3 "$RUN_ROOT/export_e08b_with_recovery_evidence.py"`. CSVs were normalized
to LF and export is restricted to numeric fields and bounded outcome labels.
[All twenty runs](benchmarks/2026-09-08-bark-incoming-with-recovery-runs.csv) and
[all 160 requests](benchmarks/2026-09-08-bark-incoming-with-recovery-requests.csv)
include the original pair 17 outcomes; later E09 recovery does not change them.

E05 isolated correctness validation has now started after all payment measurements
ended. Its temporary validation checkout excludes the broadcaster and replay
experiments and restores them afterward. It covers checkpoint bindings and routes,
recovery, minimal federation and standalone-common compilation, formatting and
scoped strict lint. These checks are running, not yet passing evidence. No E05
performance acceptance or commit has occurred.

The ten-pair ledger and collector reversion decision were committed separately as
`6a722ef73` (`docs: record ten-pair collector results and reversion`). All original
first-block CSV rows were compared field-for-field and are unchanged. The export
contains twenty runs and 160 requests, with numeric/bounded-label and identifying
content checks passing; staged whitespace checks also passed.

E05 validation progress: fifteen common checkpoint tests, 43 full-feature Axum
checkpoint tests and 46 recovery tests passed. The Axum checkpoint filter skips
two explicitly manual benchmarks; the recovery filter skips one of those same
benchmarks. They are not claimed as executed correctness coverage. The first
Axum build identified an unused `FederationConfig` import after moving the private
helpers to the runtime owner; that import was removed before strict lint. The
minimal-feature checks and lint are still running. No benchmark or E05 retention
claim follows from these passing tests alone.

E05 scoped correctness validation is complete: the minimal-federation checkpoint
filter passed 39 tests with none ignored, both common/Axum strict lib-and-test
Clippy checks passed, and standalone `cdk-common` compilation passed. These filter
counts overlap the earlier full-feature suites; they are not additive distinct
coverage. The final format check initially failed only because removal of the
unused import required regrouping lines. Pinned `cargo fmt --all` followed by
`cargo fmt --all -- --check` passed; no behavioral change was needed. All other
provisional experiments were restored after the isolated validation checkout.

Exact command lists and individual outputs are retained in the private E05
scoped-validation directory, including the failed formatting check and successful
repair. Validation uses pinned Nix stable Rust with `RUST_MIN_STACK=16777216`.
A bounded test-only digest counter build is now in progress via
`python3 "$RUN_ROOT/build_e05_summary_bench.py" --probe`. Its purpose is to confirm
configuration-digest call elimination, not to supply acceptance timings. Source
restoration is enforced by the builder's finalization block. No live service or
other heavy measurement overlaps this build.

The first E05 probe build failed because its temporary test harness referenced
`FederationCheckpointSummary` without importing that type. Production validation
is unaffected. No benchmark executable or measurement was produced; the builder
restored all source and the failed build directory is preserved separately.
The private harness now uses the existing fully qualified public re-export.
Rebuild the probe from the same production revisions; do not count compilation
failures as measurement samples.

E05 bounded work probe passed on the same five-member/full-denomination public
configuration and checkpoint at operation frontier 58. One hundred summary
validations performed 100 full configuration-digest computations in baseline and
zero in candidate: 37,268,400 versus zero configuration bytes serialized through
that method. Public fixture and summary hashes agree between arms. Config
validation, reference preparation and digest creation occur before the counter
reset; these counts specifically cover repeated summary validation. Probe
measurements are attribution only, not timing acceptance. The test harness and
atomic counter were removed from production source by the builder, and a source
scan confirms their absence. Commands:
`python3 "$RUN_ROOT/build_e05_summary_bench.py" --probe` and
`python3 "$RUN_ROOT/run_e05_summary_pairs.py" --probe`.

Proceed to separately built uninstrumented component binaries and five alternating
pairs. Fresh independent idle measurements and active-payment/resource guards
remain required before retaining E05.

### E05 uninstrumented component comparison: work savings confirmed, retention pending

Five alternating baseline/candidate release pairs completed, with 2,000 summary
validations and constructions per run. Both arms use the same five-member,
64-denomination public fixture, 372,684-byte serialized configuration and
checkpoint frontier 58. Configuration validation and reference preparation occur
outside the timed loop but inside the process resource totals. Each batch has
one monotonic timer; do not infer per-operation tails from its mean. All ten runs
are retained in the [component ledger](benchmarks/2026-09-08-checkpoint-summary-component.csv).

| Metric | Baseline run mean | Candidate run mean | Paired ratio and 95% CI |
| --- | ---: | ---: | --- |
| Mean summary validation/construction, μs | 1,107.688025 | 4.070675 | 0.003676; 0.003528–0.003830 |
| Whole-process CPU, including fixture validation, seconds | 2.926 | 0.754 | 0.257660; 0.249775–0.265793 |
| Whole-process retired instructions | 65,498,848,446 | 20,092,540,886 | 0.306762; 0.306380–0.307144 |
| Maximum process RSS, bytes | 13,094,093 | 11,832,525 | 0.903636; 0.896253–0.911080 |

These are approximately 99.63% lower component mean duration, 74.23% lower
whole-harness CPU, 69.32% fewer instructions and 9.64% lower peak RSS. They are
not payment latency, CPU/payment, sustained throughput or settled-service idle
results. The bounded probe independently confirms the predicted digest work
removal. Summary hashes match, scoped correctness passes, and the uninstrumented
binaries contain no counter. The temporary harness is removed from source after
build. No p99 conclusion applies; the measured durations are batch means.

Commands: `python3 "$RUN_ROOT/build_e05_summary_bench.py"` and
`python3 "$RUN_ROOT/run_e05_summary_pairs.py"`. Builds and runs were serial.
**E05 remains provisional:** fresh independent idle pairs, active-payment/resource
guards and recovery validation remain required for its keep decision. Runtime
artifact preparation now asserts the baseline production diff is empty and the
candidate contains exactly the three E05 source files, excluding all other
provisional implementation and test changes.

Isolated E05 service builds are now running with
`python3 "$RUN_ROOT/build_e05_summary_runtimes.py"`. The completed uninstrumented
component controller exited successfully before this build started. No service
idle or payment measurement has started yet. Keep the running build handle and
wait for source restoration before editing its three candidate files or starting
another heavy command; its baseline and candidate artifacts must use one unchanged
production base. E05 implementation and component evidence remain uncommitted
pending the primary idle and payment guards.

E05 service builds completed with restored worktree state. Both artifacts use
production base `6a722ef73`; baseline has an empty production diff and candidate
has only the three E05 files. Artifact fingerprints and probe-free manifests were
verified before starting the fresh idle series. Automatic approval review timed
out before the first launch executed. Absence of controller/fixture state was
verified, then the permitted retry succeeded. No preparation or payment sample
was created by the timed-out launch.

Five fresh idle pairs are now running via
`python3 "$RUN_ROOT/run_e05_fresh_idle_pairs.py"`. Pair 1 baseline passed all
unchanged-history/queue/checkpoint-state guards and used 2.603001 aggregate member
CPU seconds in 60 seconds; collector CPU was 0.008217 seconds. Its candidate arm
is running. One arm is not acceptance evidence. No build, test or profile will
overlap these observations.

Active-guard preparation reuses the existing persistent receiver/melt SDK drivers
and offline ecash token fixture recipe. The intended ecash guard is a bounded
64-token, concurrency-four receive burst with one 32-sat input per token,
independent recipient wallets and warm connections; funding, token packaging and
wallet setup are outside timing. Record completion/failure/late outcomes, latency,
CPU/window and CPU/receipt with explicit observation scope, actual proof shapes
and value reconciliation. Functional incoming/outgoing on-chain and Lightning
checks and restart must still follow. No sustainable throughput or full matrix
claim follows from this guard. A private restart helper for the existing stopped
idle fixtures is syntax-checked but unexecuted; it verifies the exact artifact,
empty original state and absence of concurrent campaign work and never copies
custody or creates a payment.

E05 payment-guard helpers are prepared but unexecuted while idle pairs run. They
reuse each original stopped idle fixture and the existing immutable SDK, fund a
new wallet through an actual 8,192-sat on-chain deposit with an independent
five-second miner, and package 64 unique 32-sat tokens offline. Deposit identity
is written before the sole funding call; unknown outcomes prohibit re-creation.
The token receiver uses four concurrent independent wallets, a 60-second window
and 120-second drain, with every sample preserved. Pair inputs are checked before
second-arm token submission. A differing initial queue/history or proof shape
invalidates acceptance; no state is reset to force a match.

The guard will bracket its SDK origin causally between the controller's start
signal and the post-exit native sample minus the SDK's recorded observation
duration. Native reads record their own start/end bounds. CPU is reported as
lower/upper bounds for the fixed window and, if all requests finish, the active
interval ending at the last receipt. The nominal ten-millisecond start-file poll
is not treated as a guaranteed scheduler bound. Collector CPU is recorded at a
fixed ten-Hz sample rate. Funding, wallet preparation, token packaging and a fixed
settling period remain outside transaction time; they are not hidden in faster
payment results. This is guarded measurement support, not a new production fast
path or a claim of sustainable throughput.

The idle controller was revalidated through its live process handle and continues
through pair 5; no additional workload was started. E05 remains provisional.
Private functional guards are now syntax-checked for incoming/outgoing Bark
Lightning, outgoing BDK on-chain, member convergence, wallet principal and
reported-fee reconciliation, and restart of the original post-payment state.
Incoming BDK is exercised by the preceding ecash fixture funding. These helpers
are unexecuted and do not count as completed payment or recovery gates.

The functional guard explicitly accounts for offline sender records: successfully
received tokens may still be locally `PENDING_SPENT`. Before further payments it
requires the original 64 token proofs to be spent in the federation's threshold
response, while the remaining 192 bank proofs are unspent. It neither reissues
those tokens nor treats a local pending label as permission to spend again.
Outgoing invoices and deposit intentions are preserved before their sole payment
calls. Independent recipient receipts remain separate from wallet fee accounting;
that accounting alone will not be described as a full custody audit.

### E05 fresh idle: five pairs complete; extend unchanged to ten

The original controller exited successfully. Process inspection found no running
campaign members; direct container inspection confirmed all fifty backend
containers belonging to these ten arms were stopped. All databases, custody,
identities, logs and samples remain preserved. No other build or payment workload
overlapped the observations.

The five independent AB/BA pairs used the same frozen service artifacts, five
members, HTTP, SQLite, full 64-denomination setup, accepted history zero, fixed
60-second warmup, five-second quiet period after status, and 60-second native
one-Hz sampling. Every arm passed unchanged state, readiness, empty queue and
matched-input checks. All ten runs, including the lower-RSS baseline in pair 3,
are retained in [numeric evidence](benchmarks/2026-09-08-checkpoint-fresh-idle.csv).

| Metric | Baseline mean | Candidate mean | Paired ratio; 95% interval |
| --- | ---: | ---: | --- |
| Aggregate member idle CPU, seconds | 2.414055 | 1.360148 | 0.564253; 0.524415–0.607116 |
| Retired instructions | 13,948,763,563 | 3,066,649,140 | 0.219847; 0.218470–0.221233 |
| Package wakeups | 867.0 | 1,027.4 | 1.191799; 0.956563–1.484884 |
| Interrupt wakeups | 14,072.4 | 14,090.6 | 1.001296; 0.995362–1.007267 |
| Peak sampled aggregate RSS, bytes | 325,065,114 | 334,964,326 | 1.033376; 0.926882–1.152106 |
| Peak sampled aggregate physical footprint, bytes | 121,731,078 | 122,750,173 | 1.009084; 0.946249–1.076091 |

CPU falls 43.57% and instructions 78.02%, consistent with the independently
confirmed removal of repeated configuration hashing. These are idle member
resources, not payment performance. RSS and physical-footprint intervals still
permit a regression above 5%, and package wakeups remain inconclusive. Therefore
**E05 is not accepted**. No outlier is excluded and no warmup or sampling setting
is changed. Following the five-to-ten-pair rule, the next command is
`python3 "$RUN_ROOT/run_e05_fresh_idle_pairs.py" --first 6 --last 10` using the
same binaries and unchanged fixtures recipe. It adds independent fixtures and
preserves the first five pairs. Active-payment checks remain deferred until the
idle series ends; their unexecuted support is not correctness evidence.

The ten-pair extension is live and has completed pair 6 candidate with its
original state preserved and services stopped; pair 6 baseline is preparing.
No intermediate extension sample changes the keep decision.

Restart-guard preparation now captures hashes of each original member's payment
identities, quote amounts/states, proof states, journal order/application digests
and Bark action lineage before and after restart. Post-payment checks also compare
all 65 original wallets' proof/transaction state and balances. Operational
checkpoint/backup timestamps are excluded from these logical-state comparisons.
The snapshot queries were executed read-only twice against the five stopped
members of the first baseline fixture: all expected SQL columns exist and both
captures agree. This validates the helper's empty-state SQL path only; funded
wallet snapshots and actual restarts remain unexecuted correctness gates. The
CLI token-state checks use each original token explicitly because ordinary CLI
recovery may remove spent local records before the subsequent local-state read.

The report's current-status pass corrected stale historical statements that still
called the rejected collector provisional and described earlier startup
investigations as current work. A disposition table now identifies each retained
commit, provisional candidate and reverted experiment with its actual evidence
scope. Historical samples remain in this journal and their numeric ledgers;
none were reclassified or removed. This is a reporting correction, not another
accepted implementation or completion of any final gate.

### E05 ten-pair idle result: revert after inconclusive memory guard

The unchanged extension completed successfully. The process handle returned a
terminal exit code, process inspection found no running campaign members or
controller, and direct inspection confirmed all one hundred owned backend
containers were stopped. All twenty independently generated fixtures and their
original databases remain preserved. No build, payment or profile overlapped the
idle observations. The command was
`python3 "$RUN_ROOT/run_e05_fresh_idle_pairs.py" --first 6 --last 10`.

Both arms retain production base `6a722ef73`. Immutable baseline executable SHA-256
is `3d94b694a1606af60a408d26bee454e5d418ae9472454e0e3900fc91f6ac2008`;
candidate is `61d5e9c56e48b128a0867196b8c884df978c4b2d302b65a43b7a21458d252f5e`.
No temporary probes, flags, source changes or measurement settings were added
between the first five and final five pairs. The shared analyzer checks paired
inputs, alternating order and unchanged executable fingerprints. All twenty
state-stability guards passed. The first ten rows remain byte-for-byte unchanged
in [the complete numeric ledger](benchmarks/2026-09-08-checkpoint-fresh-idle.csv).

| Metric | Baseline mean | Candidate mean | Paired ratio; 95% interval |
| --- | ---: | ---: | --- |
| Aggregate member idle CPU, seconds | 2.340848 | 1.306259 | 0.559169; 0.535238–0.584171 |
| Retired instructions | 13,936,112,314 | 3,076,173,087 | 0.220722; 0.218899–0.222560 |
| Package wakeups | 870.7 | 848.7 | 0.952776; 0.743647–1.220717 |
| Interrupt wakeups | 14,111.5 | 14,075.5 | 0.997320; 0.984441–1.010368 |
| Peak sampled aggregate RSS, bytes | 331,766,170 | 334,692,352 | 1.010453; 0.965873–1.057091 |
| Peak sampled aggregate physical footprint, bytes | 120,351,548 | 123,366,211 | 1.025454; 0.997116–1.054598 |

CPU improves 44.08% and instructions 77.93%, consistent with removed hashing.
However, both memory intervals still permit increases above 5%. Package wakeups
remain inconclusive. There is no confirmed memory regression, but uncertainty is
not a passing guard. **Revert E05 after ten pairs**, retaining every observation
and the lower-RSS baseline in pair 3. No eleventh pair or altered observation
recipe is used to obtain a passing result. Reconsideration requires new profiling
and a newly justified experiment; the component or idle savings alone do not
establish payment gains or override the agreed decision rule.

The 8,800-byte rejected patch is preserved privately with SHA-256
`d9de0ac2e6435aa58683b73b84bb24cdd90b489bd23442a2911eec7acfea6828`.
Only `checkpoint.rs` in cdk-common and `node_runtime.rs`/`checkpoint_manager.rs`
in cdk-axum were restored to retained HEAD. The first restore attempt was denied
by the filesystem sandbox before mutation; the authorized retry succeeded.
Broadcaster/replay candidates remain separate. Existing scoped correctness and
component evidence are historical results for the rejected implementation;
prepared payment guards have not been run against it. The next live work resumes
fresh payment validation on the retained executable, followed by supported
payment-path profiling. No final campaign gate is claimed complete.
