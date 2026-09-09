# Federation performance optimization journal

Status: original campaign closed; targeted digest/collector reconsideration is in progress. Full campaign acceptance remains incomplete. See the [plan](bls-federation-performance-plan-2026-09-08.md)
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

### Retained-lineage fresh payment refresh after E05 disposition

Before the next payment optimization, exercise the retained implementation on an
original, independently generated and still-unfunded five-member HTTP/SQLite
fixture. The immutable executable is the E05 baseline artifact above, whose
production tree matches current retained HEAD: the intervening commits contain
only evidence/documentation. It excludes every remaining provisional candidate
and the rejected checkpoint/collector changes. This is fresh functional and
profiling preparation, not an E05 candidate comparison or final combined matrix.

Reuse the prepared one-shot helpers via
`python3 "$RUN_ROOT/run_retained_ecash_refresh.py"`. Resume only the original
fixture and backend, verify unchanged empty journal/payment state, and open the
original funding wallet if Bitcoin did not autoload it; never create a replacement
wallet. Fund 8,192 sat on-chain with an independent five-second miner and preserve
the deposit intent before its sole submission. Require 256 proofs of 32 sat, then
package 64 unique tokens offline. The persistent SDK receives them at concurrency
four into independent wallets with warm connections, a 60-second window and
120-second drain. Keep all failures, pending outcomes, proof shapes and causal
resource bounds. Require 2,048 sat in recipients and empty member queues before
stopping owned services. A failure preserves original identities for diagnosis;
the helper refuses re-execution against an existing case.

Post-payment restart and incoming/outgoing Lightning plus outgoing on-chain
checks remain subsequent gates. The refresh does not replace comparisons against
the original campaign baseline and cannot establish a new speedup by itself.

The retained-code refresh completed all 64 receives inside the window, but its
wrapper then failed an incorrect assumption that every recipient would have one
output proof. The ordinary SDK uses `ReceiveOptions::default()` and produced
12 proofs per fresh wallet: four 1-sat, four 2-sat, three 4-sat and one 8-sat proof.
All recipients hold exactly 32 sat. The wrapper stopped owned services on failure;
its original error, native samples, SDK receipts and wallet state are preserved.
No token was resubmitted and no payment was recreated.

A read-only reconciliation checked every original recipient database: 2,048 sat
in 64 wallets plus 6,144 sat remaining in the bank equals the 8,192-sat confirmed
deposit. All five members have 77 journal operations in session zero and matching
last order/application digests; each records exactly 8,192 paid and issued. The
one-run diagnostic p50/p95 were 1,014.466/1,213.608 ms; all 64 completed before the
60-second window ended, with no late, failed or pending requests. This finite
burst does not establish sustainable throughput or a new performance gain.
Historical post-window health and collector CPU were not saved before the wrapper
assertion; they remain unavailable and are not reconstructed as passing values.

The guard now validates receipt value and records actual output counts for paired
input checks, preserving ordinary denomination behavior. The restart helper
reads original SDK receipts and stopped durable member state rather than depending
on a wrapper summary or fabricating a historical health check. The next one-shot
command, `python3 "$RUN_ROOT/run_retained_payment_directions.py"`, restarts those
same members and verifies original state and all 65 wallets, checks original token
spent states, then exercises incoming/outgoing Bark Lightning and outgoing BDK
on-chain with independent receipts and the fixed miner. A further restart compares
payment identities, quote amounts, journal/application evidence, action lineage
and wallet state. All outcomes remain pending until those commands finish.

The first post-ecash restart attempt stopped at a guard error before any new
payment direction began. All five members had the same 77-operation history,
session zero, unchanged paid/issued amounts and no audit mismatch, but each still
reported one queued recovery item at the first all-ready status response. The
helper incorrectly asserted empty queues immediately instead of waiting for its
full readiness predicate. This short attempt does not demonstrate a persistent
production queue failure or successful bounded recovery.

Its original logs, logical-state snapshot and helper source are preserved in a
separate attempt directory. The corrected predicate keeps the original
180-second deadline, records every status observation, and requires converged
ready members, empty queues and unchanged payment history. Legitimate monotonic
session advancement is permitted; session regression is rejected. Payment-state
and wallet snapshots still must match exactly. The same stopped member/backend
state is resumed with a separate controller log; no deposit, token or invoice is
recreated. New Lightning/on-chain directions remain unstarted until this gate
passes.

### Retained-code fresh directions and restart refresh passed

The corrected restart continuation exited successfully. It first reached ready,
converged members with empty queues, then verified all original ecash tokens were
spent. Incoming Bark Lightning completed with independent payer evidence and
4,096 sat issued. The independent outgoing Lightning recipient received 1,000
sat. Outgoing BDK on-chain delivered 1,000 sat to the saved destination. No
original deposit, token or invoice was resubmitted.

Functional CLI observations, including process startup (not matched performance
baselines): incoming Lightning invoice preparation 2,308.112 ms, payer completion
1,261.460 ms and subsequent CLI exit 4,189.050 ms; outgoing Lightning 5,854.660 ms;
outgoing on-chain 39,132.475 ms including required block waiting under the
independent five-second miner. These do not isolate confirmed-state notification
latency or establish an optimization gain. The on-chain observation warrants
new phase attribution before evaluating the existing broadcaster candidate.

Both the post-ecash and final post-payment restart snapshots compare identically
across each original member and all 65 wallets. The respective preserved journal
histories contain 77 and 123 operations. Final pre-restart status showed all five
members in session zero, ready and converged with zero queued/pending-finalization
items; each held two mint quotes totaling 12,288 sat paid and issued and two Bark
actions. Post-restart readiness and exact logical-state checks passed as well.
The wrapper then stopped all owned members and backends.

Wallet accounting: 12,288 sat deposited minus 2,000 sat external recipients and
315 sat reported melt fees equals 9,973 sat (7,925 bank plus 2,048 recipients).
The Lightning wallet reports 30 sat fees; the BDK wallet reports 285 sat. The
additional one-shot read-only command
`python3 "$RUN_ROOT/audit_retained_onchain_receipt.py"` opened only the original
stopped Bitcoin backend and queried the saved transaction and previous outputs.
It independently verified one input totaling 8,192 sat, two outputs totaling
7,907 sat, the exact 1,000-sat destination outpoint, a 285-sat transaction fee and
three confirmations. It submitted no payment, mined no block and stopped the
backend afterward. This is not a full Bark custody/fee audit.

All 64 original ecash request timings, outcomes and actual input/output proof
counts are retained in
[the numeric refresh ledger](benchmarks/2026-09-08-retained-ecash-refresh.csv).
Raw native samples, CLI logs, token/quote identities, payer/recipient receipts,
failed helper attempts and restart snapshots remain private. No temporary probe
or new production optimization was introduced. This closes this retained-build
functional refresh only, not the campaign matrix or final combined acceptance.

### E03a renewed attribution on retained code

Hypothesis: the retained broadcaster's five-second periodic wakeup still adds
avoidable delay after accepted `TransactionSigned` and broadcast intent. Earlier
payment comparisons were invalidated by input/start-state mismatches; their
percentages remain withdrawn. The fresh 39.132-second outgoing CLI observation
includes block waiting and cannot establish how much of that time belongs to the
broadcaster. New attribution precedes another keep-or-revert comparison.

Build a diagnostic-only executable from retained production base `6a722ef73`
using `python3 "$RUN_ROOT/build_retained_bdk_publication_probe.py"`. It excludes
both provisional implementations. A temporary probe records at most 256 public
proposal events per process on one process-local monotonic clock: accepted signed
and intent publication, subsequent broadcaster observation, backend broadcast
start/end and confirmation observation. The existing phase timers remain the
source for phase durations. No secrets, payloads or external clock subtraction
are needed; cross-member clocks must not be combined. The probe does not change
control flow, authorization, retry policy, confirmations or transaction bytes.

The builder saves the bounded patch and exact command, uses the same pinned Nix
release feature set, and restores all original source in `finally`. Its binary is
marked diagnostic and cannot be used for final performance acceptance. Subsequent
profiling will use a new independently receipted payment on preserved original
state with the fixed miner; compile/preparation and profiling overhead stay out
of accepted payment comparisons. No new optimization is accepted by this build.


#### Retained-code diagnostic result and limitations

The build completed and byte-for-byte restoration of all four provisional source
files passed. Diagnostic executable SHA-256:
`ae613189721cf5114c59936db3561373c1863d034ddf9a0d262ec21d6ad028f2`;
probe patch SHA-256:
`29af1a9af7c9615b672f70a4037d42444ef7f6eeaa48d6498bf9f25624a6e7f3`.
The committed production tree still matches retained base `6a722ef73`.

Commands, executed serially:

```sh
python3 "$RUN_ROOT/profile_retained_bdk_publication.py"
python3 "$RUN_ROOT/analyze_retained_bdk_publication.py"
```

The private profile helper resumed the original five-member HTTP/SQLite fixture
at 123 accepted operations, session zero and empty queues. Logical member state
and all 65 wallet snapshots matched across restart. It switched only the stopped
fixture's executable link, used the original custody databases, and restored the
retained executable after bounded shutdown. No database or key was copied.

The first attempt failed while serializing a binary wallet transaction ID before
creating an address, payment intent or miner thread. Original logs and the helper
at failure are preserved. The helper now serializes private binary IDs as hex;
a separately named retry preserved the first attempt and completed one new
1,000-sat outgoing BDK payment. The original payment was never retried.

Workload: five members, full denomination configuration, HTTP, SQLite, one CLI
payment from the preserved wallet; independent fixed five-second miner; independent
recipient RPC observations at 500 ms; a 20-second member-one sampling profile
starting immediately before the CLI. Compilation and restart preceded measurement.
The one-shot manifest preserves exact server and CLI hashes and original inputs.
CLI timing includes startup and diagnostic overhead; the sampling profile covers
only the early payment path, not the full operation.

Each member recorded 16 probe events for one proposal, below the 256-event cap:
five signed publications, one signed observation, five intent publications, two
intent observations, one broadcast start/end and one confirmation observation.
Repeated publications are retained rather than collapsed without explanation.
Within each member's own monotonic clock:

- First signed publication to broadcaster observation: 3,852.035–3,858.582 ms;
  measured from the last signed publication instead: 3,839.621–3,846.609 ms.
- First intent publication to observation: 4,177.248–4,185.614 ms;
  from the last intent publication: 3,769.671–3,777.634 ms.
- Backend broadcast RPC: 0.797–4.322 ms.
- Broadcast completion to confirmation observation: 4,996.744–4,999.941 ms.
  This includes external block waiting and cannot all be attributed to polling.

All member-level numeric observations are in
[the diagnostic ledger](benchmarks/2026-09-08-retained-bdk-publication-probe.csv).
Members are correlated participants in one payment, not five independent benchmark
samples. No cross-member clock subtraction or sum of overlapping spans is used.

The independent recipient first observed the transaction during the driver's
19.838–19.842-second RPC interval and its first confirmation at 20.344–20.348
seconds. CLI duration was 35,539.455 ms. The 15,259.116 ms between observed first
confirmation and CLI exit is not an isolated federation notification delay:
first confirmation need not satisfy the configured settlement policy, and the
existing phase traces are unavailable. Do not infer the missing phase breakdown.

Although the targeted `RUST_LOG` filter selected trace events, the configured
console/file writer levels remained INFO/DEBUG. Both writers suppress TRACE.
`setup_tracing` in `crates/cdk-mintd/src/lib.rs` confirms this independent filter.
The bounded INFO probe worked; existing transaction phase traces did not. Future
phase profiles must explicitly configure the relevant writer level using the
normal configuration interface, then restore it. The partial trace result is
preserved and is not relabeled a complete profile.

The sampling profile shows parked runtime threads and active BLS field/group
arithmetic among the prominent frames during its early-path window. Thread sample
counts are not CPU seconds or evidence of sustainable throughput. This profile
does not establish another crypto optimization; raw stacks remain private.

Independent Bitcoin receipt and previous-output inspection verified one input,
two outputs, exactly 1,000 sat to the saved outpoint and 285 sat transaction fee.
The original bank decreased from 7,925 to 6,640 sat, exactly amount plus fee.
All five members finished at 154 accepted operations, session zero, ready and
converged with empty queues. The recipient wallets were not spent. Cumulative
wallet accounting is now 12,288 sat deposits = 6,640 bank + 2,048 ecash recipients
+ 3,000 external recipients + 600 reported fees; full Bark custody accounting
remains a separate final gate. Sampling exited successfully, miner/observer
errors were empty, all owned services stopped, and the original executable link
was restored. Do not rerun payment helpers expecting the earlier 123-operation
or 7,925-sat state.

Disposition: fresh causal evidence supports a new uninstrumented comparison of
E03a on retained production. It establishes seconds of local scheduling delay
before millisecond-scale broadcast RPCs, not an accepted improvement. Preserve
periodic repair, exact-byte broadcast, durable authorization and confirmation
policy. Five matched alternating pairs with state/input guards are still required;
expand to ten if inconclusive. No production optimization is accepted here.


#### E03a fresh comparison preparation

The renewed retained-code probe supports measuring the existing E03a candidate.
Hypothesis: notifying the existing broadcaster after applied signed/intent
publication removes periodic scheduling waits without changing transaction work,
retry policy, authorization or confirmation requirements. Owning layer: Axum
federation publication and broadcaster lifecycle. Primary metric: uninstrumented
SDK payment latency with matched fee/proof/UTXO shapes and an independent miner;
CPU/payment, memory, idle work and queue stability remain acceptance guards.

Freeze the candidate with
`python3 "$RUN_ROOT/build_e03a_retained_runtime.py"`. Its release features and
retained source base match the uninstrumented baseline. The only source changes
are the two existing broadcaster/publication files; exclude E07 and all rejected
changes, and include no temporary probe. Save the exact patch, build command and
artifact hashes; restore all four original provisional source files in `finally`.
Building an executable does not accept the optimization. Before running the new
five-pair series, prepare independent original fixtures and verify empty startup
queues, matching fees and actual payment shapes. Do not reuse invalid historical
pair series or their already-paid identities.


The E03a candidate build completed with SHA-256
`bf9e434e3bcc94c52b91548d289caa3f8c41afd8c9f591b0241db9fc13468e34`;
its two-file patch hash is
`7ba3a68f31f80e3c6e52b16ffde43e61925daf5a76ac88ddc33bbc6f79e1f941`.
All four original provisional files were verified against their saved bytes after
restoration. Baseline remains the uninstrumented retained executable, SHA-256
`3d94b694a1606af60a408d26bee454e5d418ae9472454e0e3900fc91f6ac2008`.

Add the receive example's optional, bounded measurement-start handshake to
`federation_melt_performance`: write preparation time/balance/proof-count readiness,
wait at most 60 seconds for the private controller signal, then start the existing
monotonic transaction window. The default remains immediate execution. This is
reusable benchmark support, not a production fast path. It separates resource
sampling from wallet recovery/opening and connection warmup. Both arms use one
frozen SDK artifact; pinned release build and scoped strict Clippy passed.

Fresh serial comparison workload, declared before running: five 1,000-sat outgoing
BDK payments per arm; independent five-member HTTP/SQLite/full-denomination
ceremonies and backend custody; 131,072-sat confirmed deposits issued as 64 proofs
of 2,048 sat; 60-second fixed warmup and a separate 30-second idle observation;
empty queued/pending-finalization state with 13 accepted funding operations;
unchanged configured confirmation/checkpoint policies; independent five-second
miner. The existing private fee fixture supplies 2 sat/vB to fee-estimate calls
only, forwarding all other RPCs unchanged. The SDK checks the 3,478-sat quote
reserve before initiation. Actual proof selections, swaps, input/output counts,
fees and image/resource settings must match within each pair; independent Bitcoin
previous outputs must verify each 285-sat fee and exact destination value.

Run one fresh arm using
`python3 "$RUN_ROOT/run_e03a_retained_arm.py" --pair 1 --arm baseline --order 1`,
then its candidate arm; alternate order in successive pairs. This extends the
existing local payment driver, fixed-fee proxy, independent backend/ceremony
helpers, native process sampler and common analyzer. It never invokes the old
pair controllers or reuses their payment identities. Preserve each arm's complete
private inputs, results, native samples, independent receipts and cleanup result.
Use empirical per-run p50/p95; five payments do not establish p99 or sustainable
throughput. No candidate is accepted from an initial arm or partial pair.


#### First retained baseline arm and benchmark handshake validation

The first baseline arm completed without retries. All five SDK attempts completed
inside the measurement window, with zero failures, unfinished operations or late
completions. Original destination outputs and previous Bitcoin outputs verified
five 1,000-sat receipts and five 285-sat fees; the funded wallet decreased from
131,072 to 124,647 sat. All members agreed within the live run, beginning at 13
operations with empty queues and ending at 169 operations with empty queues.
Owned members/backends and the private fee proxy stopped with state preserved.

The SDK preparation marker recorded 779.638 ms, 131,072 sat and 64 proofs; the
controller released the handshake afterward, before the existing monotonic
payment window began. This validates the reusable optional gate on a real
five-payment run. It changes no ordinary wallet API or production runtime.
Pinned release build, scoped strict Clippy and `cargo fmt --all -- --check` passed.
Retain and commit the benchmark support separately from the provisional E03a
optimization; no production speedup is accepted by this support change.

All five baseline request latencies, milliseconds: 37,092.678542; 36,928.043292;
32,509.791209; 36,376.258792; 35,618.214666. Empirical median 36,376.258792 ms;
empirical p95 37,092.678542 ms; p99 unavailable. Aggregate member CPU per payment
4.9050518832 seconds; instructions per payment 43,185,791,095.4; sampled aggregate
peak RSS 442,384,384 bytes and physical footprint 166,236,696 bytes. The separate
30-second idle observation used 1.203483084 aggregate member CPU-seconds. These
are one baseline arm's observations, not a confidence interval, accepted gain,
or sustainable-throughput conclusion.

The full request records, actual shapes, native samples, independent receipts,
readiness and cleanup results remain private. The resource envelope starts after
SDK preparation and includes up to one 500-ms controller observation interval
at completion, plus RPC/sampling overhead; transaction latency comes from the
SDK's own monotonic clock. The nominal polling interval alone is not a strict
upper bound on completion-observation overhead.
The comparison runner reuses only an arm with complete receipts/results and
verified stopped backends. An existing partial arm fails closed and requires
identity reconciliation; it is never automatically rerun.


#### Matched-series continuation and pending E03a correctness gates

Started `python3 "$RUN_ROOT/run_e03a_retained_pairs.py" --first 1 --last 5`
after the first baseline controller exited successfully. The runner collected
that arm's original completed evidence and verified its five owned backends were
stopped. It then began the candidate counterpart using new backend custody and
an independent ceremony. Every subsequent arm is serial; order alternates by
pair. No performance decision is made from partial results.

