# Federated Cashu Performance and Issue Report

Date: 2026-07-14

## Executive summary

The current federation implementation performs well for normal local operation
and restart, but it still has a critical recovery defect. A five-member local
federation reached healthy state in 1.303 seconds, processed 100 parallel
receives at 25.09 committed swaps per second, restarted all five members at a
316-operation height in 2.958 seconds, and restarted one member with an intact
database in 1.673 seconds.

A member whose database was erased did not recover. After approximately 155
seconds it held only 1 of 316 accepted operations while reporting
`sync=healthy` and `ready=true`. Its logs showed an AlephBFT backup/collection
round mismatch followed by a new session that the other four members rejected
as the wrong session. This is both a catch-up failure and an incorrect readiness
claim. It is the highest-priority open issue in this report.

The same local run also found two load-sensitive failures: nine of ten repeated
standard-operation workers returned `Payment failed` during their second melt,
while the tenth stalled at the same step; and one heavily used member stopped
answering signed private status requests even though its public information
route remained fast. A restart cleared the status failure.

A fresh Nocean deployment exposed a separate consensus liveness and
backpressure failure. The first receive case, configured for only one concurrent
wallet, caused all five public HTTPS endpoints to become temporarily
unresponsive. Only 10 accepted journal operations produced approximately 744
AlephBFT DAG units. `UnitBroadcast` queues grew to roughly 80-160 messages per
member while receive requests timed out, then the public endpoints recovered
without a restart after the queues drained. Mint CPU remained far below host
saturation. The strongest current hypothesis is that the 100 ms active unit
cadence continues creating empty units while network-activity leases remain
active, amplifying a small workload into enough consensus traffic to starve
HTTP work.

Earlier Nocean testing found wallet fanout, retry, and ambiguous-result problems
under member catch-up pressure. Those findings remain relevant even though the
new local receive-only result is much faster. The local and remote throughput
numbers are not directly comparable because the network, host capacity,
software revisions, and workload shapes differ.

## Scope and evidence labels

This report consolidates:

- the 2026-07-13 five-member Nocean receive stress test documented in
  [federated-cashu-nocean-stress-test.md](federated-cashu-nocean-stress-test.md);
- the 2026-07-14 local five-member restart, recovery, and transaction benchmark;
- the local CPU regression investigation already recorded in that stress-test
  document.

The labels used below are:

- **Confirmed:** directly reproduced with process, journal, database, or client
  evidence.
- **Most likely cause:** consistent with the trace and code path, but not yet
  proven by a minimal failing regression.
- **Resolved and verified:** reproduced before the fix and measured or tested
  after the fix.
- **Needs revalidation:** previously observed, but affected code has changed and
  the exact process workload has not yet been repeated.

## Test configurations

### Local benchmark

- Five members on one local machine.
- BLS signature threshold: 3 of 5.
- Consensus threshold: 4 of 5.
- AlephBFT consensus, SQLite databases, and fakewallet payment processors.
- Current release-mode `cdk-mintd` and federation-enabled `cdk-cli` built from
  the same working tree before the run.
- Strict restart health required ready members, one converged height and digest,
  catch-up-certificate coverage, and no digest or invalid-operation alerts.
- Temporary processes were stopped after the run.

### Nocean benchmark

- Five co-located members on a four-logical-CPU development server.
- Remote load generation over HTTPS.
- The same 3-of-5 signature and 4-of-5 consensus thresholds.
- Full details, binary identities, method, and raw result tables are in the
  dedicated Nocean stress-test document.

### Fresh Nocean backpressure reproduction

- The current local working tree was compiled for Linux on the development
  machine and copied to the server with `scripts/deploy_nocean_federation.sh`.
- The remote federation was stopped, all member databases, AlephBFT backup
  state, and DKG state were erased, and a new five-member federation was set up
  and started. This was an intentionally destructive reset of a development
  deployment.
- Before load, all five members were healthy and converged at zero operations,
  with empty mempools and finalized cursors at zero.
- Idle interval CPU was approximately 1.1-1.2% per mint and each new SQLite
  database occupied approximately 4 KiB.
- The default `scripts/benchmark_nocean_federation.sh` matrix was started. The
  failure appeared during its first timed case, `N=10, parallelism=1`, after
  token and receiver-wallet preparation had completed.
- CPU, public and loopback endpoint latency, signed federation status, journal
  height, DAG statistics, and AlephBFT repeat counters were observed while the
  case ran and after it was interrupted.

## Performance results

### Current local results

