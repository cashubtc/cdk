# Federated Cashu Nocean Receive Stress Test

Date: 2026-07-13

## Executive summary

This test measured end-to-end wallet receipt of independent Cashu tokens against
the five-member Nocean development federation. Receiving a token performs a
federated swap, so each successful workload item is one federation operation.

The highest observed result was a 20-request burst that confirmed and committed
20 of 20 swaps in 35.342 seconds: **0.566 confirmed transactions per second**.
Median client latency was 31.715 seconds. This is an observed burst result, not a
proven maximum or a low-latency service rate.

The longer, multi-wave cases exposed a different practical limit. Confirmed
wallet throughput plateaued near **0.27 transactions per second** at concurrency
5 and 10. The federation journal still advanced for every submitted swap, at an
estimated 0.393 and 0.515 operations per client-batch second respectively, but
some wallet processes exited without collecting the required three signature
shares. Their errors reported members catching up, lagging, or verifying quorum
catch-up certificates.

For this deployment and workload, the defensible conclusion is:

- observed raw journal throughput was approximately **0.5-0.57 operations/s**;
- observed sustained wallet-confirmed throughput under multi-wave overload was
  approximately **0.27 transactions/s**;
- latency rose from about 8 seconds at concurrency 1 to about 32 seconds for the
  successful concurrency-20 burst;
- member catch-up and post-catch-up certificate verification, rather than
  outright consensus rejection, caused the observed client failures.

The federation was fully converged and healthy across all five members after
the final run, at 187 accepted journal operations.

## Test subject

### Federation

- Deployment: fresh Nocean development deployment
- Members: 5
- Signature threshold: 3
- Consensus threshold: 4
- Consensus: AlephBFT
- Payment backend: fakewallet
- Public entry URL: `https://cdk-1.nocean.xyz`
- Federation ID:
  `f2e8c4bd165cc01bf362bf0ecedcd11afae074347bfd06001655ce0330953324`
- All five mint processes were co-located on one development server.
- Server architecture: x86-64
- Server capacity: 4 logical CPUs, 7.6 GiB RAM, no swap

The deployed binaries had these SHA-256 digests:

| Binary | SHA-256 |
| --- | --- |
| `cdk-mintd` | `33ad50ee01e85b2b96fe4ded93ae7f1649c5c92bb7f4dcf2becb6a6fa79a8173` |
| server `cdk-cli` | `288ad10f56e1d112432a409d0eb93c0246ca774499bb09d1b003fab462fb4850` |

### Load generator

The load generator ran on a separate workstation and communicated with the
public mint URLs over HTTPS. The benchmark used the native release CLI from
repository commit `81fc96374405183fe012bfd71e431ee5f7147823`:

| Item | Value |
| --- | --- |
| Local `cdk-cli` SHA-256 | `ba75769b577a1833ff21ea9743ea0f076b67b8fd24ecb194da1ccf0532788f3c` |
| Rust compiler | `rustc 1.95.0 (59807616e 2026-04-14)` |
| Cargo | `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| CLI build | `cargo build --locked --release -p cdk-cli --no-default-features --features federation` |

The benchmark implementation is
[`scripts/benchmark_nocean_federation.sh`](../scripts/benchmark_nocean_federation.sh).

## What was measured

One workload item is receipt of one independently prepared 1-sat bearer token
into a fresh wallet. `cdk-cli receive` swaps the input proof for new wallet-owned
proofs through the federation.

Each case is written as `N:P`:

- `N` is the number of independent tokens and receiver wallets;
- `P` is the maximum number of concurrent `cdk-cli receive` processes;
- `xargs -P P` implements a closed-loop workload: when a process completes,
  another starts until all `N` tokens have been attempted.

This is an end-to-end wallet benchmark. The timed interval includes:

- `xargs` worker scheduling;
- native CLI process startup;
- each isolated wallet's SQLite access;
- DNS, TCP, TLS, and public-network latency;
- HTTP request/response handling;
- federation consensus, swap application, and threshold-share collection;
- client aggregation, verification, and persistence.

The timed interval excludes:

- compiling the CLI;
- copying and importing federation configuration;
- minting the funding balance;
- splitting the funding balance into independent tokens;
- creating and configuring receiver wallets;
- the pre-case convergence gate and 12-second idle-settle delay;
- the post-case signed convergence check.

## Metrics

### Confirmed TPS

`confirmed TPS = successful CLI receives / client batch wall time`

A receive is successful only when `cdk-cli receive` exits with status zero. This
is the wallet-facing service metric.

### Journal delta and committed TPS estimate

The script records the signed, converged accepted-operation count immediately
before and after each case. In every completed case, the operation delta equaled
`N`, including cases where some clients exited nonzero.

`committed TPS estimate = accepted journal operation delta / client batch wall time`

This estimate needs an important qualification: the client batch timer stops
before the post-case convergence wait. An operation that finishes converging
after its client exits is included in the numerator without extending the
denominator. It therefore demonstrates that operations reached the accepted
journal, but it is not a precise measurement of time-to-durable-convergence and
may overestimate strict commit throughput. A future run should time both the
client batch and the interval through signed convergence.

### Latency

The script records wall time around each successful CLI receive using Bash's
built-in `time` and `TIMEFORMAT=%R`. Percentiles are computed from successful
receives only using a nearest-rank-style index. Failed client latencies are not
included in p50, p95, p99, or maximum latency.

## Method

### 1. Build and validate the CLI

The current native CLI was built in release mode with only federation support:

```bash
cargo build --locked --release -p cdk-cli \
    --no-default-features \
    --features federation
```

The script checks that `cdk-cli federation add --help` succeeds before creating
test material.

### 2. Freeze federation configuration for the run

The first version of the benchmark imported the federation with:

```bash
cdk-cli federation add --mint-url https://cdk-1.nocean.xyz
```

The new discovery endpoint itself was healthy: a direct request to
`/federation/v1/config` returned HTTP 200 in approximately 1.2 seconds with a
roughly 49.8 KiB response. However, the CLI hung during post-discovery
import/validation, both before and after rebuilding the current native CLI.
Those attempts were stopped before minting test funds.

Configuration discovery is setup work rather than the operation under test. To
make each benchmark deterministic, the final script copies one immutable public
configuration snapshot from the deployment before timing and imports it with:

```bash
cdk-cli federation add \
    --public-config federation-public.json \
    --local-mint-url https://cdk-1.nocean.xyz