A read-only coverage pass identified the focused post-measurement gates below.
These are planned commands, not new passing test claims. Run with the pinned Nix
toolchain and the existing 16 MiB test thread stack after all measured services
stop, isolating E03a from provisional E07:

```sh
cargo test --locked -p cdk --features mint,federation --lib transaction_signing
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib federation::broadcast::tests
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib federation::services::tests
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib federation::tests::frost_signing
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib federation::tests::finalization
cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib recovery
```

`transaction_signing` covers rejected premature observations/wrong settlement
policy, persisted intent, unknown external results, exact already-known recovery,
and restored signed/broadcast state. The broadcaster tests preserve backend
classification, retry delays and safe diagnostics. Service tests cover draining
children, grace-period abort, first-failure propagation and sibling cancellation.
FROST scenarios cover unavailable quorum, catch-up admission before nonce use,
slow peers, lost commitments/shares/acknowledgements and exact attempt recovery.
Finalization tests cover lost applied and rejected publication. Recovery tests
cover trusted checkpoints, deterministic suffix replay and restart/catch-up.

These tests are distinct safety boundaries; retain them. Existing publication
suppression deliberately skips notification, so the unchanged periodic repair
path remains essential. Source inspection confirms notifications select only
applied signed/intent entries; each awakened round still performs readiness,
authorization/state, retry and exact-transaction checks. Inspection is not evidence
that runtime gates passed. Scoped strict lint, ordinary/core federation feature
checks and a real candidate restart/payment-state reconciliation remain necessary
before retaining E03a, in addition to the matched performance decision.


#### First complete retained-code pair: provisional observations

Pair one completed both arms and passed the runner's actual-input comparison.
All ten payments have independent 1,000-sat recipient receipts, independently
audited 285-sat Bitcoin fees, exact wallet debits and terminal SDK results inside
the window. Both arms began with 13 operations/empty queues and stopped all owned
services afterward. Pair two then started in candidate-first order.

Candidate request latencies, milliseconds, retaining every observation:
21,021.722708; 35,387.600375; 19,608.195666; 20,238.056042; 19,803.11125.
Candidate empirical p50/p95 are 20,238.056042 / 35,387.600375 ms, versus baseline
36,376.258792 / 37,092.678542 ms. The longer candidate second payment is retained;
it must not be removed to improve a tail comparison. No p99 or run-level
confidence conclusion is available from one pair.

Aggregate CPU/payment: baseline 4.9050518832 s, candidate 3.8976814418 s.
Instructions/payment: 43,185,791,095.4 versus 37,442,062,783.4. Sampled peak
aggregate RSS: 442,384,384 versus 439,861,248 bytes; physical footprint:
166,236,696 versus 162,386,456 bytes. Separate 30-second idle CPU: 1.203483084
versus 1.106509374 s. These observations do not yet establish repeatability or
pass the regression guards.

Read-only inspection of the stopped original journals further checked accepted
proof shapes, beyond SDK preparation counts: each arm had one swap with three
input proofs and 38 outputs; the five melts consumed 8, 9, 9, 9 and 11 proofs,
with 12 requested outputs each. The independently receipted Bitcoin transactions
had matched one-input/two-output shapes. Retain this additional journal-derived
shape comparison for the final series audit.

Final accepted history was 169 operations baseline and 164 candidate. Counts by
operation type show the entire difference is chain observations (55 versus 50);
all other operation-type counts match, including five proposals, five signed
transactions, five broadcast intents, fifty broadcast observations and five
melts. The fixed-schedule miner naturally produces fewer blocks over a shorter
run; final history is an observed outcome, not the initial-history matching gate.
Do not call these five fewer chain observations fewer user payments or assume
that they prove a separate optimization. Digests agree within each live run;
unrelated ceremony digests are not compared across revisions.

Disposition remains provisional. Complete all five matched pairs, calculate the
agreed intervals and expand to ten if inconclusive; preserve the slower sample
and every rejected/incomplete result. No production change is accepted here.


#### Follow-up attribution queue from the first matched pair

The SDK's own monotonic phase boundaries further locate the long candidate
second request: quote creation 941.069875 ms, local preparation 30.4795 ms and
`confirm()` 34,416.051 ms. Other candidate confirmation calls ranged from
18,444.386458 to 20,373.734125 ms. The call includes federation submission,
optional swap, signing, broadcast, block waiting and settlement; it is not a
measurement of Bitcoin confirmation alone. These sequential SDK boundaries share
one clock. All per-request phase differences are preserved privately.

Read-only source inspection identifies existing five-second wallet-liveness and
on-chain-observer rounds, a separately configured descriptor-wallet sync loop,
and confirmation reads from the locally persisted wallet graph. The liveness
worker also has explicit proposer/signing leases and aggregator grace. These
facts nominate attribution points; they do not prove that any particular delay
caused the slower request or authorize reducing a safety-related lease.

After the matched series and its decision, profile retained/candidate operation
phases with the normal logging writer explicitly permitting TRACE, keeping
connection setup and fixed mining separate. Correlate proposal acceptance,
liveness/signing, persisted wallet synchronization, local confirmation evidence,
accepted settlement and SDK completion. Preserve independent validation,
confirmation requirements, durable identities, retry delays and failure backoff.
Only consider another notification or work-removal change if that profile proves
its owning layer and cost. No new production optimization or timer tuning is
introduced by this inspection.


#### Additional accepted-change work audit

Added the private read-only audit command
`python3 "$RUN_ROOT/audit_e03a_retained_completed_pairs.py" --through-pair 1`.
It checks only terminal, cleaned-up pairs against their original databases and
SDK/recipient receipts; it cannot submit a payment. The first ten receipts pass.
Across all five members in each arm, accepted melt input values are 4,478 sat,
with 12 requested outputs and seven persisted change signatures totaling 3,193
sat per payment. Each reconciles 4,478 = 1,000 recipient + 285 fee + 3,193 change.
The three-input/38-output preparatory swap and all five actual melt input counts
match across arms. This strengthens the actual-work guard beyond the SDK's
preparation selections, which precede any necessary swap.

The audit preserves per-request quote, preparation, confirm and total durations,
accepted proof input/output counts, actual change counts/value, Bitcoin shapes,
fees and recipient amounts in a private numeric CSV. It checks every member's
accepted shape, terminal history, original mint funding and exact SDK quote
association. Repeat it over all complete pairs before final series acceptance;
never infer actual returned change count solely from requested output slots.
No production instrumentation is added and no performance gain is accepted.


#### Two completed pairs: retained numeric evidence, decision pending

The reversed-order second pair completed with all ten receipts and actual-input
checks passing. Its candidate median/p95 were 20,213.234167 / 21,369.064041 ms;
baseline median/p95 were 35,303.692 / 38,227.130833 ms. CPU/payment was
3.8233765664 versus 4.5956104998 seconds. These remain descriptive observations;
no interval is calculated before the prescribed five complete pairs.

The accepted-change audit was extended with
`python3 "$RUN_ROOT/audit_e03a_retained_completed_pairs.py" --through-pair 2`.
All twenty original requests, independent receipts and every member's accepted
proof/change shapes passed. The third pair proceeded with new independent state;
no original payment was rerun. Save all observations, including the longer first
candidate second request, in the numeric-only
[per-request ledger](benchmarks/2026-09-08-bdk-retained-broadcast-requests.csv) and
[per-run ledger](benchmarks/2026-09-08-bdk-retained-broadcast-runs.csv).

The request ledger includes SDK phase durations, preparation selections, actual
accepted input counts/value, requested versus returned change output counts,
change value, reserves, audited fees and Bitcoin transaction shapes. The run
ledger includes all measured latency/CPU/instruction/memory/idle counters,
collector CPU, initial/final history and terminal-window accounting. Empty p99
values mean insufficient samples, not zero latency. Raw identifiers, bearer
material, original transactions, process samples and environment details remain
outside Git. Update these ledgers with every later completed arm; do not select
only favorable observations. Correctness gates and final acceptance remain open.

#### Five complete retained-code pairs: performance gate passed

The prescribed five pairs finished in alternating AB/BA order, with no failed,
unfinished, late or pending payments. All fifty independent recipient receipts
and every member's accepted input/change records pass the read-only audit. The
[request ledger](benchmarks/2026-09-08-bdk-retained-broadcast-requests.csv) now has
all fifty requests; the [run ledger](benchmarks/2026-09-08-bdk-retained-broadcast-runs.csv)
has all ten arms, including the slower first-pair candidate request. Existing
ledger rows are unchanged. The `order` column records execution order; rows are
presented baseline/candidate within each pair.

Commands completing the series and recomputing the decision:

```sh
python3 "$RUN_ROOT/run_e03a_retained_pairs.py" --first 1 --last 5
python3 "$RUN_ROOT/audit_e03a_retained_completed_pairs.py" --through-pair 5
python3 "$RUN_ROOT/summarize_e03a_retained_comparison.py" --pairs 5
```

The pair controller exited successfully and stopped every owned fixture. Both
artifacts are uninstrumented, built with the same pinned release settings from
retained production base `6a722ef7305ba941ac8d5267d95413a89c45247c`. Baseline
SHA-256 is `3d94b694a1606af60a408d26bee454e5d418ae9472454e0e3900fc91f6ac2008`;
candidate is `bf9e434e3bcc94c52b91548d289caa3f8c41afd8c9f591b0241db9fc13468e34`.
The candidate contains only E03a's accepted-state broadcaster notification,
patch SHA-256 `7ba3a68f31f80e3c6e52b16ffde43e61925daf5a76ac88ddc33bbc6f79e1f941`.
E07, reverted E05/E08b and diagnostic probes are excluded. The persistent SDK
artifact is identical across arms, SHA-256
`e68f38ff26b411c44126a05ffd7460d44540be91f6e3c58cbae7832776c4358f`.

Five independent five-member HTTP/SQLite ceremonies per arm each began payment
measurement with 13 accepted operations, empty queues, identical policies,
131,072 sat in 64 proofs, and the SDK preparation handshake completed. Five
serial 1,000-sat payments per arm used the fixed five-second miner and matched
2-sat/vB fee estimate. Actual per-payment fee was 285 sat; final bank balance was
124,647 sat. Accepted melt input counts were 8, 9, 9, 9, 11; each consumed 4,478
sat and returned seven change signatures totaling 3,193 sat. Bitcoin transactions
had one input and two outputs. Histories and digests agree within each live arm;
independently generated ceremony digests are not compared across revisions.

The common analyzer uses paired log ratios and a Student-t interval over five
runs (four degrees of freedom). Ratios below are candidate/baseline, not ratios
of the displayed arithmetic means.

| Metric | Mean baseline | Mean candidate | Paired ratio, 95% interval |
| --- | ---: | ---: | --- |
| Per-run p50 | 36,572.242 ms | 19,909.262 ms | 0.54442 [0.52053, 0.56942] |
| Per-run p95 | 38,096.401 ms | 23,764.280 ms | 0.60859 [0.44516, 0.83203] |
| Member CPU/payment | 4.79509 s | 3.80769 s | 0.79426 [0.76670, 0.82280] |
| Instructions/payment | 42.883 billion | 36.740 billion | 0.85667 [0.83033, 0.88386] |
| Peak aggregate RSS | 439,730,176 bytes | 440,270,848 bytes | 1.00123 [0.99370, 1.00883] |
| Peak aggregate footprint | 161,658,982.4 bytes | 162,927,123.2 bytes | 1.00794 [0.97494, 1.04206] |
| Idle CPU per 30 seconds | 1.14967 s | 1.18969 s | 1.03330 [0.92013, 1.16038] |
| Idle interrupt wakeups | 6,384.2 | 6,398.6 | 1.00226 [0.99782, 1.00672] |

Idle package wakeups contain zero values, so use the absolute paired difference:
+457.2, interval [-460.19, 1,374.59], rather than a log ratio. Neither idle metric
establishes a saving. The idle CPU interval is broad: it detects no reproducible
increase but does not prove unchanged CPU. Retain this uncertainty and the final
idle/stress coverage obligation. The prespecified idle guard tests a reproducible
increase; it is not the active-load 5% non-regression guard.

The primary latency/CPU improvements are repeatable; p95, CPU/payment and both
memory upper bounds exclude regressions over 5%. The performance gate therefore
passes at five pairs under the recorded rules. This is not sustainable throughput,
not an original-`8e507cdf8` combined comparison, and not a p99 result. Five completed
samples per arm are insufficient for p99. The process resource envelope extends
through the controller's completion observation (at most one 0.5-second polling
period plus RPC/sampling overhead after completion), while SDK latencies use its
own monotonic clock. The 0.5-second interval is the nominal polling cadence, not
a hard bound on the complete observation loop.

Two evidence-helper issues did not affect payment measurements. First, recomputed
analyzer output used Python tuples where saved JSON used lists; exact comparison
failed despite identical numbers. JSON normalization fixed the helper, retaining
the failed version and all data. Second, a ledger extension initially emitted
execution-order rows, while existing CSV rows use baseline/candidate presentation.
The prefix-preservation assertion rejected the run-file write. Re-export in the
existing presentation order preserved every old row; no numeric data changed.

Disposition: proceed with the recorded focused correctness commands, scoped
strict lint, ordinary/core federation feature checks and a real candidate restart.
E03a remains uncommitted until those gates pass. The broader campaign remains open.

#### E03a correctness, original candidate restart and retention

`python3 "$RUN_ROOT/validate_e03a_retained_candidate.py" --pairs 5` completed
successfully. It first checked the exact measured two-file patch and temporarily
excluded E07, then restored every original provisional source file with a
concurrent-edit guard. No unexpected concurrent edit occurred. Commands ran
serially in the pinned Nix stable shell with `RUST_MIN_STACK=16777216`.
The six test commands listed above used `-- --test-threads 1`:

| Check | Passed | Ignored |
| --- | ---: | ---: |
| CDK transaction signing | 2 | 0 |
| Broadcast classification/redaction | 2 | 0 |
| Service lifecycle | 4 | 0 |
| FROST signing and lost-response recovery | 11 | 0 |
| Finalization and terminal publication | 9 | 0 |
| Recovery/checkpoint/replay | 46 | 1 |

The ignored test is `benchmark_unchanged_checkpoint_idle_rounds`, explicitly a
manual ten-minute release benchmark; no correctness test was skipped. These
74 passed tests cover the existing independent authorization, exact unknown-result
retry, restoration, failure propagation and lost-publication boundaries. Strict
lint and feature checks also passed:

```sh
cargo clippy --locked -p cdk-axum --no-default-features --features federation-bdk,federation-bark,prometheus,iroh --lib -- -D warnings
cargo check --locked -p cdk-axum --no-default-features --features federation,iroh
cargo check --locked -p cdk-axum --no-default-features
cargo fmt --all -- --check
```

Next, `python3 "$RUN_ROOT/restart_e03a_retained_candidate.py"` restarted only
pair five's stopped original candidate federation and original backends using
the exact measured candidate binary. It created no payment, copied no custody,
and performed no mining. All five members reached certified readiness and empty
queues at the same 163-operation history and session, with matching order and
application digests. Per-member logical hashes before/after restart matched for
mint/melt quotes, proof state, signatures, ordered journal and Bark actions.
The original wallet's proofs and transaction records also matched.

All five previously confirmed Bitcoin transaction bytes, the tip and mempool
were unchanged. The wallet retained 124,647 sat; exact value reconciliation is
131,072 funding = 124,647 spendable + 5,000 independently received + 1,425 fees.
The helper exited successfully and stopped every owned service, preserving all
state. This is a post-payment restart check, not active-payment crash injection
or completion of the full campaign recovery matrix.

**Keep E03a and commit it separately with the complete measured evidence.**
The retained implementation is a coalescing notification to the existing
broadcaster from the common terminal publication path, only for applied signed
transactions and broadcast intents. The periodic repair round, readiness gate,
consensus authority, exact bytes, independent checks and all retry delays are
unchanged. There is no new wire format, public API, dependency or temporary
production probe. E07 remains separate and unaccepted. The prior two invalid
E03 series and their withdrawn claims remain historical evidence, not inputs to
this accepted comparison. CSV line endings were normalized to LF for whitespace
validation; every cell and prior row value is unchanged.

Continue next with the documented BDK phase-attribution queue on retained code.
The five-pair serial result does not close Iroh, parallel/open-loop load,
long-history, PostgreSQL, all-direction final payments, original-baseline combined
comparison or the two final profiling passes.

#### Next experiment: retained BDK completion-phase attribution

Before editing production code, profile the accepted E03a artifact on the stopped
original candidate fixture, after its successful restart check. Hypothesis:
residual serial completion time includes recurring wallet-liveness, wallet-sync
or confirmation-observation waiting, rather than expensive Bitcoin submission.
Owning layers are the existing Axum liveness/observation workers and descriptor
wallet synchronization. The expected evidence is phase duration/count and sampled
stack attribution; this experiment has no production candidate or keep decision.

Use one new 1,000-sat payment on the original 124,647-sat wallet, the same fixed
five-second miner/fee estimate and persistent SDK. Enable the existing transaction
phase TRACE target through the normal stored logging configuration, preserving
and restoring all original configuration. Record preparation separately, retain
independent chain observations and exact receipts, and sample one member over
the payment. Keep raw logs/profiles private. Existing phase durations are
monotonic; do not subtract unrelated log/SDK/observer clocks or sum overlapping
spans as wall time. Diagnostic logging and sampling overhead exclude this run
from performance acceptance. Preserve the new payment identity if it is pending;
never rerun the initiation command to repair a diagnostic helper failure.

#### Retained BDK diagnostic: completed, attribution still open

The optimization was committed as `9aa2ae395`. The follow-up diagnostic used its
same uninstrumented runtime artifact, with the normal stored logging writer set
to permit the existing `cdk_axum::federation::transaction_latency=trace` target.
No source probe, protocol setting, quorum, confirmation rule or retry delay changed.

Two preparation-only failures are preserved. The first configuration apply lacked
the fixture's environment-resolved test RPC credential; restoration encountered
the same validation error. A subsequent read-only export proved the original
configuration had never changed. No member started and no payment was created.
The second attempt started the fee proxy before its upstream Bitcoin backend;
the proxy's unchanged regtest-chain guard correctly refused startup. All original
configurations were restored and the owned backends stopped. No payment existed.
The helper was repaired to wait for backend readiness before starting the proxy;
it does not suppress the guard or extend a production timeout.

The third attempt completed successfully. Commands, with the failed helper
versions retained in their original private case directories:

```sh
python3 "$RUN_ROOT/profile_e03a_retained_completion.py"
python3 "$RUN_ROOT/analyze_e03a_completion_phases.py"
```