| Measurement | Result | Qualification |
| --- | ---: | --- |
| Initial five-member startup | 1.303 s | Zero-operation federation to five healthy live sessions |
| Sequential standard batch | 7 entries in 4.797 s | Mint, swap, melt, restore; not a raw swap benchmark |
| Parallel receive workload | 100/100 successful | One accepted swap per receive |
| Parallelism | 20 | Isolated receiver wallet per token |
| Client-completion journal rate | 25.09 entries/s | 100 entries in 3.986 s |
| Post-client convergence delay | 1.710 s | All five members converged after clients completed |
| Commit-to-convergence rate | 17.56 entries/s | 100 entries over the full 5.696 s interval |
| Final converged height | 316 entries | Matching order and materialized-state digests |
| Full five-member restart | 2.958 s | Exact timed rerun at height 316 |
| One-member intact restart | 1.673 s | Height 316 with certificate coverage |
| One-member erased recovery | Failed | Only height 1 after approximately 155 s |

The 100-receive result is the cleanest current local throughput measurement. It
includes CLI process startup, HTTP fanout, consensus, threshold-share
collection, wallet verification, and wallet persistence. Wallet creation and
federation import occurred before the timed interval.

### Earlier Nocean results

| Workload | Observed result |
| --- | ---: |
| Sustained confirmed receive plateau | Approximately 0.27 swaps/s |
| Highest confirmed one-wave burst | 0.566 swaps/s |
| Highest journal-rate estimate | Approximately 0.566 entries/s |
| Concurrency-5 failure rate | 30.0% |
| Concurrency-10 failure rate | 47.5% |

These numbers must not be compared as if they were two revisions of one
controlled benchmark. The local run removed public-network latency and used a
different machine, newer code, a bounded one-wave receive test, and a corrected
commit-through-convergence timer. The improvement is encouraging, but it does
not by itself prove a 40-90x production throughput increase.

### Fresh Nocean backpressure incident

The fresh-state run did not complete a benchmark row because it was stopped
when the public endpoints became unavailable. Its partial measurements are
still diagnostically important:

| Observation | Result |
| --- | ---: |
| Timed case | `N=10, parallelism=1` |
| Operations before timed case | 6 |
| Operations after interruption and convergence | 10 |
| First successful receive | 6.587 s |
| Failed receive | 108.233 s |
| Failure detail | 0 of 3 required shares; all five member requests timed out after 90 s |
| Later successful receive | 35.082 s |
| Finalized items after recovery | 40 per member |
| AlephBFT DAG units after 10 accepted operations | Approximately 744 |
| Peak observed `UnitBroadcast` backlog | Approximately 80-160 per member |
| Mint CPU during observed bursts | Generally 4-12%, versus approximately 1-2% idle |
| Recovery | HTTPS recovered after backlog drainage; no restart required |

The interrupted case must not be presented as a completed TPS result. It does
show that the federation can enter a severe latency and availability failure at
parallelism 1, so this incident is not explained by excessive client
concurrency.

## Open correctness and liveness issues

### 1. Fresh-database member does not catch up

**Severity:** Critical  
**Status:** Confirmed

Member 5 was stopped at converged height 316 and its SQLite database, WAL, and
shared-memory files were deleted. The other four members remained live. The
fresh member was then given approximately 155 seconds to reach the existing
height. It reached only height 1.

The member did not merely recover slowly. Its logs showed:

- AlephBFT backup state behind the unit-collection state;
- the AlephBFT creator and consensus service terminating early;
- the member starting or advertising session 1 while the existing federation
  remained in session 0;
- repeated non-retryable session-mismatch responses in both directions;
- the private AlephBFT network receiver closing.

**Most likely cause:** startup ordering allows an empty member to install a live
AlephBFT session and mark itself healthy before peer-height discovery and REST
journal recovery. Once it is `Healthy` with a live session and inbound network,
`should_defer_catch_up_to_live_aleph_bft` defers REST catch-up. The empty member
then tries to participate without the persisted AlephBFT backup and finalized
history required to join the peers' current session. Session rotation makes the
split explicit rather than repairing it.

**Required fix:** a member with no trustworthy local checkpoint must begin
fail-closed in startup recovery, compare signed peer status, recover journal,
finalized-item, application, certificate, and AlephBFT backup state, and only
then install the live session and become healthy. Deferral to live AlephBFT is
valid only when the local live session is known to be continuous with the peer
session and history.

