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