Existing phase messages were verified on all five members before the single new
payment was offered. The persistent SDK selected 14 proofs without a preparatory
swap. Its monotonic total duration was 20,530.463291 ms: quote creation
947.947291 ms, local preparation 30.922167 ms and confirmation call
19,551.593833 ms. The confirmation call includes federation processing and block
waiting. This is one diagnostic, not a baseline or a repeatable improvement.

An independent observer using one monotonic clock saw the destination transaction
in the [14,218.068208, 14,221.591208] ms RPC bracket and its first confirmation in
[15,234.312041, 15,237.349791] ms. From that confirmed observation to SDK process
exit was [5,336.239084, 5,339.276834] ms on the same observer clock. This residual
includes federation settlement, client work and process exit; it is not an exact
notification delay. Do not subtract member log timestamps from those durations.

[All grouped phase durations](benchmarks/2026-09-08-bdk-retained-completion-phases.csv)
are retained by member, phase and outcome. Their scope is runtime startup through
shutdown, including startup readiness errors and periodic background rounds.
They are not an isolated payment CPU or error-rate population. In member one's
single relevant preparation/signing path, existing monotonic timers measured:

| Phase | Duration |
| --- | ---: |
| Fee estimation | 0.490 ms |
| PSBT construction | 0.222 ms |
| Address allocation, including accepted operation | 830.176 ms |
| Proposal preparation, including nested phases | 1,863.586 ms |
| FROST commitment round | 29.241 ms |
| FROST signature round | 41.797 ms |
| Signed transaction finalization | 1,129.201 ms |
| Bitcoin broadcast RPC | 2.577 ms |

The member's 15 wallet-sync calls ranged 1.040–6.910 ms, and its three local
confirmation reads ranged 0.016–0.023 ms. Ten consensus submissions ranged
600.579–1,037.083 ms. Nested and overlapping phases must not be summed as wall
time. Phase results and independent receipts support investigating scheduling
and accepted-state publication before speculative PSBT/RPC or crypto changes.
They do not yet quantify the delay from a particular accepted operation to the
next liveness/observation round.

A 20-second, one-millisecond-interval stack sample of member one returned
successfully; its controller envelope was approximately 20.72 seconds. Parked
condition-variable, semaphore and event-loop frames dominated the aggregate
thread samples; active frames included SHA-256, BLS field arithmetic, SQL writes
and synchronization. Thread sample counts are not CPU seconds or a payment's
critical path. Keep the raw sample private and do not interpret this diagnostic
as an idle-CPU saving or a reason to weaken durable writes or validation.

The new 1,000-sat receipt and 285-sat fee were independently verified. The original
wallet now holds 123,362 sat; the six total outgoing payments reconcile exactly:
131,072 = 123,362 + 6,000 received + 1,710 fees. All members converged with empty
queues at 189 accepted operations. The original five matched measurements remain
unchanged; subsequent state must not be mistaken for their post-window snapshot.
Both helpers exited, every owned service stopped, and all five stored logging
configurations were exported and compared equal to their originals after restore.

Next attribution step: capture process-local monotonic accepted-operation
publication and worker-entry boundaries, using bounded temporary probes only if
the existing phase timers cannot provide them. Measure pending melt/proposal to
liveness entry and local chain evidence to confirmed-observation/settlement entry.
Preserve proposer/signing leases and independent confirmation evidence. Only then
form a narrow notification/work-removal candidate and run the prescribed matched
comparison. No additional production optimization is accepted from this profile.

#### BDK scheduling boundary probe: preregistered diagnostic

The previous turn made progress: retained E03a was committed and a real payment
profile narrowed the next attribution step. No campaign build or member process
was live at continuation. Retained production is `9aa2ae395`; E07 remains excluded.

Build a private diagnostic artifact with one process-local monotonic origin and
at most 1,024 records per process. Record applied operation IDs, existing phase
starts/finishes (excluding the high-frequency AlephBFT pump), pending intent and
unsigned-proposal observations, and broadcaster confirmation observations. Resolve
public operation/quote/proposal associations from the original accepted journal;
do not log payloads or secrets. Preserve every publication, including duplicates.

Hypotheses: accepted melt/proposal state waits for a wallet-liveness round, and
accepted confirmed-transaction evidence waits for quote observation/settlement.
Measure these waits within each process before proposing any production change.
Reuse the existing phase timers and earlier stack sample for execution-cost
context. The private artifact preserves scheduling, leases, validation, retry
behavior and all payment policies. Build from retained production with E07
excluded, restore source immediately after the build, and preserve the diagnostic
patch/artifact privately. The run uses one new 1,000-sat payment on the original
123,362-sat wallet, fixed five-second mining and independently audited receipt/fee.
It is not a performance acceptance run; temporary probe/logging overhead is present.

#### Scheduling probe result and E10 candidate contract

The bounded artifact built successfully with the same pinned release flags.
Base: `9aa2ae3953cc69d33ee8a74dea3d1ed61dd14d4a`; artifact SHA-256:
`98d73ad2bea50086db0c812c70c42f50963a333f23298a6353dd8dd73ea2db33`;
diagnostic patch SHA-256:
`073c09d293aeec43944ef75988410ebbf78bff2d18b34c7d7611d5ea0e330e6e`.
The builder restored every source file, preserving E07 separately. Commands:

```sh
python3 "$RUN_ROOT/build_bdk_scheduling_probe.py"
python3 "$RUN_ROOT/profile_bdk_scheduling.py"
python3 "$RUN_ROOT/analyze_bdk_scheduling_probe.py"
```

The new payment completed in 23,044.581792 ms of SDK time, including a necessary
sender swap; this fragmented-wallet diagnostic is not matched with the prior
no-swap diagnostic. Its 1,000-sat recipient output, 285-sat fee and exact wallet
debit were independently checked. The original wallet now holds 122,077 sat:
131,072 funding = 122,077 spendable + 7,000 received + 1,995 fees. Members
converged with empty queues. Original configurations and the retained runtime
executable link were restored, and all owned services stopped.

The [scheduling boundary ledger](benchmarks/2026-09-08-bdk-scheduling-boundaries.csv)
uses only same-process monotonic timestamps, with operation/quote/proposal IDs
resolved from the original journal. Every event index is unique and the probe
limit was not reached. All publication records, including any duplicates, are
retained in the private correlation data. First-publication and last-publication
before observation are reported separately; no negative delay is silently dropped.

Accepted melt → first pending-intent observation was 256.442–267.191 ms across
five members. Accepted proposal → first unsigned-proposal observation was
2,992.098–2,997.496 ms. Member four began signing 2,995.011 ms after publication,
immediately after observing the proposal at 2,994.978 ms. Source inspection
confirms the existing liveness loop only selects cancellation or a five-second
interval; its round uses a transaction-state snapshot loaded before proposing.
Thus an accepted proposal waits for a subsequent round even though the exact
accepted state is already available. This supports a proposal wakeup experiment.

After the fourth independent confirmed broadcast observation was published,
the first paid-quote observation was published 1,624.793–1,625.518 ms later.
Member one's next quote probe started after 498.441 ms; other members' probes
started just after the first paid observation, consistent with the existing
accepted-payment wakeup. No probe overlapped that quorum-publication boundary.
This run does not support attributing the entire earlier 5.34-second residual
to a missing quote notification. Keep confirmation/settlement attribution open;
do not change it together with the proposal experiment.

**E10 hypothesis, before editing:** wake the existing wallet-liveness worker on
an applied `TransactionProposal` through the common terminal-publication path.
Use one coalescing permit owned by the existing worker. Expected work change:
remove the roughly three-second observed wait until the next signing round;
no new queue, periodic task or state cache. Primary metric: serial BDK outgoing
latency, with member CPU/payment, p95, memory and separate idle guards unchanged.
Do not wake on all melts or tune leases, quorum, confirmations, retries or backoff.
Every awakened round still reloads authoritative state and follows the same
readiness, scope, proposer/signing schedule, nonce and independent-validation checks.
The periodic round remains necessary for lost publications and restart.

Build separate uninstrumented retained baseline/candidate artifacts. Confirm
boundary removal with bounded diagnostic evidence, then run five alternating
matched pairs using the existing fresh five-member HTTP/SQLite BDK workload
(five serial 1,000-sat payments, 131,072-sat initial funding, 64 initial proofs,
13 initial accepted operations, fixed five-second miner and matched fee estimate).
Expand inconclusive results to ten and revert if still inconclusive. Keep E10
provisional until the comparison and relevant FROST/lifecycle/recovery, feature,
lint and real candidate restart checks pass. The wider campaign matrix is unchanged.

#### E10 diagnostic validates the boundary, exposes a work-risk

The candidate diagnostic built and ran successfully. Its accepted-proposal to
unsigned-observation delay was 1.259–1.538 ms across five members, compared with
2,992.098–2,997.496 ms in the earlier retained-code diagnostic. All signing starts
followed within 1.272–1.551 ms of publication. See the complete
[candidate boundary ledger](benchmarks/2026-09-08-e10-scheduling-boundaries.csv).
Each run uses its own process-local clock; this comparison is of within-process
durations, not subtraction across process origins. Both diagnostic probes stayed
below their 1,024-event caps, and no temporary probe remains in source.

The candidate also exposes an important risk: all five members completed FROST
coordination in this run, versus one in the earlier run. The unchanged signing
aggregator grace allows this depending on the current attempt/lease position.
Do not discard these extra coordinations or assume fewer elapsed seconds means
less CPU. Their resource effect must pass the matched CPU/payment and memory
criteria. Do not change the grace or quorum to make this experiment pass.

The candidate diagnostic payment completed in 17,740.111875 ms, including a
four-input preparatory swap; the prior diagnostic required twelve swap inputs.
These different holdings, lease positions, block alignment and instrumentation
make the totals unsuitable for a paired latency claim. The narrower observed
worker boundary supports proceeding to a properly matched comparison.

The 1,000-sat recipient amount and 285-sat fee were independently verified.
Original wallet value is now 120,792 sat: 131,072 = 120,792 + 8,000 received +
2,280 fees. All five members converged with empty queues at 253 accepted
operations. All seven pre-existing receipts and logical state were checked on
startup. The controller stopped every owned service and restored original
configuration and the retained executable link after the new payment.

The first diagnostic launch encountered an automatic approval-review timeout
before command execution. Absence of the one-shot case proved that no run/payment
had started; the review's allowed single retry succeeded. This was an execution
approval delay outside every measured transaction window, not a payment failure.

Commands completing the candidate diagnostic and freezing clean comparison code:

```sh
python3 "$RUN_ROOT/build_e10_scheduling_probe.py"
python3 "$RUN_ROOT/profile_e10_scheduling.py"
python3 "$RUN_ROOT/analyze_e10_scheduling_probe.py"
python3 "$RUN_ROOT/build_e10_runtime.py"
```

Diagnostic artifact SHA-256:
`0e6514ea3af15d40bda547b845f2769437af11d3cd63d57ede3c19e001b4e2a0`;
candidate-only patch SHA-256:
`2c230ece887cf2219aa7306d78e93724359a663dfcf15ae6053eb3bc50dab968`;
diagnostic patch SHA-256:
`122edcfaa1be349b009e3a825fe5d43c80478b11efdb61ade085779427d1469d`.
The clean comparison builder verifies that exact candidate-only patch, excludes
E07, and restores both provisional changes afterward. Neither comparison binary
contains the probe.

The matched runner now also scrapes the existing phase-count metrics once per
member before timing and after post-payment convergence, identically in both
arms. It records FROST coordination counts to expose extra work. Scrapes are
outside the CPU/latency measurement envelope; their counts bracket a wider
interval and are not CPU seconds. No operational metric or production fast path
was added. Keep all independent fixtures, initial inputs, failure accounting and
five/ten-pair acceptance rules unchanged. E10 is still provisional.

#### E10 first matched pair: complete, no retention decision

The prior goal turn made progress by measuring the scheduling boundary, building
isolated artifacts and starting the comparison. Continuation verified the same
live controller and preparation child; no fixture or payment was restarted.

Uninstrumented baseline SHA-256:
`bf9e434e3bcc94c52b91548d289caa3f8c41afd8c9f591b0241db9fc13468e34`;
candidate SHA-256:
`162f71bc9f4ba5b57d0a01fc4d1c5cd075e6ac354bde12fc8e1549a09241329b`.
Both use the recorded pinned release flags and the same persistent SDK artifact.
The candidate contains only the E10 patch on retained E03a; E07 and temporary
probes are excluded. The serial comparison was launched with:

```sh
python3 "$RUN_ROOT/run_e10_pairs.py" --first 1 --last 5
```

Pair one completed baseline then candidate. Both arms started with independent
ceremonies/custody, 13 accepted operations, empty queues and matched funded proof
state. All ten payments completed inside their measurement windows, with no
failure, unfinished, late or pending result. Every recipient received 1,000 sat;
the independently audited fee was 285 sat and the final bank balance was 124,647
sat per arm. The controller verified matching actual inputs and stopped both
owned fixtures. Pair two proceeds in candidate-first order with fresh state.

A read-only audit of the original stopped databases verified all five members'
paid quotes and exact receipt identities, then all accepted input/change shapes.
Each arm had one three-input/38-output swap, followed by melt input counts
8, 9, 9, 9 and 11. Every melt consumed 4,478 sat with 12 requested outputs and
seven actual change signatures totaling 3,193 sat. All Bitcoin transactions had
one input and two outputs. The commands were:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 1
python3 "$RUN_ROOT/export_e10_completed_runs.py"
```

Preserve all [request observations](benchmarks/2026-09-08-e10-bdk-requests.csv),
[run metrics](benchmarks/2026-09-08-e10-bdk-runs.csv) and
[phase-count deltas](benchmarks/2026-09-08-e10-bdk-phase-counts.csv). Run and count
rows follow execution order; request rows group baseline/candidate within each
complete pair. Counters come from the existing metrics family, confirmed present
on every member before SDK execution. The wider scrape interval is labelled;
phase counts are not CPU seconds and do not form a wall-time breakdown.

| First-pair observation | Baseline | Candidate |
| --- | ---: | ---: |
| Empirical p50 | 20,424.465584 ms | 19,899.255417 ms |
| Empirical p95 | 36,356.135208 ms | 20,212.671417 ms |
| Member CPU/payment | 4.1964433082 s | 4.1951285998 s |
| Instructions/payment | 37,493,214,182.6 | 38,308,597,751.2 |
| Peak aggregate RSS | 491,454,464 bytes | 493,617,152 bytes |
| Peak aggregate footprint | 204,510,776 bytes | 201,250,336 bytes |
| Idle CPU per 30 seconds | 1.418483334 s | 1.23742625 s |
| Successful FROST coordinations for five payments | 21 | 17 |

The slower baseline fifth payment (36,356.135208 ms) is retained. Candidate
request latencies were 19,899.255417; 17,750.192750; 20,212.671417;
20,000.982917; 19,783.327375 ms. One pair establishes neither repeatability nor
a confidence interval. There is no p99 or sustainable-throughput conclusion.

Repeated FROST work occurs in retained code too: four baseline members completed
four coordinations and one completed five. Candidate totals were lower in this
pair, but instructions/payment were higher and CPU/payment almost equal. Accepted
history ended at 156 versus 169 operations; the difference was twelve additional
chain observations and one additional broadcast observation, with the same five
user payments. These are workload outcomes, not initial-state mismatches or extra
confirmed payments. Keep the five/ten-pair decision protocol; do not infer a net
system gain solely from the earlier millisecond wakeup result.

### E10 second matched pair: full observations retained

Pair two completed candidate first, then baseline, using the same frozen release
artifacts and workload. All ten payments completed within their windows. The
independent receipt, fee, member digest, queue and wallet checks passed. The
read-only proof audit also passed through both complete pairs:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 2
python3 "$RUN_ROOT/export_e10_completed_runs.py"
```

The public request, run and phase-count ledgers now preserve both pairs. Actual
proof/change and Bitcoin transaction shapes match the first-pair description.
Each original bank again retains 124,647 sat after five 1,000-sat receipts and
1,425 sat in independently checked fees. Both owned fixtures are stopped with
their state preserved.

| Second-pair observation | Baseline | Candidate |
| --- | ---: | ---: |
| Empirical p50 | 23,307.860416 ms | 19,022.309292 ms |
| Empirical p95 | 36,932.175625 ms | 35,253.213542 ms |
| Member CPU/payment | 4.2854584918 s | 4.1641662916 s |
| Instructions/payment | 38,306,369,938 | 38,454,227,304 |
| Peak aggregate RSS | 488,472,576 bytes | 491,438,080 bytes |
| Peak aggregate footprint | 204,019,232 bytes | 203,773,472 bytes |
| Idle CPU per 30 seconds | 1.253610791 s | 1.252933541 s |
| Successful FROST coordinations for five payments | 13 | 17 |

Keep the candidate's 35,253.213542-ms third payment and both baseline payments
over 36 seconds. The candidate's SDK phases were 921.670917 ms for quote,
31.730750 ms for preparation and 34,299.811875 ms for confirmation. Existing
phase counters contain no nonzero error/cancelled counts for that arm, so these
observations do not establish a failed signing retry as the long-tail cause.
Do not change retry policy based on this observation.

The baseline ended at 164 accepted operations versus 169 for the candidate,
with five additional candidate chain observations and matching counts for the
other operation kinds. Successful signing work varies in both revisions: the
candidate has fewer coordinations in pair one and more in pair two. Neither
this count nor the single-pair CPU difference establishes a repeatable net gain.
E10 remains provisional pending the full five/ten-pair protocol and correctness
gates. Pair three continues baseline first; no concurrent build or test load is
introduced during its measurement.

### E10 third matched pair and remaining notification coverage