**Required regression:** start four members with several hundred operations,
erase the fifth database, and restart it. The fifth member must initially report
startup/catch-up rather than healthy, must not start a conflicting session, and
must reach the exact peer height, order digest, materialized-state digest,
finalized cursor, and catch-up-certificate coverage.

### 2. A behind member can falsely report healthy and ready

**Severity:** Critical  
**Status:** Confirmed; exposed by Issue 1

At local height 1 versus peer height 316, the erased member reported:

```text
sync=healthy ready=true operations=1
```

Its catch-up certificate covered its one local operation, so
`--require-ready --require-catch-up-certificate-quorum` was also insufficient to
detect the 315-operation deficit. The benchmark had to impose the expected
height explicitly.

**Most likely cause:** readiness proves internal consistency only against the
member's local tip. A fresh runtime defaults to healthy before it has established
a recent signed peer-height view, and the live-session catch-up deferral returns
without first marking the node lagging.

**Required fix:** distinguish local consistency from federation currency. A
member must not advertise authoritative readiness until it has compared itself
with quorum peer evidence during startup. The status response should expose the
last successful peer comparison and its age. The operator status command should
continue to compare members, but a single-member health check must also fail
closed when startup peer validation has never completed.

### 3. Wallet operations can commit while the client reports failure

**Severity:** High  
**Status:** Confirmed on Nocean; needs revalidation after wallet fanout changes

In the Nocean concurrency-5 and concurrency-10 cases, every attempted swap
advanced the converged journal even though 25 wallet processes exited without
enough shares. Database inspection later showed that all five mints applied all
25 operations and persisted operation-bound signature shares.

This creates an ambiguous client result: blind retry can encounter spent inputs
even though the original command reported failure.

**Most likely cause:** the wallet previously cached only the first
threshold-sized compatible member subset. One temporary catch-up/readiness error
therefore reduced a nominal 3-of-5 operation to 2-of-3. Even after scheduling all
members, returning after threshold success and dropping unfinished request
futures can leave minority handlers accepted-but-unapplied. Catch-up errors were
also treated too much like terminal request failures instead of retryable member
readiness states.

**Required behavior:** all validated roster members remain eligible; retryable
member states do not count as rejection votes; the identical deterministic
request remains recoverable until threshold success, threshold definitive
rejection, or an overall deadline; and a deadline returns a typed
pending/ambiguous result rather than an ordinary failure.

### 4. Threshold completion can cancel minority member application

**Severity:** High  
**Status:** Confirmed state divergence; cancellation is the strongest hypothesis

A local full-fanout follow-up found four members with an applied operation while
one member retained the same operation as accepted but unapplied. The client had
already obtained a threshold result.

**Most likely cause:** pending HTTP request futures are dropped once the wallet
has enough shares. Cancellation can occur after a minority handler has accepted
the journal item but before local application and response publication finish.

The wallet may stop waiting for surplus responses, but it must not be the owner
of whether an accepted mint operation finishes durably. The server submission
path needs cancellation-safe completion after acceptance, and the wallet should
drain or detach surplus requests in a bounded way that does not abandon server
work.

### 5. Wallet catch-up handling remains too brittle

**Severity:** High  
**Status:** Confirmed on Nocean; partially addressed, needs process revalidation

Wallet errors named members that were lagging, catching up, or verifying
catch-up certificates. These are retryable availability states, not definitive
transaction rejection votes. A catching-up member caused the command to fail
even when enough unused federation members could eventually provide shares.

The wallet must preserve operation IDs and blinded outputs, continue fanout to
all eligible members, retry with bounded backoff, and retain its saga on an
overall deadline. The mint should use a typed federation catch-up error and may
be able to return a strictly operation-bound persisted share for an exact replay
before applying the general new-request readiness gate.

### 6. Repeated concurrent melts fail or stall

**Severity:** High for federation melt availability  
**Status:** Confirmed locally; root cause not yet isolated

Ten concurrent standard-operation workers each completed one full mint, swap,
melt, and restore batch. During the second batch:

- nine workers returned `Payment failed` from melt confirmation;
- one worker stalled at `standard.melt-confirm.start` until the benchmark was
  stopped;
- mint, swap, check-state, and melt-quote stages had already succeeded.

This is not evidence of insufficient consensus throughput: the receive-only
test later committed 100 swaps successfully at parallelism 20.

**Most likely cause:** a race in concurrent melt payment ownership,
payment-observation finalization, or quote/saga state transitions. The current
evidence does not distinguish wallet polling from mint-side observer election or
fakewallet payment-state handling. It should not be dismissed as fakewallet-only
until the operation IDs, elected payer, payment identifiers, quote transitions,
and observations from one failed melt are traced across all members.