```

This avoided the discovery/import hang and does not change the timed receive
path. The discovery hang remains a separate issue to diagnose.

Wallet federation settings used a preferred quote coordinator of member 1,
90-second request/fallback/status timeouts, and fanout concurrency 5.

### 3. Create funding and independent tokens

For a matrix containing a total of `T` tokens, the funding wallet mints
`T + 50` sat through fakewallet. It then creates `N` separate 1-sat bearer
tokens for each case with sequential `cdk-cli send` commands.

Token preparation is deliberately outside the timed receive interval. Exact
1-sat splitting can itself require a federated swap, so preparation became slow
after overloaded cases. That slowdown is an observed recovery effect, but it is
not counted as receive TPS.

Token files are mode 0600. The run directory and wallet directories are mode
0700 because a failed or aborted case can leave spendable bearer tokens.

### 4. Isolate receiver wallets

Every token receives into a separate wallet work directory. This prevents one
SQLite wallet database lock from serializing otherwise parallel CLI processes.
Federation configuration is imported into receiver wallets before timing, with
up to 12 setup processes running concurrently.

### 5. Gate each case on convergence

Before each timed case, the script repeatedly runs the mint daemon's signed
`federation compare-digests` command. It starts the case only when the response
contains both:

```text
converged=true
members=1,2,3,4,5 ready=5/5 sync_states=healthy
```

After convergence, it waits another 12 seconds to let the federation return to
its idle cadence. The accepted-operation count from this signed status is the
case's starting journal height.

### 6. Run and time parallel receives

Each token path is passed to `xargs -P`. A worker reads the token, starts the CLI
against its isolated wallet, records its own duration, log, and exit status, and
marks the receive as successful or failed.

The entire `xargs` invocation is also timed with Bash's built-in timer. This
avoids adding a separate timing process to every operation.

### 7. Verify post-case state

After all clients exit, the script waits for signed five-member convergence
again. It records the final accepted-operation count, verifies that all worker
results are accounted for, computes latency percentiles and TPS, and writes a
CSV report.

## Runs and results

All run identifiers below are UTC timestamps used by the script under
`target/federation-load-tests/`. These directories are local, ignored benchmark
artifacts and must remain private because they contain bearer tokens.

### Smoke run: `20260713T185437Z`

The immutable-config version was first tested with `CASES='2:1 4:2'`, CLI
rebuilding disabled, and a shortened two-second settle delay.

| N | Parallelism | Success | Failure | Batch wall | Confirmed TPS | Journal TPS estimate | p50 | p95 | p99 | Max | Journal delta |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2 | 1 | 2 | 0 | 15.828s | 0.126 | 0.126 | 7.583s | 7.583s | 7.583s | 8.217s | 2 |
| 4 | 2 | 4 | 0 | 17.137s | 0.233 | 0.233 | 8.194s | 8.449s | 8.449s | 8.917s | 4 |

The operation delta exactly matched the six successful receives.

### Main sweep: `20260713T185716Z`

The main sweep used the normal 12-second settle delay. It completed concurrency
1, 5, and 10 cases.

| N | Parallelism | Success | Failure | Failure rate | Batch wall | Confirmed TPS | Journal TPS estimate | p50 | p95 | p99 | Max | Journal delta |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10 | 1 | 10 | 0 | 0% | 83.709s | 0.119 | 0.119 | 8.388s | 8.639s | 8.639s | 8.693s | 10 |
| 20 | 5 | 14 | 6 | 30.0% | 50.843s | 0.275 | 0.393 | 12.670s | 13.786s | 13.786s | 13.905s | 20 |
| 40 | 10 | 21 | 19 | 47.5% | 77.662s | 0.270 | 0.515 | 19.866s | 21.250s | 21.250s | 24.462s | 40 |

The original fourth case would have prepared 80 new tokens for concurrency 20.
It was stopped after four tokens because concurrency 5 and 10 had already shown
the confirmed-throughput plateau and sequential token splitting was taking
tens of seconds per token during recovery. A smaller saturation probe was run
instead. The aborted local artifact contains four unspent fakewallet test
tokens and must not be published.

### Saturation probe: `20260713T192249Z`

The final probe used `CASES='20:20'` and the normal 12-second settle delay.

| N | Parallelism | Success | Failure | Failure rate | Batch wall | Confirmed TPS | Journal TPS estimate | p50 | p95 | p99 | Max | Journal delta |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 20 | 20 | 20 | 0 | 0% | 35.342s | **0.566** | **0.566** | 31.715s | 35.299s | 35.299s | 35.309s | 20 |

The post-case signed comparison reported one digest group, five of five healthy
members, and convergence at operation count 187.

## Failure analysis

### Client-visible error

All six concurrency-5 failures and all 19 concurrency-10 failures were variants
of:

```text
Invalid mint response: insufficient federation signature shares:
required 3, received 1 or 2
```

The member-specific details were:

- member catching up from a peer;
- member lagging behind a peer;
- member verifying quorum catch-up certificates after catching up.

For concurrency 5, the six failed client logs contained one catch-up response
and five certificate-verification responses. For concurrency 10, failed logs
contained multiple member errors in some responses: 15 catch-up observations,
one lag observation, and six certificate-verification observations. These are
error observations, not counts of distinct failed operations.

### Operations committed despite client failures

The signed journal delta was 20 for the 20-request concurrency-5 case and 40 for
the 40-request concurrency-10 case. No other workload was intentionally active,
and the script gated the cases on convergence. This strongly indicates that
the swaps were accepted even though clients did not receive enough usable
shares before exiting.

This distinction matters operationally: retrying a nominally failed receive may
encounter an already-spent input. A production wallet needs an unambiguous
result after threshold-response failure instead of treating every nonzero
receive exit as an uncommitted swap. CDK already retains a `swap_requested`
receive saga and supports replay-first recovery followed by proof-state and
`/restore` recovery. The remaining issue is that the original command exits as
a normal failure and recovery is deferred until a later eligible wallet startup.

### Catch-up behavior during recovery

During token preparation after the concurrency-5 case, a signed digest check
temporarily showed members 1, 2, and 3 at 108 operations and members 4 and 5 at
109. All members reported healthy sync state, but the federation was not yet
converged. It later converged and continued processing.

Sequential token splitting also slowed to tens of seconds per swap after load.
Because preparation was outside the receive timer, this did not change reported
receive TPS, but it shows that the effects of a burst survive beyond client
completion. The convergence gate and settle delay reduced cross-case
contamination but cannot prove that every cache, queue, or scheduling state was
equivalent between cases.

## Three errors requiring investigation

The post-test trace changes the likely classification of the client failures.
The strongest current hypothesis is a **wallet availability/fanout defect
triggered by real federated-mint catch-up pressure**, not a BLS aggregation
failure. The stress-test findings should be tracked as exactly three separate
errors. Fixing one may reduce the symptoms without resolving the others.

### Error 1: The wallet does not fan out to every federation member

**Evidence:** `validate_member_active_keysets_with_mint_info` starts concurrent
member metadata/keyset probes but returns as soon as the signature threshold is
valid. It drains only probes that are immediately ready, stores that partial map
in `valid_member_connectors_cache`, and drops the remaining pending probes when
the function returns. A cached map with at least the threshold is reused without
trying to add the remaining configured members. `post_swap` then passes only
that cached map to `collect_mint_responses`.

The failed responses consistently reported two received member responses plus
one named member failure. One nondefinitive catch-up failure is not enough to
trigger the collector's threshold-rejection early return. If five connectors
had been present, the collector would have continued waiting for the remaining
two. The observed `2 responses + 1 failure` shape therefore strongly indicates
that the live wallet fanout map contained exactly three members.

An existing wallet test also expects the number of signing calls to equal the
signature threshold. That behavior minimizes traffic when every selected member
remains healthy, but it removes the availability benefit of a five-member
roster: one unavailable selected member turns a 3-of-5 federation into a 3-of-3
client operation.

The required behavior is to validate and contact all configured federation
members. The wallet should collect member results until one of these outcomes
crosses the configured threshold:

- a compatible set of valid blind-signature shares produces success;
- an exact matching set of definitive rejections produces a definitive error;
- neither side reaches threshold before the overall operation deadline, in
  which case the result remains explicitly pending or ambiguous.

Catch-up, lagging, transport, timeout, and temporary-readiness responses are not
definitive rejection votes. They must not be counted toward a threshold error.
The wallet may stop waiting once a threshold result is mathematically decided,
but every valid roster member must be available to the fanout/fallback process.

**Questions to investigate:**

- Change member validation so all reachable roster members remain eligible for
  signing fanout instead of freezing the first threshold-sized subset.
- Decide whether all requests start concurrently or use staged fanout, while
  guaranteeing that every member is tried before an ambiguous failure.
- Preserve exact-match threshold handling for definitive errors rather than
  allowing one member's error to terminate the federation request.
- Can this be fixed without admitting a member whose mint-info/config digest or
  active-keyset advertisement has not been validated?

**Required regression:** in a 3-of-5 wallet test, make the first three members
validate fastest, then make one of those three return a retryable catch-up error.
Two slower but valid spare members must allow the identical swap request to
reach three valid shares without constructing new outputs.

#### Supporting finding: shares were generated; aggregation was not reached

**Evidence:** all 25 failed wallet databases retained a `swap_requested` receive
saga. A representative failed wallet contained one pending input proof and no
incoming transaction, showing that the wallet preserved the ambiguous operation
rather than compensating or silently discarding it.

The blinded output from every failed saga was matched against private snapshots
of all five member databases after the test:

- all 25 operations were `applied` in every member journal;
- all five members persisted their own federation blind-signature share for
  every failed output;
- the five members associated each output with the same federation operation
  ID.

These snapshots prove eventual generation and persistence, not the exact moment
each share became available. The client errors and server logs establish the
request-time shape: two selected members returned parsed share responses while
one selected member returned a catch-up/readiness error. The other configured
members were apparently not queried.

There is no evidence that the wallet received three shares and failed BLS
aggregation. The reported response count was one or two, below the threshold at
which compatible-set selection and aggregation can succeed. An actual
cryptographic aggregation failure should be instrumented and reported
separately from insufficient response count.

**Diagnostics required for Error 1:**

- Add low-noise diagnostics for queried member IDs, parsed share responses,
  validated share responses, aggregation candidates, and unused configured
  members.
- Ensure malformed or incompatible shares produce a distinct error from
  `required 3, received 2`.
- Confirm through a replay test that the persisted shares from any three members
  reconstruct the failed wallet's expected outputs.

### Error 2: The wallet does not robustly wait through member catch-up

The request-time catch-up response needs a precise classification. A mint that
has not yet applied an operation or cannot prove its authoritative state is
correct to refuse to manufacture or return a new share. That fail-closed behavior
is not itself the bug. The mint should, however, expose catch-up as an explicitly
retryable readiness state rather than generic error code 50000.

The primary observed failure is wallet-side orchestration. The wallet used only
the selected threshold-sized subset, configured zero additional member retries,
and returned as soon as that subset produced two responses plus one catch-up
error. It did not keep the deterministic swap request pending, wait for the
member to become ready, or try the unused federation members. The operation then
applied on all five mints and every mint eventually persisted its own share.

After Error 1 is fixed, the wallet still needs robust catch-up handling for cases
where fewer than threshold members are immediately ready. It should:

1. keep the original operation ID, blinded outputs, and request unchanged;
2. treat catch-up, lagging, and certificate verification as retryable states;
3. continue collecting responses from all members;
4. retry unavailable members with bounded backoff until a threshold success,
   threshold definitive rejection, or overall operation deadline;
5. return a typed pending/ambiguous result if the deadline expires, preserving
   the receive saga for replay and `/restore` recovery.

There is also a narrower mint-side availability question. The public swap path
checks readiness before it reaches `persisted_swap_share_response`. An exact
duplicate request can therefore receive a catch-up error even if that member has
already applied the operation and persisted the operation-bound share. It may be
safe to return an immutable persisted share before the general public-submit
readiness gate, but only after strict operation-ID, output, keyset, journal, and
materialized-state checks. This should be investigated rather than assumed.

The wallet's durable saga behavior prevents this from being a straightforward
fund-loss bug: `SwapRequested`, blinded messages, and derivation counters are
written before the request, and later wallet startup performs replay-first
recovery followed by proof-state and `/restore` recovery. The command-level
behavior is still wrong because it reports a normal failure for an operation
that may already be committed and recoverable.

**Required regressions:**

- One of five members remains in catch-up long enough to miss the first response;
  the wallet obtains three shares from the other four without error.
- Three members are temporarily catching up and later recover; the wallet waits
  with bounded backoff and completes the same operation exactly once.
- Three members return the same definitive rejection while two are unavailable;
  the wallet returns that definitive threshold error without waiting forever.
- A member already holding the persisted share enters catch-up; test whether a
  strictly bound replay can safely return the stored share.
- If the overall deadline expires, the API and CLI report pending/ambiguous and
  a later recovery reconstructs the outputs exactly once.

**Assessment:** the immediate stress-test error is more likely a wallet bug than
a mint response bug. The mint must improve its retryable error semantics, and
possibly persisted-share replay ordering, but a catching-up member should never
cause a 3-of-5 wallet operation to fail while three other members can answer.

### Error 3: Mints enter catch-up too readily under lightweight load

**Evidence:** member logs during the failed cases showed repeated catch-up and
certificate-verification transitions, network deliveries accepted by three
remote members while another was requeued, catch-up certificate range-content
mismatches, and temporary failure to reach the required four-member catch-up
certificate quorum. Signed status also briefly showed a one-operation height
split, and sequential preparation swaps remained slow for tens of seconds after
the timed batch.

Failing closed while a member cannot prove authoritative state is correct. The
unexpected behavior is how frequently a co-located five-member federation enters
that state at concurrency 5-10 and how long the recovery pressure persists.

**Questions to investigate:**

- Which stage first causes the lag: AlephBFT delivery, finalized-item apply,
  SQLite contention, journal projection, or sync-loop scheduling?
- Why are messages repeatedly requeued to one peer when the other three remote
  members accept them?
- Are catch-up certificate range mismatches normal during a moving journal head,
  or is the range selection unstable under concurrent operations?
- Is the sync loop switching to catch-up too aggressively for a transient
  one-operation difference?
- Does fanout of the identical swap to several members amplify duplicate
  consensus submissions enough to cause the observed churn?

This is a mint-side performance/liveness issue even if wallet spare-member
fallback removes the immediate client failures.

**Verified cause and fix (2026-07-14):** the original process-level regression
did not actually run the background peer-sync loop, so its passing result could
not validate this issue. After the harness was corrected to run the same phased
peer-sync rounds as production, the pre-fix test reproduced the problem. During
one run, member 1 recorded 27 lagging transitions, 15 catch-up transitions, and
3 catch-up-certificate-verification transitions under otherwise convergent
swap load.

Temporary trigger classification showed that every captured transition began
with a real but transient operation-count difference, such as local height 5
versus peer height 6. The finalized-item cursor was also behind because it
tracked the same operation; no cursor-only trigger was observed. The immediate
cause was therefore the peer-sync loop treating normal in-flight AlephBFT
propagation as recovery-worthy lag. It changed readiness to fail-closed and
started REST journal catch-up while the healthy live consensus session was
already delivering the operation.

The minimal fix keeps the ownership boundary explicit: while federation
readiness is `Healthy` and both the live AlephBFT session and its network input
are installed, AlephBFT owns operation delivery and peer status polling does
not start REST journal catch-up. Startup, already-lagging, catching-up,
certificate-verification, materialized-state-mismatch, and halted states retain
the existing fail-closed catch-up behavior. This avoids adding a timeout, retry
counter, alternate queue, or cursor exception.

The corrected regression now runs five sequential valid swaps through five real
in-process AlephBFT members, restores startup readiness as the production router
does, and runs phased peer-sync polling every 10 ms. Post-fix it requires and
observes identical journal order, identical materialized-state digest, zero
height skew, and zero lag/catch-up transitions. The separate five-member
one-offline value-window test also passes: the intentionally non-healthy member
still catches up and replay does not duplicate the outgoing payment.

The network-pump retry ordering and moving-head catch-up-certificate range
mismatch remain worthwhile diagnostics, but this reproduction did not establish
either as the initiating cause. They were not changed. Likewise, ignoring
finalized-cursor-only differences would have been hypothetical and could weaken
exact recovery, so that proposed change was rejected.

### Relationship and priority

The three errors interact but must not be collapsed into one fix:

1. **Wallet fanout is the first correctness/availability fix.** The code and the
   `2 responses + 1 failure` shape make this the most likely immediate cause of
   the observed receive errors.
2. **Catch-up waiting is the second wallet protocol fix.** Full fanout handles
   one or two unavailable members, but the wallet still needs bounded retry and
   an explicit pending outcome when fewer than threshold members are ready.
3. **Mint catch-up chatter is an independent performance/liveness defect.** A
   robust wallet should tolerate it, but such a lightweight workload should not
   drive healthy co-located members repeatedly into catch-up.

BLS aggregation is currently a lower-probability hypothesis. Reopen it only if
a run captures at least three returned shares that fail compatible-set
validation or cryptographic aggregation. The separate federation-discovery
import hang remains recorded in the setup methodology, but it is not one of
these three stress-response errors.

### Regression harnesses

The three errors now have separate regression coverage:

1. `federated_mint_connector_fans_signing_request_out_to_every_member` creates
   a five-member, three-share wallet fixture in which one member is unavailable
   and verifies that all five receive the identical swap request while any
   three compatible shares still complete the swap. Existing rejection tests
   separately require an exact threshold of matching Cashu errors.
2. `federated_mint_connector_waits_for_catching_up_member_with_identical_request`
   makes a required member return the new retryable Cashu error code
   `FederationCatchUp` (`40001`) twice before returning its share. The test
   verifies success and byte-for-byte-equivalent request reuse.
   `federated_mint_connector_returns_typed_ambiguous_error_after_catch_up_deadline`
   covers the bounded deadline and requires a typed, non-definitive outcome.
   `post_swap_reports_typed_federation_catch_up_error` covers the mint HTTP
   response contract.
3. `equal_height_peer_sync_does_not_enter_catch_up` verifies that a normal
   equal-height sync poll makes no catch-up request or sync-state transition.
   The ignored process-level test
   `aleph_bft_live_healthy_swaps_do_not_trigger_catch_up` runs five sequential
   swaps against five real in-process AlephBFT members backed by SQLite while
   production-equivalent peer-sync loops poll at an accelerated 10 ms interval.
   It requires every operation to apply exactly once, identical order and
   materialized digest on every member, zero final height skew, and zero
   lag/catch-up transitions. The existing one-member-offline value-window
   control asserts that only the intentionally offline member records lagging
   and catch-up transitions.

The process-level stress regression is intentionally ignored in the default
unit-test run and can be selected explicitly with `--ignored --exact` when the
slower federation harness is desired.

## Interpretation

### Single-request baseline

Concurrency 1 was stable across the smoke and main runs at 0.119-0.126
confirmed TPS, with approximately 7.6-8.7 second individual receive latency.
Concurrency 2 nearly doubled throughput to 0.233 TPS without failures.

### Multi-wave saturation

Concurrency 5 and 10 used four waves each (`N/P = 4`). Confirmed throughput did
not improve beyond about 0.27 TPS when concurrency increased from 5 to 10, while
the client failure rate rose from 30% to 47.5%. The accepted journal continued
to advance faster, reaching the 0.515 operations-per-batch-second estimate.

This is the strongest evidence for a practical sustained limit in this run:
additional concurrency increased catch-up pressure and accepted work, but not
successful wallet responses.

### One-wave concurrency-20 burst

The concurrency-20 case was one wave (`N/P = 1`), began after signed convergence
and settling, and succeeded completely. All clients took roughly 32-35 seconds,
so completion was clustered and total throughput reached 0.566 TPS.

Its better success rate than the concurrency-5 and concurrency-10 cases is not
monotonic scaling. A plausible explanation is workload shape: one large batch
can be ordered and eventually answered together, whereas a four-wave closed-loop
test repeatedly submits new work while members are already entering catch-up.
That is an inference from the results, not yet proven by consensus traces.

### Capacity statement

There is no single context-free “maximum TPS” in these results:

- **Maximum observed burst throughput:** 0.566 confirmed swaps/s.
- **Maximum observed journal-rate estimate:** 0.566 accepted operations per
  client-batch second.
- **Observed sustained wallet-confirmed plateau:** about 0.27 swaps/s in the
  four-wave concurrency-5 and concurrency-10 tests.
- **Latency at maximum observed burst throughput:** p50 31.715 seconds and p95
  35.299 seconds.

For capacity planning, 0.27 TPS is the safer wallet-facing number from this
small experiment. Approximately 0.5 TPS describes the federation's observed
journal progression under load, but it should not be advertised as reliable
client throughput without repeated tests and a commit-inclusive timer.

## Limitations

- Each completed case was sampled once; there are no confidence intervals.
- The cases were not randomized, so history and recovery effects can correlate
  with increasing concurrency.
- `N` was not constant. Concurrency 5 and 10 were four-wave sustained cases;
  concurrency 20 was a one-wave burst.
- All five federation members shared one 4-CPU host. This tests process and host
  contention, not a geographically distributed federation.
- The load generator was remote. Public-network and TLS latency are included.
- The fakewallet backend removes Lightning-node and payment-settlement costs.
- The client workload is closed-loop, not a fixed-rate open-loop arrival test.
- The post-case convergence wait was not included in the committed-TPS timer.
- Server CPU, RSS, disk I/O, SQLite lock time, AlephBFT round timing, and network
  traffic were not sampled synchronously with each batch.
- Successful latency percentiles exclude failed clients.
- The script assumes no other accepted federation workload runs between its
  signed before/after checks.
- Token preparation performs real federation work and can influence subsequent
  scheduling even though convergence and a settle delay are enforced.

## Recommended next benchmark

To turn this exploratory measurement into a defensible capacity curve:

1. Run at least five repetitions per concurrency and report median, range, and
   confidence intervals.
2. Use a fixed wave count, for example `N = 4P`, for `P = 1, 2, 4, 8, 12, 16,
   20, 24, 32`.
3. Randomize case order or redeploy a fresh federation for each case.
4. Record two timers: client completion and signed five-member convergence.
5. Sample per-member CPU, RSS, disk I/O, SQLite lock time, accepted height,
   catch-up state, and AlephBFT lifecycle timestamps throughout each run.
6. Add an open-loop rate test to find the offered rate at which queueing,
   latency, and catch-up errors begin to grow without bound.
7. After a failed wallet response, query proof/operation state to classify the
   result as committed, pending, or rejected without unsafe blind retry.
8. Pre-generate test tokens in a fresh, fully converged setup so recovery from
   one measured case does not make preparation for the next case expensive.
9. Repeat on separate member hosts and with a real payment backend after the
   co-located fakewallet bottleneck is understood.
10. Trace the concurrency-20 one-wave success and concurrency-10 multi-wave
    failures to prove or reject the batch-shape explanation.

## Reproduction

To reproduce the original three completed phases and collect them under one
timestamped private directory, run:

```bash
scripts/reproduce_nocean_federation_stress_test.sh
```

The wrapper runs the `2:1 4:2` smoke phase, the `10:1 20:5 40:10` sustained
phase, and the separate `20:20` burst phase. It produces a combined `results.csv`
in addition to the complete per-phase benchmark artifacts. Set `REPETITIONS` to
repeat the entire sequence, for example:

```bash
REPETITIONS=5 scripts/reproduce_nocean_federation_stress_test.sh
```

The default matrix is intentionally smaller than the initial aborted
`80:20` plan:

```bash
scripts/benchmark_nocean_federation.sh
```

The current default is:

```text
CASES="10:1 20:5 20:10 20:20"
```

A custom matrix can be supplied without editing the script:

```bash
CASES="20:1 40:5 40:10 40:20" \
    scripts/benchmark_nocean_federation.sh
```

Useful controls include `TOKEN_AMOUNT`, `PREPARE_PARALLELISM`,
`SETTLE_SECONDS`, `BUILD_CLI`, `CLI`, `MINT_URL`, `SSH_HOST`, `REMOTE_DIR`, and
`RUN_DIR`. Run directories must remain private because unsuccessful or aborted
receives can leave spendable bearer tokens.
