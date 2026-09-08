# Federation performance optimization campaign

Status: implementation in progress. Approved campaign; acceptance remains pending.

Baseline: CDK `8e507cdf8`, Bark `8d1eda643dd11e9eb600ec44e36622c8f77b6322`.

## Objective and boundaries

Optimize DKG, incoming/outgoing Bark Lightning, incoming/outgoing BDK on-chain,
and direct ecash. Balance latency, sustained throughput, CPU efficiency, and idle
resource use. Preserve readability and maintainability. Federation-specific Bark
changes are in scope when profiles establish a dependency bottleneck.

Preserve ordinary CDK/Bark behavior, public wallet APIs, wire formats, canonical
encodings, cryptographic constructions, consensus authorization, independent
validation, nonce safety, durable payment identities, atomicity, and recovery.
Never weaken quorum, confirmations, durability, or validation; never increase
timeouts or queues to hide overload. Prefer removing work and waiting over new
abstractions. Do not deploy, rewrite published history, or delete existing state.

## Reproducible measurement

Extend the existing DKG phase benchmark, AlephBFT swap benchmark, parallel ecash
harness, and local payment driver. Add a common result format and analyzer, not
another orchestration framework. Pin Nix toolchains, release profiles, features,
dependencies, and resource limits. Preserve unchanged baseline artifacts.
Compilation, preparation, funding, and human approval are outside transaction
timing. Historical/debug timings are context, never campaign baselines.

Use independent disposable fixtures. Never run cloned live custody or nonce
state concurrently. Keep existing databases and services intact. Use persistent
SDK clients for federation execution; measure CLI startup and wallet opening
separately. Match connection state, liquidity, amounts, denominations, checkpoint
policy, and actual proof/output/UTXO/VTXO counts across comparisons.

| Dimension | Coverage |
|---|---|
| Members | Five primary; eleven validation; twenty for DKG scaling |
| Transport | HTTP and Iroh; connection establishment separately |
| Concurrency | 1, 4, 16, 64 independent payments; separate shared-wallet contention |
| Network | Local; controlled 50/150 ms RTT with jitter; final asymmetric/slow-member stress |
| History | Fresh, 1,000 and 10,000 accepted operations |
| Storage | SQLite primary; PostgreSQL correctness and matched comparisons for SQL changes |
| Holdings | Standard and fragmented; actual input/output counts recorded |
| State | Cold startup, warm steady state, quiescent idle, restart/recovery |

Run the primary matrix first, then vary scaling dimensions individually instead
of multiplying every dimension. Use closed-loop concurrency and bounded open-loop
arrivals. Include client queueing in latency. Report offered load, completed load,
failures, timeouts, unfinished work, and late completions. Sustainable throughput
requires a stable queue; completions after the window do not enter window TPS.

Record p50/p95; report p99 only with at least 1,000 completed samples. Record member
and aggregate CPU seconds, CPU/payment, RSS, allocation samples, SQL reads/writes and
rows decoded, network bytes, retries, idle CPU/wakeups, and load-related queue,
lock, scheduler, semaphore, and external I/O waits. Distinguish user-confirmed
payments/s, consensus operations/s, and Bitcoin transactions/s. Batched outputs
and protocol operations must not inflate payment throughput.

Reuse existing phase timers and counters. Temporary probes must be bounded and
cover missing DKG phases, lock acquisition/hold, CPU admission, SQL, crypto, and
notifications. Use local sampling profiles for attribution. Keep raw traces,
profiles, credentials, payment secrets, and identifying environment information
outside Git. Use monotonic durations and causal correlation, never timestamps
from unrelated clocks or sums of overlapping spans as wall time.

## Hypothesis queue

Each experiment requires measured cost before production edits.