**Required regression:** run at least ten wallets through two consecutive melts
in parallel and require exactly-once payment, a threshold-consistent terminal
state, no stalled confirmations, and identical quote/payment observations on all
members.

### 7. Signed private status can become unresponsive under load

**Severity:** High operationally  
**Status:** Confirmed locally

After the concurrent standard-operation workload, member 1 continued to answer
the public information endpoint in approximately 30 ms but did not answer signed
private status within 20 seconds. Other members logged repeated 10-second status
timeouts against member 1 and increased reconnect backoff to 60 seconds. A
normal restart restored member 1 status immediately and it was healthy at the
existing height in 0.172 seconds.

**Most likely cause:** a private-status handler, admission permit, or state lock
was held or starved, rather than the HTTP runtime being generally unavailable.
Member 1 was also the preferred quote coordinator during the failing workload,
which concentrated quote and payment-observation work there. Status generation
serializes increasingly large payment reports and inspects several federation
caches, so lock duration and response size are plausible amplifiers. No stack
sample captured the exact holder.

**Required regression:** continuously issue bounded signed status requests while
running repeated parallel mint/swap/melt batches. Every member must retain a
bounded status latency, and a timed-out client must not leak a private REST
admission permit or retain a state lock.

### 8. Small receive load can amplify empty consensus units and starve HTTPS

**Severity:** Critical for remote availability  
**Status:** Confirmed liveness failure; unit-creation feedback is the strongest
current hypothesis

On a freshly reset federation, the first sequential receive case temporarily
made all five public mint endpoints unavailable. Loopback and public HTTP
recovered without a process restart after the AlephBFT broadcast backlog
drained. This rules out a persistent reverse-proxy configuration failure and
makes protocol/runtime backpressure much more likely than raw CPU exhaustion.

The following evidence was captured:

- only 10 accepted journal operations corresponded to approximately 744 total
  DAG units and 40 finalized items per member;
- reconstructed-DAG reports showed approximately 666 units during the first
  affected period and approximately 744 after a second burst;
- `UnitBroadcast` repeat counters reached approximately 161, fell through
  roughly 56-68 to approximately 5, then rose again to approximately 80-83 and
  drained again;
- creator logs printed `Delay passed.` at approximately 100 ms intervals during
  the bursts;
- all five wallet requests in one receive timed out after 90 seconds and the
  wallet returned `insufficient federation signature shares: required 3,
  received 0`;
- interval CPU generally remained around 4-12% per mint, with no host CPU
  saturation; and
- once the repeat queues drained, all five public `/v1/info` routes returned
  HTTP 200 again without restarting the federation.

**Most likely cause:** production AlephBFT uses a 100 ms unit-creation delay
while the shared activity hint is active, versus a 2 s idle delay. Inbound
consensus activity is retained for a 10 s grace period. When there is no local
payload but the activity hint remains active, the data provider returns no
payload without imposing the idle wait, allowing AlephBFT to continue creating
empty units at the active cadence. One real operation wakes all members;
multiple inbound activity identifiers can keep their activity leases alive
after the payload is drained. The resulting empty units create additional
network traffic and appear capable of extending or reinforcing the active
period, building `UnitBroadcast` backlogs and starving wallet HTTP requests.

The principal code paths to instrument are:

- `crates/cdk-axum/src/federation/aleph_bft_session.rs`, especially
  `CDK_ALEPH_BFT_ACTIVE_UNIT_CREATION_DELAY`,
  `CDK_ALEPH_BFT_IDLE_UNIT_CREATION_DELAY`, delay configuration, and the data
  provider's no-payload behavior;
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`, especially
  `CDK_ALEPH_BFT_NETWORK_ACTIVITY_GRACE` and
  `CdkAlephBftActivityHint`; and
- `crates/cdk-axum/src/federation/aleph_bft_network.rs`, where inbound
  consensus messages extract activity identifiers and call
  `mark_network_activity`.

This causal loop is not yet proven. In particular, the current trace did not
record the activity-ID map size or demonstrate that an empty unit directly
renews the activity lease that caused it. The unit counts, 100 ms log cadence,
queue growth, request timeouts, and recovery timing make it the leading
hypothesis rather than a confirmed root cause.

**Reproduction:** on the development deployment only, start from a fresh
federation and then run the smallest benchmark case while monitoring every
member:

```bash
# Build locally, upload with SCP, install, and verify the current binaries.
scripts/deploy_nocean_federation.sh