Pair three completed baseline first, then candidate. All ten receipts, exact
value checks, matched inputs, member digests and empty queues passed. A read-only
audit now covers thirty original payments and all five members in every arm:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 3
python3 "$RUN_ROOT/export_e10_completed_runs.py"
```

The public ledgers retain thirty requests, six runs and 853 phase-count deltas.
No earlier rows changed. Both owned fixtures are stopped and their state is
preserved; pair four starts candidate first.

| Third-pair observation | Baseline | Candidate |
| --- | ---: | ---: |
| Empirical p50 | 20,000.668709 ms | 20,573.092333 ms |
| Empirical p95 | 21,115.458625 ms | 21,031.351042 ms |
| Member CPU/payment | 3.9934188502 s | 4.0849083748 s |
| Instructions/payment | 37,861,072,611.4 | 38,253,926,720.6 |
| Peak aggregate RSS | 490,717,184 bytes | 491,536,384 bytes |
| Peak aggregate footprint | 202,167,840 bytes | 201,135,672 bytes |
| Idle CPU per 30 seconds | 1.278884833 s | 1.251032665 s |
| Successful FROST coordinations for five payments | 21 | 17 |

This pair's candidate median and CPU/payment are higher, despite fewer successful
signing coordinations. Liveness round counts are 26 successful/79 skipped for
baseline and 22 successful/108 skipped for candidate. Both report 255 wallet
syncs; pending confirmation probes are 50 versus 75. Counts bracket the labelled
wider scrape interval and are descriptive, not a causal CPU attribution. The
five-pair comparison remains necessary; no acceptance or regression conclusion
is drawn from this pair alone.

Static coverage review found that existing wallet-liveness tests cover lease,
fee and scheduling rules, but do not directly exercise the new notification.
Before retaining E10, add focused coverage for applied-proposal filtering and a
stored/coalesced wake during an active round, using deterministic barriers or
polling. Preserve lost-publication repair and bounded shutdown coverage. A private
acceptance checklist records this prerequisite; do not equate the existing broad
test filters with coverage of the new boundary.

The private original-candidate restart helper is prepared and syntax-checked,
but has not run. Its terminal-manifest guard was corrected against the observed
schema before execution. It requires the completed comparison and scoped
correctness, refuses concurrent campaign members, checks the measured artifact,
and compares original durable payment/wallet state and external transaction
identities without creating a new payment or copying custody. Performance
acceptance, the notification regression, scoped checks and the actual restart
remain pending. No production source or running benchmark was changed during
this review.

### E10 fourth pair: higher candidate CPU retained in evidence

Pair four completed candidate first, then baseline. All ten payments passed
independent receipts, exact fees/value, member digest and empty-queue checks.
Both fixtures are stopped with state preserved. The read-only audit through
pair four passed for forty original payments and all five members per arm:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 4
python3 "$RUN_ROOT/export_e10_completed_runs.py"
```

The ledgers now contain forty requests, eight runs and 1,136 phase-count deltas,
preserving earlier rows. Baseline/candidate p50 was 20,381.788375/20,069.583208 ms;
p95 was 20,483.653291/21,125.974959 ms. CPU/payment was
3.7315482418/4.1803923670 seconds, a 12.03% increase in this pair, and
instructions/payment were 36,239,028,970.6/38,413,276,085.0. Idle CPU was
1.136397957/1.322913582 seconds per 30 seconds. These observations remain in the
comparison; no outlier is dropped and no guard is relaxed.

Both arms completed 17 FROST coordinations and 22 successful liveness rounds;
skipped liveness rounds were 83 baseline versus 108 candidate. Histories ended
at 162/169 accepted operations. The aggregate CPU difference cannot be explained
by coordination counts alone. A single pair does not establish a reproducible
regression; the fifth pair proceeds baseline first under the existing protocol.
E10 remains uncommitted and no correctness or final acceptance gate is claimed.

### E10 five-pair decision: inconclusive, extend to ten

The original controller exited successfully after five alternating pairs.
All fifty payments completed inside their windows, with no failed, unfinished,
late or pending result. Every original member's accepted proof/change shapes,
independent recipient receipt and exact Bitcoin fee passed the read-only audit.
Each arm ends with 124,647 sat after 131,072 sat funding, 5,000 sat delivered and
1,425 sat in independently checked fees. All ten owned fixtures are stopped with
their original state preserved. No custody or nonce state was copied.

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 5
python3 "$RUN_ROOT/export_e10_completed_runs.py"
python3 "$RUN_ROOT/summarize_e10_comparison.py" --pairs 5
python3 "$RUN_ROOT/decide_e10_next_gate.py" --pairs 5
```

The review recomputed the existing analyzer output and required exact agreement
with the controller's saved comparison. The public ledgers retain all fifty
requests, ten runs and 1,425 phase-count deltas, including every long observation.
The intervals below are paired log-ratio Student-t intervals over independent
runs; mean columns are arithmetic means of run metrics.

| Metric | Baseline mean | Candidate mean | Candidate/baseline ratio (95% interval) |
| --- | ---: | ---: | --- |
| Per-run p50 | 20,942.237 ms | 19,994.222 ms | 0.95587 (0.85408–1.06979) |
| Per-run p95 | 30,199.779 ms | 23,853.484 ms | 0.79954 (0.55103–1.16015) |
| Member CPU/payment | 4.05873 s | 4.13367 s | 1.01951 (0.95120–1.09274) |
| Instructions/payment | 37,435,605,420.12 | 38,354,130,637.44 | 1.02471 (0.99815–1.05198) |
| Peak aggregate RSS | 489,724,313.6 bytes | 491,808,358.4 bytes | 1.00427 (0.99751–1.01107) |
| Peak aggregate footprint | 202,554,516.8 bytes | 203,406,480 bytes | 1.00422 (0.97966–1.02939) |
| Idle CPU per 30 seconds | 1.26219 s | 1.24513 s | 0.98820 (0.86721–1.12606) |

There is no repeatable primary latency or CPU improvement. The p95 and CPU
intervals still allow regressions above 5%; these guards are inconclusive, not
passing and not confirmed regressions. Both sampled memory guards exclude an
increase over 5%. Idle CPU shows no reproducible increase, but its difference
remains inconclusive. Package wakeup variability is large and retained; interrupt
wakeups have a small upward ratio interval of 1.00019–1.00527. No idle improvement,
p99 or sustainable-throughput conclusion is supported.

Existing wider-interval counts total 87/85 successful FROST coordinations,
112/110 successful liveness rounds, 473/540 skipped liveness rounds,
1,435/1,290 wallet syncs and 271/375 pending confirmation probes for
baseline/candidate. Thus the observed notification boundary improvement has not
established lower aggregate CPU or payment latency. Counts are work observations,
not a causal CPU decomposition or a sum of wall-time phases.

Decision: extend to ten matched pairs, as prescribed before the first run:

```sh
python3 "$RUN_ROOT/run_e10_pairs.py" --first 6 --last 10
```

Retain the exact same artifact hashes, candidate patch, SDK, workload, independent
fixture preparation and alternating order. Existing pairs are only read and
verified; their payments are never repeated. No source changes, builds, tests or
profiles overlap the extension. If ten pairs remain inconclusive, remove E10's
production change and retain the investigation. Correctness testing and candidate
restart are still conditional on a supported performance result.

Two deterministic notification regression tests were drafted privately while
waiting, using normal protocol/journal constructors. They are not installed,
compiled or claimed passing. The measured production patch remains byte-for-byte
identical to the frozen artifact patch; E07 remains excluded from the comparison.

### E10 offline phase attribution during the extension

Without adding probes or load to the running extension, extract the existing SDK
phase timestamps and independent Bitcoin observations from the five completed
pairs. Private transaction/output identities correlate each request; the public
ledger contains only numeric request indexes and clock-local durations:

```sh
python3 "$RUN_ROOT/extract_e10_payment_phases.py" --through-pair 5
```

All fifty SDK quote/preparation/confirm phase sums agree with their total request
duration. Here `sdk_confirm_ms` is the SDK confirm call, including federation and
external-payment work; it is not Bitcoin confirmation time. The maximum quote
phase is 1,215.456667 ms and maximum preparation phase is 37.775792 ms.

The five requests above 30 seconds spend 34,299.811875–35,758.879875 ms in the
SDK confirm phase. Each has an observed unconfirmed receipt followed by a
confirmed receipt; their independent RPC-observation intervals range from
1,013.135083 to 4,588.540167 ms. This prioritizes further attribution within the
confirm call, including work before broadcast and after confirmation; it does
not isolate a signing retry, lease delay, notification gap or backend bottleneck.

The [phase ledger](benchmarks/2026-09-08-e10-bdk-payment-phases.csv) records the
observer interval endpoints as bounds between RPC observations, not exact block
waiting times. Two baseline requests in pair two were first observed already
confirmed; their unconfirmed-to-confirmed intervals remain missing, never zero.
SDK and observer origins are separate. Do not subtract their timestamps or add
these potentially overlapping intervals as a wall-time breakdown. No new
instrumentation, performance acceptance or change to the extension follows from
this descriptive extraction.

### E10 extension: sixth pair audited

Pair six completed candidate first, then baseline, with unchanged artifacts and
workload. All ten payments passed the independent receipt, exact fee/value,
member digest, queue and actual-input checks. Both original fixtures are stopped
with state preserved. Read-only audits now cover sixty original payments:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 6
python3 "$RUN_ROOT/export_e10_completed_runs.py"
python3 "$RUN_ROOT/extract_e10_payment_phases.py" --through-pair 6
```

All earlier public rows are unchanged. Ledgers now contain sixty request/phase
rows, twelve runs and 1,706 phase-count deltas. Baseline/candidate p50 is
19,395.533792/18,912.928125 ms; p95 is 20,984.472459/37,282.233625 ms;
CPU/payment is 3.6467906752/4.2734894334 seconds. Retain both candidate requests
above 30 seconds. Both arms completed 17 FROST coordinations and 22 successful
liveness rounds; skipped liveness rounds were 78/133. No aggregate conclusion is
drawn at six pairs; the prescribed extension continues to ten.

The two new long candidate requests spend 33,449.601959 and 36,115.224334 ms in
the SDK confirm call. Their separately observed receipt-to-confirmation intervals
are roughly 3.55–3.56 seconds. The same clock restrictions apply, and the two
previously missing unconfirmed observations remain missing. These observations
do not authorize a production change or establish a cause for the long calls.

### Resource-envelope scope clarification (September 9)

Read-only inspection confirms that native CPU samples and before/after phase
scrape envelopes use the same controller's `time.monotonic_ns()` clock. For the
first six completed E10 pairs, post-payment receipt audits and converged status
checks finish before the final counter scrape, whose end is
1,157.585917–1,203.995334 ms after the last native CPU sample. Preserve the exact
twelve observations privately in `resource-envelope-scope-through-6.json`.

The CPU/payment result covers SDK execution through the controller's completion
observation; it does not include that subsequent audit/convergence interval.
The broader phase counts therefore cannot be converted into CPU costs. The
existing `process_cpu_usage_percent` gauge does not supply the missing cumulative
native CPU endpoint, and elapsed time alone cannot reconstruct it. No CPU value
or comparison sample has been changed or filled in.

Earlier descriptions of the completion observation as bounded by one 500-ms
poll were too precise: the loop also performs RPC and resource-sampling work.
Clarify those descriptions to include that overhead. Actual native sample
endpoints and SDK request durations remain unchanged. The frozen E10 extension
continues with its original method; final resource verification must distinguish
the execution window from post-window/drain work so a reduction cannot merely
shift cost outside the measured interval. This is a measurement-scope correction,
not a measured regression or a new optimization.

### E10 extension: seventh pair audited

Pair seven completed baseline first, then candidate. All ten independent receipts,
exact fee/value checks, member digests, empty queues and actual-input comparisons
passed. Both original fixtures are stopped with state preserved. The read-only
audit and phase extraction now cover seventy payments:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 7
python3 "$RUN_ROOT/export_e10_completed_runs.py"
python3 "$RUN_ROOT/extract_e10_payment_phases.py" --through-pair 7
```

Public ledgers retain seventy request/phase rows, fourteen runs and 1,989
phase-count deltas; no earlier row changed. Baseline/candidate p50 is
34,175.454708/20,391.443083 ms, p95 is 37,636.737125/20,584.644167 ms, and
CPU/payment is 4.4707737252/3.9140983582 seconds. Keep the three baseline requests
above 30 seconds, just as the long candidate observations in other pairs remain.
Both histories end at 169 accepted operations. Successful FROST coordinations
are 13/17 and skipped liveness rounds are 137/108. This pair favors the candidate
despite more signing coordinations; counts alone are not CPU attribution.
The comparison continues to ten with unchanged methods and no interim acceptance.

### E10 extension: eighth pair audited

Pair eight completed candidate first, then baseline. All ten payments passed
independent receipts, exact fee/value, member digest, queue and actual-input
checks. Both original fixtures are stopped with state preserved. The read-only
audit and phase extraction now cover eighty payments:

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 8
python3 "$RUN_ROOT/export_e10_completed_runs.py"
python3 "$RUN_ROOT/extract_e10_payment_phases.py" --through-pair 8
```

Public ledgers contain eighty request/phase rows, sixteen runs and 2,277
phase-count deltas, preserving all prior rows. Baseline/candidate p50 is
35,722.041584/20,153.085333 ms; p95 is 37,751.314750/21,332.634708 ms;
execution-envelope CPU/payment is 4.8574522754/3.9520410412 seconds. Four baseline
requests exceeded 30 seconds; each remains in the dataset. Both histories end
at 169 accepted operations. Successful FROST coordinations are 21/17,
successful liveness rounds 26/22 and skipped rounds 139/108. This favorable
candidate pair does not replace the unfavorable observations in other pairs.
The original ten-pair protocol continues without a new interim decision.

### E10 extension: ninth pair audited

Pair nine completed baseline first, then candidate. Read-only original-state
checks confirm all ten independent receipts, exact fees and value conservation,
matching proof shapes, member digests and empty queues. Both fixtures are stopped
with state preserved. The same audit, run export and phase extraction commands
used for pair eight were repeated with `--through-pair 9` where applicable.
Public ledgers contain ninety request/phase rows, eighteen runs and 2,568
phase-count deltas; earlier rows are unchanged.

Baseline/candidate p50 is 35,280.223833/20,375.456333 ms and p95 is
38,891.031667/20,922.762500 ms. Execution-envelope CPU/payment is
4.9160932664/4.0687561086 seconds. Preserve all four baseline requests above
30 seconds alongside the long candidate observations in earlier pairs. Both
histories end at 169 accepted operations. These are descriptive observations;
the unchanged ten-pair protocol supplies the next acceptance decision.

### E10 ten-pair decision: inconclusive overall; production change reverted

The original extension controller completed successfully. Pair ten ran candidate
first: p50 20,409.745375 ms, p95 21,835.879125 ms, CPU/payment 4.1410867332 s.
Baseline followed: p50 20,561.496667 ms, p95 20,790.420167 ms, CPU/payment
3.8109448584 s. Every request completed inside its measurement window with no
failed, unfinished or late payment. Histories end at 169/158 operations; each
arm's five members agree. Independent ceremonies are not expected to share keys
or history digests. Original custody and all terminal identities are preserved.

```sh
python3 "$RUN_ROOT/audit_e10_completed_pairs.py" --through-pair 10
python3 "$RUN_ROOT/export_e10_completed_runs.py"
python3 "$RUN_ROOT/extract_e10_payment_phases.py" --through-pair 10
python3 "$RUN_ROOT/summarize_e10_comparison.py" --pairs 10
python3 "$RUN_ROOT/decide_e10_next_gate.py" --pairs 10
```

All 100 original receipts passed exact fee/value and actual proof/change-shape
audits. The four public ledgers now contain 100 request/phase rows, twenty runs
and 2,848 phase-counter observations, preserving every previous row. Each arm's
131,072-sat issuance reconciles to 124,647 sat remaining, 5,000 sat independently
received and 1,425 sat actual Bitcoin fees.

The common analyzer reproduces the controller's final comparison exactly.
Mean per-run p50 is 24,984.5934585/20,021.3767915 ms with paired ratio
0.827282 (95% CI 0.689904–0.992017): a supported median benefit. P95 is
30,705.287125/24,122.557621 ms with ratio 0.795415 (0.594381–1.064444).
Execution-envelope CPU/payment is 4.19956836592/4.10178430666 s with ratio
0.981166 (0.897759–1.072322). The latter two guards do not exclude more than
5% regression. Neither establishes a confirmed regression. Peak RSS and physical
footprint guards pass; idle CPU ratio 1.004633 (0.940028–1.073679) is inconclusive
with no reproducible increase. No sustainable-throughput or p99 conclusion applies.

Work counters across the broader convergence envelope show baseline/candidate
wallet syncs 3,160/2,640, successful FROST signature rounds 168/178, successful
liveness rounds 218/228, skipped liveness rounds 1,057/1,097 and pending
confirmation probes 580/750. Preserve the baseline's one commitment-round error
and two signature-round errors as well as all long observations. Counts overlap
and do not explain execution-envelope CPU by themselves. Earlier diagnostic
removal of the three-second wait cannot overrule the final acceptance guards.

Decision: **revert E10 as inconclusive overall after ten pairs**, preserving the
median finding and all evidence. No extra pairs were added. After verifying the
controller and owned members had stopped, the guarded revert helper checked the
exact frozen patch, saved it privately, and restored only `operation_pipeline.rs`
and `wallet_liveness.rs` to retained HEAD. Its patch SHA-256 remains
`2c230ece887cf2219aa7306d78e93724359a663dfcf15ae6053eb3bc50dab968`.
E07's separate provisional source is byte-for-byte unchanged. No database,
nonce/custody state or external payment was changed by the revert.

```sh
python3 "$RUN_ROOT/revert_e10_after_comparison.py"
nix --extra-experimental-features 'nix-command flakes' develop .#stable --command cargo fmt --all -- --check
```

Both commands passed. Temporary production probes were already absent from the
matched binaries and source. Draft notification regressions and candidate
validation/restart helpers remain private, unexecuted investigation artifacts;
they are not passing correctness evidence. Because E10 is removed, those
candidate acceptance steps are not prerequisites for this revert. The report now
consolidates the earlier interim pair commentary into this final disposition.

Next work remains the separate cross-session recovery finding and the missing
primary payment measurements. Original-baseline combined comparison, full
resource/drain accounting, scaling matrix, real payments in every direction,
restart/catch-up/value conservation and two final profiling passes remain open.

### E07 quorum verification resumed after E10

After the E10 controller completed and the inconclusive E10 source was removed,
reran the existing E07 candidate's scoped quorum tests on retained production.
The pinned command passed **18 tests, zero failed or ignored**:

```sh
RUST_MIN_STACK=16777216 nix --extra-experimental-features 'nix-command flakes' develop .#stable --command cargo test --locked -p cdk-axum --features federation-bdk,federation-bark,prometheus,iroh --lib quorum_ -- --test-threads 1
```

This includes the old-session replay-floor/vote regression and neighboring quorum,
checkpoint and recovery scenarios. It does not prove that a real member crossing
sessions recovers with the candidate, nor establish performance acceptance.
Only `sync_loop.rs` and its existing test addition remain provisional. No E07
production runtime has been started. Next validation must exercise the original
cross-session failure and candidate recovery with durable frontier/digest checks;
repeating a same-session restart cannot substitute for that boundary.

### E07 actual startup-path regression under development

The helper-only regression leaves a coverage gap at the caller: startup captures
local status before advancing the consensus session, then passes that status to
the replay-target calculation. Add a five-member AlephBFT/SQLite scenario using
the existing router transport fixture. First finalize an operation in session
zero, then persist four peers in a later empty session. Hold their new unit
creation with test-only delay configuration while reconciling the remaining
member through `reconcile_federation_startup_with_quorum`. Require its persisted
session, replay readiness and unchanged journal/materialized state, then resume
normal fixture scheduling and require another operation to finalize on all five.