| Area | Complete path and candidate costs |
|---|---|
| DKG | Authorized start through BLS/FROST generation, delivery/persistence, transcript validation and activation; readiness separate. Measure ceremony-scoped reads versus whole-history filtering, clones/serialization, unchanged-plan work, CPU permits, phase polling and delivery concurrency. Reuse existing validated-state and plan caches. |
| Bark incoming | Quote/invoice, payer completion, HTLC observation, secret/evidence, claim, paid state, spendable ecash. Measure residual polling/notification gaps, projections/probes, independent serialized RPCs, derivation/validation and lock scope. Preserve replication, evidence, claim ordering and acknowledgements. |
| Bark outgoing | Quote/melt admission, accepted plan, custody signing, initiation, terminal evidence, settlement/change. Measure coordinator queues, materialization, signing, RPC reuse, polling and validation. Coalesce equivalent observations only within existing ownership. Ambiguous outcomes never authorize another initiation. |
| BDK incoming | Address allocation, deposit visibility, required confirmation, member observation, paid quote and issuance. Measure scans, per-quote queries, unchanged-tip work and observation scheduling. Test shared per-member observations only with evidence. |
| BDK outgoing | Quote/proposal, PSBT, FROST, durable broadcast, confirmation, paid quote/change. Measure fee estimation, locks, derivation, unavailable-input retries, repeated broadcasts/probes and existing batching. Preserve independent checks and exact transaction identity. |
| Ecash | Token preparation, optional sender swap, recipient swap, proof verification, share collection and persistence. Separate offline packaging; profile serialization, transactions, fanout, connection reuse and wallet contention. Preserve request-bound verification reuse and disagreement handling. |

DKG workloads cover one, eight, and the full configured denomination set. Every
ceremony requires every roster member, even when the signing threshold is lower.
Use an independent fixed-schedule miner for BDK; separate broadcast and
post-confirmation federation latency from block waiting. Record independent
Bark payer/server settlement to attribute external delay correctly.

After subsystem work, re-profile shared submission, consensus application,
checkpoints, notifications and transport. Change existing scheduling, pump limits
or batching only with causal evidence. Previously rejected experiments require
new evidence before reconsideration.

## Keep-or-revert protocol

1. Before editing, journal hypothesis, owning layer, expected counter reduction,
   primary metric and workload manifest.
2. Confirm the cost using timings/counters and sampling profiles.
3. Make one narrow change.
4. Run five matched baseline/candidate pairs in alternating order, recording all
   samples and run-level confidence intervals.
5. Expand inconclusive comparisons to ten pairs; if still inconclusive, remove
   the production change and retain the investigation record.
6. Run relevant adversarial/recovery checks, then commit accepted code/evidence.

Retention requires predicted work reduction and a repeatable latency, CPU, idle,
or sustainable-throughput benefit. Small improvements qualify; no minimum 15%
gain applies. Reject confirmed regressions greater than 5% in p95, CPU/payment,
sustainable throughput or memory at matched load. Idle CPU may not reproducibly
increase. Noise is inconclusive, not passing. Compound improvements need measured
individual work savings and a combined system gain; use ablation for uncertainty.

Remove temporary probes before final performance acceptance. Retain only reusable
benchmark support and useful bounded operational metrics with measured overhead.
No benchmark-only production fast paths or speculative caches.

## Correctness and delivery

Required gates: ordinary-CDK isolation, feature matrix, FFI, formatting and scoped
strict lint; malformed/missing DKG artifacts, lost acknowledgements, cancellation,
CPU admission and restart at durable phases; invalid/conflicting shares, quorum,
partial change, restore, overlapping spends and replay; SQL rollback, deterministic
application, digest agreement and long history; lost notifications, ambiguous
external outcomes, duplicate requests, slow members, worker failure, bounded
shutdown and catch-up; fresh real regtest payments in every direction with
independent receipts, restart recovery and exact value conservation.

Compare member digests within each run. Cross-revision comparisons use identical
recorded replay fixtures, not independently generated ceremony keys/digests.

Deliver small reviewable commits. Bark changes remain separate; update CDK only
to an immutable fetchable Bark revision, with no local dependency override.

The [journal](bls-federation-performance-journal-2026-09-08.md) records commands,
revisions, workloads, every sample, profiles/counters, interpretation, tests,
probe disposition and decisions, including rejected/inconclusive experiments.
The [report](bls-federation-performance-report-2026-09-08.md) separates measured
results from hypotheses and limitations, with baseline/final operation results,
phase breakdowns, throughput curves, CPU, history scaling and external costs.

Completion requires a disposition for every measured dominant bottleneck and
two successive profiling passes finding no further supported maintainable
improvement. Compare the combined candidate against the original baseline,
repeat final correctness/payment gates and remove probes. Report actual gains
without claiming an unmet objective was achieved.