# Destructive: erase only the disposable remote development federation.
ssh nocean 'cd ~/cdk-federation && \
  bash scripts/stop-federation.sh && \
  bash scripts/reset-federation-data.sh --yes && \
  bash scripts/setup-federation.sh && \
  bash scripts/start-federation.sh'

# Use a single sequential receive case before increasing concurrency.
CASES='10:1' scripts/benchmark_nocean_federation.sh
```

Before the timed receive, require five healthy converged members, record an
idle interval CPU baseline, and confirm all public and loopback information
routes answer. During the receive, sample process CPU rather than relying on a
single instantaneous value; continuously time `/v1/info`; retain signed status,
operation height, pending mempool size, finalized cursor, reconstructed-DAG
size, and AlephBFT repeat counters. Stop the case after a bounded deadline if
HTTPS becomes unavailable. Keep the benchmark directory private because it can
contain bearer tokens.

The failure is reproduced if sequential receives grow the DAG and
`UnitBroadcast` queues far beyond the number of accepted operations while
wallet requests and public routes time out. Continue observing after stopping
load: recovery synchronized with queue drainage, without a mint restart, is an
important discriminator from proxy failure, database corruption, or permanent
deadlock.

**Required instrumentation and regression:** expose active versus idle unit
counts, empty versus payload-bearing units, activity-ID cardinality and lease
age, per-peer send-queue depth, event-loop delay, and HTTP handler latency. A
five-member process test should submit a small bounded number of sequential
swaps, require a bounded number of empty units per payload and bounded queue
depth, and continuously require public and signed private routes to respond.
The test must wait beyond the network-activity grace period and prove that all
members return to idle cadence without a self-sustaining activity loop.

### 9. Federation discovery/import can hang after a healthy config response

**Severity:** Medium  
**Status:** Confirmed in the Nocean setup; needs revalidation

`GET /federation/v1/config` returned HTTP 200 with a valid configuration, but
`cdk-cli federation add --mint-url ...` hung during post-discovery
import/validation. Importing the same immutable config from a file succeeded.

**Most likely cause:** the problem is after config retrieval, probably member
metadata/keyset validation or incomplete fanout cancellation, not the discovery
route itself. The exact import path must be rerun after the wallet fanout work.

## Performance-related issues

### 10. Sequential token preparation degrades with wallet fragmentation

**Status:** Observed, not precisely quantified

The bounded local preparation run produced 240 independent tokens before it was
stopped. Progress slowed materially over the run. Only 84 new consensus entries
were added while producing those 240 tokens, because many sends could use exact
local proofs without an online swap.

The slowdown therefore cannot be attributed solely to consensus journal growth.
The most likely contributors are increasing wallet proof fragmentation, coin
selection, repeated CLI startup and SQLite work, plus occasional federated swaps.
A future benchmark must separately count offline sends and online pre-send swaps
and record per-send latency against wallet proof count and journal height.

### 11. Remote sustained throughput and latency remain poor

**Status:** Confirmed for the tested Nocean revision

The Nocean run sustained only about 0.27 confirmed swaps/s at concurrency 5 and
10, with failure rates rising to 30-47.5%. A one-wave concurrency-20 burst
reached 0.566 swaps/s but had approximately 32-35 second successful latency.

The dominant observed limitation was not raw rejection by consensus. Operations
continued to enter the journal while wallets failed to collect threshold shares
through member catch-up/readiness churn. Wallet fanout and catch-up orchestration
must be fixed and the same remote matrix repeated before treating these numbers
as a consensus-engine capacity limit.

### 12. Status and recovery work can scale with journal and report size

**Status:** Partly resolved; residual risk confirmed by the status hang

Status, digest, certificate, and startup paths previously reloaded or replayed
unchanged journal state. Large payment-observation reports also make signed
status responses grow with workload history. The new caches remove major
repeated computation, but the member-1 status failure shows that bounded status
latency still needs a process-level load test.

Operator endpoints should expose summaries by default and retrieve detailed
rows or reports only on demand. Cache invalidation must remain tied to journal,
application, certificate, and observation mutation rather than polling.

## Resolved and verified performance defects

### Healthy live consensus was mistaken for recovery-worthy lag

Under modest swap load, a normal one-operation propagation gap caused the
peer-sync loop to mark a member lagging and start REST journal catch-up even
though its healthy live AlephBFT session was already delivering that operation.
In the corrected process-level reproduction, one member entered lagging 27
times, catch-up 15 times, and certificate verification 3 times during otherwise
convergent load.

The peer-sync loop now leaves delivery to AlephBFT while readiness is healthy
and both the live session and network input are installed. Startup and genuinely
non-healthy states retain fail-closed REST recovery. The post-fix five-member
regression converged with zero height skew and zero lag/catch-up transitions.

The erased-database failure in Issue 1 exposes the necessary boundary on this
fix: the optimization is correct only after startup has established that the
local persisted history and live session are continuous with quorum peer state.
A new empty member must not default into the healthy condition that suppresses
its required recovery.

### Effective federation config replayed the full journal on every read

The cache was consulted only after loading and deserializing the journal. BLS
key decoding and subgroup checks made repeated config reads increasingly
expensive. The cache is now checked before database access and invalidated after
an applied keyset rotation. Focused regressions verify one replay for repeated
unchanged reads and correct rotation invalidation.

### AlephBFT backup persistence had quadratic write amplification

Every backup flush appended to one growing SQL BLOB. At 130 operations, roughly
1.3 MB of logical backup produced about 3.34 GB of cumulative SQL expression
output per member. Backups are now stored as ordered independent rows, with
legacy migration and exact restart-prefix suppression. Identical chunks remain
valid and are not content-deduplicated.

### Repeated digest and certificate verification reloaded unchanged state

Materialized-state digests and validated catch-up-certificate quorum results are
now cached until relevant mutation. Detailed materialized rows are fetched only
after a digest mismatch. Focused cache, mutation, and migration tests pass.

### CPU runaway during idle/retry state

Before these fixes, the reproduced problematic state used approximately 8-17%
CPU per mint. After the fixes, local idle and accepted-but-unapplied states used
approximately 0.0-0.1% per mint, including repeated signed digest checks. This
proves the runaway CPU defect is fixed; it does not prove that all throughput or
liveness defects are fixed.

The fresh Nocean failure in Issue 8 is distinct. It showed consensus-message
amplification, queue backpressure, and HTTP starvation at moderate CPU rather
than journal-length-dependent idle CPU. Fixing the earlier replay and cache
costs therefore does not resolve or contradict the new observation.

## Positive performance findings

- Initial empty-federation startup is fast at 1.303 seconds.
- Intact local restart replay is fast: 1.673 seconds for one member and 2.958
  seconds for all five at 316 operations.
- Parallel local receive throughput reached 25.09 client-completed committed
  swaps/s with no failures at parallelism 20.
- All five members converged 1.710 seconds after those clients completed.
- The CPU fixes eliminated journal-length-dependent idle spin in the reproduced
  state.

These results establish that normal restart and local swap throughput are not
the immediate blockers. Fresh recovery, melt concurrency, status liveness, and
wallet ambiguity are.

## Priority order

1. Fix fresh-member startup recovery and prevent false healthy readiness.
2. Prove cancellation-safe server completion and robust all-member wallet
   fanout with typed ambiguous outcomes.
3. Reproduce and fix concurrent melt failure/stall behavior.
4. Bound empty-unit amplification and preserve HTTP liveness under small remote
   workloads.
5. Reproduce the private-status hang with lock, permit, and handler-latency
   instrumentation.
6. Revalidate federation discovery/import after wallet fanout changes.
7. Repeat controlled local and remote capacity curves only after the correctness
   and liveness fixes above.

## Required next benchmark and regression matrix

- Fresh member at histories of 1, 100, 1,000, and 10,000 operations; measure
  time through exact digest/certificate convergence and assert non-healthy
  startup state.
- Intact one-member and full-cluster restart at the same heights.
- Repeated 3-of-5 fanout with one and two slow/catching-up members.
- Ten or more wallets performing at least two consecutive melts concurrently.
- Continuous signed status polling during mint, swap, melt, restart, and catch-up
  load, with bounded latency and admission-permit accounting.
- Fixed-wave receive tests with at least five repetitions per concurrency,
  measuring client completion and signed convergence separately.
- A fresh-state `N=10, parallelism=1` remote receive test with continuous public
  route probes, event-loop delay, activity-hint cardinality, empty/payload unit
  counts, and per-peer `UnitBroadcast` queue depth.
- Synchronized CPU, RSS, SQLite lock time, disk I/O, journal height, AlephBFT
  session/round, response-cache, and catch-up-transition telemetry.

Hard wall-clock limits should guard exploratory measurements. A single case
should normally stop after two minutes and report its partial height and state;
longer recovery tests should be explicit soak tests rather than accidental
benchmark stalls.