This is a correctness scenario, not a production latency benchmark or a regtest
payment. No new production API or instrumentation is introduced. The test must
fail against the original replay-target helper and pass with the candidate;
compile/runtime failures and revisions will be retained privately. Performance
acceptance and real cross-session payment recovery remain open.

### E07 startup-path regression and scoped verification results

The new five-runtime scenario passed with the candidate. Against the original
helper it failed exactly at `a later empty session must not wait for old-session
units`, after the later session had been durably selected. The guarded original
comparison restored the candidate source automatically. The initial fixture's
first-operation wait was then tightened to wait for all five members before
stopping any member; this avoids making fixture shutdown order part of the test.
The candidate now normalizes local cursors lazily rather than allocating an
intermediate map before constructing the result. This is refinement of the
unaccepted candidate, not an independently claimed optimization.

The final source passed the serial scoped validator:

| Check | Result |
| --- | --- |
| Full-feature `quorum_` tests | 19 passed |
| One/quorum/all-member live restart matrix | 3 passed |
| Recovery tests | 46 passed; one manual benchmark ignored |
| Strict full-feature Axum library Clippy | passed |
| Core `federation`-only `quorum_` tests | 17 passed |
| Ordinary Axum without default features | compiled |
| Workspace formatting check | passed |

These are 85 passing test executions across configurations, not 85 distinct
scenarios. The new scenario uses real AlephBFT and SQLite with the existing router
transport and test key fixtures. Its delayed initial unit creation only controls
the later-session observation boundary. It is not an HTTP/Iroh regtest payment,
a production timeout measurement, or a performance acceptance result.

```sh
python3 "$RUN_ROOT/run_e07_cross_session_baseline.py"
python3 "$RUN_ROOT/validate_e07_cross_session.py"
```

The validator's private command manifest and logs retain every command, exact
source hashes and result. Test commands use `RUST_MIN_STACK=16777216`, the pinned
Nix stable shell, `cargo test --locked -p cdk-axum --lib`, the documented full
feature set or explicit core-only features, and `--test-threads 1`. Source bytes
remained unchanged throughout validation. Full core/FFI/correctness and campaign
payment gates remain separate from these scoped checks.

The original-helper comparison was repeated against the final tightened fixture:
`run_e07_cross_session_baseline_final.py` again exited through the expected
obsolete-watermark assertion, then restored the exact validated candidate bytes.
This confirms the final test detects the original defect; it is not another
performance sample. An uninstrumented release build is now in progress using
`build_e07_runtime.py`. It checks the validated source hashes, isolates the E07
production diff, and reuses the unchanged retained E03a baseline artifact. No
candidate production runtime or payment has been started by that build.

The E07 release build subsequently completed successfully. The uninstrumented
candidate fingerprint is
`a634f60ceb671a5328c2e7ee9471aca969412ea72c39364a815578c26ab944f8`,
with production patch SHA-256
`5c38a47ed65116fba78c39d6e2f783c3bd439de2021e1558b1e1652d89a4d67a`.
It was built from `f42ae42b9` plus only the provisional E07 production change;
subsequent HEAD changes are documentation. The baseline reuses retained E03a
artifact `bf9e434e3bcc94c52b91548d289caa3f8c41afd8c9f591b0241db9fc13468e34`.
Both use the matched release feature/profile configuration. Source hashes still
match the scoped validation snapshot, and no temporary production probes or
local dependency overrides were added. No candidate services were started.
The next required step is isolated regtest recovery, not another helper-only test.

### E07 isolated regtest recovery protocol

Before execution, prepare one independent five-member release fixture with real
Bitcoin, Bark and Lightning backends using the existing fixture/payment helpers.
Use the existing local warmup setting of 256 rounds with one rotation solely to
create a controlled session boundary; quorum, confirmation policy, retry limits,
production timeouts and queues remain unchanged. After issuing a 131,072-sat
on-chain deposit, stop member one with its original durable state. Wait for the
four peers to rotate normally and agree on session one with the same application
frontier. Then restart the original member, require certified readiness within
the existing 180-second recovery observation bound, and verify that all financial
state and the original wallet value are unchanged. Finally use the existing SDK
and independent miner to send five real 1,000-sat on-chain payments and audit
recipient outputs, actual fees, change, member state and exact conservation.

The rotation wait is fixture preparation, outside payment timing. This initial
run is functional recovery evidence, not a paired performance comparison. No
custody/nonce state is cloned and no SQL state is edited. The one-shot controller
preserves every original identity after failures and stops only owned processes;
partial membership is an expected cleanup case. An unexpected frontier change,
pre-existing rotation or missing readiness fails the scenario instead of changing
its state to manufacture success. Raw status, timing and process evidence remain
private. E07 remains provisional pending observed results and acceptance.

### E07 regtest attempt one: controller extraction failure, excluded

The independent fixture completed full DKG and a real 131,072-sat on-chain
issuance. The controller then failed before stopping any member because its
status extractor omitted `next_unit`; the underlying status output did include
that field. This is a harness error, not a candidate recovery failure or passing
recovery sample. Cleanup stopped every owned service and preserved the fixture.
Read-only original-state audit confirms all five members at 13 operations/session
zero, one quote paid and issued for exactly 131,072 sat, no melt quotes, and the
original wallet holding exactly 131,072 unspent sat. No payment is pending and
no deposit will be repeated against this wallet.

Correct the extractor and validate every required field against the saved initial
and funded status lines before another run. A new independent fixture is required
because restarting the old fixture intentionally disables the fresh-session warmup
cap; editing its database to restore the cap is prohibited. The next attempt uses
128 warmup rounds instead of 256 to shorten fixture preparation: the invalid
attempt's funded members had unit cursors 28–43. This changes only the controlled
rotation schedule, not a production performance setting or a comparison sample.
The recovery observation limit remains 180 seconds. Preserve the invalid attempt
and its full funding/cleanup evidence; do not mix it into future comparisons.

### E07 second fixture: observer takeover, idle activity and original-member recovery

The second independent fixture completed its deposit and stopped member one in
session zero. The direct four-peer observer omitted the RPC environment variable
supplied by the ordinary fixture wrapper, so it failed before querying peers.
Corrected read-only queries confirmed all four members healthy with the original
13-operation frontier. Stop only the faulty observer controller and preserve the
running members, backends and original funded state; no new deposit or custody
copy is involved in this continuation.

Inspection also showed why a passive rotation wait was insufficient: the existing
AlephBFT data provider intentionally waits for activity while quiescent. Build a
temporary, bounded client using the existing `FederationSignedRequest` and
`AlephBftActivity` API to mark the original accepted operation as peer activity.
This uses normal identity/receiver/session/sequence validation, submits no payment
operation, and changes no database directly. Its source was removed after the
release build. Compilation and hints are fixture preparation, excluded from timing.
The build emitted a missing crate-documentation warning for the temporary example;
it is not retained production or benchmark support.

The activity client received 68 acknowledgements, followed by four rejected calls.
A signed status query then confirmed all four peers ready in session one with zero
unit cursors, empty queues and the unchanged 13-operation order/application
frontier. The stopped original member still had session zero and unit cursor 35.
This establishes the intended boundary without creating more financial operations.

The first original-member start failed before consensus startup because the fee
proxy had exited with the old controller's process group. Bark wallet opening
reported connection refusal. Exclude this timing from candidate recovery evidence.
The original member remained in session zero, the four peers in session one, and
all issued value unchanged. Automatic approval review timed out while starting a
separately tracked replacement proxy; its explicitly permitted retry succeeded.
The restored proxy was verified to return the same 2-sat/vB estimate before another
start of the original member. No invoice, deposit or payment was repeated.

With the proxy available, the original member adopted session one and all five
became ready with matching digests and empty queues. Readiness was observed after
**7.354839750 seconds**, including process launch and controller polling. The
financial-state hashes and original 131,072-sat wallet value are unchanged. This
is one successful real regtest recovery observation, not a matched speedup or an
uncensored replacement for the historical original-binary failure. Preserve both
invalid harness/startup attempts separately. Five subsequent real outgoing
on-chain payments from this original recovered wallet passed the audit below.

Commands and exact private source snapshots include:

```sh
python3 "$RUN_ROOT/run_e07_regtest_recovery_arm.py" --pair 2 --arm candidate --order 1
python3 "$RUN_ROOT/build_e07_activity_preparation.py"
python3 "$RUN_ROOT/resume_e07_original_member_after_proxy.py"
python3 "$RUN_ROOT/verify_e07_post_recovery_payments.py"
```

Do not rerun these one-shot scripts against their advanced state. The observer was
explicitly stopped, its original runtime was retained, and the signed activity
client and proxy were controlled separately. Further continuation must inspect the
current result and original identities first. The second fixture's payment driver
also records member CPU through the post-execution receipt audit/convergence
interval separately, addressing the previously identified resource-scope gap.

### E07 post-recovery payment audit: passed; performance acceptance still open

All five original SDK requests completed and every member records the same paid
melts, accepted proof shapes and application/order digests. The independent
Bitcoin audit confirms five 1,000-sat recipient outputs and five actual 285-sat
fees. Exact conservation is **131,072 = 124,647 + 5,000 + 1,425 sat**. The original
funding quote remains paid and issued for 131,072 sat. All members converge at
169 accepted operations in session one with empty queues.

The [five-request ledger](benchmarks/2026-09-08-e07-regtest-post-recovery-requests.csv)
retains all observations. Melt input counts are 8/9/9/9/11; each spends 4,478 sat,
requests 12 outputs and returns seven change proofs worth 3,193 sat. One preceding
swap consumes three proofs worth 6,144 sat and requests 38 outputs. Each Bitcoin
transaction has one input and two outputs. These shapes agree across all five
original databases and with the SDK receipts.

The five serial latencies are 37,110.176209 / 18,088.520125 / 37,119.508000 /
36,158.942584 / 35,164.088166 ms. The actual SDK window is 163.641854208 seconds:
five offered and completed, zero failures, timeouts, unfinished or late payments.
There are too few samples for p99 and no stable-load throughput claim. Aggregate
member CPU is 23.086349665 seconds in the execution envelope; the subsequent
receipt-audit/convergence interval consumes another 0.062041709 seconds, measured
separately. This is an unpaired functional run, not a retained performance win.

The read-only audit command was:

```sh
python3 "$RUN_ROOT/audit_e07_regtest_payments.py"
```

It reads the original SDK receipts, journal rows and wallet/payment records;
it does not alter custody or reinitiate payment. Original private manifests,
source snapshots, receipts and native counters are preserved. All owned members,
backends and the separately tracked fee proxy are stopped, and all fixture state
is preserved. The temporary activity client source is absent from the worktree;
no production probes remain. E07 remains provisional pending its performance
acceptance decision. Broader transport/load/history and final campaign gates
remain open.

### Primary ecash concurrency observations: prespecified next workload

Resume missing direct-ecash coverage with the accepted E03a release artifact
(`bf9e434e3bcc94c52b91548d289caa3f8c41afd8c9f591b0241db9fc13468e34`),
excluding provisional E07 and rejected E10. Use the existing persistent receive
SDK (`7f36c5aea2a2adbe77f608e6cd6687d1ed80752d652ce189b0437931d40bb3e1`),
with tracing disabled. This is baseline characterization, not a candidate or
keep decision. Hypothesis to investigate: serial ecash latency and scaling depend
on consensus/response waiting versus proof verification; only measured costs and
subsequent sampling attribution may authorize an optimization.

Run closed-loop concurrency 1, 4, 16 and 64 separately, each with fresh independent
five-member HTTP/SQLite custody, full production DKG, 64 unique 32-sat tokens,
one input/output proof each and 64 independent recipient wallets. Fund 8,192 sat
as 256 proofs, package tokens offline, and require the same 13-operation starting
history. Fixture creation, funding, packaging, 60-second settling and SDK connection
preparation stay outside transaction timing. Use a fixed 180-second observation
window and 120-second drain allowance; this is a finite burst, not sustainable
throughput. These windows differ from historical 60-second guards and are not
production timeout changes. Retain not-offered, late and unresolved outcomes.

Collect existing native member CPU/memory/wakeup/I/O counters at 10 Hz with causal
clock bounds; record controller cost and post-SDK status-audit CPU separately.
After convergence, audit original member proof states and recipient databases,
require exact 8,192 = 6,144 + 2,048 sat conservation, then measure 30 seconds of
unchanged quiescent state. Preserve every original fixture, including failures;
never repeat token submission after an uncertain result. Initial characterization
has no temporary production probes or sampling profiler. Allocation/SQL/network
attribution, shared-wallet contention, open-loop load and original-baseline
comparisons remain separate open requirements.

The private controller extends the existing fixture, offline-token and bounded
SDK guard scripts, preserving source snapshots with each manifest:

```sh
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 1
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 4
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 16
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 64
```

These commands are prespecified; only completed result records establish which
ones actually ran. Run them serially with no overlapping builds or profiles.

### Primary ecash c1: all receipts passed; output-shape assertion corrected

The serial observation completed all 64 receives inside the fixed window with
zero failures, timeouts, unfinished or late requests. The last receipt arrived
at 66.039149167 seconds; p50/p95 are 1,016.563292/1,233.608708 ms. Active aggregate
member CPU is causally bounded at 24.681448918–24.733759791 seconds, or
0.385647639–0.386464997 seconds per receipt. Fixed-window CPU is
28.842828208–28.887852126 seconds over 180 seconds; it includes the post-burst
interval and must not be described as active CPU. Peak sampled aggregate RSS is
451,395,584 bytes and the collector consumed 0.364829708 CPU seconds.

The controller then failed its post-measurement one-output assertion. Actual
ordinary wallet denomination selection returns **twelve outputs** per 32-sat
receive: 1/1/1/1/2/2/2/2/4/4/4/8 sat. The token has one input as planned. Preserve
the original manifest and controller source instead of rewriting the expectation
retroactively. This is a harness assumption failure, not failed payment evidence.
Future concurrency cases explicitly require this same observed twelve-output
shape; changing the wallet's denomination policy to manufacture a faster workload
would not be a valid optimization comparison.

Cleanup stopped all owned services and preserved all original state. A separate
read-only audit verified every original recipient database, all 64 accepted swaps
and 64 spent input proofs on all five members, matching journals/digests at 77
operations and exact **8,192 = 6,144 + 2,048 sat** conservation. No token was
resubmitted. Live readiness and empty queues were observed before the assertion.
The planned quiescent-idle sample and full post-SDK proof-audit CPU interval were
not collected; record them as missing. The already measured post-SDK status-only
CPU is 0.029647125 seconds and does not cover the later stopped-state proof audit.

```sh
python3 "$RUN_ROOT/audit_primary_ecash_c1_originals.py"
python3 "$RUN_ROOT/export_primary_ecash_observation.py" --concurrency 1
```

The [request ledger](benchmarks/2026-09-08-primary-ecash-c1-requests.csv) and
[run record](benchmarks/2026-09-08-primary-ecash-c1-run.csv) preserve the corrected
actual shape and missing fields. Retain this as descriptive baseline evidence;
it is one finite burst, with no p99, stable-load throughput or matched win claim.
Next run the prespecified c4/c16/c64 cases sequentially with the corrected audit,
fresh custody and unchanged production/SDK artifacts.

### Primary ecash c4: full observation and cleanup passed

The corrected controller completed all 64 original requests, twelve-output proof
shape checks, member/recipient audits and quiescent idle observation. All members
agree at 77 operations with empty queues; exact 8,192 = 6,144 + 2,048 sat
conservation passes. All owned members, backends and the proxy are stopped before
starting c16. No token/custody state was copied or reused across fixtures.

The last receipt arrived at 19.198747792 seconds. p50/p95 are
1,211.640375/1,361.728791 ms; all 64 completed in-window, with no failures or late
results. Active member CPU is bounded at 18.084706500–18.557099541 seconds,
0.282573539–0.289954680 seconds per receipt. The fixed 180-second window consumes
25.954016458–26.076580084 member CPU seconds. Peak sampled aggregate RSS is
444,825,600 bytes; collector CPU is 0.391306916 seconds.

The post-SDK native endpoint through receipt audit/convergence consumes
0.069328125 member CPU seconds, including the 0.041510041-second status-only
subinterval. Do not add those overlapping intervals. The separate unchanged
30-second quiescent interval consumes 1.339761081 CPU seconds, 6,409 interrupt
wakeups and 587 package wakeups across all five members. The serial case's
missing idle endpoint prevents a matched idle comparison.

```sh
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 4
python3 "$RUN_ROOT/export_primary_ecash_observation.py" --concurrency 4
```

These commands completed in the sequential controller. The
[request ledger](benchmarks/2026-09-08-primary-ecash-c4-requests.csv) and
[run record](benchmarks/2026-09-08-primary-ecash-c4-run.csv) retain every sample.
Compared with the single serial observation, the same 64-receipt finite burst
finishes earlier with somewhat higher request latency and lower active CPU per
receipt. This is descriptive concurrency scaling, not an accepted code change,
a confidence interval over repeated runs, p99 or sustainable throughput. c16
fixture preparation started only after c4 cleanup completed; c64 remains queued.

### Primary ecash c16: full observation and cleanup passed

All 64 original twelve-output receives completed in-window, with zero failures,
timeouts, unfinished or late requests. Original recipient/member proof audits,
exact 8,192 = 6,144 + 2,048 sat conservation, matching 77-operation digests, empty
queues, unchanged idle state and owned-service cleanup all pass. The last receipt
arrived at 5.544575 seconds; p50/p95 are 1,221.654250/1,621.248583 ms.

Active member CPU is bounded at 15.614516749–16.129808333 seconds, or
0.243976824–0.252028255 seconds per receipt. Fixed-window CPU is
23.046484043–23.455425751 seconds over 180 seconds. Peak sampled aggregate RSS is
493,748,224 bytes and collector CPU is 0.369328917 seconds. Post-SDK receipt
audit/convergence CPU is 0.056142457 seconds, including the 0.025998375-second
status-only subinterval. Separate quiescent CPU over 30 seconds is 1.178623541
seconds, with 6,423 interrupt and 427 package wakeups. Do not add overlapping
status/audit intervals or interpret a single idle observation as an improvement.

```sh
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 16
python3 "$RUN_ROOT/export_primary_ecash_observation.py" --concurrency 16
```

The [requests](benchmarks/2026-09-08-primary-ecash-c16-requests.csv) and
[run record](benchmarks/2026-09-08-primary-ecash-c16-run.csv) preserve every sample.
The sequential controller started c64 only after c16 audit/export and cleanup.
This remains descriptive finite-burst scaling, not a production change, repeated
run confidence interval, sustainable throughput or p99 conclusion.

### Primary ecash c64: full observation and cleanup passed

All 64 original receives completed in-window with no failed, timed-out, unfinished
or late requests. Original recipient/member proof audits, exact value conservation,
matching digests at 77 operations, empty queues, unchanged idle state and cleanup
passed. The sequential primary controller exited successfully after this case.

The last receipt arrived at 3.586554250 seconds; p50/p95 are
2,598.164167/3,581.489583 ms. Active member CPU is bounded at
13.012305459–14.898588665 seconds, or 0.203317273–0.232790448 seconds per receipt.
Fixed-window CPU is 20.888431710–22.475621416 seconds over 180 seconds. These
causal bounds are wider for this short burst; do not turn them into precise point
estimates or statistical confidence intervals. Peak sampled aggregate RSS is
658,751,488 bytes and collector CPU is 0.374841625 seconds.

Post-SDK receipt-audit/convergence CPU is 0.069138416 seconds, including the
0.042073-second status-only subinterval. The separate 30-second quiescent interval
consumes 1.171451583 CPU seconds, 6,390 interrupt wakeups and 316 package wakeups.

```sh
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 64
python3 "$RUN_ROOT/export_primary_ecash_observation.py" --concurrency 64
```

The [requests](benchmarks/2026-09-08-primary-ecash-c64-requests.csv) and
[run record](benchmarks/2026-09-08-primary-ecash-c64-run.csv) retain all observations.
Across c1/c4/c16/c64, all 256 original receipts reconcile. Higher concurrency
shortens the finite burst while increasing the observed tail and, at c64, sampled
RSS. These are single-run workload observations, not a code improvement, a
sustainable-throughput result, p99 or a final campaign acceptance gate.

### Next ecash attribution: existing timers and bounded stack samples

Use a separate c4 diagnostic after all primary measurements stop. Its observed
19-second burst provides a longer active sampling interval than the c16/c64 cases.
Resume the original c4 fixture once, verify unchanged 77-operation state and all
original wallet receipts, and package 64 different 32-sat tokens offline from the
remaining 6,144 sat. Do not create another deposit, copy custody, or reuse an
already submitted token. New recipient wallets preserve the same one-input,
twelve-output shape; exact final conservation must be 8,192 = 4,096 bank + 2,048
prior recipients + 2,048 new recipients, with 141 matching accepted operations.

Enable the existing `cdk::wallet::federation_performance` debug stage timers only
for this diagnostic. Collect ten-second, one-millisecond native stack samples
from the SDK and one member, plus bounded native resource observations. Record
SDK child CPU separately from member CPU and exclude its measured pre-start
preparation. Profile-enabled timings are excluded from acceptance comparisons.
No temporary production probes or code changes are needed.

Source inspection identifies two hypotheses for attribution: complete keyset
cloning for each returned signature in `validate_mint_response_signatures`, and
recomputing the same signer interpolation coefficients for each output in
`aggregate_blind_signature_shares`. Existing phase timers start after key lookup,
so their verification durations alone cannot measure that earlier work. Neither
hypothesis authorizes an implementation until counters/timings and profiles show
its contribution. Do not sum overlapping stage durations as wall time or CPU.

```sh
python3 "$RUN_ROOT/profile_primary_ecash.py"
```

This command is prespecified and not yet run. Private source snapshots accompany
the diagnostic manifest. Preserve all failures and original state; audit receipts
and stop owned services before any subsequent build or matched comparison.

### Ecash profiling setup attempt one: proxy startup order failure

The diagnostic failed before token packaging or measurement. The fixed-fee proxy
checks its upstream chain when starting; launching it while the original Bitcoin
backend was still stopped made that preflight fail. Member startup subsequently
failed opening Bark wallets through the absent proxy. This is a controller startup
order error, not an ecash performance result or recovery comparison.

The controller exited nonzero after bounded cleanup. An independent read-only
check confirms every original member journal/payment/proof record and all 65
original wallet proof/transaction records are unchanged. No new tokens were
packaged, no new payment began, and all owned members/backends are stopped. The
failed source snapshots, proxy/startup logs and audit remain private and excluded
from timings.

The second one-shot attempt uses separate output directories and verifies the
original Bitcoin backend first, then the proxy's unchanged 2-sat/vB estimate,
before starting other backends and members. It still requires unchanged original
financial state before token packaging. The proxy remains owned by the live
profiling controller throughout the run; no original state or failure record is
replaced. This corrected command has not yet run.

### Ecash profiling setup attempt two: extra CLI output after successful packaging

Corrected backend/proxy startup and original-state restart verification passed.
The offline send command completed successfully, but the controller expected every
stdout line to be a token. The CLI first reported recovery of 64 prior operations,
with zero compensated, skipped or failed operations, then printed the 64 tokens.
The strict line-count assertion stopped the controller before SDK preparation or
submission. Preserve the original output and failure; this is not a payment or
performance failure.

Read-only audit confirms 64 unique new tokens, disjoint from the previous batch,
and unchanged member state at 77 operations with 64 spent proofs worth 2,048 sat.
All prior recipient wallets remain unchanged. The original sender now holds
4,096 unspent sat, 2,048 sat in the newly packaged pending proofs, and 2,048 sat
marked spent after normal CLI recovery of the prior sends. All owned services
were stopped and no new SDK state exists.

The next continuation consumes these exact saved tokens once; it does not repeat
packaging or funding. It accepts only the verified recovery-summary line alongside
the 64 token lines, retains the original stdout, and records its hash. Startup
compares the current original wallet/member state, including the legitimate
packaging/recovery changes, before any submission. Separate attempt directories
preserve both failures and the original successful restart evidence.

### Ecash profile continuation: receipts, profiles and cleanup passed

The third attempt resumed the original fixture with its already packaged tokens,
without another send or deposit. Original-state comparison passed before
submission. All 64 new receives completed, all five members agree on the same
141-operation journal, all old/new recipient proof shapes match, and exact
**8,192 = 4,096 bank + 2,048 prior recipients + 2,048 new recipients** conservation
passes. Both ten-second native profiles completed successfully. Owned members,
backends and the controller-owned proxy are stopped; original state and all three
attempts remain preserved.

SDK preparation consumed 1.099030541 CPU seconds. SDK execution through process
exit consumed 3.123889459 CPU seconds after subtracting that pre-start cumulative
CPU; member execution through SDK exit consumed 22.526005667 CPU seconds. These
include diagnostic tracing/sampling and the fixed 60-second observation window,
so they are neither an acceptance comparison nor active-only CPU. Process-exit
scope includes SDK result persistence. Do not add concurrent stage spans to obtain
CPU or end-to-end wall time.

The [wallet-stage ledger](benchmarks/2026-09-08-ecash-profile-wallet-stages.csv)
records 64 input-proof verification calls, 192 successful twelve-share batch
verification calls, and 64 twelve-output aggregations. The configured signing
threshold is three, while the independent consensus threshold is four. Thus
2,304 member shares were verified and 768 outputs aggregated. No scalar fallback
was observed. Summed stage durations are 0.124417085 / 1.641902839 / 0.492543127
seconds respectively; share batch p50 is 8.505667 ms and aggregation p50 is
7.632792 ms. These sums describe measured stage work across concurrent requests,
not a wall-time breakdown.

Both SDK and member stack samples show BLS field multiplication/squaring and
curve arithmetic as leading sampled computation. Blocked-thread frames are also
present and must not be counted as CPU work. Keyset cloning does not appear as a
dominant cost in this sample: defer that source-only hypothesis. Aggregation cost
is measurable, but this profile does not isolate coefficient recomputation;
retain that attribution question rather than implementing a cache speculatively.

### E11 proposed experiment: one final exponentiation per batch equation

The supported next target is BLS batch pairing verification in federation-gated
`cashu`. Both proof and blinded-share batch functions already combine messages
by identical public key, but then call the full pairing operation separately for
the signature term and each grouped message term. With four denomination keys,
that performs five final exponentiations for one equality check. Test whether the
pinned library's `multi_miller_loop` can evaluate the same signed pairing product
with **one** final exponentiation. Preserve every transcript-derived weight,
input/group validation, key grouping, signer check and fallback decision.

The measured share-verification stage and sampled BLS arithmetic establish the
owning cost. Expected counter change is final exponentiations from K+1 to one for
K distinct public keys, with identical verified results; this is a hypothesis,
not yet a measured saving. The library requires its allocation feature for
prepared G2 terms, so allocation/RSS are explicit guards. No long-lived cache,
wire change, public wallet API change or altered cryptographic construction is
proposed. Reuse one private arithmetic helper for the two existing batch paths
if the experiment is retained.

Before production editing, add/reuse bounded release benchmark fixtures for a
single input, the observed twelve-output/four-key shape, and larger fragmented
batches. Keep fixture generation outside timing. Record five alternating
baseline/candidate pairs of latency, CPU and work counts; extend inconclusive
results to ten and revert if still inconclusive. Check malformed lengths, empty
batches, invalid/mixed shares, signer mismatches and unchanged deterministic
weight vectors, plus standalone isolation and scoped strict lint. A component
result alone will not close runtime acceptance: repeat matched real ecash
measurements and relevant correctness/recovery gates before accepting code.
No E11 production edit or benchmark has run yet.

### E11 baseline established; bounded-preparation variant selected

Added a federation-only release benchmark example with deterministic non-custodial
fixtures, separate fixture/warmup readiness, bounded start/release handshakes and
per-batch monotonic results. The baseline twelve-share/four-key pilot completed
256 verified batches: p50 7.575250 ms, p95 7.896500 ms, CPU 7.589111 ms per batch,
and peak RSS 2,392,064 bytes. CPU excludes fixture construction and warmup but
includes bounded handshake/result-write overhead. This pilot validates the
measurement path; it is not one of the five matched pairs.

The frozen baseline benchmark SHA-256 is
`131e225d66f1c10a85a8fd67bbca82bd8d7c39384f9d311b32d73f5e9a60ef35`.
Automatic approval review timed out once before the freeze command executed;
the tool-permitted retry completed successfully. No pending approval remains.

For the first candidate, accumulate single-term Miller-loop results and perform
one final exponentiation after the complete equation. Keep each prepared G2 term
scoped to its loop iteration, including the generator term, so prepared-key
storage does not grow with distinct denomination count. This preserves the K+1
Miller loops while removing K final exponentiations. It trades the possible shared
loop savings of a full multi-term vector for bounded transient preparation memory;
measure both the remaining cost and memory rather than presuming either outcome.
The matched component workload uses 1,024 single-item, 256 twelve-item/four-key,
and 64 fragmented sixty-four-item/eight-key batches for each of proofs and shares.
Native samples are at 100 Hz, with identical bounded control overhead in both arms.

### E11 first five pairs: CPU/latency gains; one memory guard inconclusive

The bounded-preparation candidate passes 21 focused BLS tests, including unchanged
weight vectors and new equivalence/cancelling-corruption checks. Five alternating
pairs completed for all six component workloads with identical benchmark source
and deterministic inputs. CPU and p95 improve in every workload. For twelve-share
batches, CPU ratio is 0.79360 (95% CI 0.79127–0.79593), p95 ratio 0.79071
(0.77165–0.81024), and instruction ratio 0.78418 (0.78402–0.78434).

The twelve-share peak physical-footprint ratio is 1.02307 (0.99301–1.05404),
narrowly crossing the 5% guard. Other memory bounds remain below that guard, but
the overall component decision is inconclusive. Preserve all first-five-pair
records and extend the same frozen artifacts/workloads to ten alternating pairs;
no production adjustment or favorable-sample selection occurs between stages.
No E11 code is accepted or committed yet, and real runtime acceptance remains open.

```sh
python3 "$RUN_ROOT/build_e11_batch_benchmark.py" --arm baseline
python3 "$RUN_ROOT/run_e11_batch_arm.py" --pair 1 --arm baseline --order 1 --pilot
python3 "$RUN_ROOT/build_e11_batch_benchmark.py" --arm candidate
python3 "$RUN_ROOT/run_e11_batch_pairs.py"
python3 "$RUN_ROOT/extend_e11_batch_pairs.py"
```

The first four commands completed; the extension is the next scheduled step.
Builds use the pinned Nix release toolchain. The pilot is excluded from pairing.
Exact commands, artifact/source fingerprints, all durations and native envelopes
remain private pending the reviewed public evidence export.

### E11 ten pairs and component correctness: passed; runtime acceptance pending

All ten alternating pairs completed for each of six workloads: 120 process runs
and 53,760 verified batches, excluding warmup and the baseline pilot. Every CPU
and p95 ratio interval is below one. Every RSS, physical-footprint and lifetime
footprint upper bound is below 1.05. The formerly inconclusive twelve-share
physical-footprint ratio is now 1.02078 (95% CI 1.00804–1.03369): a small measured
increase within the guard, not zero additional memory. No production change was
made during the ten-pair series.

| Workload | CPU reduction | Candidate/baseline CPU ratio, 95% CI |
| --- | ---: | --- |
| proofs / single | 22.59% | 0.77144–0.77685 |
| proofs / wallet | 18.34% | 0.81505–0.81808 |
| proofs / fragmented | 9.52% | 0.90323–0.90643 |
| shares / single | 23.64% | 0.76186–0.76544 |
| shares / wallet | 20.72% | 0.79097–0.79457 |
| shares / fragmented | 10.79% | 0.89067–0.89352 |

The [run ledger](benchmarks/2026-09-08-e11-bls-batch-runs.csv) preserves all 120
results, and the [comparison ledger](benchmarks/2026-09-08-e11-bls-batch-comparison.csv)
contains run-level intervals. Raw per-batch durations/native samples and both
five-/ten-pair analyses remain private. Final-exponentiation counts in the ledger
come from inspected caller/library control flow, not a runtime probe: K+1 becomes
one while K+1 Miller loops and all transcript weights remain. Instruction counts
also decrease in every workload. Component batch rates are not payment throughput.

The full Cashu federation suite passes **644 tests, two ignored**, and the
ordinary no-default-feature suite passes **506 tests**. Scoped strict Clippy,
ordinary dependency-tree isolation (no BLS dependency) and formatting pass.
Source hashes stayed unchanged throughout validation. The focused 21 BLS tests
reported earlier overlap the full federation suite; do not add them as distinct
coverage. These checks do not replace the full CDK/FFI/federation runtime gates.

```sh
python3 "$RUN_ROOT/extend_e11_batch_pairs.py"
python3 "$RUN_ROOT/validate_e11_component.py"
python3 "$RUN_ROOT/export_e11_batch_evidence.py"
```

All commands completed. Retain the reusable benchmark support and evidence.
The arithmetic candidate remains provisional and uncommitted until matched real
ecash/runtime guards and relevant adversarial/recovery checks pass. Prepare
isolated source builds so the unrelated provisional E07 startup change cannot
enter either runtime arm. Both runtime arms must use the same release features,
SDK source and fixtures; compilation and funding remain outside measurements.

### E11 runtime comparison: prespecified before the first payment

Continue the existing primary ecash fixture, token preparation and measurement
scripts with explicit artifact/identity arguments. Build baseline and candidate
from one isolated code-only checkout with identical pinned Nix release profiles,
features and SDK source. Baseline is `df7ebb753`; the candidate adds only E11's
Cashu arithmetic/optional dependency feature patch. E07 remains excluded. No
custody or nonce state is copied, and no production instrumentation is enabled.

Run five alternating pairs of fresh five-member HTTP/SQLite federations with
signing threshold three and consensus quorum four. Each arm uses 64 independent
recipient wallets at concurrency four, 32-sat tokens with one input and twelve
outputs (four ones, four twos, three fours and one eight), zero input fees, and
8,192 sats of independently confirmed funding. Preparation ends at 13 accepted
operations. All 64 receipts must bring the journal to 77, with 6,144 sats in the
funding wallet and 2,048 sats in independent recipient wallets. Compare member
digests within each run, not across independently generated ceremonies.

Use a fixed 60-second observation window, 120-second bounded drain, 60-second
pre-measurement settling and a separate 30-second quiescent idle observation.
The previous c4 observation completed in about 19 seconds; shortening the fixed
window from the exploratory 180 seconds reduces unrelated post-burst idle while
leaving both comparison arms identical. Do not change it after seeing results.
A finite burst does not establish sustainable throughput, and 64 samples do not
support a payment p99 claim. Preserve every failure and late completion.

Primary attribution is SDK CPU per completed receipt, measured from native CPU
counters after wallet/connection preparation to wait4 process-exit usage. This
includes result writing and fixed-window idle, which are identical in both arms.
Record member CPU with the existing causal lower/upper bounds for the window and
last-receipt interval, plus the combined member/SDK execution envelope. Sample
SDK and member RSS/physical footprint at 10 Hz; preserve collector CPU and all
native samples. Capture latency, time to last receipt, and separate idle CPU and
wakeups. The expected counter reduction is the component-validated removal of
K final exponentiations per weighted batch; no new cryptographic probe is needed.

Apply the plan's paired confidence-interval protocol to CPU, p95, memory and idle
work. Extend to ten pairs if inconclusive, retaining all samples. Do not accept
E11 arithmetic until runtime guards and relevant wallet adversarial/recovery
checks pass. Keep E11 uncommitted while those gates remain open.

The isolated baseline/candidate mintd and persistent SDK builds completed with
identical release feature sets and no E07 or temporary probes. The code checkout
contains no copied custody state. Root provisional source files stayed unchanged.
A synthetic child-process check confirms native CPU counters and wait4 agree;
it validates accounting only and is excluded from performance evidence.

```sh
python3 "$RUN_ROOT/build_e11_runtimes.py"
python3 "$RUN_ROOT/run_primary_ecash_observation.py" --concurrency 4 --pair 1 --arm baseline --order 1
```

The build completed. The first runtime arm is scheduled next; no E11 runtime
performance or payment acceptance is claimed yet.

### E11 first real ecash baseline: 64 receipts audited; paired series started

The first baseline arm completed all 64 original requests within the fixed window,
with no failures or pending drain outcomes. p50 was 1,026.052 ms, p95 1,249.199 ms,
and time to the last receipt was 17.346694 seconds. SDK execution-through-exit CPU
was 3.117365 seconds, excluding preparation. Member active-interval CPU was
17.795852–18.033172 seconds under the native causal bounds. These are one baseline
run's observations, not a gain or sustainable throughput conclusion.

All five members reached 77 accepted operations with matching digests and empty
queues. Independent SQL audits found 64 spent 32-sat inputs per member, and every
recipient had the required twelve proofs totaling 32 sats. Value reconciles:
8,192 sat funding = 6,144 bank + 2,048 independent recipient wallets. The separate
30-second idle audit passed with unchanged state. All owned services stopped;
all original custody, nonce, recipient and trace state is preserved.

```sh
python3 "$RUN_ROOT/run_e11_ecash_pairs.py" --through-pair 5
```

The alternating series has started from the preserved successful first baseline.
It checks source snapshots before each arm and creates new custody for each run.
Each accepted receipt record and failed attempt will remain in the journal's
private evidence. No E11 arithmetic commit or runtime performance acceptance has
occurred; five-pair analysis and relevant correctness/recovery gates remain open.

### E11 first pair: receipts pass; performance conclusion remains open

Both original arms completed and stopped with all 128 receipts, matching member
state, exact proof shapes and independent value conservation. Observed workload
and matched-input manifests are identical. No instrumentation or production
source changed between arms.

| Metric | Baseline | Candidate |
| --- | ---: | ---: |
| SDK execution-through-exit CPU, seconds | 3.117365 | 2.763051 |
| Combined execution CPU per receipt, seconds | 0.357477 | 0.349499 |
| p95 receipt latency, ms | 1,249.199 | 1,316.215 |
| Time to last receipt, seconds | 17.346694 | 17.906443 |

The single-pair SDK CPU ratio is 0.88634, but p95 ratio is 1.05365. These are
observations without a repeatability interval: neither a win nor a confirmed
regression is declared. The prescribed alternating series continues with pair
two's candidate first; the first pair and all its raw samples remain preserved.

Prepared an offline review using the existing shared analyzer and all prescribed
pairs. It checks SDK/combined CPU, p95, separate SDK/member memory, and idle CPU.
For member active CPU, its guard conservatively compares candidate upper bounds
to baseline lower bounds; it does not present those bounds as exact counters.
The scoped post-performance validation uses the isolated measured source, the
full CDK mint/wallet federation library suite, the standalone downstream API
checker, ordinary/federation FFI compilation and formatting. FFI's federation
feature already includes its Iroh dependency; no nonexistent standalone Iroh
feature is requested. These checks are prepared, not executed or claimed passing.

### E11 second pair: all receipts retained; latency concern persists

The second pair ran candidate first, then baseline. Both completed and stopped
with all 128 receipts audited, exact value conservation, matching member digests
and unchanged measurement source. The first two pairs therefore preserve 256
successful original receipts; no failed or late operations were hidden.

| Metric | Baseline | Candidate |
| --- | ---: | ---: |
| SDK execution-through-exit CPU, seconds | 3.138054 | 2.773765 |
| Combined execution CPU per receipt, seconds | 0.362550 | 0.357927 |
| p95 receipt latency, ms | 1,264.507 | 1,332.068 |
| Time to last receipt, seconds | 18.753802 | 18.745053 |

SDK CPU is lower in both first pairs, while candidate p95 is about 5.3% higher
in both. This recurring direction is a concern, not yet the prescribed five-pair
confidence-interval result. Continue the original alternating series without
changing the candidate, load, timings, inputs or acceptance criteria.

The original-state restart audit and numeric-only evidence exporter are prepared
and syntax-checked. Neither restart correctness nor five-pair performance
acceptance has been executed or established. All raw inputs remain private.

### Ecash aggregation follow-up: narrow the next profiling question

Read-only inspection while E11's unchanged runtime series runs confirms that
`RequestShareVerification::aggregate` starts its aggregation timer after share
verification and agreement selection. `aggregate_blind_signature_shares` performs
threshold/signer validation, Lagrange coefficients and G1 scalar multiplication;
it does not perform another pairing check. There is no redundant aggregate
pairing in that measured stage to remove.

Coefficients are recomputed for the selected signer set for each output, but the
existing aggregation-stage timing does not isolate their cost from G1 arithmetic
and output construction. Keep coefficient reuse in the profiling queue, as
previously deferred; do not add a speculative cache or batch abstraction. No
production code or active measurement source changed during this inspection.

### E11 five real ecash pairs: CPU gain; latency guard inconclusive

All ten original runs completed and stopped: 640 of 640 independent receipts,
zero failed or late requests, matching member digests and empty queues. The
additional read-only identity audit confirms 960 distinct spendable notes per
run (192 bank notes and 768 recipient notes), totaling 8,192 sats. None overlaps
the 64 spent input identities; every member's spent set matches the bank's
original sent notes exactly. Original wallets and custody remain preserved.

| Metric | Candidate/baseline ratio | Run-level 95% CI |
| --- | ---: | --- |
| SDK CPU per receipt | 0.89191 | 0.88028–0.90370 |
| Combined execution CPU per receipt | 0.98137 | 0.95944–1.00380 |
| p50 receipt latency | 0.96348 | 0.90358–1.02736 |
| p95 receipt latency | 0.99524 | 0.89973–1.10088 |
| SDK peak RSS | 1.00790 | 1.00481–1.01099 |
| SDK peak physical footprint | 1.00838 | 1.00416–1.01262 |

SDK CPU falls repeatably by 10.81%. Combined CPU's interval includes no change;
no combined-system gain is yet established. SDK memory rises slightly, within
the 5% guard. Other CPU/member-memory guards pass, including the conservative
member active-CPU guard (candidate upper/baseline lower interval 0.97999–1.03513).
Idle CPU has no reproducible increase; its ratio interval is 0.67334–1.10178, so
no idle improvement is claimed either. The p95 interval cannot exclude a greater
than 5% regression. The prescribed decision is **extend to ten pairs**, with no
production change, workload adjustment or sample removal.

The [run ledger](benchmarks/2026-09-08-e11-ecash-5-pairs-runs.csv),
[request ledger](benchmarks/2026-09-08-e11-ecash-5-pairs-requests.csv) and
[comparison ledger](benchmarks/2026-09-08-e11-ecash-5-pairs-comparison.csv) retain
all ten runs and 640 receipts. Private records also retain native samples,
causal clock bounds, exact source/artifact fingerprints, custody, and identity
audits. Per-run p99 remains unavailable at 64 samples. Finite-burst completion
rates are not sustainable throughput.

```sh
python3 "$RUN_ROOT/review_e11_ecash.py" --pairs 5
python3 "$RUN_ROOT/audit_e11_unique_receipts.py" --pairs 5
python3 "$RUN_ROOT/export_e11_ecash_evidence.py" --pairs 5
python3 "$RUN_ROOT/extend_e11_ecash_pairs.py"
```

All four commands have now been dispatched: the first three completed and the
extension is running. It preserved immutable first-five record/comparison copies
and uses the same frozen artifacts, measurement source, release features, amounts,
timing and acceptance criteria. The initial CSV whitespace check caught CRLF
line endings; converting to LF preserved every parsed row, and the exporter now
writes LF directly.
E11 arithmetic remains uncommitted. Scoped correctness, real restart, all payment
directions and the remaining campaign gates are still open.

### Report summary reconciliation during the unchanged E11 extension

The report's opening tables lagged its detailed results. Reconciled E10 to its
recorded reverted disposition, identified the retained broadcaster commit, and
reflected E07's completed functional recovery/payment evidence without implying
performance acceptance. Added the measured E11 receive scope and kept offline
preparation/sender swaps explicitly unmeasured. This is a documentation correction;
no experiment, source, artifact, workload or keep/revert criterion changed.

### Aggregation hypothesis queue: public small-coefficient multiplication

A non-custodial arithmetic enumeration of all ten three-of-five signer sets
finds 18 small signed integer coefficients among the 30 interpolation
coefficients. For example, members 1/2/3 yield 3, -3, 1; members 1/3/5 yield
15/8, -5/4, 3/8. This is a classification of public mathematics, not a measured
selection distribution or work-saving percentage.

The existing aggregation profile and inspected general G1 scalar multiplication
justify measuring the selected-set distribution and the separate costs of
coefficient construction and multiplication. Only after that attribution should
a public-only small-coefficient path be considered, following the established
DKG public-integer approach. It would need exact scalar-multiplication equivalence,
unchanged threshold/share validation, and the general field-scalar fallback for
other coefficients. Private-scalar constant-time arithmetic must remain untouched.
No cache, new abstraction, production implementation, or performance claim was
added. E11's source and workload remain fixed while its extension runs.

### Aggregation attribution from the existing SDK profile

Re-read the preserved ten-second SDK sampling profile from the successful c4
diagnostic. Counted only immediate descendants of
`aggregate_blind_signature_shares` in the call graph, excluding the profiler's
summary tables. Checked that descendant counts sum to their aggregation frames
and that the profile's SHA-256 matches the private attribution record. This
avoids counting nested multiplication/field-arithmetic frames again.

| Immediate aggregation work | Stack observations |
| --- | ---: |
| G1 scalar multiplication | 202 |
| G1 affine conversion | 9 |
| Scalar inversion | 2 |
| Public signer ID to scalar conversion | 1 |
| Total | 214 |

These are observations within a bounded diagnostic profile, not CPU seconds,
whole-process percentages or acceptance measurements. The actual selected-signer
distribution remains unknown. The evidence prioritizes G1 multiplication over
coefficient caching for the next investigation; the earlier overlapping stage
timer alone could not make that distinction.

Next, after E11's fixed series finishes, measure aggregation with explicit public
signer sets: all ten three-of-five subsets, eleven-member signing subsets, and
the existing single/wallet/fragmented output shapes. Record the selected-set
distribution in a separate bounded SDK diagnostic before weighting component
results as representative of payment work. A potential change must preserve
exact interpolation for arbitrary valid IDs and ordering, including coefficients
that need general field-scalar multiplication. Any variable-time helper must
derive its coefficient solely from public signer IDs; secret-scalar arithmetic
remains on the existing library path.

No production or benchmark source changed for this attribution. Raw profiles,
addresses, machine metadata and the parsing ledger remain outside Git. E11's
unchanged ten-pair extension is still running; no further arithmetic candidate
has been implemented or accepted.

### E11 ten real ecash pairs: CPU gains; revert under the latency guard

The unchanged extension completed normally. All twenty runs passed receipt,
member-digest, queue, idle and bounded-shutdown audits. The separate read-only
identity audit then verified all 1,280 original receipts: each run has 960 unique
spendable notes totaling 8,192 sat, disjoint from its 64 original spent inputs.
Every member's spent set matches those inputs exactly. No payment was resubmitted,
no custody was copied, and all original databases and artifacts are preserved.

```sh
python3 "$RUN_ROOT/review_e11_ecash.py" --pairs 10
python3 "$RUN_ROOT/audit_e11_unique_receipts.py" --pairs 10
python3 "$RUN_ROOT/export_e11_ecash_evidence.py" --pairs 10
```

All three commands completed successfully. The prescribed review returned
`revert_inconclusive`:

| Metric | Candidate/baseline ratio | Run-level 95% CI |
| --- | ---: | --- |
| SDK CPU per receipt | 0.88694 | 0.87855–0.89540 |
| Combined execution CPU per receipt | 0.97676 | 0.96285–0.99087 |
| p50 receipt latency | 1.00168 | 0.95080–1.05529 |
| p95 receipt latency | 1.01460 | 0.96143–1.07070 |
| SDK peak RSS | 1.00744 | 1.00521–1.00967 |
| SDK peak physical footprint | 1.00811 | 1.00484–1.01139 |

The SDK CPU reduction is 11.31%; the combined execution-envelope CPU reduction
is 2.32%. All CPU/memory guards pass; the conservative member active-CPU guard
interval is 0.98384–1.01983. Idle CPU has no reproducible increase (interval
0.79611–1.06817). However, p95's upper bound still permits a 7.07% regression.
This is inconclusive, not a confirmed regression, and does not meet retention.
No samples, workload parameters or acceptance criteria were changed to obtain
a different decision. No sustainable-throughput, idle-benefit or p99 claim follows.

Restored only `crates/cashu/Cargo.toml` and
`crates/cashu/src/nuts/nut01/bls.rs` to the measured baseline. Before writing,
checked both current file hashes against the frozen candidate, the complete diff
against its preserved patch, and both HEAD blobs against the baseline hashes.
After writing, their Git diff is empty. The candidate patch and restoration
ledger remain private. The unrelated provisional E07 files are unchanged.
No temporary production probe was introduced by E11; reusable benchmark support
remains. Candidate-only runtime/FFI/restart follow-up checks were not launched
after rejection. The full campaign still requires its final correctness and
payment gates.

The [complete runs](benchmarks/2026-09-08-e11-ecash-10-pairs-runs.csv),
[complete requests](benchmarks/2026-09-08-e11-ecash-10-pairs-requests.csv) and
[comparison](benchmarks/2026-09-08-e11-ecash-10-pairs-comparison.csv) contain all
twenty runs and 1,280 receipts. The original five-pair evidence remains intact.
The next aggregation benchmark and bounded public-signer diagnostic are private,
unapplied drafts; neither has been compiled, measured or accepted.

Post-restoration checks: pinned stable `cargo fmt --all` exited zero (with the
existing unsupported-nightly-setting warnings), `git diff --check` passed, and
the CSV audit confirmed 20 run rows, 1,280 request rows and 26 comparison rows,
with LF endings and no identifying paths, network addresses, credentials or
tokens. Both restored Cashu files still match the frozen baseline hashes.

### Aggregation benchmark support: validated on unchanged production arithmetic

Extended the existing bounded BLS example with `mode: aggregation` and explicit
public `signer_ids`. It reuses the single, twelve-output wallet and 64-output
fragmented shapes, deterministic non-custodial polynomial fixtures, and the
ready/start/results/release handshake. Individual share validation and direct
reference signing happen before readiness. Each timed aggregation must equal
the direct signature, using the existing production aggregation API without a
shortcut. Benchmark schema version two records the public signer set and shares
per output; existing proof/share modes remain available.

Example manifest (the controller supplies the two handshake signals):

```json
{"mode":"aggregation","shape":"wallet","iterations":256,"signer_ids":[1,2,3],"control_dir":"aggregation-control"}
```

Commands, run serially after E11 stopped:

```sh
cargo fmt --all
cargo build --locked --release -p cashu --no-default-features --features federation --example bls_batch_verification_benchmark
python3 "$RUN_ROOT/smoke_aggregation_benchmark.py"
cargo clippy --locked -p cashu --no-default-features --features federation --example bls_batch_verification_benchmark -- -D warnings
```

Cargo commands used the pinned stable Nix shell; all exited zero. The functional
driver passed 49 valid manifests: all three existing shapes in proof/share modes,
all ten three-of-five signer subsets in every shape, three six-of-eleven subsets
in every shape, reversed order, boundary signer IDs and a twenty-signer fixture.
Eight invalid manifests were rejected before readiness, covering omitted/empty,
zero, duplicate, excessive and out-of-range IDs, incompatible mode and unknown
fields. Every successful measured iteration matched the direct reference; source
hashes remained unchanged through smoke and strict lint.

These are benchmark correctness checks, not performance samples or evidence of
an optimization. No production arithmetic, API, feature default, live payment
fixture or E07 file changed. The bounded public-signer SDK diagnostic remains an
unapplied private patch. The next step is measuring actual selection frequency
and baseline aggregation cost before evaluating a production candidate.

### Aggregation baseline: nineteen explicit signer-set observations

Ran the committed example at `21df7c945` with unchanged production arithmetic and
the existing native sampler/handshake protocol. Preparation and five warmup
batches precede resource accounting; bounded start/result-write overhead remains
in the CPU envelope. Sampling is 100 Hz. These are unpaired attribution runs,
not keep/revert comparisons.

The workload manifest contains all ten three-of-five subsets with twelve-output
wallet batches (256 iterations each), three six-share wallet sets (256 each),
and single/fragmented shapes for sets 1/2/3, 1/2/4 and 1/3/5 (1,024 and 64
iterations respectively). All 6,592 batches and 55,296 output aggregates matched
their direct reference signatures. Every process exited normally; source hashes
remained unchanged. No production probe was used.

```sh
python3 "$RUN_ROOT/run_aggregation_baseline.py"
```

| Workload | Observed CPU per output |
| --- | ---: |
| Three-share wallet, all ten subsets | 0.58061–0.58693 ms |
| Six-share wallet, three subsets | 1.14900–1.15375 ms |

These ranges span different signer-set workloads; they are not confidence
intervals. Together with the existing G1 multiplication profile, the costs support
measuring whether public interpolation coefficients can avoid general scalar
multiplication. They establish no candidate gain or real payment throughput.
The [nineteen-run ledger](benchmarks/2026-09-08-aggregation-baseline-runs.csv)
records every observation; all per-batch durations, native samples, manifests and
the frozen binary remain private. p99 is reported only for the 1,024-iteration
single-output component runs, never as payment p99.

### Bounded signer-selection diagnostic: isolated build and fresh run

Created a code-only detached checkout from `21df7c945`, excluding provisional E07.
Its sole source difference is a temporary wallet diagnostic that records at most
256 public signer-ID sets and output counts, after share validation/selection and
before aggregation. It uses the existing opt-in performance tracing target.
No secret, proof, signature, invoice or custody identifier is logged by this
probe. Root production source is unchanged.

The pinned release SDK build (`wallet,federation,iroh`, no default features) and
formatting completed successfully. Binary and source fingerprints verify that
only the diagnostic module differs; Cashu arithmetic and feature configuration
match the retained baseline. No live custody was copied. A read-only Docker
network preflight initially encountered a null IPAM configuration; handling
networks without IPAM completed the collision check without changing any network.

```sh
python3 "$RUN_ROOT/build_aggregation_selection_sdk.py"
python3 "$RUN_ROOT/run_aggregation_selection.py" --concurrency 4 --arm baseline
```

The second command is running on a new disposable five-member HTTP/SQLite
fixture. It reuses the original funding, native-accounting, receipt, conservation,
idle and bounded-shutdown helpers. Only the SDK tracing flag and diagnostic
artifact differ. Instrumented timings are explicitly excluded from performance
acceptance. Selection frequency and receipt results are pending; no production
optimization has been implemented from this hypothesis. The probe remains confined
to the private diagnostic build and must be absent from acceptance builds.

### Selection diagnostic completed; E12 public interpolation hypothesis

The fresh diagnostic completed all 64 receives, with 64 successful aggregation
events and twelve outputs each. Independent stopped-state auditing verifies 960
unique spendable notes, the exact original spent-input set on all five members,
77 accepted operations and 8,192 = 6,144 + 2,048 sat. All owned services stopped;
no original state was changed by the audit. The
[selection counts](benchmarks/2026-09-08-aggregation-selection-counts.csv) contain
every observed signer set. Public arithmetic classifies 1,728 of 2,304 terms as
small signed integers (75% in this diagnostic); this is not a measured saving or
a claim about all future signer selections.

Inspection of the locked `bls12_381` 0.8.0 implementation confirms that
`G1Projective::multiply` invokes a double, addition and conditional selection for
each of 255 scalar bits. Its fixed-work implementation serves secret scalars.
Federation interpolation derives its coefficients entirely from public signer
IDs. This evidence broadens the earlier small-integer hypothesis: a single
public-only binary multiplication can also omit additions for unset bits in
fractional field coefficients, avoiding a separate small-integer fast path and
fallback.

E12 is therefore prescribed before production edits:

- Owning layer: Cashu's federation-only blind-share interpolation, reached by the
  production wallet aggregation caller. Other located callers are test code.
- Derive the coefficient inside a private helper accepting public shares/IDs,
  choose its smaller signed field representative, and use binary double/add on
  those public bits. Keep secret-scalar library calls, interpolation mathematics,
  threshold validation, encodings, point validation and public APIs unchanged.
- Expected work reduction: omit leading-zero work and additions for unset public
  bits. Confirm instruction/CPU savings rather than treating source counts as
  measured instruction counts. The primary component metric is CPU per output.
- Use the nineteen explicit signer-set/shape workloads above for five alternating
  matched component pairs, extending inconclusive guards to ten and reverting if
  still inconclusive. Apply the approved latency/CPU/memory/idle rules unchanged.
- Check direct library multiplication equivalence, five/eleven-member subsets,
  boundary IDs, subgroup rejection and the existing threshold, corruption,
  permutation, restore and partial-change coverage before retention.
- Pre-specify the subsequent primary runtime comparison at 256 independent
  32-sat receipts, concurrency four, five-member HTTP/SQLite, 8,192-sat funding,
  one input/twelve outputs per receipt, warm persistent clients, a 180-second
  observation window, unchanged 120-second drain and 30-second idle audit.
  This larger new workload improves tail sampling; it does not reopen or change
  E11's rejected series. Report all failures/late outcomes and no payment p99 from
  256 samples. The observation window is not an increased operation timeout or
  queue. The final matrix and original-baseline comparison remain mandatory.

The temporary diagnostic module was restored after verifying its exact patch
and source hash. Both a clean SDK and component benchmark have now been rebuilt
and frozen from the same isolated checkout for future matched comparisons. The
shared target outputs are clean again; only explicitly named private diagnostic
artifacts retain the probe for evidence. No E12 production change or performance
gain is accepted yet.

### E12 component implementation and five-pair result

Implemented the prescribed private public-coefficient interpolation helper. It
retains the original scalar interpolation and validated group points, choosing
the smaller signed coefficient and processing only its significant public bits.
Secret-scalar multiplication and all public APIs remain unchanged. Provisional
E07 is excluded from the isolated comparison source and both artifacts.

Validation commands (pinned stable Nix environment, 16 MiB test thread stack):

```sh
python3 "$RUN_ROOT/validate_e12_component.py"
python3 "$RUN_ROOT/build_e12_candidate.py"
python3 "$RUN_ROOT/run_e12_component_pairs.py"
python3 "$RUN_ROOT/review_e12_component.py" --pairs 5
```

The component validation passed: 644 Cashu federation tests, two ignored;
506 ordinary tests; strict scoped library/benchmark Clippy; formatting; and the
ordinary normal dependency tree without BLS. The new equivalence test compares
5,654 point products against library scalar multiplication across every
three-of-five and six-of-eleven subset, singleton/boundary IDs and twenty members.
The separate focused invocation overlaps the full suite and is not an additional
test count. An on-curve non-subgroup fixture verifies rejection at point decoding.

Both arms use base `21df7c945`, identical pinned release commands and benchmark
source; only the BLS implementation hash differs. No temporary probes are present.
The five alternating pairs completed all nineteen prescribed workloads: 190 runs,
65,920 batches and 552,960 output aggregates, every aggregate checked against its
reference. The [run ledger](benchmarks/2026-09-08-e12-aggregation-5-pairs-runs.csv)
and [comparison ledger](benchmarks/2026-09-08-e12-aggregation-5-pairs-comparison.csv)
retain all run-level metrics and intervals. Native samples, per-batch durations,
commands, source patches and frozen artifacts remain private.

CPU per output decreased by 24.10–94.68% across the distinct workloads. Every
workload's CPU and instruction-count ratio interval is below one. All prescribed
component guards pass: p95, CPU, sampled RSS/physical footprint, lifetime physical
footprint, and observed batch rate. The range compares different workloads;
it is not one combined confidence interval or an end-to-end payment improvement.
Only the 1,024-sample component cases support p99.

Decision: advance to the already prescribed 256-receipt runtime comparison.
Production retention remains pending real payment CPU/latency/memory/idle guards,
wallet adversarial tests and recovery. The matched runtime comparison varies the
SDK only: inspection locates the production aggregation caller in the wallet;
the member service remains the same immutable retained artifact in both arms.
The larger workload spends all 256 original 32-sat bank notes: expected final
state is 269 accepted operations, zero bank balance and 8,192 sat across 256
recipient wallets (3,072 unique notes). New one-shot controllers preserve older
64-receipt experiments and all their original custody; observation stays at the
pre-specified 180 seconds, drain at 120 seconds, and no operation timeout changes.

### E12 real ecash series started

```sh
python3 "$RUN_ROOT/run_e12_ecash_pairs.py"
# After five completed pairs, without changing artifacts or workloads:
python3 "$RUN_ROOT/review_e12_ecash.py" --pairs 5
# Only after a passing performance review, outside live measurements:
python3 "$RUN_ROOT/validate_e12_runtime.py" --pairs 5
```

The first command is running; the latter two have not run. The one-shot series
freezes controller hashes and both SDK fingerprints, alternates arm order, and
runs the existing fresh-backend, federation-setup, funding and measurement
helpers with explicit 256-receipt expectations. Every arm must stop its owned
services and pass a read-only note-identity audit before the next starts.
Existing 64-receipt helpers and custody remain unchanged. A preflight confirmed
no live campaign members or overlap with the planned disposable networks.

The first baseline completed backend preparation, real five-member DKG and
confirmed incoming on-chain funding. Offline packaging produced exactly 256
unique tokens from 256 original notes, totaling 8,192 sat; all bank notes are
pending-spent, with no unspent bank remainder. Warm SDK preparation completed
without changing accepted history, and the timed window started with tracing
disabled. Payment results, all five paired comparisons and acceptance remain
pending. The prepared follow-up checks cover the exact SDK candidate source,
ordinary downstream APIs, ordinary/federation FFI and formatting; they are gated
on the performance review and do not run alongside measurements.

The first baseline arm subsequently completed all 256 receipts inside the window,
with zero failed, late or unfinished requests. Its observed p50/p95 are
1,205.40/1,620.98 ms, SDK CPU is 53.020 ms per receipt, and the last receipt arrived
at 77.462 seconds. These are one baseline observation, not a paired improvement
or sustainable-throughput result; payment p99 remains unavailable. The independent
identity audit confirms 3,072 unique spendable recipient notes disjoint from all
256 original inputs, identical spent sets on all five members, 269 accepted
operations, and 8,192 = 0 bank + 8,192 recipient sat. Idle state stayed unchanged
and all owned services stopped. The first candidate arm has started with fresh
custody. Service setup logs retain the baseline member-binary label because the
same retained service artifact is used in both arms; SDK fingerprints and arm
manifests identify the actual comparison. No production retention decision yet.

### E12 real ecash: five pairs complete, prescribed extension required

All ten fresh runs completed their 256 receives inside the fixed window: **2,560
receipts**, zero failures, late receipts or unfinished requests. Each independent
stopped-state audit verifies 3,072 unique spendable recipient notes, disjoint from
all 256 original inputs, identical member spent sets, 269 accepted operations,
and exact 8,192 = 0 bank + 8,192 recipient sat conservation. Member digests agree
within every run. Idle audits preserve quiescent state, all owned services stopped,
and all original custody remains intact. No temporary probe was used.

```sh
python3 "$RUN_ROOT/review_e12_ecash.py" --pairs 5
python3 "$RUN_ROOT/export_e12_ecash_evidence.py" --pairs 5
```

The review returns `extend_to_ten_pairs`. All
[ten runs](benchmarks/2026-09-08-e12-ecash-5-pairs-runs.csv),
[2,560 requests](benchmarks/2026-09-08-e12-ecash-5-pairs-requests.csv) and
[paired comparisons](benchmarks/2026-09-08-e12-ecash-5-pairs-comparison.csv)
are preserved, including the less favorable observations. No run was discarded.

| Metric | Candidate/baseline ratio | Run-level 95% CI |
| --- | ---: | --- |
| SDK CPU per receipt | 0.96693 | 0.87457–1.06904 |
| Combined execution CPU per receipt | 1.03982 | 0.97288–1.11137 |
| p50 receipt latency | 0.99995 | 0.97387–1.02674 |
| p95 receipt latency | 1.06050 | 0.93124–1.20770 |
| SDK peak RSS | 1.00059 | 0.99875–1.00242 |
| SDK peak physical footprint | 1.00056 | 0.99858–1.00255 |
| Member aggregate peak physical footprint | 1.04415 | 0.92481–1.17890 |
| Idle member aggregate CPU | 0.98908 | 0.90461–1.08145 |

The arithmetic mean SDK CPU values are 56.513 and 54.963 ms per receipt;
paired ratio estimates use the analyzer's run-level comparison and are not ratios
of these arithmetic means. The primary CPU improvement is **inconclusive**.
The p95, SDK CPU, combined CPU, member physical-footprint and conservative active
member CPU guards also remain inconclusive. The latter compares candidate upper
CPU bounds against baseline lower bounds and has interval 0.99044–1.12884; it is
not an exact change estimate. SDK memory and member RSS guards pass. Idle CPU
shows no reproducible increase, but establishes no idle benefit. No guard shows
a confirmed regression under the prescribed decision rule.

The later runs vary materially from the first pair. These results do not establish
the cause; neither a production regression nor external interference is assumed.
The component work savings therefore do not yet justify production retention.
No payment p99 is available from 256 samples per run, and this finite workload
establishes no sustainable-throughput gain. Candidate-only domain/FFI/restart
acceptance checks remain gated and have not run.

Next, use the pre-existing extension controller, preserving the first-five files,
source hashes, immutable artifacts, workload, ordering rule, observation/drain
bounds, resource accounting and receipt audits:

```sh
python3 "$RUN_ROOT/extend_e12_ecash_pairs.py"
python3 "$RUN_ROOT/review_e12_ecash.py" --pairs 10
python3 "$RUN_ROOT/export_e12_ecash_evidence.py" --pairs 10
```

If ten pairs remain inconclusive, revert E12's production change and retain the
investigation evidence. Neither E12 nor provisional E07 is accepted by this
milestone. The complete campaign matrix, original-baseline comparison, all payment
directions, recovery gates and final profiling passes remain open.

Between series, the pinned `nix develop .#stable --command cargo fmt --all --
--check` completed successfully. The review/export commands also completed
successfully; private logs and the first-five source/artifact fingerprints remain
preserved. Only evidence is committed at this point.


## September 9 — bounded closeout at user request

- Decision: stop extending the campaign. Keep previously accepted commits; revert
  E12 public interpolation and E07 session replay cursor because acceptance is
  unresolved. Save exact patches privately before applying their reverse diffs.
- Source: retained HEAD `71d57b437`; Bark pin unchanged. After reversal there are
  no provisional production changes. Reusable benchmark support is retained.
- E12 extension: 17 completed and audited arms (eight pairs plus ninth baseline),
  4,352 receipts. Export every available scalar metric to the closeout CSV, with
  pair/arm/order preserved. Do not recompute an acceptance decision on an
  opportunistically truncated series. Original first-five ledgers remain intact.
- Interrupted ninth candidate: measurement/result and SDK/results are absent.
  All original recipient databases nevertheless contain the completed receipts.
  A read-only adaptation of the identity audit (without completed-measurement
  prerequisites) verified 256 recipients, 3,072 unique notes, exact 8,192 sat,
  no spendable bank notes, disjoint spent inputs, identical original spent sets
  on all five members and 269 journal entries per member. This is receipt
  reconciliation, not a recovered timing sample or complete runtime digest audit.
- Shutdown: the existing bounded owned-fixture shutdown helper exited zero;
  process inspection found no campaign mint, payment driver or fee proxy.
  Preserve all custody, recipient and backend state. Do not rerun one-shot helpers.
- Checks: `git diff --check` passed. `nix develop .#stable --command cargo fmt
  --all -- --check` passed (existing stable-rustfmt option warnings). The first
  sandboxed Nix invocation could not open its cache lock; the authorized retry
  succeeded. No new heavy test suite was started at closeout.
- Instrumentation: no temporary production probes retained. Candidate patches,
  interrupted-run audit and raw extended records remain private and ignored.
- Delivery limitation: this closes investigation at the user's deadline; it does
  not satisfy the original full campaign completion criteria. See the report's
  closeout section for missing combined correctness and performance gates.


## September 9 — authorized targeted reconsideration, digest reuse first

The user authorized reconsidering E05 and the incoming collector sequentially,
committing each only after confirming its benefit. This does not reopen the full
campaign. E05's new question is memory behavior on the same settled original
state across sequential baseline/candidate restarts, avoiding independently
generated fixture variation. The implementation reuses the existing immutable
`FederationRuntimeConfig` digest: no new cache, allocation or stored field.
Preserve checkpoint shape, federation, digest and member binding checks.

Before measurement: run current-source checkpoint/recovery/standalone checks and
freeze release binaries on the same retained base. Use five alternating matched
pairs, a fixed 60-second warmup and a 180-second idle observation, with no other
heavy work. Primary metrics are aggregate CPU and instructions; retain the 5%
RSS/physical-footprint regression guard. Report memory over the full window and
its first/last thirds to expose growth. Preserve original fixture identities and
state; never clone or replay payment initiation. The earlier ten-pair results
remain unchanged. If guards are inconclusive, do not claim acceptance.

Current-source validation passed: common checkpoint 15, full-feature Axum
checkpoint 43 (two manual benchmarks ignored), recovery 46 (one overlapping
manual benchmark ignored), minimal federation checkpoint 39; counts overlap.
Common and Axum strict lib/test Clippy, standalone common compilation and
formatting passed. Commands match the earlier scoped validation with current
source and pinned Nix stable, `RUST_MIN_STACK=16777216`. An initial idle compiler
was terminated and the complete retry passed; both attempts remain private.
No benchmark overlapped validation or release compilation.

The first fixed-state series aborted on pair 2 baseline: a member advanced its
AlephBFT session while peers remained in the prior session. The end-of-window
convergence guard failed; no acceptance statistics are computed from this
incomplete series. Preserve all three completed records and the failed arm's
resource trace. Pair 1 alone showed 43.15% lower idle CPU, but higher candidate
memory; it is not acceptance evidence. All owned services stopped cleanly.

Before further measurement, select a newer settled ecash fixture whose original
269 operations and 8,192 paid/issued sat agree on all five members. Repeat the
same frozen five-pair, 60-second warmup/180-second idle recipe under a new series
identifier, changing only fixture/history and its required fixed-fee RPC proxy.
Use the same immutable binaries. Preserve all existing wallets and original
backend state; create no new payment, clone no custody, and do not repair the
older fixture by changing database contents or adding unrelated runtime fixes.

The newer fixture's first setup failed before measurement: the helper started
the fixed-fee proxy before Bitcoin readiness, and its upstream check failed.
The controller was interrupted through its cleanup handler; owned services
stopped and the original binary link was restored. The corrected helper starts
the proxy after backend readiness, records a separate retry directory, and
retains the same binaries, warmup, duration, sampling and guards. No payment was
initiated or retried, and no performance sample exists for the failed setup.

The restarted-fixture approach is rejected as an acceptance design. Inspection
of retained status records showed member one's backup growing from 2,403 chunks
(1,211,235 bytes) before the first baseline to 7,970 chunks (1,940,512 bytes)
before the third candidate despite unchanged payment history. The first baseline
also had 14 response-cache entries versus zero subsequently. These inputs were
not controlled by the original projection-only guard. Stop through bounded
cleanup, preserve every record and do not compute acceptance from this series.
The apparent CPU reductions remain diagnostic observations, not a retained win.

Use the established independent fresh-fixture recipe instead: fresh five-member
DKG and empty payment history per arm, matched backend economics and policies,
60-second warmup plus five seconds after status, and 180-second idle observation.
Reuse frozen current-source binaries and original startup helpers. Record initial
and final backup sizes in addition to memory thirds. No persisted custody is
cloned or reset. Five alternating pairs; original ten-pair evidence is untouched.
The private harness watchdog is adjusted for the longer observation duration;
no production timeout, payment timeout, queue or consensus policy changes.

### Fresh reconsideration: five pairs completed, extend unchanged

Ten independent fresh arms completed with matching semantic inputs, unchanged
backup sizes within every observation, stable checkpoint/payment state and
bounded shutdown. Source files still match the frozen candidate. Relative to
retained base `197fef899`, idle CPU ratio is 0.57200 (95% CI 0.51965–0.62962):
42.80% lower CPU. Instructions ratio is 0.21995 (0.21761–0.22231): 78.01% lower.
Peak RSS ratio is 0.93051 (0.79686–1.08659); physical footprint ratio is 1.01378
(0.95259–1.07889). Thus both memory guards are inconclusive despite no confirmed
regression. Package wakeups are inconclusive; interrupt wakeups are similar.
Do not discard the fifth pair's lower candidate RSS or higher baseline physical
peak. No memory-growth claim follows from those differences.

Preserve all ten rows in the five-pair CSV and the original raw records. Extend
the exact fresh-fixture recipe to pairs 6–10, with unchanged binaries, helper
hashes, windows and guards. No implementation is accepted or committed yet.

### Fresh reconsideration: ten-pair result and retention boundary

`run_e05r_fresh_pairs.py --first 6 --last 10` completed and stopped every owned
fixture. `review_e05r_fresh.py --pairs 10` preserved every row and returned:
CPU ratio 0.574526 (95% CI 0.541044–0.610080), instructions 0.218896
(0.217485–0.220317), RSS 1.028829 (0.916384–1.155073), physical footprint
1.012685 (0.987671–1.038334). Physical memory passes the 5% guard; RSS does not.
No confirmed RSS regression is established. No sample or outlier is removed.

`audit_e05r_final.py` passed: all 20 observations stable, all 100 member databases
preserve zero accepted payment operations/quotes, owned containers stopped,
source matches the frozen candidate, helper hashes unchanged and first-five raw
records unchanged. Response-cache entries remain zero; backup sizes are unchanged
within every window. The largest observed candidate first-to-last-third physical
footprint increase is 286,063 bytes across five members. No production probe,
new state field, mutable cache, wire change or dependency override was introduced.
`git diff --check` passed. Earlier rejected designs and the session-divergence
window are exported separately; the interrupted partial window has no complete
sample. None enters the fresh-series comparison.

Decision boundary: implementation and scoped validation are complete, but the
original rule requires removal after an inconclusive ten-pair guard. Keeping
the measured CPU win requires an explicit exception for RSS; physical-footprint
and correctness gates are not being waived. Candidate remains uncommitted while
that retention decision is resolved. The Bark collector is still pending.


### Digest retention approved

The user explicitly approved the pending RSS-only exception on September 9.
Keep the tested digest reuse and all twenty fresh measurements. The original
RSS guard remains inconclusive; no statistical method, sample selection or
acceptance result is changed. Physical-footprint and scoped correctness gates
passed and remain mandatory. Commit this isolated optimization before modifying
the Bark collector. The exception does not extend to the collector or any other
experiment.
