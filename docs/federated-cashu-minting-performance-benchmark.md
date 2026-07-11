# Federated Cashu Minting and Receive Performance Benchmark

Date: 2026-07-09; updated 2026-07-11

This note records benchmark results from fresh 5-member, threshold-3 fakewallet
AlephBFT federations and fresh CLI wallets. The first run identified why
federated minting took tens of seconds. The second run verifies the immediate
post-quote payment-observation fix and keeps the remaining delay classified.
The third run verifies pending-work-aware AlephBFT tail pacing after idle gossip
was excluded from the activity hint. Later runs accept a 100ms active-only
cadence, deduplicate replayed consensus-data wakeups, stop empty-unit creation
when no operation is active, and remove an O(history) finalized-item scan from
peer status. A receiver-side token receive benchmark is also complete. A later
rebenchmark exposed a phase-dependent idle scheduling bug where an indefinite
inactive data-provider wait could strand a queued operation until another
member proposed data. The signed member-wide activity wake closes that bug,
and the latest exact-HEAD process reproduction below independently confirms
the repaired path.

The measurements below are development-environment measurements. Absolute values will vary by machine, but the stage ordering and relative delays are the important result.

## Benchmark Setup

- Federation: 5 members
- Threshold: 3
- Payment backend: fakewallet
- Consensus backend: AlephBFT
- Wallet: fresh `cdk-cli` work directory
- Operation tested: two full federated mints after adding the federation registry
- Receiver-side token receive/swap benchmark: completed at 7.92s wall
- Instrumentation policy: capture an untouched command/log baseline first.
  After that baseline, targeted timing, tracing, counters, or profiling hooks
  may be added directly around production code paths when useful to isolate a
  delay. Instrumentation must not change protocol behavior, signing semantics,
  payment semantics, consensus ordering, retry policy, or wallet fanout
  behavior. Remove temporary instrumentation before finishing the checkpoint;
  retain it only when it has focused tests and is intentionally useful as
  normal low-noise production diagnostics.
- "Do not change production code first" is a sequencing requirement, not a
  blanket ban on instrumentation. Establish the unmodified baseline, add the
  smallest behavior-neutral probe needed to answer a specific missing timing
  question, compare the instrumented run with the baseline, and remove the
  probe before closing the checkpoint unless it meets the permanent diagnostic
  bar above.
- Process lifecycle traces are available without changing normal log volume by
  setting
  `RUST_LOG=info,cdk_axum::federation::aleph_bft_latency=trace` and
  `CDK_MINTD_LOGGING_FILE_LEVEL=trace`. The trace emits
  `submit_queued`, `data_provider_yielded`, `network_message_delivered`,
  `finalized_data_received`, `apply_loop_received`, `apply_completed`,
  `operation_notified`, and `waiter_returned`, keyed by operation ID. Opaque
  AlephBFT network messages are decoded for operation IDs only while this exact
  trace target is enabled.
- Mint operation sequence confirmed in the journal:
  - `mint_quote`
  - `mint_quote_payment`
  - `mint`

Each mint currently requires three ordered consensus operations. Quote and
payment observation can share one AlephBFT data unit when both are queued in
time, but the accepted journal still contains all three operations. For two
mints, all members converged on six accepted journal operations in the same
order:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

Finalized item count was higher than accepted operation count because wallet/member fanout causes duplicate finalizations. The duplicates did not change the accepted journal order.

## Current Rebenchmark: Phase-Dependent Idle Wake Stall

On 2026-07-10 a new isolated five-member, threshold-three federation and fresh
wallet were measured with the existing opt-in lifecycle trace. No production
code was changed before or during the run. The first mint was healthy, but the
second exposed a phase-dependent liveness failure hidden by earlier successful
samples.

### Command timings

| Command | Wall | User | System |
| --- | ---: | ---: | ---: |
| direct `curl /v1/info` | 0.91s | 0.00s | 0.00s |
| non-federated CLI `mint-info` | 4.91s | 0.03s | 0.02s |
| `federation add` | 1.44s | 1.41s | 0.02s |
| federated CLI `mint-info` | 1.58s | 0.60s | 0.01s |
| federated mint 1, 50 sat | 7.78s | 1.98s | 0.03s |
| federated mint 2, 50 sat | 94.84s | 1.19s | 0.02s |

The old 31-second local-CPU symptom did not reproduce: federated `mint-info`
used 0.60s of user CPU, and the stalled mint used only 1.19s. The 94.84-second
wall time was therefore waiting rather than wallet-side computation.

### Mint 1 timeline

| Stage | UTC timestamp | Delta/evidence |
| --- | --- | ---: |
| CLI start | 08:35:02.465 | - |
| quote request admitted on member 1 | 08:35:06.904 | 4.439s after CLI start |
| `mint_quote` data-provider yield | 08:35:06.905 | 0.001s after admission |
| backend paid observation | 08:35:06 | persisted in the same clock second as quote admission |
| `mint_quote` accepted and waiter returned | 08:35:07.426 | 0.522s after admission |
| payment observation submitted | 08:35:07.429 | 0.003s after quote acceptance |
| `mint_quote_payment` accepted | 08:35:08.044 | 0.615s after submission |
| `mint` submitted by member fanout | 08:35:08.130 | 0.086s after paid acceptance |
| threshold member waiters returned shares | 08:35:08.980-08.984 | about 0.85s after submission |
| wallet aggregation, verification, persistence done | 08:35:10.263 | about 1.28s after shares |

This mint appended exactly `mint_quote -> mint_quote_payment -> mint`. Direct
private REST delivery after each data-provider yield took milliseconds to tens
of milliseconds. Consensus finality and quote/payment apply were sub-second;
accepted mint signing/materialization was the largest mint-side apply stage at
about 0.43s.

### Mint 2 timeline

| Stage | UTC timestamp | Delta/evidence |
| --- | --- | ---: |
| CLI start | 08:35:10.270 | - |
| original quote admitted on member 1 | 08:35:11.827 | 1.557s after CLI start |
| original backend paid observation | 08:35:11 | paid in the same clock second as admission |
| original payment observation submitted | 08:35:15.682 | 3.854s after quote admission |
| fallback quote admitted on member 2 | 08:36:41.820 | 89.993s after original admission |
| fallback quote data-provider yield | 08:36:41.821 | 0.001s after fallback admission |
| original quote and payment finally yield | 08:36:41.831 | 90.003s after original quote admission |
| fallback quote accepted | 08:36:42.296 | 0.476s after fallback admission |
| original quote and payment accepted | 08:36:42.403 | both drained in one later unit |
| fallback payment accepted | 08:36:42.913 | 0.607s after its submission |
| `mint` submitted by member fanout | 08:36:44.087 | - |
| threshold member waiters returned shares | 08:36:44.593-08:36:44.595 | about 0.51s after submission |
| wallet aggregation, verification, persistence done | 08:36:45.131 | about 0.54s after shares |

The original member had two durable pending operations and two waiters but no
`data_provider_yielded` event. The delay ended at the configured 90-second
member-fallback timeout, not at an AlephBFT round, payment-observer tick, DB
timeout, or signing boundary. The fallback member yielded its new quote in
under 1ms; that data-bearing unit woke member 1, which yielded both older
operations about 10ms later.

All members ultimately converged with matching order and state digests, zero
pending work, and no invalid-operation alert. They converged on eight accepted
operations rather than six:

1. first `mint_quote`
2. first `mint_quote_payment`
3. first `mint`
4. fallback `mint_quote`
5. timed-out original `mint_quote`
6. timed-out original `mint_quote_payment`
7. fallback `mint_quote_payment`
8. second `mint`

The successful path still requires exactly three operations. The extra two are
a distinct paid quote lifecycle abandoned by wallet fallback, not duplicate
acceptance of one operation ID.

### Confirmed cause

The dominant delay is the CDK/AlephBFT idle data-provider integration. Upstream
`run_session` creates a preunit, then awaits `DataProvider::get_data()`, and only
after that await packs and sends the unit. CDK currently lets `get_data()` wait
indefinitely when local and tracked peer activity are empty. A local activity
hint wakes the data provider only if that creator is currently inside the
provider wait. If the creator is instead waiting for parent units, other
members can be holding those parent preunits inside their own inactive data
provider waits.

A separate no-code diagnostic run enabled upstream `AlephBFT-creator` tracing.
It directly showed members stopping after `Created a new preunit` with no
following `Received data` line. The same run also showed why the bug is
intermittent: when the selected proposer happens to be waiting in
`get_data()`, local admission wakes it and the mint remains fast. When it is in
the parent-wait phase, local notification alone cannot make the distributed
creator set advance. A peer's data-bearing fallback unit supplies the missing
network-wide wake and immediately restores progress.

Classification:

- **Dominant delay:** AlephBFT integration liveness before data-provider yield.
- **Wallet/CLI:** about 1.6s to the original quote handler; not the stall.
- **Fakewallet:** paid in the quote-admission clock second; not the stall.
- **Payment observer:** added the original observation after about 3.9s, but it
  stalled in the same queue and did not cause the 90-second delay.
- **Ordinary AlephBFT finality:** sub-second after data yield; not the stall.
- **Private REST:** millisecond-scale after yield; not the stall.
- **Catch-up/sync:** members began converged and ended converged; not the stall.
- **DB contention:** one roughly 0.36s SQLite lock wait appeared in mint 1; not
  dominant.
- **Signing/aggregation:** about 0.5-0.9s for member shares and 0.5-1.3s for
  wallet aggregation; not the stall.

### Smallest clean fix candidate and gate

Do not rely on an indefinite local `DataProvider` wait as the sole idle-control
mechanism. The smallest candidate that preserves low idle CPU is a signed,
session-bound member-wide activity wake over the existing private REST
transport whenever a local AlephBFT operation is newly admitted. Every member
must validate the federation, sender, session, sequence, and operation-bound
wake before marking its live session active. Waking all pending providers
releases held parent preunits and lets creators already waiting for parents
advance; the wake is control traffic and must not become an accepted mint
operation. If this cannot be made mechanically reliable, the safe fallback is
to restore a nonblocking data provider first and solve idle scheduling above
the upstream creator rather than withholding its units.

Required regression gate before accepting a fix:

1. Deterministically place a five-member live session in asymmetric
   creator/data-provider phases and prove one local submission yields and
   finalizes without fallback.
2. Run repeated fixed-primary and rotating-primary post-idle submissions with
   randomized idle gaps; no operation may remain queued without a yield.
3. Run two full fresh-wallet mints repeatedly and require exactly six accepted
   operations, no extra quotes, no pending work, and matching digests.
4. Preserve one-offline-member progress and below-quorum fail-closed behavior.
5. Measure at least 30 seconds of true idle CPU and finalized-unit cursor
   growth; the previous periodic empty-unit/DAG-growth regression must not
   return.
6. Reject unknown-member, wrong-federation, wrong-session, stale-sequence, and
   replayed activity wakes before they can affect cadence.
7. Remove temporary creator/provider instrumentation after the fix, retaining
   only the existing opt-in lifecycle trace or similarly low-noise diagnostics.

### Accepted fix and process verification

The accepted fix keeps the inactive provider wait and adds a separate signed
private REST activity request. On a newly admitted public operation, the local
member marks its own pending work and broadcasts the current session and
operation ID to every configured peer. An authenticated peer records bounded
network activity for that operation, allowing an empty provider to return
`None` once so AlephBFT can send its already-created preunit. The signal is
control traffic only: it is not an operation, cannot sign outputs, and never
appears in the federation journal. A duplicate private operation wakes only
the local provider, avoiding broadcast loops; a public retry may rebroadcast
the wake. Finalization shortens operation-ID activity to the same one-second
tail already used for finalized body hashes.

Focused tests prove that the signed request and response are bound to the
expected operation, a peer pulse releases an empty provider, finalization
shortens its activity tail, a wrong session is rejected without consuming the
sender sequence, replay is rejected, and a five-member threshold-three live
session finalizes after a three-second idle period.

Fresh process timings from the fixed build:

| Command | Wall | User | System |
| --- | ---: | ---: | ---: |
| direct `curl /v1/info` | 0.92s | 0.00s | 0.00s |
| non-federated CLI `mint-info` | 2.00s | 0.03s | 0.02s |
| `federation add` | 1.42s | 1.41s | 0.00s |
| federated CLI `mint-info` | 1.56s | 0.61s | 0.00s |
| full federated mint 1, 50 sat | 7.76s | 1.98s | 0.03s |
| full federated mint 2 after 15s idle, 50 sat | 4.95s | 1.18s | 0.01s |

Every member converged on exactly six accepted operations:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

There were no fallback quotes, duplicate submissions, pending mempool items,
pending waiters, payment conflicts, digest alerts, or invalid-operation alerts.
Order and state digests matched on all five members. This both confirms the fix
and reconfirms that each successful mint uses three consensus operations.

The post-work idle safety sample ran for 30 seconds. Every member's reconstructed
DAG remained exactly at 293 units for the whole sample. CPU-time deltas were
0.64-0.65 seconds per debug member, about 2.1% per process, rather than a busy
loop. No temporary source instrumentation was needed or retained; the existing
opt-in lifecycle trace remains the diagnostic path.

## Independent Exact-HEAD Reproduction

On 2026-07-10, the liveness-fix commit was rebuilt before measurement and run
in a new five-member, threshold-three fakewallet AlephBFT federation with new
SQLite databases and a new CLI wallet. Member request, fallback, and quote
status timeouts were 90 seconds. The second mint followed a deliberate
15-second idle gap.

### Untouched baseline

| Command | Wall | User | System |
| --- | ---: | ---: | ---: |
| direct `curl /v1/info` | 0.93s | 0.00s | 0.00s |
| non-federated CLI `mint-info` | 2.31s | 0.03s | 0.02s |
| `federation add` | 1.45s | 1.43s | 0.01s |
| federated CLI `mint-info` | 1.59s | 0.61s | 0.01s |
| full federated mint 1, 50 sat | 7.71s | 2.01s | 0.03s |
| full federated mint 2 after 15s idle, 50 sat | 4.96s | 1.18s | 0.02s |

The old 31-second user-CPU `mint-info`, 46-second pre-handler wait, and
fallback-driven mint delay did not reproduce. In particular, federated
`mint-info` consumed only 0.61 seconds of user CPU, and the post-idle mint was
the faster of the two.

The baseline logs already contained operation-specific AlephBFT lifecycle
events. They lacked only an exact fakewallet notification-emission timestamp,
because the durable payment record is second-resolution. After the untouched
baseline, a single identifier-free trace event was temporarily placed after
the fakewallet successfully enqueued an incoming payment notification. A
second independent fresh federation measured 7.86s and 4.93s, within 0.15s and
0.03s of the baseline. The temporary trace was then removed completely.

### Mint 1 timeline

| Stage | UTC timestamp | Delta from previous stage |
| --- | --- | ---: |
| CLI start | 09:57:30.878844 | - |
| quote request admitted by member 1 | 09:57:35.409945 | 4.531s |
| fakewallet notification emitted | 09:57:35.409982 | 0.000037s |
| `mint_quote` applied | 09:57:35.934717 | 0.525s |
| payment observation submitted | 09:57:35.936877 | 0.002s |
| `mint_quote_payment` applied | 09:57:36.547350 | 0.610s |
| `mint` submitted by member fanout | 09:57:36.616787 | 0.069s |
| `mint` applied and signing response persisted | 09:57:37.469858 | 0.853s |
| threshold member handlers returned shares | 09:57:37.476513 | 0.007s |
| wallet aggregation, verification, persistence, and CLI completion | 09:57:38.750849 | 1.274s |

### Mint 2 timeline after 15 seconds idle

| Stage | UTC timestamp | Delta from previous stage |
| --- | --- | ---: |
| CLI start | 09:57:53.799596 | - |
| fakewallet notification emitted | 09:57:55.297119 | 1.498s |
| quote request admitted by member 1 | 09:57:55.314997 | 0.018s |
| `mint_quote` applied | 09:57:55.893263 | 0.578s |
| payment observation submitted | 09:57:55.905002 | 0.012s |
| `mint_quote_payment` applied | 09:57:56.489981 | 0.585s |
| `mint` submitted by member fanout | 09:57:57.631598 | 1.142s |
| `mint` applied and signing response persisted | 09:57:58.236233 | 0.605s |
| threshold member handlers returned shares | 09:57:58.239201 | 0.003s |
| wallet aggregation, verification, persistence, and CLI completion | 09:57:58.747092 | 0.508s |

The fakewallet task can emit just before or just after consensus admission,
depending on scheduler order, but both samples prove settlement is immediate
and not the delay. The immediate post-quote observation path was used in both
mints; the periodic observer tick was not on the critical path.

### Classification

Each mint still required exactly three accepted operations. Across the two
instrumented confirmation mints, submit-to-apply latency was:

| Consensus operation | Mint 1 | Mint 2 |
| --- | ---: | ---: |
| `mint_quote` | 0.525s | 0.578s |
| `mint_quote_payment` | 0.610s | 0.585s |
| `mint` including signing/materialization | 0.853s | 0.605s |

- **Wallet/library and CLI:** the largest first-mint stage was 4.53 seconds
  before quote admission. The warm post-idle mint reached admission in 1.52
  seconds. This matches the earlier probe that attributed cold first use to
  online metadata/keyset loading plus ordinary registry/command startup; it is
  not an AlephBFT stall. The cost is partly wallet-library work, not solely CLI
  presentation code.
- **Mint handler and fakewallet:** payment notification was immediate. Quote
  handler admission and observation dispatch added milliseconds around the
  serialized consensus stages.
- **AlephBFT consensus:** all six operations applied in 0.52-0.85 seconds.
  There was no post-idle data-provider stall.
- **Private REST:** first operation-bearing deliveries followed data-provider
  yield by roughly 6-29ms.
- **Payment observer:** the immediate observation path submitted 2-12ms after
  quote apply; the 5-second periodic loop was only a fallback.
- **Catch-up/sync:** all five members were healthy before the run and stayed in
  one converged digest group afterward.
- **Database contention:** one 361ms SQLite `BEGIN IMMEDIATE` wait occurred
  during the larger first mint and 64-66ms waits occurred during the second;
  they were visible but not dominant.
- **Duplicate submissions/fallback:** both runs ended with zero duplicate
  submissions, zero pending items or waiters, and no fallback quote.

Every member accepted exactly:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

All five members reported matching order and state digests, two paid payment
reports, and no payment conflicts, unresolved observations, digest alerts, or
invalid-operation alerts.

No production fix is justified by this reproduction: the pathological delay
is already repaired and the current debug-build result is inside the 5-10s
target. If further startup optimization becomes a priority, the smallest clean
candidate remains a tamper-bound cache or validated wrapper for repeated
federation config/keyset validation. Its gate must separately benchmark CLI
dispatch, wallet repository load, connector validation, and first online
metadata discovery; reject changed, mismatched, or non-v3 configs even when a
cache exists; and preserve standalone behavior. Reducing three consensus
operations to two could save more wall time, but that is a protocol redesign,
not a performance bug fix.

## Fresh CLI and Mint Rebaseline After Command-Routing Audit

On 2026-07-10 local time, a fresh five-member, threshold-three fakewallet
AlephBFT federation and fresh CLI wallet were measured before changing
production code. Member, request-fallback, and quote-status timeouts were 90s
so timeout-driven quote fallback could not distort the run.

### Untouched baseline

| Command | Wall | User | System |
| --- | ---: | ---: | ---: |
| direct `curl /v1/info` | 0.92s | 0.00s | 0.00s |
| CLI `mint-info` without imported federation metadata | 2.28s | 0.03s | 0.02s |
| `federation add` | 1.42s | 1.41s | 0.01s |
| CLI `mint-info` with imported federation metadata | 1.56s | 0.61s | 0.01s |
| first full federated mint, 50 sat | 9.15s | 3.46s | 0.03s |
| second full federated mint, 30 sat | 6.05s | 1.45s | 0.01s |

The previously observed 36s federated `mint-info`, 46s pre-handler delay, and
66s mint did not reproduce. Both mints succeeded, and all five members
converged on exactly six accepted operations with matching order/state digests,
zero pending mempool items or waiters, zero duplicate submissions, and no
digest, payment, or invalid-operation alerts.

### Trace-confirmed timelines

The untouched baseline inherited `RUST_LOG=warn`, so it intentionally lacked
fine-grained stage events. A second fresh federation enabled the existing
behavior-neutral AlephBFT lifecycle trace plus normal mint/payment debug logs.
It measured 9.14s and 6.22s, within 0.01s and 0.17s of the untouched run, so the
trace did not materially perturb the result.

First mint, 50 sat and 14 outputs:

| Stage | UTC timestamp | Delta |
| --- | --- | ---: |
| CLI start | 23:07:12.592 | - |
| quote request created by member 1 | 23:07:17.052 | 4.460s |
| fakewallet payment notification | 23:07:17.054 | 0.002s |
| `mint_quote` consensus submitted | 23:07:17.054 | effectively immediate |
| `mint_quote` applied and primary waiter returned | 23:07:17.481 | 0.427s |
| immediate payment observation submitted | 23:07:17.483 | 0.002s |
| `mint_quote_payment` applied and waiter returned | 23:07:18.108 | 0.625s |
| `mint` submitted by member fanout | 23:07:18.189 | 0.081s |
| threshold member handlers returned shares | 23:07:19.045 | 0.856s |
| wallet aggregation, verification, persistence, and CLI completion | 23:07:21.750 | 2.705s |

Second mint, 30 sat and four outputs:

| Stage | UTC timestamp | Delta |
| --- | --- | ---: |
| CLI start | 23:07:21.760 | - |
| quote request created by member 1 | 23:07:24.235 | 2.475s |
| fakewallet payment notification | 23:07:24.237 | 0.002s |
| `mint_quote` consensus submitted | 23:07:24.255 | 0.018s |
| `mint_quote` applied and primary waiter returned | 23:07:24.825 | 0.570s |
| immediate payment observation submitted | 23:07:24.838 | 0.013s |
| `mint_quote_payment` applied and waiter returned | 23:07:25.449 | 0.611s |
| `mint` submitted by member fanout | 23:07:26.602 | 1.153s |
| threshold member handlers returned shares | 23:07:27.199 | 0.597s |
| wallet aggregation, verification, persistence, and CLI completion | 23:07:27.997 | 0.798s |

The journal on every member was exactly:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

This confirms that a successful mint still requires three consensus-visible
state transitions. The immediate payment-observation path was used; the 5s
periodic observer tick was not on the happy-path critical path.

### Temporary wallet/CLI probe

After the untouched and trace-only runs, temporary elapsed-time markers were
added behind a local environment flag. They were removed completely before
this checkpoint was finalized. Two further production-equivalent mints
measured the following stages:

| Stage | First mint, 14 outputs | Second mint, 4 outputs |
| --- | ---: | ---: |
| CLI startup through command dispatch | 0.508s | 1.431s |
| persisted federation registry validation | 0.494s | 0.494s |
| wallet repository rebuild | about 0.009s | about 0.932s |
| mint metadata/keyset load | 3.023s | 0.041s |
| quote request including quote/payment consensus | 2.047s | 2.122s |
| mint member calls, consensus, share checks, and aggregation | 2.870s | 2.292s |
| aggregate blind-signature verification | 0.309s | 0.084s |
| proof construction/unblinding | 0.035s | 0.009s |
| final BLS proof verification | 0.307s | 0.087s |
| wallet proof persistence after verification | about 0.012s | about 0.004s |
| full command wall time | 9.19s | 6.12s |

The first mint's metadata cost is a necessary initial online member/keyset
validation and is persisted: it falls from about 3.02s to 41ms on the next
process. BLS work scales with output count. The federation connector validates
each member's partial blind signatures before interpolation, then the wallet
validates aggregate blind signatures, unblinds them, and validates the final
proofs.

The warm repository stage exposed a separate wallet-library bug:
`WalletRepository::load_wallets` ignored persisted `MintInfo` and performed a
direct single-mint `/v1/info` fetch before the CLI loaded and installed its
federation registry. This was both a roughly 0.93s startup cost and a read-path
bypass of the federation connector.

### Accepted startup fix and verification

Repository reconstruction now derives initial wallet units from persisted mint
metadata. A missing metadata row retains the existing sat fallback.
Online metadata refresh still occurs through the wallet's configured connector
when an operation requests it; no value or trust semantics changed.

Focused connection-counter tests prove repository startup opens no mint TCP
connection for either a persisted `MintInfo` row or a missing-metadata `None` row. A
fresh post-fix five-member process run measured:

| Command | Before | After | Change |
| --- | ---: | ---: | ---: |
| first 50 sat mint | 9.15s | 9.21s | within run variance |
| warm second 30 sat mint | 6.05s | 4.97s | 1.08s faster |

All five post-fix members again converged on the exact six-operation sequence
with matching digests and no pending work or alerts. One member recorded one
idempotent duplicate submission/waiter join; there was no duplicate accepted
journal operation.

### Pre-batching classification

- **Dominant current cost:** wallet/library work, specifically cold metadata
  discovery on first use and output-count-dependent BLS share/proof validation.
- **AlephBFT consensus:** measurable but no longer dominant; each traced
  operation completed in roughly 0.43-0.86s.
- **Mint handler and fakewallet:** fakewallet notification took about 2ms;
  quote/payment apply and handler work were small relative to wallet setup and
  cryptography.
- **Payment observer:** the immediate path was used, so the periodic tick did
  not delay either mint.
- **Private REST:** first operation-bearing deliveries followed data-provider
  yield by milliseconds to tens of milliseconds.
- **Catch-up/sync:** all members were ready and converged before commands and
  remained converged afterward.
- **Database contention:** one roughly 0.29-0.36s SQLite `BEGIN IMMEDIATE` wait
  appeared during a 14-output mint, but wallet proof persistence itself took
  about 12ms and DB contention was not dominant.
- **Duplicate submissions:** zero in the untouched/trace baseline; one
  idempotent duplicate in the post-fix repeat, with the same six accepted
  operations.

## Accepted BLS Batch Verification

The wallet now batches all three existing v3/BLS verification layers without
removing any of them:

1. Each member's blind threshold-signature shares are verified as one batch
   after response length, amount, keyset, signer, and public-share metadata is
   checked. A failed batch falls back to scalar checks so the invalid member and
   output remain attributable and the collector can continue toward a valid
   threshold.
2. The aggregate blind signatures returned by mint, swap, melt-change,
   restore, and recovery paths are batch verified by the common mint-response
   validator.
3. Final unblinded v3 proofs are batch verified in the issue, receive, and
   explicit token-verification paths that previously verified them one by one.

Weights are deterministic non-zero BLS scalars derived from a domain-separated
SHA-256 transcript containing every signature, public key, and blinded message.
The proof and blind-signature transcripts use distinct domains. V1/v2 DLEQ
verification and standalone non-federated behavior remain on their existing
paths.

The Criterion fixture cycles over four independent denomination-style threshold
keysets so repeated-key pairing aggregation does not hide mixed-key behavior.
Every scalar and batch fixture is asserted valid before timing. The latest
local Criterion benchmark measured:

| Federation | Outputs | Scalar | Batch | Speedup |
| --- | ---: | ---: | ---: | ---: |
| 3-of-5 | 4 | 15.00ms | 13.44ms | 1.12x |
| 3-of-5 | 14 | 53.12ms | 25.83ms | 2.06x |
| 3-of-5 | 64 | 243.62ms | 77.34ms | 3.15x |
| 5-of-7 | 4 | 26.20ms | 22.77ms | 1.15x |
| 5-of-7 | 14 | 88.75ms | 40.64ms | 2.18x |
| 5-of-7 | 64 | 412.63ms | 131.03ms | 3.15x |

Fresh process tests then used brand-new fakewallet databases, wallets, and real
private-REST AlephBFT sessions:

| Federation | Mint | Wall | User | System |
| --- | --- | ---: | ---: | ---: |
| 3-of-5 | cold 50 sat | 7.87s | 2.02s | 0.03s |
| 3-of-5 | warm 30 sat | 5.05s | 1.34s | 0.01s |
| 5-of-7 | cold 50 sat | 10.39s | 2.92s | 0.07s |
| 5-of-7 | warm 30 sat | 6.51s | 1.93s | 0.05s |

Both topologies converged after exactly six accepted operations, latest index
5, with matching order and state digests on every member, no pending mempool
items or response waiters, and no invalid-operation alerts. The live result is
deliberately more modest than the 14/64-output microbenchmarks: ordinary mints
have small output sets and still pay for three serialized consensus operations.
Batching removes an output-count scaling cost; it does not disguise consensus
latency as cryptography.

Focused gates cover valid batch/scalar equivalence, mixed public keys,
single-share and aggregate-signature corruption, malformed lengths and signer
IDs, metadata rejection before point parsing, slow/offline/bad-share member
continuation, and standalone v3/BLS issue with federation disabled. The broad
CDK federation run initially passed 678 of 681 tests and exposed two collector
bugs where restore/check-state stopped after any successful threshold instead
of a matching threshold, plus one stale source-order test marker. The collectors
now use the final restore/check-state aggregators as their early-stop predicates,
the marker follows the renamed `published_quote` variable, and the complete
repeat passed all 681 tests. These were correctness/audit failures discovered by
the broad gate, not batch-verification failures.

## Accepted 100ms Active Cadence and Idle CPU Fix

CDK now uses a 100ms unit-creation delay only while the existing activity hint
reports local pending work or a newly seen inbound CDK consensus payload. Early
idle rounds keep the upstream delay and cooled tail rounds still use 2s. This
does not change consensus thresholds, operation ordering, journal semantics,
signing rules, or network message formats.

The first process attempt exposed a replay bug: AlephBFT idle gossip can keep
carrying old CDK data, and every replay refreshed the 10s network-activity
window. After a mint, members remained around 45-82% CPU after 15s, with one
member still at 24.8% in a later direct sample. The accepted fix tracks CDK
unit-body hashes already seen by each live session. A new body wakes active
cadence once; duplicate gossip is still delivered to AlephBFT but cannot extend
the activity deadline. The tracker has a hard 16,384-entry cap and is reset on
session rotation or restart. If the cap is reached, new payloads stop granting
fast-cadence wakeups while consensus remains live on the safe idle cadence.

Final fresh five-member, no-trace results:

| Check | Result |
| --- | ---: |
| full mint #1 | 8.45s wall |
| full mint #2 | 8.75s wall |
| member 1 idle CPU after 15s cool-down | 0.4% |
| member 2 idle CPU after 15s cool-down | 0.3% |
| member 3 idle CPU after 15s cool-down | 0.7% |
| member 4 idle CPU after 15s cool-down | 0.4% |
| member 5 idle CPU after 15s cool-down | 0.4% |

The traced 500ms baseline was 14.36s and 10.13s, so the accepted active cadence
removed about 5.91s from the first mint and 1.38s from the second. A cold
operation can still wait for an already-scheduled 2s idle sleep; that is the
intentional cost of keeping inactive members cold.

The default-cadence in-process probe measured cold submit-to-waiter completion
at about 2.53s and immediate warm completion at about 0.50s. A five-member
threshold-three operation returned its local waiter in about 0.83s.

Restart and catch-up gate:

- A mint with one of five members offline completed in 9.22s.
- The offline member restarted and reached strict ready/converged status on the
  third status attempt.
- All five members converged on nine accepted operations, latest index 8, and
  identical order and state digests.
- Pending mempool and waiter counts returned to zero and no digest or invalid
  operation alerts were reported.

The first short CPU samples understated a remaining issue. A later 30-second
CPU-time delta measured every member at about 31% of one core while idle. The
AlephBFT status reports showed why: even with no CDK operation pending, every
member still created an empty unit every two seconds, growing the shared DAG by
25 units per 10 seconds. Trace-disabled repetition remained at 27-29%, ruling
out lifecycle instrumentation.

The clean follow-up makes the async data provider wait when there is neither
local pending/finalization work nor a bounded peer-data activity window. Local
submission and newly seen authenticated CDK data wake it immediately. Peer
activity is tracked by unit-body hash, shortened to a one-second tail after
local finality so slower members can finish, and replayed data cannot extend it.
An intentionally too-short no-tail experiment was rejected by the process gate
because it stranded `mint_quote_payment` at rounds 3-4; the accepted one-second
tail completed full mints normally.

A stack sample then isolated the remaining post-work CPU in peer sync rather
than AlephBFT cadence: every cheap order-status read loaded and JSON-decoded the
entire durable finalized-item table only to calculate its length. Order status
now reads the already-maintained AlephBFT runtime finalized cursor. This keeps
catch-up semantics unchanged and removes the O(finalized history) parse.

Final corrected process results:

| Check | Result |
| --- | ---: |
| fresh fully idle member CPU | 0.9% each over 20s |
| full mint from a sleeping session | 8.20s wall |
| warm mint after persisted restart | 5.45s wall |
| post-work member CPU after mint #1 | 3.9-4.0% each over 20s |
| post-work member CPU after mint #2 | 4.5-4.8% each over 20s |
| accepted operations after two mints | 6, identical on all members |

The final strict gate reported identical order/state digests, zero pending
mempool items and waiters, and no digest, payment, or invalid-operation alerts.
The unbounded CPU/DAG-growth bug is closed. The remaining low single-digit
debug-build load is bounded periodic peer status work, not a spin loop.

## Lifecycle-Traced 500ms Active-Cadence Baseline

After process lifecycle tracing was enabled, a fresh five-member run measured
two full mints at 14.36s and 10.13s. All members converged on the same six
accepted journal operations, latest index 5, matching order and state digests,
and 13 finalized AlephBFT items.

The first mint's primary-member lifecycle deltas were:

| Operation | Submit to Data Yield | Submit to Finality | Finality to Apply | Submit to Waiter Return |
| --- | ---: | ---: | ---: | ---: |
| `mint_quote` | 0.269s | 2.281s | 0.005s | 2.286s |
| `mint_quote_payment` | 0.493s | 3.007s | 0.006s | 3.013s |
| `mint` | 0.412s | 2.430s | 0.430s | 2.860s |

The first successful private REST deliveries followed data-provider yield in
roughly 12-58ms. Quote and payment-observation durable apply took about 5-6ms.
Mint apply took about 430ms because it includes accepted output signing and
materialization, but it was still much smaller than the consensus-finality
interval.

During the second mint, `mint_quote` and `mint_quote_payment` entered the same
AlephBFT data unit and were finalized together. The final `mint` operation was
first yielded by another member about 0.296s after local admission, finalized
about 1.833s after local admission, and durably applied in about 0.177s. This
explains why the second full mint was faster while preserving the same ordered
three-operation journal.

Classification from the process trace:

- Dominant: AlephBFT active round/finality cadence.
- Secondary: CLI and wallet setup before the first consensus submission.
- Small but measurable: accepted mint signing/materialization.
- Not dominant: private REST delivery, quote/payment durable apply, waiter
  notification, fakewallet settlement, catch-up, or DB contention.

This evidence supports testing a faster active-only unit-creation cadence while
retaining the existing 2s idle cadence and pending-work activity hint.

## Pre-Active-Cadence Fresh Process Verification Run

A fresh five-member, threshold-three fakewallet AlephBFT federation and fresh
wallet were started from scratch and measured end to end. This run did not add
production instrumentation; timings were taken from command wall/user/sys time,
mint logs, federation status, and the member databases.

Command timing:

| Command | Wall Time | User CPU | Sys CPU | Classification |
| --- | ---: | ---: | ---: | --- |
| `curl /v1/info` | 0.88s | 0.00s | 0.00s | direct HTTP info path is not the delay source |
| non-federated `cdk-cli mint-info` | 2.70s | 0.03s | 0.01s | standalone CLI startup baseline |
| `cdk-cli federation add` | 1.42s | 1.40s | 0.01s | federation config validation/key aggregation remains CPU-heavy |
| federated `cdk-cli mint-info` | 1.53s | 0.60s | 0.00s | old 36s `mint-info` CPU burn did not reproduce |
| federated mint #1, 50 sat | 15.99s | 2.43s | 0.03s | dominated by three serialized AlephBFT operations plus CLI setup |
| federated mint #2, 30 sat | 12.70s | 1.17s | 0.01s | same shape with lower CLI/setup overhead |

All members converged on six accepted journal operations in the same order:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

All five members had operation count 6, latest operation index 5, and 14
finalized AlephBFT items. Member status reported two duplicate submissions and
two duplicate pending-waiter joins in this run, but the accepted journal order
remained exactly one quote, one payment observation, and one mint operation per
logical wallet mint.

Current mint #1 timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 19:18:53 | - | command invocation |
| quote request received by mint | 19:18:57.816 | about 4.8s | CLI startup, wallet setup, member selection/fanout setup |
| fakewallet payment notification | 19:18:57.817 | about 2ms | fakewallet payment is effectively immediate |
| quote consensus accepted before or at | 19:19:04.717 | about 6.9s | immediate payment-observation path could only run after `mint_quote` consensus returned |
| payment observation submitted | 19:19:04.717 | 0.000s | immediate observation path, not the periodic observer tick, submitted the paid observation |
| payment observation peer-visible | 19:19:06.982 | about 2.3s | peer sync observed operations through index 1 |
| mint/signing accepted and shares returned | by 19:19:09 | at most about 2.0s | command completed with issued proofs before later peer-sync visibility |
| wallet aggregation done | 19:19:09 | - | command completed |

Total wall time: 15.99s.

Current mint #2 timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 19:19:25 | - | command invocation |
| quote request received by mint | 19:19:27.731 | about 2.7s | lower CLI/setup overhead than mint #1 |
| fakewallet payment notification | 19:19:27.733 | about 3ms | fakewallet payment is effectively immediate |
| quote consensus accepted before or at | 19:19:34.395 | about 6.7s | immediate payment-observation path ran after quote consensus returned |
| payment observation submitted | 19:19:34.395 | 0.000s | immediate observation path submitted the paid observation |
| payment observation peer-visible | 19:19:37.173 | about 2.8s | peer sync observed operations through index 4 |
| mint/signing peer-visible | 19:19:37.537 | about 0.4s | peer sync observed operations through index 5 |
| wallet aggregation done | 19:19:38 | about 0.5s | command completed with issued proofs |

Total wall time: 12.70s.

Current classification:

- Primary delay: AlephBFT consensus cadence. Each mint still requires the three
  accepted operations `mint_quote`, `mint_quote_payment`, and `mint`.
- Secondary delay: CLI/wallet setup before the mint receives the quote request,
  about 4.8s for the first command and 2.7s for the second in this run.
- Not dominant: fakewallet payment settlement, direct `/v1/info` HTTP latency,
  database contention, catch-up after startup, private REST networking, or
  duplicate submissions.
- The payment observer tick is no longer the dominant happy-path delay for
  fakewallet minting because the route submits an immediate paid observation as
  soon as quote consensus returns.
- At this earlier checkpoint CPU was still a follow-up risk: after the mint benchmark, all members were
  healthy and converged, but AlephBFT continued producing idle units and member
  processes were observed with nontrivial CPU usage while the task queue still
  carried dozens of `UnitBroadcast` tasks. Treat the earlier CPU issue as
  improved, not fully closed. The accepted active-cadence section above records
  the later replay-deduplication fix and passing idle-after-work CPU gate.

Fix candidate recorded at that checkpoint:

- Add low-noise, production-safe timing diagnostics around the existing
  AlephBFT lifecycle stages: submit queued, data provider yielded, network
  message delivered, finalized data received, apply completed, operation
  notified, and waiter returned.
- Use those diagnostics to tune the live-session proposal/finality cadence
  without changing protocol behavior. The evidence points at consensus cadence,
  not payment settlement or raw HTTP.
- Re-run this same process benchmark plus an idle-after-work CPU check before
  considering larger protocol changes such as merging payment observation and
  mint signing.

The accepted active-cadence and replay-deduplication section above completes
this candidate and records the passing process gate.

## Pre-Fix Command Timing Summary

| Command | Wall Time | User CPU | Sys CPU | Classification |
| --- | ---: | ---: | ---: | --- |
| `curl /v1/info` | 0.88s | 0.00s | 0.00s | HTTP/mint info path is not the delay source |
| non-federated `cdk-cli mint-info` | 2.86s | 0.03s | 0.02s | CLI startup overhead exists but is not dominant |
| `cdk-cli federation add` | 1.36s | 1.35s | 0.00s | CPU-heavy federation config validation/key aggregation |
| federated `cdk-cli mint-info` | 1.48s | 0.55s | 0.00s | federation registry validation still costs measurable CPU |
| federated mint #1, 50 sat | 27.67s | 2.36s | 0.02s | dominated by consensus cadence and observer delay |
| federated mint #2, 30 sat | 24.63s | 1.15s | 0.01s | same dominant delays, lower CLI startup cost |

The direct HTTP control shows `/v1/info` itself was fast. The slow mint path is not explained by raw HTTP latency or fakewallet payment settlement.

## Mint #1 Timeline

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 16:46:55.031 | - | command invocation |
| quote request received by mint | 16:46:59.348 | 4.317s | CLI startup, wallet setup, member selection/fanout setup |
| fakewallet payment notification | 16:46:59.350 | 0.002s | fakewallet payment itself is effectively immediate |
| quote response visible to CLI | 16:47:08.612 | 9.262s | quote creation is gated by `mint_quote` consensus |
| payment observer submitted observation | 16:47:12.618 | 4.006s | periodic observer tick delay after quote became locally visible |
| payment observation accepted/visible | 16:47:17.056 | 4.438s | `mint_quote_payment` consensus |
| mint/signing accepted | 16:47:21.048 | 3.992s | `mint` consensus |
| wallet aggregation done | 16:47:22.704 | 1.656s | share collection and aggregation |

Total wall time: 27.67s.

## Mint #2 Timeline

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 16:47:22.723 | - | command invocation |
| quote request received by mint | 16:47:25.029 | 2.306s | lower startup/setup overhead than mint #1 |
| fakewallet payment notification | 16:47:25.031 | 0.002s | fakewallet payment itself is effectively immediate |
| quote response visible to CLI | 16:47:34.724 | 9.693s | quote creation is gated by `mint_quote` consensus |
| payment observer submitted observation | 16:47:38.729 | 4.005s | periodic observer tick delay after quote became locally visible |
| payment observation accepted/visible | 16:47:42.188 | 3.459s | `mint_quote_payment` consensus |
| mint/signing accepted | 16:47:46.869 | 4.681s | `mint` consensus |
| wallet aggregation done | 16:47:47.362 | 0.493s | share collection and aggregation |

Total wall time: 24.63s.

## Post-Fix Command Timing Summary

After quote consensus is accepted, the mint route now checks only the newly
created quote once. If it is already paid, it immediately submits the existing
`mint_quote_payment` observation path and keeps the periodic observer as a
fallback for delayed payments.

| Command | Wall Time | User CPU | Sys CPU | Classification |
| --- | ---: | ---: | ---: | --- |
| `curl /v1/info` | 0.90s | 0.00s | 0.00s | unchanged direct HTTP control |
| non-federated `cdk-cli mint-info` | 2.35s | 0.03s | 0.01s | standalone CLI control remains near startup cost |
| `cdk-cli federation add` | 1.39s | 1.39s | 0.00s | config validation/key aggregation remains CPU-heavy |
| federated `cdk-cli mint-info` | 1.54s | 0.60s | 0.00s | registry validation cost remains measurable |
| federated mint #1, 50 sat | 12.58s | 2.45s | 0.02s | observer gap removed; first command still pays CLI/setup cost |
| federated mint #2, 30 sat | 8.65s | 1.17s | 0.01s | observer gap removed; steady-state run is inside target range |

All five members converged on the same six accepted journal operations:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

The post-fix final status reported 6 accepted operations, latest index 5,
matching order digest and state digest on all members, 14 finalized AlephBFT
items, and one duplicate submission/duplicate waiter join caused by a harmless
race between the immediate observation path and the periodic observer.

## Post-Fix Mint #1 Timeline

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 16:59:48 | - | command invocation |
| quote request received by mint | 16:59:52.703 | about 4.7s | CLI startup, wallet setup, member selection/fanout setup |
| fakewallet payment notification | 16:59:52.706 | 0.003s | fakewallet payment itself is effectively immediate |
| quote consensus accepted | 16:59:56.248 | 3.542s | `mint_quote` accepted before immediate payment observation returns |
| payment observation submitted/accepted | 16:59:56.248 | 0.000s | immediate observation path submitted `mint_quote_payment`; periodic observer raced harmlessly |
| mint/signing accepted | about 16:59:58.856 | about 2.608s | peer sync observed operations through index 2 |
| shares returned and wallet aggregation done | 17:00:00 | about 1.1s | command completed |

Total wall time: 12.58s.

## Post-Fix Mint #2 Timeline

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 17:00:00 | - | command invocation |
| quote request received by mint | 17:00:03.248 | about 3.2s | lower startup/setup overhead than mint #1 |
| fakewallet payment notification | 17:00:03.250 | 0.002s | fakewallet payment itself is effectively immediate |
| quote consensus accepted | about 17:00:05.940 | about 2.690s | quote became locally visible around the observer/immediate observation window |
| payment observation submitted/accepted | 17:00:05.940 | 0.000s | periodic observer won the race in this run; accepted operation is still one `mint_quote_payment` |
| mint/signing accepted | about 17:00:08.985 | about 3.045s | peer sync observed operations through index 5 |
| shares returned and wallet aggregation done | 17:00:09 | about 0.6s | command completed |

Total wall time: 8.65s.

## Rejected AlephBFT Delay-Cap Experiment

A follow-up experiment capped the early AlephBFT unit-creation delay at 100ms
while keeping the existing 2s idle-tail delay after round 64. This change was
tested and then removed; it is not part of the production code.

Clean five-member, threshold-three fakewallet timings from that experiment:

| Command | Wall Time | User CPU | Sys CPU | Classification |
| --- | ---: | ---: | ---: | --- |
| `curl /v1/info` | 0.88s | 0.00s | 0.00s | direct HTTP unchanged |
| non-federated `cdk-cli mint-info` | 0.90s | 0.02s | 0.00s | standalone CLI control was fast |
| `cdk-cli federation add` | 1.35s | 1.34s | 0.00s | config validation/key aggregation remains CPU-heavy |
| federated `cdk-cli mint-info` | 1.49s | 0.56s | 0.00s | federation info path was not the bottleneck |
| federated mint #1, 50 sat | 30.26s | 2.37s | 0.01s | worse than the post-fix baseline |
| federated mint #2, 30 sat | 22.67s | 1.16s | 0.01s | still dominated by consensus pacing |

The cap made every member reach round 64 within roughly 10s after startup, so
normal wallet traffic landed in the 2s idle-tail cadence. During the mint
commands, member logs showed `AlephBFT-creator: Delay passed` every about 2s.
The first mint received its quote request at 17:18:35.695 UTC, fakewallet
reported payment at 17:18:35.697 UTC, and the first accepted journal operation
became visible to member-1 peer sync at 17:18:47.909 UTC. Payment observation
was submitted at 17:18:49.537 UTC, operation count reached 2 at 17:18:52.910
UTC, and the wallet completed at 17:19:01 UTC. The second mint had the same
shape: quote request at 17:19:03.970 UTC, fakewallet payment at 17:19:03.972
UTC, payment observation at 17:19:13.651 UTC, operation count reached 5 at
17:19:18.075 UTC, and the wallet completed at 17:19:24 UTC.

The experiment also reproduced the old CPU symptom: after the mints, several
member processes were observed near a full core while the AlephBFT task queue
still carried dozens of `UnitBroadcast` tasks. Conclusion: a fixed faster delay
cap is the wrong fix. The next candidate should be pending-work-aware: keep
idle behavior cold, but wake/propose quickly when the federation mempool gains
new consensus work, or rotate/reset an idle session in a way that does not
spin or burn through the active rounds before user traffic arrives.

## Dominant Delay Classification

### Primary: AlephBFT Consensus Cadence

Each full mint currently performs three serialized consensus operations:

1. Quote creation: `mint_quote`
2. Payment observation: `mint_quote_payment`
3. Signing/issuance: `mint`

Observed consensus-related delays were multiple seconds per operation:

- `mint_quote`: about 9.3-9.7s from quote request received to quote response visible to the CLI.
- `mint_quote_payment`: about 3.5-4.4s from observer submission to payment observation visible locally.
- `mint`: about 4.0-4.7s from paid observation visibility to mint/signing acceptance.

This is the largest structural reason minting takes 24-28s. Even with all other overhead removed, three consensus rounds at the observed cadence exceed the 5-10s target.

### Secondary: Periodic Payment Observer Tick

The fakewallet recorded payment about 2ms after quote creation, but the federation did not submit the payment observation until about 4.0s after the quote response became visible.

This is caused by the periodic mint payment observer loop. It wakes every 5s and scans quotes. In the happy path, this adds up to one observer interval even though the payment is already known.

Expected saving from fixing this alone: about 4-5s per mint in the fakewallet happy path.

### Secondary: Wallet/CLI Startup and Federation Setup

The first mint spent about 4.3s before the mint saw the quote request. The second mint spent about 2.3s.

Likely contributors:

- CLI process startup and wallet initialization.
- Federation registry load.
- federation config validation.
- aggregate BLS public key recomputation.
- member connector setup.

The standalone and federated `mint-info` measurements are not catastrophic after recent work, but federation config validation remains measurable CPU work.

Expected saving: about 1-3s per command if config validation/key aggregation is cached and activation is lazy.

### Not Dominant In This Run

- Fakewallet settlement: payment notification was about 2ms after quote creation.
- Direct HTTP `/v1/info`: under 1s.
- Database contention: no evidence from this run.
- Catch-up/sync after startup: members remained healthy and converged.
- Private REST networking after startup: no evidence it dominated steady-state minting.
- Duplicate submissions: created extra finalization noise, but accepted journal operations remained six for two mints.
- Receiver-side receive/swap: measured independently at 7.92s with exactly one
  accepted consensus operation; its dominant delay was pre-submit CLI setup.

## Improvement Candidates

### 1. Submit Mint Payment Observation Immediately After Quote Consensus

Status: implemented.

Previous behavior:

- The quote route submits `mint_quote` to consensus.
- The fakewallet marks the quote paid almost immediately.
- The payment observation is usually submitted later by the periodic observer.

Implemented behavior:

- After `mint_quote` consensus is accepted, check only that quote's backend payment status once.
- If it is already paid, submit the existing `mint_quote_payment` observation immediately.
- Keep the periodic observer as a fallback for delayed or missed payments.

Observed impact:

- Reduced full mint wall time from 27.67s and 24.63s to 12.58s and 8.65s.
- Removed the periodic observer tick as a dominant happy-path delay.
- Left the journal sequence unchanged: every mint still requires `mint_quote`, `mint_quote_payment`, and `mint`.
- Does not change the protocol state machine.
- Does not make quote creation wait for arbitrary payment polling; unpaid quotes still keep only the `mint_quote` operation and rely on later checks or the periodic observer.

Test coverage:

- `cargo test -p cdk-axum --features federation post_mint_custom_quote_ -- --test-threads 1`
- `cargo test -p cdk-axum --features federation mint_payment_observer_round -- --test-threads 1`
- `cargo test -p cdk-axum --features federation federation_payment_observation_handlers_are_backend_agnostic -- --test-threads 1`
- `cargo test -p cdk-axum --features federation public_endpoints_are_consensus_gated_in_federation_mode -- --test-threads 1`
- Fresh 5-member threshold-3 fakewallet AlephBFT benchmark with two full mints.

### 2. Tune AlephBFT Proposal/Finality Cadence

Current observation:

- Each consensus operation takes roughly 3.5-9.7s.
- A mint needs three of these operations.

Checkpoint update:

- A fixed always-fast active delay was tested and rejected because it pushed
  sessions into the idle tail before wallet traffic and reproduced high CPU.
- The live session now uses an in-memory activity hint instead: inactive tail
  rounds keep the 2s delay, local accepted AlephBFT submissions mark local
  pending activity, authenticated inbound AlephBFT private REST messages that
  actually carry CDK consensus data mark a bounded recent-network-activity
  window, and tail rounds use the normal upstream active cadence only while one
  of those activity signals is present.
- This keeps idle members cold while avoiding repeated 2s tail intervals during
  real consensus work. It does not change operation ordering, journal semantics,
  payment observation semantics, signing rules, or member networking formats.
- Benchmark correction: the first activity-hint implementation marked every
  inbound AlephBFT private REST message as activity. Idle empty-unit gossip kept
  sessions warm forever, with `AlephBFT-creator: Delay passed` every about
  500ms and member processes around 4-7% CPU. The corrected implementation only
  marks inbound messages whose decoded AlephBFT network message includes CDK
  consensus data. After an idle wait past the tail threshold, creator cadence
  slowed to about 2s and each member sat around 0.4-0.5% CPU.
- Final correction: messages containing old CDK data can also be replayed by
  idle gossip. The inbound session now refreshes activity only for a previously
  unseen CDK unit-body hash. This preserves the first-arrival wakeup while
  preventing old consensus-data replay from keeping the 100ms active cadence
  alive indefinitely.
- Final idle correction: an inactive data provider waits instead of emitting an
  empty unit every two seconds. Local work and new peer operation bodies wake
  it, and local finality shortens peer activity to a one-second propagation
  tail. Peer order status also uses the maintained AlephBFT finalized cursor
  instead of loading and decoding the entire finalized-item table to count it.

Selective activity-hint command timing:

| Command | Wall Time | User CPU | Sys CPU | Classification |
| --- | ---: | ---: | ---: | --- |
| `curl /v1/info` | 0.87s | 0.00s | 0.00s | direct HTTP unchanged |
| non-federated `cdk-cli mint-info` | 0.95s | 0.02s | 0.02s | standalone CLI control remained fast |
| `cdk-cli federation add` | 1.35s | 1.33s | 0.00s | config validation/key aggregation remains CPU-heavy |
| federated `cdk-cli mint-info` | 1.52s | 0.58s | 0.00s | registry validation cost remains measurable |
| federated mint #1, 50 sat | 13.90s | 2.38s | 0.06s | cold idle-tail wakeup plus three consensus operations |
| federated mint #2, 30 sat | 12.01s | 1.14s | 0.03s | follow-on mint faster but still consensus-dominated |

Selective activity-hint mint #1 timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 17:57:31.667 | - | command invocation after idle tail cooled to 2s |
| quote request received by mint | 17:57:36.071 | 4.404s | CLI startup, wallet setup, member selection/fanout setup |
| fakewallet payment notification | 17:57:36.073 | 0.002s | fakewallet payment itself is effectively immediate |
| quote/payment observation submitted | 17:57:41.429 | 5.356s | quote consensus accepted before the immediate observation path returned; periodic observer raced harmlessly |
| payment observation peer-visible | 17:57:42.881 | 1.452s | peer sync observed operations through index 1 |
| mint/signing accepted and wallet aggregation done | 17:57:45.591 | 2.710s | command completed with issued proofs |

Total wall time: 13.90s.

Selective activity-hint mint #2 timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| CLI start | 17:57:45.619 | - | command invocation |
| quote request received by mint | 17:57:48.016 | 2.397s | lower startup/setup overhead than mint #1 |
| fakewallet payment notification | 17:57:48.019 | 0.003s | fakewallet payment itself is effectively immediate |
| quote peer-visible | 17:57:53.077 | 5.058s | peer sync observed operations through index 3 |
| payment observation submitted | 17:57:53.606 | 0.529s | immediate observation path and periodic observer raced harmlessly |
| mint/signing accepted and wallet aggregation done | 17:57:57.651 | 4.045s | command completed with issued proofs; peer sync saw operations through index 5 at 17:57:58.104 |

Total wall time: 12.01s.

## Receiver-Side Receive/Swap Benchmark

Status: completed on a fresh five-member, threshold-three fakewallet AlephBFT
federation with fresh sender and receiver wallets.

Receiving a Cashu token is not just a local wallet import. The receiver redeems
the sender's proofs by creating new blinded outputs and submitting a swap to the
mint. In federation mode, that swap must be serialized through consensus before
the receiver can aggregate returned shares into fresh proofs. This makes
receiver-side token receive the next critical wallet-facing latency benchmark.

Expected accepted operation sequence for one receive:

1. `swap`

The receive benchmark should use a fresh receiver wallet that has already
imported the same federation public config as the sender. The sender should
create a token from already-minted proofs, preferably with `--offline`, so the
measurement isolates the receiver-side swap rather than sender-side token
preparation.

Suggested command shape:

```bash
export FED_DIR=target/perf-fed-5
export SENDER_WALLET_DIR=target/perf-wallet-sender
export RECEIVER_WALLET_DIR=target/perf-wallet-receiver
export MINT_URL=<primary federation member URL>

target/debug/cdk-cli --work-dir "$RECEIVER_WALLET_DIR" --non-interactive \
  federation add --public-config "$FED_DIR/federation-public.json" \
  --member-request-timeout-secs 90 \
  --member-fallback-timeout-secs 90 \
  --quote-status-member-timeout-secs 90

TOKEN=$(
  target/debug/cdk-cli --work-dir "$SENDER_WALLET_DIR" --non-interactive \
    --unit sat send --mint-url "$MINT_URL" --amount 30 --offline
)

date -u +"RECEIVE CLI start UTC: %Y-%m-%dT%H:%M:%SZ"
time target/debug/cdk-cli --work-dir "$RECEIVER_WALLET_DIR" --non-interactive \
  --unit sat receive "$TOKEN"
date -u +"RECEIVE CLI end UTC: %Y-%m-%dT%H:%M:%SZ"
```

If the exact `send` command shape changes, keep the benchmark invariant the
same: the token must already exist before the timed receiver command starts.

Measured receive timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| receiver CLI start | 20:24:17 | - | command invocation |
| all-member swap submission | 20:24:21.932-21.935 | about 4.9s | CLI activation, token decode, connector setup, and member fanout |
| primary data-provider yield | 20:24:23.891 | 1.957s | one already-scheduled cold idle wait |
| first private REST delivery | 20:24:23.950 | 0.060s | member networking was not dominant |
| first finalized data | 20:24:24.308 | 0.358s | 2.374s submit-to-finality |
| apply/materialization complete | 20:24:24.817 | 0.509s | inputs spent and output shares persisted |
| waiter/share responses returned | 20:24:24.828 | 0.011s | all participating member waiters released |
| receiver aggregation done | 20:24:25 | under 1s | command exited after storing fresh proofs |

Command timing was 7.92s wall, 1.73s user CPU, and 0.02s system CPU. Exactly
one `swap` was accepted at journal index 3; no duplicate logical swap was
appended. The sender ended with 70 sat and 14 unspent local proofs, while the
receiver ended with 30 sat and 11 unspent proofs. Both wallets had zero pending
or spent local proofs. All five members converged on four operations, latest
index 3, identical order/state digests, zero pending work, and no alerts.

The dominant receive delay was before mint submission: about 4.9 seconds of CLI
startup, registry activation, token parsing, and connector construction. The
single consensus operation took about 2.9 seconds from submit through waiter
return; private REST delivery was about 60ms and aggregation/storage finished
in under a second. The under-10-second target therefore passes.

Target: receiver-side receive should complete in under 10 seconds on a fresh
five-member, threshold-three fakewallet AlephBFT federation. Since receive is
expected to require one consensus operation rather than the mint path's three
serialized operations, a healthy warm receive should be materially faster than
minting. A cold receive may still expose the idle-tail wakeup penalty, but it
should not approach the old 24-28s mint timings.

Classification rules:

- If `/v1/swap` reaches a mint quickly but `swap consensus accepted` is delayed,
  classify the delay as AlephBFT consensus cadence or idle-tail wakeup.
- If `swap consensus accepted` is fast but `shares returned` or
  `receiver aggregation done` is delayed, classify the delay as wallet fanout,
  member response waiting, share verification, aggregation, or storage.
- If the mint never sees `/v1/swap` for several seconds, classify the delay as
  CLI startup, federation registry validation, token parsing, or connector
  setup.
- If accepted journal order shows more than one logical `swap` for a single
  receive, classify and investigate duplicate submissions separately.

Investigation targets:

- AlephBFT session proposal interval.
- Network pump tick interval.
- batch timing.
- finality notification path.
- local operation acceptance wait path.
- whether proposals are delayed until the next status/pump tick instead of being flushed immediately.

Expected impact:

- This is the main path to reaching 5-10s total.
- If each consensus operation can finalize in about 1-2s, three-operation minting becomes feasible in the target range.

Test plan:

- Done for the activity-hint behavior: focused unit tests cover inactive and
  active tail delays, local pending clearing after finalization drains, inbound
  private REST gossip carrying consensus data waking the hint, idle inbound
  messages not waking the hint, and a live private-REST AlephBFT finality smoke.
- Done for the first non-production AlephBFT latency harness: focused
  two-member live-session test records operation-specific stage timing for real
  upstream `run_session` over private REST. With fast in-process test delays,
  one mint-quote operation recorded:

  `SubmitQueued@1ms -> DataProviderYielded@20ms -> NetworkMessageDelivered@24ms -> FinalizedDataReceived@68ms -> ApplyLoopReceived@68ms -> ApplyCompleted@70ms -> OperationNotified@70ms -> WaiterReturned@70ms`.

  The duplicate later finalization/apply events in that test come from the
  second live member applying the same operation. This is not a full process
  fakewallet timing number; it proves the local apply and waiter-notification
  path is not the 12-14s dominant delay under fast consensus options.
  A follow-up three-operation serial probe recorded submit-to-waiter deltas in
  the roughly 50-70ms range for three consecutive mint-quote operations under
  the same fast in-process options.
  A duplicate-fanout probe now submits one mint-quote operation twice
  concurrently and verifies one `SubmitQueued`, one `DataProviderYielded`, two
  `WaiterReturned` events, and one duplicate-pending waiter join. In the focused
  run both waiters returned around 70ms from recorder installation.
  A five-member threshold-three BFT-sized probe records the same stage sequence
  across all five live members; focused runs returned the local waiter around
  1.0-1.3s after recorder installation and then confirmed all members applied
  the operation.
  A seven-member threshold-five BFT-sized probe records the same stage sequence
  across all seven live members; the focused run returned the local waiter
  around 2.5-3.3s and confirmed all members applied by around 3.0-3.8s.
  A default-cadence cold/warm probe records the idle-tail shape without fast
  test delay overrides. After an idle pause, the cold operation yielded data
  after about 2.0s and returned the local waiter at about 4.0-4.5s. The
  immediate follow-up yielded after about 0.48s and returned at about 2.5s.
- Still needed: use the combined process and in-process evidence to choose and
  test a production-safe latency improvement:
  - default-cadence process repetitions before and after any candidate tuning
  - CPU and catch-up safety checks under idle and active workloads
- Record proposal submitted, proposal broadcast, finalized, applied, and waiter notified timestamps.
- Compare default timing with tuned intervals.
- Confirm no CPU spin and no lost catch-up behavior.

## Direct Swap, Change Melt, Offline Member, and Peer-Sync CPU

Status: completed on a fresh five-member, threshold-three fakewallet AlephBFT
federation with trace-level operation lifecycle diagnostics.

An ordinary exact-value send can select existing proofs without contacting the
mint. To guarantee a real swap, the benchmark sent P2PK-locked outputs. To
guarantee real melt change, a separate 20 sat wallet first moved all four 1 sat
proofs into offline tokens, leaving only 2 and 4 sat proofs. Paying a 10 sat
invoice with a 1 sat fee reserve then required selecting 12 sat and returning
change.

Measured command results:

| Scenario | Accepted operations | Wall | User | System |
| --- | --- | ---: | ---: | ---: |
| forced P2PK send, all members live | `swap` | 4.71s | 1.05s | 0.02s |
| forced change melt, all members live | `melt_quote -> swap -> melt` | 8.06s | 1.25s | 0.03s |
| forced P2PK send, member 5 offline | `swap` | 4.90s | 1.03s | 0.02s |
| forced change melt, member 5 offline | `melt_quote -> swap -> melt` | 8.45s | 1.19s | 0.02s |
| forced P2PK send after peer-status fix | `swap` | 5.45s | 1.18s | 0.01s |

The all-live swap reached the mint about 4.3 seconds after CLI start. Once
submitted, its data-provider yield was immediate, finality arrived in about
429ms, durable apply completed in about 607ms, and the waiter returned in about
610ms. The dominant swap delay was therefore CLI/wallet setup, not consensus,
private REST, apply, share return, or aggregation.

The all-live change melt spent about 3.85 seconds before the mint received the
quote operation. Its consensus stages were:

| Operation | Submit to waiter return |
| --- | ---: |
| `melt_quote` | about 590ms |
| preparatory `swap` | about 762ms |
| terminal `melt` | about 786ms |

The wallet finished with 5 sat, including a newly returned 1 sat change proof.
The one-offline-member timings remained within 0.4 seconds of the all-live
commands, confirming that wallet aggregation and AlephBFT finality did not wait
for all five members. After member 5 restarted under the AlephBFT backend, it
applied the missed suffix; all five members reported 17 operations, latest
index 16, identical order and state digests, zero pending work, and zero digest
or invalid-operation alerts.

The post-catch-up CPU gate revealed an independent O(history) status cost. With
the DAG fixed and no pending operations, trace-enabled members used
15.45-17.05% CPU. A three-second stack sample placed the active work in the
five-second peer-sync loop:

`sync_once_from_member -> local_peer_order_status -> durable_federation_journal -> get_federation_journal_entries`

That path deserialized every historical operation repeatedly, and compressed
BLS point decoding was the hottest sampled arithmetic. Order-only status needs
only the latest durable index, order digest, and consensus-visible state
digest. The storage trait and SQL implementation now expose exactly that scalar
journal tip; full history loading remains in materialized-state status and
diagnostic paths that actually inspect operation contents.

Holding the same 17-entry databases, federation size, debug build, and trace
filter constant, the 20-second CPU sample changed as follows:

| Member | Before | After |
| --- | ---: | ---: |
| 1 | 17.05% | 3.95% |
| 2 | 15.55% | 3.85% |
| 3 | 15.45% | 3.85% |
| 4 | 15.65% | 3.95% |
| 5 | 15.80% | 3.90% |

A new forced swap after the change completed in 5.45 seconds. Strict status
then converged all five members on 18 operations, latest index 17, identical
order/state digests, no pending work, and no alerts. Focused verification:

- `cargo test -p cdk-sqlite --features federation federation_journal_persistence -- --nocapture`
- `cargo test -p cdk-axum --features federation peer_sync_ -- --nocapture`
- `cargo check -p cdk-mintd --features federation,fakewallet,sqlite,prometheus`

No temporary instrumentation was added for this fix; the existing opt-in trace
target and an external process sample provided the required evidence.

### 3. Consider Combining Quote Payment Observation With Mint Signing When Safe

Current behavior:

- Wallet gets a quote.
- Wallet waits for paid state.
- Wallet submits mint outputs.
- Mint path may check backend status again and submit/ensure payment observation before signing.

Potential optimization:

- If the wallet submits mint outputs before a separate `mint_quote_payment` observation is finalized, the mint handler already checks quote payment status before signing.
- There may be a safe design where the payment observation and mint request are linked or processed in one consensus item, provided all members independently verify that the quote is paid and the outputs are identical.

Risk:

- This touches consensus semantics and must preserve the anti-mix-and-match guarantee.
- The federation must not sign before a quorum-agreed paid observation or equivalent deterministic consensus item exists.

Expected impact:

- Could reduce minting from three consensus operations to two in the paid-happy-path.
- This is a larger protocol design change and should follow the simpler observer and cadence fixes.

Test plan:

- Formalize the operation semantics before implementation.
- Add adversarial tests for different outputs per member for the same quote.
- Add duplicate quote/mint submission tests.
- Add restart/catch-up tests proving the final journal applies the same state on all members.

### 4. Cache Validated Federation Config and Aggregate Key Material

Current observation:

- `federation add` used 1.35s user CPU.
- federated `mint-info` used 0.55s user CPU.
- Earlier slow-path hypotheses pointed at repeated aggregate BLS public key recomputation during config validation and connector activation.

Proposed behavior:

- Introduce or strengthen a validated federation config wrapper.
- Cache config digest and aggregate keyset validation results.
- Avoid eager activation for commands that do not need a live federation wallet.
- Avoid repeated `public_config.validate()` calls inside digest/member-info paths.

Expected impact:

- Saves about 0.5-3s per CLI command depending on command and registry size.
- Reduces user CPU.
- Does not change protocol behavior.

Test plan:

- Benchmark `mint-info`, `balance`, and `mint` startup before/after.
- Unit test that invalid v3/BLS federation keysets are still rejected.
- Unit test that mismatched member keysets are still rejected.
- Confirm standalone wallet behavior is unchanged.

### 5. Reduce Duplicate Finalization Noise

Current observation:

- Two mints produced six accepted journal operations but twelve finalized items.
- Duplicate submissions were correctly deduplicated by operation identity/conflict handling.

Proposed behavior:

- Keep duplicate submissions valid and idempotent.
- Reduce redundant fanout if one accepted response is already known.
- Improve logging so duplicate finalizations do not obscure operation timing.

Expected impact:

- May reduce log volume and some network/processing work.
- Not expected to save the bulk of the 24-28s by itself.

Test plan:

- Concurrent identical wallet fanout test still aggregates once.
- Journal operation count remains one per logical operation.
- Response cache still returns persisted shares for duplicate requests.

## Target Performance Budget

To reach 5-10s full minting, a realistic near-term budget is:

| Stage | Target |
| --- | ---: |
| CLI startup/member setup | <= 1s |
| `mint_quote` consensus | <= 2s |
| payment observation dispatch | <= 0.25s after quote finality if already paid |
| `mint_quote_payment` consensus | <= 2s |
| `mint` consensus | <= 2s |
| share collection/aggregation | <= 1s |

This totals about 6-8s in the fakewallet happy path. If the protocol remains at three serialized consensus operations, each consensus operation must be consistently near 1-2s. If consensus remains 4-10s per operation, the 5-10s target is not reachable without reducing the number of consensus operations.

Receiver-side receive/swap target budget:

| Stage | Target |
| --- | ---: |
| CLI startup/token decode/member setup | <= 1s |
| `swap` consensus | <= 2s |
| share collection/aggregation/storage | <= 1s |

This leaves a normal warm receive target around 3-4s and confirms the hard
target below 10s. Further improvement should focus first on CLI activation and
connector setup, then on eliminating the one already-scheduled cold idle wait.

## Slow-Member CLI Value Matrix

On 2026-07-10, `just fed-regtest-cli-smoke-slow-member` ran a fresh
five-member, threshold-four AlephBFT federation with member 5 configured for a
deterministic 30-second fakewallet incoming-payment delay. The test uses two
wallets so denomination selection cannot hide either value operation: the
first creates a P2PK-locked send that requires a swap, while the second removes
all four 1 sat proofs before paying a 10 sat invoice plus 1 sat reserve, forcing
a 1 sat change output through another swap.

| Operation | Wall | User CPU | System CPU | Result |
| --- | ---: | ---: | ---: | --- |
| forced P2PK swap while slow member is live | 3.55s | 0.93s | 0.01s | threshold token returned |
| mint with delayed member 5 as quote primary | 37.60s | 2.27s | 0.03s | 20 sat issued after configured delay |
| forced-change melt through member 5 URL | 6.74s | 1.21s | 0.02s | 10 sat + 1 sat fee paid; 5 sat remains |

The final durable journal contained exactly:

```text
mint_quote -> mint_quote_payment -> mint -> swap ->
mint_quote -> mint_quote_payment -> mint ->
melt_quote -> swap -> melt
```

All five members reported 10 operations through index 9, identical order and
state digests, zero pending mempool items/waiters, zero duplicate submissions,
and zero payment, digest, or invalid-operation alerts. Exact journal cardinality
replaces the older log-text quote-count assertion: any fallback-created quote
would append another `mint_quote` lifecycle and fail the 10-operation gate.
The 37.60-second mint is intentional evidence of the configured 30-second
backend delay; it is not an AlephBFT performance regression. The swap and melt
remain below the 10-second value-operation target while that member is live.

## Eleven-Member Transport and SQLite Investigation

On 2026-07-10, a fresh 11-member, threshold-eight fakewallet federation was
used to separate private-REST overload from the later SQLite lock failure. The
baseline reproduced request-timeout bursts before the severe writer lock, so
database contention was not treated as the initiating cause.

### Confirmed causal sequence

1. The federation build selected the synchronous `bitreq` transport. Each
   private request ran through `spawn_blocking`, opened a fresh connection, and
   provided no connection pooling. AlephBFT all-to-all gossip, peer sync, and
   session-evidence polling therefore amplified one-host thread and connection
   pressure.
2. A standalone 2,000-request comparison measured about 27.7 seconds through
   `bitreq` and about 57 milliseconds through pooled async `reqwest`. Running
   the `bitreq` comparison against the federation immediately reproduced the
   synchronized 10-second network-message timeout bursts.
3. Once handlers and catch-up paths backed up, independent SQLite connections
   competed for the single WAL writer. The 20-connection pool and
   `BEGIN IMMEDIATE` did not serialize writers; one member eventually waited
   through the 10-second busy timeout and returned HTTP 500 from journal reads.
4. The same accepted melt-quote operation reached concurrent replay/apply
   paths. Its read-then-insert persistence was not atomic, so both paths saw no
   row and one hit the unique quote-ID constraint.
5. A failed explicit transaction could then retain its connection until an
   asynchronously scheduled drop rollback ran. This was an avoidable lock
   amplifier, although the earlier transport storm proves it was not the
   original cause.

The six `range content mismatch` observations were fail-closed stale-evidence
rejections. Validation compares the signed session range with the local
accepted range before persisting evidence or changing readiness. Focused tests
prove mismatched evidence is not stored, does not satisfy quorum, and does not
mutate journal or materialized state.

### Accepted fixes

- Federation-enabled Axum now selects pooled async `reqwest` for private REST.
- AlephBFT peer deliveries run concurrently, return after the required remote
  quorum plus a short completion grace, and durably requeue unfinished peers.
- SQLite shares one async writer gate per database while reads remain
  concurrent. Dropped explicit transactions roll back synchronously before the
  connection can be reused; commit and rollback failures retain ownership for
  deterministic recovery.
- Melt-quote creation is atomic and idempotent. An existing quote is accepted
  only when every consensus-controlled request field matches; mismatched data
  is rejected.
- Startup replay ignores terminal rejected entries, restores durable
  AlephBFT apply status, phases peer-sync startup, and keeps gossip sequence
  watermarks in memory.
- Federation mint-info advertises the startup-validated cached config digest,
  avoiding repeated aggregate BLS interpolation on every info request.

### Clean 11-of-8 verification

A fresh sender/receiver run completed two mints, a send/receive swap, and a
melt. All 11 members converged on nine accepted operations with identical order
and materialized state digests. Logs contained zero private-request timeouts,
database locks, duplicate quote constraints, range mismatches, or HTTP 500
responses.

| Measurement | Result |
| --- | ---: |
| direct `/v1/info` before cached digest | 2.03s |
| direct `/v1/info` after cached digest | 0.04s |
| federated CLI `mint-info` before cached digest | 4.34s |
| federated CLI `mint-info` after cached digest | 2.28s |
| full mint 1 | 17.22s |
| full mint 2 | 11.07s |
| receiver swap | 13.41s |
| melt | 12.77s |

A separate fresh true-AlephBFT run stopped one member, minted with the ten live
members, and restarted the missing member. The initial mint took 7.86 seconds,
the one-member-offline mint took 6.21 seconds, and the restarted member caught
up in about five seconds. All 11 members then reported six operations,
identical order and state digests, healthy readiness, and no timeout, lock,
duplicate, mismatch, or HTTP 500 warning.

The live five-member partition regression was also corrected to model actual
daemon recovery: recovered members rebuild consensus and replay state from the
database, and the test uses the production activity-aware 100 ms cadence rather
than an artificial fixed 10 ms cadence that allowed surviving peers to race
thousands of rounds ahead during a long partition. It passed repeatedly in
isolation and in the full parallel Axum suite.

## Dealer-Free DKG Performance

Milestone 20A starts with a benchmark-only harness over the pure `cdk-common`
DKG state machine. Harness version 2 has no database, REST, AlephBFT, wallet, or
mint daemon dependency. It supports the `5-of-3`, `11-of-8`, and `20-of-14`
profiles; one or eight denominations; phase selection; debug or release builds;
raw samples; and a hard process timeout. The `just dkg-perf` recipe is the
stable entry point, while direct invocation of the already-built example keeps
Cargo compilation outside resource measurements.

The production-code baseline is commit `efa3bca01`. The harness and recipe are
measurement-only additions: no DKG validation, derivation, persistence,
networking, or protocol behavior changed before these measurements.

### Harness-v1 quick baseline

Environment: arm64 development machine, low-power mode disabled. The power
status command reported AC power while the battery was discharging, so absolute
numbers should not be treated as machine-independent budgets. Each phase uses
three raw samples. Process resource totals include fixture construction, all
phase repetitions, and three complete in-memory ceremonies, but exclude Cargo
compilation. macOS `/usr/bin/time -lp` required network/process permission in
the managed sandbox to read process accounting without a sandbox-only `sysctl`
failure.

Commands:

```bash
cargo build -p cdk-common --features federation,bench \
  --example dkg_performance --release

/usr/bin/time -lp target/release/examples/dkg_performance \
  --profile 5-of-3 --denominations one --iterations 3 \
  --timeout-secs 180 --full-ceremony

/usr/bin/time -lp target/debug/examples/dkg_performance \
  --profile 5-of-3 --denominations one --iterations 3 \
  --timeout-secs 300 --full-ceremony
```

| Phase median | Debug | Release | Release speedup |
| --- | ---: | ---: | ---: |
| Fresh participant generation | 1.598s | 67.972ms | 23.5x |
| Persisted reload and validation | 1.162s | 50.105ms | 23.2x |
| Commitment ingestion | 4us | below 1us median | not meaningful |
| Reveal ingestion | 887us | 45us | 19.7x |
| Exact-duplicate commitment/reveal ingestion | 3.798ms | 200us | 19.0x |
| Received secret-share verification | 1.130s | 47.657ms | 23.7x |
| All-member local keyset derivation | 5.704s | 243.407ms | 23.4x |
| Unchanged complete planner tick | 227.590ms | 9.797ms | 23.2x |
| Changed-record complete planner tick | 227.024ms | 9.853ms | 23.0x |
| Final transcript/config derivation | 4.685s | 198.676ms | 23.6x |
| Complete in-memory ceremony | 30.792s | 1.308s | 23.5x |

| Whole-process resource measurement | Debug | Release |
| --- | ---: | ---: |
| Wall | 167.55s | 7.13s |
| User | 165.90s | 7.08s |
| System | 1.17s | 0.04s |
| Average CPU from `(user + sys) / wall` | 99.7% | 99.9% |
| Maximum RSS | 6.85 MB | 5.59 MB |

All five members completed with one transcript hash, keyset ID, aggregate-key
map, and public-share map. Every local secret share mapped to the transcript's
configured public share. The modeled all-to-all ceremony carried about 44.1 KB
of canonical JSON and produced, per member, five commitments, five reveals,
five incoming secret-share records, one keyset-share record, and five transcript
signatures. The phase harness intentionally reports zero REST attempts,
successes, and timeouts; those belong to the optional real-process runner.

### Initial classification

- **Confirmed:** debug cryptography is about 23x slower across every expensive
  phase. Debug-only multi-process observations cannot accept or reject a
  cryptographic optimization.
- **Confirmed:** public/secret polynomial verification and repeated final public
  derivation dominate; commitment/reveal map bookkeeping does not.
- **Not yet decided:** no production optimization candidate is accepted or
  rejected from the 5-of-3 quick profile alone.
- **Next measurements at this checkpoint:** the one-denomination scaling results
  are recorded below. Extend the optional process runner with REST
  responsiveness, attempt/success/timeout, peak CPU, and retry metrics before
  the large-process acceptance gate.

### One-denomination scaling baseline

The committed release harness was then run unchanged at 11-of-8 and 20-of-14.
The 11-of-8 profile completed three repetitions. The 20-of-14 profile was
interrupted after 1,116.72 seconds on operator instruction because it had not
produced one complete report; this is sufficient evidence that the current
implementation fails the quick-profile requirement. The eight-denomination
20-of-14 acceptance run is intentionally deferred until the one-denomination
path completes within the quick gate.

| Release one-denomination measurement | 5-of-3 | 11-of-8 | 20-of-14 |
| --- | ---: | ---: | ---: |
| Complete ceremony median | 1.308s | 32.084s | incomplete |
| Complete ceremony range | 1.306-1.319s | 32.066-32.291s | incomplete |
| Participant generation median | 67.972ms | 614.476ms | no report |
| Reload validation median | 50.105ms | 527.077ms | no report |
| Secret-share verification median | 47.657ms | 514.370ms | no report |
| All-member local derivation median | 243.407ms | 5.582s | no report |
| Final transcript/config derivation median | 198.676ms | 2.347s | no report |
| Whole process wall | 7.13s | 158.18s | 1,116.72s interrupted |
| Whole process user | 7.08s | 155.95s | 1,097.06s |
| Whole process system | 0.04s | 1.23s | 6.56s |
| Maximum RSS | 5.59 MB | 7.60 MB | 14.88 MB |
| Modeled canonical bytes | 44.1 KB | 331.5 KB | no report |
| Final equality checks | all true | all true | not reached |

The 11-of-8 complete ceremony is about 24.5x slower than 5-of-3 despite only
2.2x as many members. The process remains overwhelmingly CPU-bound and memory
use stays modest. This points to repeated cryptographic derivation and
validation rather than record storage, network fanout, or memory pressure. It
does not yet distinguish immutable-state revalidation from repeated public
polynomial evaluation, so the next step is short phase-selected/subphase probes,
not a production optimization chosen from intuition.

Decision: the 20-of-14 eight-denomination baseline is **deferred**, not passed
or failed as a ceremony. The current implementation already fails the preceding
one-denomination quick gate by a wide margin, and no final transcript result was
available from the interrupted run.

### Candidate 1: trust-boundary participant validation

Decision: **Kept** for its intended planner-tick metric.

The production driver previously called the complete persisted participant
validation on every changed-record planner rebuild. That validation rechecks
every private contribution against its public polynomial even though the
participant state is immutable for the life of the driver. Harness v2 compares
the unchanged legacy planner with a non-serializable validated wrapper created
after generation or durable reload. The wrapper owns immutable state through
shared ownership and binds it to the complete validated public-config digest;
the legacy planner still performs full validation for callers without that
trust-boundary proof.

Command shape for both profiles:

```bash
target/release/examples/dkg_performance \
  --profile PROFILE --denominations one --iterations 10 \
  --timeout-secs 120 \
  --phases planner_unchanged,planner_validated_participant --pretty
```

| Release planner measurement | 5-of-3 | 11-of-8 |
| --- | ---: | ---: |
| Legacy full-validation median | 9.842ms | 48.160ms |
| Legacy range | 9.781-9.965ms | 47.820-48.655ms |
| Validated-participant median | 0.223ms | 0.844ms |
| Validated range | 0.218-0.278ms | 0.827-0.962ms |
| Median speedup | 44.1x | 57.1x |
| Complete-fixture setup | 1.248s | 31.370s |
| Correctness checks | all true | all true |

Focused verification passed 30 `cdk-common` DKG tests and 7 permissioned
`cdk-axum` DKG driver/route tests. Coverage includes persisted-state tampering,
changed keyset policy, restart/resume, authenticated member delivery, and exact
legacy/validated plan equality. No ceremony messages, phase ordering,
persistence, REST admission, retries, activation, or cryptographic checks were
changed.

A separate bounded five-member debug process smoke did not complete. Every
member persisted all five commitments and reveals but only four secret-share
contributions; the same final sender timed out against all receivers at the
existing ten-second peer-request deadline. Source tracing showed that the
secret-share route performs final local keyset derivation before committing the
last contribution. Concurrent debug derivation exceeded the request lifetime,
so cancellation prevented that final contribution from becoming durable and
retries repeated the work. The run was interrupted after this was established;
no timeout increase or production behavior change was made.

This process result neither invalidates the planner microbenchmark nor proves a
full-ceremony improvement. It identifies the next bounded work: Candidate 2's
aggregate-polynomial derivation, followed by a release process responsiveness
gate. The 20-of-14 eight-denomination run remains deferred.

### Candidate 2: aggregate public polynomial coefficients

Decision: **Kept** for public-keyset derivation and complete pure-ceremony time.

The original CDK path evaluated every participant's public polynomial at every
member ID and then summed those evaluated points. Harness v3 preserves that
algorithm as a benchmark-only reference. The candidate instead sums each public
coefficient position across participants once, then evaluates the resulting
aggregate polynomial once per member. This is the same algebraic shape used by
Fedimint's G2 DKG result construction, while CDK retains its existing reveal,
roster, threshold, commitment-hash, keyset-policy, and final-keyset validation.

Command shape:

```bash
target/release/examples/dkg_performance \
  --profile PROFILE --denominations one --iterations N \
  --timeout-secs 120 \
  --phases public_keyset_derivation_reference,public_keyset_derivation_aggregated \
  --pretty
```

Ten samples were used at 5-of-3 and 11-of-8; five were used at 20-of-14.

| Release public derivation | 5-of-3 | 11-of-8 | 20-of-14 |
| --- | ---: | ---: | ---: |
| Independent reference median | 38.493ms | 466.559ms | 2.676s |
| Reference range | 38.384-38.865ms | 465.465-473.369ms | 2.676-2.721s |
| Aggregate-coefficient median | 9.919ms | 47.510ms | 143.897ms |
| Aggregate range | 9.684-16.003ms | 47.300-47.732ms | 143.856-144.269ms |
| Median speedup | 3.9x | 9.8x | 18.6x |
| Byte-identical federated keyset | yes | yes | yes |

The independent reference remains in the harness after the production switch,
so future candidate runs cannot accidentally compare the optimized method with
itself. The production path still calls complete reveal validation before
coefficient aggregation. A malformed coefficient-count test now exercises the
full derivation entry point and confirms rejection occurs before aggregation.

Complete one-denomination ceremony measurements use three samples at 5-of-3
and 11-of-8 and one bounded sample at 20-of-14:

| Complete release ceremony | Baseline | Candidate 2 | Improvement |
| --- | ---: | ---: | ---: |
| 5-of-3 median | 1.308s | 0.457s | 2.9x |
| 11-of-8 median | 32.084s | 4.498s | 7.1x |
| 20-of-14 | incomplete after 1,116.72s | 23.725s | now completes |

All complete candidate ceremonies produced one transcript hash, keyset ID,
aggregate-key map, and public-share map, and every local secret share matched
its configured public share. Thirty release-mode `cdk-common` DKG tests and
seven permissioned release-mode Axum DKG driver/route tests passed. The latter
cover authenticated delivery, bounded execution, partition/restart recovery,
transport policy, and changed persisted keyset policy.

This checkpoint makes the one-denomination 20-of-14 quick gate viable. It does
not yet certify the 20-of-14 eight-denomination acceptance gate or prove Axum
responsiveness during a real multi-process ceremony. Those remain explicit
later gates; the interrupted debug process run was not resumed for this
checkpoint.

### Candidate 3: validate fresh participant state by construction

Decision: **Kept** for fresh live-driver participant generation.

The raw participant generator creates secret coefficients, their public G2
commitments, and every recipient share from the same in-memory coefficients.
It then historically repeated the recipient-side verification locally by
turning every secret share back into a public point and reevaluating every
public polynomial. Candidate 3 does not weaken the ordinary public constructor:
`FederationDkgParticipantState::generate` still performs that complete check.
Instead, the live driver can create a non-serializable validated-by-construction
wrapper while the coefficients are still the authoritative source.

Harness v4 measures the complete public constructor and the fresh-driver path
side by side. Before timing the candidate, it independently runs the full raw
validator over a candidate-generated state; failure aborts the report.

```bash
target/release/examples/dkg_performance \
  --profile PROFILE --denominations one --iterations N \
  --timeout-secs 120 \
  --phases participant_generation,participant_generation_validated_by_construction \
  --pretty
```

Ten samples were used at 5-of-3 and 11-of-8; five were used at 20-of-14.

| Release participant generation | 5-of-3 | 11-of-8 | 20-of-14 |
| --- | ---: | ---: | ---: |
| Full self-validation median | 70.569ms | 645.072ms | 3.299s |
| Full-validation range | 69.602-86.056ms | 638.594-691.477ms | 3.267-3.331s |
| Validated-by-construction median | 20.461ms | 106.529ms | 347.621ms |
| Construction range | 20.078-20.884ms | 104.999-116.622ms | 344.617-348.105ms |
| Median speedup | 3.4x | 6.1x | 9.5x |
| Independent full validation | pass | pass | pass |

The optimized proof cannot be serialized. Fresh state is persisted as the same
raw participant record as before; after restart or deserialization, the driver
must construct a new proof with complete config, commitment, receiver, amount,
signer, and secret/public polynomial validation. Incoming transport artifacts
retain their existing receiver-side validation. The public raw constructor also
remains fully validating for all non-driver callers.

Verification passed 31 release-mode common DKG tests and 7 permissioned
release-mode Axum DKG driver/route tests. The focused coverage includes an
independent full validation and serialize/reload/revalidate round trip for the
fresh proof, durable participant equality, restart/resume, tampered private
contribution rejection, changed keyset policy rejection, authenticated member
delivery, and bounded driver execution. No ceremony payload, phase order,
persistence format, transport admission, retry, or activation behavior changed.

### Candidate 4: bound DKG CPU work off the async runtime

Decision: **Kept** for async responsiveness and bounded CPU concurrency. This
candidate makes no cryptographic speed claim.

The live driver had two CPU-heavy operations running directly on async tasks:
fresh participant generation and complete validation after durable reload.
Planner rebuilds and final member-share derivation already used
`spawn_blocking`, but all paths relied only on Tokio's broad blocking-pool limit.
The retained implementation routes all four operations through one
process-local DKG CPU permit and a blocking worker. One mint daemon therefore
runs at most one DKG cryptographic job at a time.

The permit is moved into the blocking closure. Tokio blocking work cannot be
cancelled after it starts; keeping the permit in the awaiting HTTP future would
allow a timeout to release the bound while the abandoned closure continued.
A focused regression aborts the first waiter, proves a second job remains
queued, then releases the first closure and observes the second complete.

A current-thread Axum test compares direct and worker-scheduled generation over
five release samples. It runs the same five-member, threshold-three,
one-denomination generation closure in both modes while issuing an in-process
route probe:

| Five-sample release median | Direct async task | Bounded CPU worker |
| --- | ---: | ---: |
| Generation wall time | 3.944ms | 3.945ms |
| Axum probe latency | 3.971ms | 21us |

Probe latency improves by about 189x while generation time is unchanged within
measurement resolution. This is the intended result: the event loop remains
available, while the cryptographic operation count and total CPU work do not
change. The retained test uses ratio and upper-bound assertions rather than the
recorded absolute values so normal machine variance does not make it brittle.

Twenty-seven permissioned release-mode DKG tests pass. Coverage includes the
two worker/cancellation regressions, complete authenticated setup routes,
participant persistence and restart, conflicting and invalid contributions,
transcript signatures, partition recovery, and peer-client HTTP delivery. No
ceremony data, validation rule, phase ordering, persistence transaction, route
admission, retry policy, or activation behavior changed.

This is a scheduler-level in-process gate. A real multi-process ceremony must
still record process user/system CPU, peak CPU, private REST attempt/timeout
counts, and status/info request latency before Candidate 4 can support the
large-federation acceptance claim.

### Candidate 5: exact-duplicate persistence fast paths

Decision: **Kept** for idempotent DKG replay handling.

Before this candidate, every exact replay followed nearly the same path as a
new artifact. Commitment and reveal replays rebuilt accumulated setup state;
transcript-signature replays recomputed the expected transcript hash; and a
completed secret-share replay loaded the full ceremony and rederived the local
member keyset. The database writes were idempotent, but the cryptographic and
state-reconstruction work had already happened.

The retained path first loads the narrow durable artifact collection. If the
same record is present exactly, it writes only the newer authenticated member
sequence and returns. It does not replace the artifact or derived keyset.
Reveal replays also locate the durable prior commitment and validate the reveal
against it. Secret-share validation and transcript-signature authentication
still happen before duplicate classification. A same-key record with different
content remains a conflict and cannot advance the sequence.

The dominant completed secret-share path was measured with ten release samples
in the same two-member route-derived fixture:

| Completed secret-share replay | Before | After |
| --- | ---: | ---: |
| Median persistence time | 7.579ms | 4us |
| Relative improvement | - | about 1,895x |
| Re-enters DKG CPU derivation | yes | no |

The retained regression acquires the sole DKG CPU permit before issuing ten
exact completed-share replays. Every replay completes inside a 100ms bound,
the sender sequence advances through all ten requests, and the previously
derived keyset-share collection remains byte-identical. This mechanically
proves the fast path does not wait for or invoke final keyset derivation.

Twenty-nine permissioned release DKG tests pass. New coverage sends exact
signed commitment, reveal, secret-share, and transcript-signature requests with
new monotonic sequences, confirms one durable artifact of each type, and checks
the final sequence. Existing tests continue to reject conflicting commitment,
reveal, secret-share, and transcript-signature records before sequence
persistence, along with wrong receiver, cross-ceremony, bad share, bad
signature, and wrong transcript hash cases.

The 1,895x result applies to the completed secret-share hot path, not every
duplicate class. Commitment and reveal duplicates are cheaper to begin with;
their retained value is avoiding repeated state reconstruction and retry-wave
amplification while preserving exact idempotency semantics.

### Candidate 6: suppress acknowledged DKG deliveries

Decision: **Kept** for network/retry amplification.

The DKG plan intentionally carries earlier phase artifacts so lagging peers can
recover. Before this candidate, every tick sent every carried artifact to every
target again, even after receiving and validating that peer's acknowledgement.
In a healthy three-member ceremony this produced 126 private delivery attempts
over five rounds despite only 36 unique deliveries:

- 3 participant commitments to 3 peers: 9
- 3 participant reveals to 3 peers: 9
- 3 participants' private shares to 3 recipients: 9
- 3 participant transcript signatures to 3 peers: 9

The retained tracker is deliberately small and volatile. It stores at most one
commitment broadcast, one reveal broadcast, one transcript-signature broadcast,
and one acknowledged secret-share request per recipient. A payload change
clears that phase's acknowledgement set. The peer client reports success only
after validating the signed response's federation, sender, receiver, message
kind, and artifact owner; only that verified success suppresses a later send.

| Three-member authenticated ceremony | Before | After |
| --- | ---: | ---: |
| Driver rounds | 5 | 5 |
| Delivery attempts | 126 | 36 |
| Unique-delivery overhead | 3.5x | 1.0x |
| Reduction | - | 71.4% |

Timeouts and errors do not create acknowledgements and therefore retry on the
next tick. The tracker is not persisted: restarting the sender constructs an
empty tracker and rebroadcasts all durable artifacts, relying on the exact
duplicate persistence fast paths from Candidate 5 at receivers. A changed-
payload unit test proves prior acknowledgements cannot suppress a new artifact.

Thirty permissioned release DKG tests pass. The authenticated completion test
asserts the 36-attempt minimum and identical final member configs. The existing
partition test proves one successful self-delivery is suppressed while two
partitioned targets remain retryable, then restarts the sender, clears the
partition, and completes with convergent durable records. No acknowledgement is
consensus state, no new REST message exists, and ceremony ordering, validation,
persistence, and activation behavior are unchanged.

### Candidate 7: jittered phase-aware retry backoff

Decision: **Rejected**. No production retry behavior changed.

The preserved logs from an interrupted 20-member, threshold-14,
eight-denomination release process run provide enough evidence to distinguish
retry cadence from the underlying work. The harness started the 20 listeners
sequentially over 38.258 seconds, used a 60-second private-peer request timeout,
and configured a one-second DKG driver retry delay.

| Preserved process observation | Result |
| --- | ---: |
| Total retryable DKG delivery failures | 422 |
| Commitment failures | 382 |
| Secret-share failures | 40 |
| Listener startup span | 38.258s |
| Configured peer request timeout | 60s |
| Configured driver retry delay | 1s |
| Secret-share retry waves | 2 |
| Separation between secret-share waves | about 130s |

Commitment failures correlate directly with target readiness: the earliest
unavailable target failed once, while progressively later targets accumulated
more failures, reaching 52 for the last listener. Once all listeners were up,
the commitment failure series stopped. An all-members-ready barrier is the
direct candidate for that measured startup race.

The 40 secret-share failures were two complete 20-target waves emitted by two
senders around 09:52:33-34 and 09:54:42-44. Their roughly 130-second separation
is dominated by the 60-second request lifetime plus handler/CPU work, not by the
one-second sleep between driver ticks. Small jitter is negligible at that
scale; large exponential backoff would delay recovery, and neither removes the
expensive first-time secret-share processing that made responses ambiguous.

Candidate 6 already prevents acknowledged targets from participating in later
waves. Without evidence that another retry scheduler improves completion or
REST responsiveness, adding it would be speculative complexity. The ceremony
was interrupted and is not an acceptance result; its logs are used only to
reject this retry-policy hypothesis and prioritize the readiness and
first-time-processing investigations.

### Candidate 8: exact all-member readiness barrier

Decision: **Kept** for startup-race elimination. The large process acceptance
gate remains open.

The preserved 20-of-14 release logs showed that commitment failures rose with
the harness's sequential listener order: 382 commitment requests failed while
the 20 listeners came online over 38.258 seconds, and the last target alone
accumulated 52 failures. The driver was spawned while each router was still
being assembled, before the HTTP listener could accept private REST requests.

The retained barrier runs only for a fresh local ceremony. Each member sends a
signed `dkg_readiness` probe to every configured peer. The payload binds the
federation ID, ceremony ID, and complete keyset policy; the receiver accepts it
only when its own validated auto-start configuration is identical. The probe
does not generate participant secrets, persist a DKG record, alter a phase, or
enter consensus.

| Staggered three-member transport probe | First round | After missing route appears |
| --- | ---: | ---: |
| Readiness attempts | 3 | 1 |
| Verified successes | 2 | 1 |
| Unavailable routes | 1 | 0 |
| Durable commitments written | 0 | 0 |
| Previously acknowledged peers reprobed | - | 0 |

Successful readiness responses are cached only in memory for the current
driver invocation. A timeout or error remains pending, and a process restart
forgets volatile acknowledgements. Participant state is generated and persisted
only after the barrier; once it exists, restart skips the barrier and resumes
the durable ceremony even if a peer has since gone offline or activated its
finalized configuration. A crash before participant persistence simply repeats
the safe readiness probes.

Verification passes in release mode:

- 42 `cdk-common` federation transport tests, including signed exact-policy
  readiness round trip and mismatch rejection;
- 32 `cdk-axum` DKG tests, including staggered availability, acknowledgement
  suppression, zero readiness persistence, policy mismatch, restart bypass,
  authenticated completion, partition recovery, and prior invalid-share and
  transcript checks;
- three `cdk-mintd` federation DKG auto-start configuration tests.

This focused result directly proves that unavailable startup targets cannot
receive commitments and that ready targets do not amplify probes. Per the
operator's stop request, the interrupted 20-member process ceremony was not
restarted. Candidate 8 therefore does not certify 20-of-14 completion, CPU/REST
responsiveness, or the separate first-time secret-share bottleneck.

### Candidate 9: bounded redacted delivery diagnostics

Decision: **Kept** for diagnosis and benchmark classification. No speed
improvement is claimed.

Before this candidate every failed DKG request produced the same sender log:
phase and member were visible, but timeout, connection failure, local admission,
remote admission, validation, storage, and response-validation failures were
indistinguishable. The underlying error was discarded, which made the preserved
secret-share waves impossible to classify beyond their timing.

The retained classifier reads existing typed error chains and emits only these
fixed labels:

| Failure class | Evidence source |
| --- | --- |
| `timeout` | typed peer-client or HTTP timeout |
| `connection` | typed HTTP connection failure |
| `outbound_admission` | local bounded outbound queue full/closed |
| `remote_admission` | HTTP 429 |
| `remote_validation` | other HTTP 4xx |
| `remote_storage` | generic DKG database HTTP 503 |
| `remote_server` | other HTTP 5xx |
| `http_status` | remaining HTTP status |
| `response_decode` | response serialization/decoding failure |
| `response_validation` | invalid signed/bound federation response |
| `transport_configuration` | proxy/client construction failure |
| `transport` | other typed transport failure |
| `unknown` | no recognized typed source |

The sender log includes member ID, DKG phase, fixed failure class, and an
optional numeric status only. It never includes an endpoint, error string,
response body, ceremony message, or secret-share payload. On the receiver, DKG
database errors return a generic retryable 503 and other internal processing
errors a generic 500; underlying receiver logs continue through the existing
operator redactor. No response-text parsing is used.

A release regression injects private-looking connection details, proxy
addresses, response bodies, storage paths, payload text, federation-validation
reasons, and unknown messages. Every resulting classified value is checked not
to contain that text. A separate response test proves database and internal
failures map to 503 and 500 respectively. All 34 focused release Axum DKG tests
pass, including the prior partition, restart, invalid-share, transcript,
duplicate, readiness, and completion coverage.

This diagnostic can classify a future process run without temporary payload
logging. It does not retroactively identify the exact receiver-side cause of the
already preserved secret-share timeouts, and it does not close the process
responsiveness gate.

### Candidate 10 baseline: decouple contribution acknowledgement from derivation

Decision: **Kept** for HTTP liveness and earlier invalid-share rejection. No
total-CPU reduction is claimed.

Code tracing showed that the secret-share route calls final local keyset
derivation before committing and acknowledging the last incoming contribution.
The same derivation is already available in the restartable DKG driver from the
complete durable record set. Harness v5 therefore adds two narrow release
phases: verify one contribution against its sender's revealed polynomial, and
derive one receiver's complete local keyset share.

Command shape:

```bash
target/release/examples/dkg_performance \
  --profile PROFILE --denominations eight --iterations ITERATIONS \
  --timeout-secs 120 \
  --phases single_secret_share_verification,single_local_keyset_derivation \
  --pretty
```

Ten iterations were used at 5-of-3, five at 11-of-8, and three at 20-of-14.

| Eight-denomination release phase | 5-of-3 | 11-of-8 | 20-of-14 |
| --- | ---: | ---: | ---: |
| One contribution verification median | 15.094ms | 33.756ms | 56.148ms |
| Verification range | 14.942-15.328ms | 33.686-33.890ms | 55.837-56.329ms |
| One member final derivation median | 159.791ms | 752.266ms | 2.276s |
| Derivation range | 159.170-162.526ms | 751.440-755.082ms | 2.270-2.277s |
| Derivation / verification | 10.6x | 22.3x | 40.5x |

The 20-of-14 fixture setup took 25.054s, but each timed phase remained bounded
and the run used one process with no REST, databases, daemons, or federation
fanout. This does not reproduce the 60-second process timeout by itself. It does
show that the final request carries a multi-second CPU obligation before normal
system-wide contention, while every non-final first-time contribution needs
only the much smaller sender-polynomial verification.

The candidate is to verify each contribution before persistence, commit the
contribution and authenticated sequence atomically, return its acknowledgement,
and let the existing driver derive/persist the final member share from complete
durable records. It can be retained only if invalid shares still fail before
persistence, restart derives the same keyset, transcript/config equality remains
unchanged, and the measured final HTTP path loses the derivation cost.

The retained implementation uses the reveal's shared verifier in participant
state validation, benchmark measurement, and route admission. A first-time
request is verified on the bounded DKG CPU worker before the transaction opens;
only then are the contribution and authenticated sequence committed atomically.
No reveal or secret-share HTTP route derives a member keyset. The existing
driver observes the complete durable record set on its next tick, performs the
same full final derivation and validation, and persists the member keyset before
the transcript-signature phase can advance.

The same profiles were repeated after the change:

| Eight-denomination release phase | 5-of-3 | 11-of-8 | 20-of-14 |
| --- | ---: | ---: | ---: |
| Route-critical verifier median | 15.286ms | 34.399ms | 56.793ms |
| Verifier range | 15.192-15.440ms | 34.115-34.495ms | 56.545-56.848ms |
| Deferred driver derivation median | 161.116ms | 771.336ms | 2.322s |
| Driver derivation range | 160.187-172.720ms | 760.746-783.412ms | 2.297-2.340s |
| Old route derivation / new verifier | 10.5x | 21.9x | 40.1x |
| Route-critical crypto removed | 90.4% | 95.4% | 97.5% |

The comparison is deliberately phase-level: database and HTTP framing overhead
are not included, so these figures are not full request-latency claims. They do
show that the 20-of-14 final contribution no longer holds its HTTP response open
for a 2.276-second uncontended derivation; its required cryptographic admission
work is about 56.8ms before persistence. Under process contention, final driver
derivation can take longer without turning an already-durable contribution into
an ambiguous sender timeout.

Total CPU can increase because every share is now rejected early if invalid and
the driver later revalidates the complete deserialized record set. That duplicate
trust-boundary validation is intentional for restart and database-tamper safety.
The retained claim is request liveness plus fail-before-persistence security,
not faster complete ceremony time.

Verification: 32 common release DKG tests and 34 permissioned release Axum DKG
tests pass. Coverage includes a bad first contribution leaving no durable share
or sequence, final contribution acknowledgement before keyset persistence,
driver derivation from complete durable records, restart activation with the
same keyset and transcript, authenticated multi-member completion, partition
recovery, duplicates, malformed shares, and transcript failures.

### Bounded DKG ceremony admission

Decision: **Kept** as a resource/liveness bound. No complete-ceremony speed
improvement is claimed, and the interrupted 20-member process run was not
restarted.

The post-Candidate-10 audit found three distinct gaps around otherwise bounded
cryptographic work:

- dealer-free keyset policy accepted any non-empty denomination count;
- the shared federation config format allowed a dealer-free ceremony to scale
  its all-to-all state without a DKG-specific roster cap;
- mintd had a broad 1 MiB body cap, but the reusable Axum federation router did
  not independently protect DKG routes, and setup handlers did not all bind
  incoming requests to the active local ceremony before ingestion.

The retained bounds are 64 members, 64 denominations, and 1 MiB of decoded JSON
per private DKG route. Sixty-four conventional power-of-two keys cover the full
`u64` Cashu amount domain. At the maximum supported reveal shape, 64 amounts by
64 compressed G2 coefficients plus the signed REST wrapper serialize to
800,039 bytes. That is 76.3% of the body limit, leaving 248,537 bytes of
headroom. The measurement is a single deterministic release serialization
probe, not a latency benchmark.

All DKG routes now require an active auto-start ceremony. Signed commitments
must match its ceremony ID; reveals must also match its complete keyset policy
and exact amount map; secret-share requests must match its ceremony and amount
set. These shape checks run after member authentication but before expensive
public-polynomial/share verification or database mutation. Transcript
signatures retain their existing locally derived transcript-hash check and are
now unavailable outside an active ceremony as well.

Verification:

- 35 `cdk-common` release DKG tests pass, including 65-member and 65-amount
  rejection plus the maximum-shape wire-size assertion;
- 40 permissioned `cdk-axum` release DKG tests pass, including inactive-route,
  wrong-ceremony,
  wrong-policy, and a valid signed request padded over 1 MiB returning HTTP 413
  without sequence or DKG-record persistence;
- three `cdk-mintd` release startup tests pass, including the explicit
  operator-facing amount-limit error;
- the temporary wire-size print used to record 800,039 bytes was removed.

This checkpoint prevents oversized setup work and cross-ceremony route
ingestion. It does not yet provide fair per-member concurrency/rate limiting,
encrypted-at-rest DKG material, mTLS/pinning, a durable activation barrier, or
abort/reconfiguration semantics.

### Fair authenticated-member DKG admission

Decision: **Kept** as a fairness and mutation-serialization bound. No speed
improvement is claimed.

The prior global private-REST semaphore limits aggregate in-flight work to 128
requests, but all authenticated members share it. It therefore could not prove
that one member's long-running or duplicated DKG request left an independent
slot for another member after authentication. DKG phase delivery is sequential
per sender and receiver, so concurrent state-mutating requests from the same
sender are not needed for healthy progress.

Each receiver now constructs one nonblocking DKG semaphore slot for every
member in its validated roster. All five setup routes authenticate the signed
request first, then acquire the sender's slot before active-ceremony validation,
cryptographic work, or persistence. A second in-flight request from that sender
returns HTTP 429 immediately. There is no wait queue, timer, retry-policy
change, durable admission state, ceremony message, or consensus traffic.

The focused three-member release regression manually holds one sender's slot.
That sender's signed commitment receives HTTP 429 and leaves its sequence and
commitment absent; a second sender's commitment succeeds while the first slot
is held; after release, the first sender's exact sequence-1 request succeeds.
Active ceremony state without the per-member map fails closed with HTTP 503.
The complete focused Axum DKG suite passes 40/40 tests. This establishes
concurrency fairness and same-sender mutation serialization, not a sustained
requests-per-second policy; authenticated rate limiting remains open.

### Private DKG file-at-rest permissions

Decision: **Kept** as a secret-file hardening checkpoint. No performance claim
and no complete database-encryption claim are made.

The secret inventory found four generated filesystem classes outside the mint
database:

- final and setup-only member private JSON contain identity keys and, after
  completion, final key shares;
- generated member TOML contains a mnemonic and can contain database
  credentials;
- optional signed DKG route-probe JSON includes private scalar contributions;
- finalized DKG export already used its own mode-`0600` writer.

The first three classes previously used the generic writer and inherited the
process umask. They now use one dedicated private writer. On Unix, a new file is
opened with `create_new` and mode `0600`. A forced overwrite writes and syncs a
new mode-`0600` file in the same directory, then atomically renames it over the
destination. A regression places a symlink at the destination and proves force
replacement leaves the symlink target unchanged. Mint startup independently
rejects a private federation config with any group/other permission bits.

Verification: all 13 release local federation-setup tests, both backend-agnostic
core-generator tests, and all six focused release federation-startup tests
pass. Coverage checks every generated member private JSON, setup-only JSON,
normal/setup/live TOML, and signed route-probe file; forced overwrite repairs
mode `0644`; permissive startup fails while valid runtime configs still load.

Residual risk is explicit. SQL rows
`federation_dkg_participant_states`,
`federation_dkg_secret_share_contributions`, and
`federation_dkg_member_keyset_shares` contain plaintext JSON. Database
encryption at rest is intentionally out of scope and is not required for
Milestone 20A. Non-Unix ACL verification remains separate future hardening
work; no backup or backup-encryption requirement belongs to this milestone.

### Sustained authenticated-member DKG admission

Decision: **Kept** as a resource-bound robustness checkpoint, with no speed
claim. The prior one-in-flight-request semaphore prevented concurrent mutation
by one authenticated sender but allowed an unlimited sequence of requests as
soon as each permit was released.

Each roster member now has an independent token bucket attached to that same
nonblocking admission record. The production default allows a 16-request burst
and restores one token every 250ms. Mint startup rejects a burst outside
`1..=64` or a refill interval below 10ms. The limiter exists only while a
validated live DKG ceremony is active and is checked after signed-roster
authentication but before ceremony payload validation, CPU admission, sequence
advancement, or database mutation. Exhaustion returns retryable HTTP 429 and
does not consume another member's capacity.

The bounded-work accounting is now explicit: no more than 64 per-member DKG
requests can be in flight or waiting behind the single DKG CPU worker, the
existing private REST admission caps aggregate work at 128, each route body is
at most 1 MiB, and SQLite's shared database write gate admits one explicit
transaction writer. There is no new queue, background task, durable rate
record, ceremony message, or retry phase.

Focused verification:

- 42/42 release `cdk-axum` DKG tests pass. The new route regression exhausts
  one sender, proves its sequence and DKG records do not change, admits an
  independent sender, and accepts the exact first sender retry after refill.
- 3/3 release `cdk-mintd` DKG auto-start policy tests pass, including invalid
  zero/excessive burst and sub-10ms refill configurations.
- 13/13 release local federation-generation tests pass with the bounded policy
  emitted in every generated live-DKG member config.

### All-member DKG activation barrier

Decision: **Kept** as a safety checkpoint, with no speed claim. The prior
driver called a ceremony complete as soon as one member had all transcript
signatures. Every signature proved agreement with the same public transcript,
but it was created before its signer knew that every other signature had
arrived. A member with the luckiest network view could therefore restart and
advertise the keyset while another remained setup-only.

The planner now inserts a final `Activation` phase. After independently
assembling the complete finalized config, each member signs a domain-separated
binding over federation ID, ceremony ID, signer, transcript hash, and final
config digest. The transcript commits roster, threshold and keyset policy,
keyset ID, aggregate keys, and all member public shares; the config digest also
commits the exact complete signed transcript. The existing authenticated REST
driver broadcasts the confirmation to every member and persists one exact
record per ceremony/member. Only all roster confirmations produce `Complete`.
This adds no AlephBFT item, consensus phase, wallet request, or cryptographic
key derivation.

Validation reconstructs the signed setup-time finalized config from the
transcript before checking durable confirmations. Later permitted keyset-state
changes therefore do not invalidate the original activation evidence.

At three members, the verified acknowledged-delivery minimum increases from 36
to 45, exactly one additional all-to-all artifact. Activation payloads remain
far below the existing 1 MiB DKG body bound and share the per-member in-flight
slot and token bucket. Exact duplicates only advance the authenticated sender
sequence; conflicting payloads fail before replacement.

Startup and operator gates are fail-closed. Setup-record activation waits while
one member confirmation is missing. A directly supplied finalized Pedersen-DKG
config cannot bypass the database evidence. `dkg-status --require-ready` and
final-config export use the same full-roster validator. Trusted-dealer and
standalone paths do not require these dealer-free activation records.

Focused verification:

- 35/35 release `cdk-common` DKG tests cover the new phase, deterministic local
  confirmation, all-member completion, and tamper rejection.
- 43/43 release `cdk-axum` DKG tests complete over authenticated routes, resume
  after partition/restart, and prove a tampered confirmation changes neither
  sender sequence nor durable activation records.
- Five focused release mint-builder tests prove full activation, missing-member
  fail-closed behavior, incomplete transcript behavior, member mismatch, and
  rejection of a finalized-config bypass.
- 13/13 mintd DKG status tests and 13/13 generated-config/script tests pass.
  Six SQLite DKG persistence tests and SQLite/Postgres migration parity pass;
  the focused Postgres activation persistence test compiles with `--no-run`.

### Multi-ceremony durable record scope

Decision: **Kept** as the storage prerequisite for explicit abort and
replacement, with no speed claim. Commitment/reveal rows are now unique by
ceremony, phase, and member; private contribution rows are unique by ceremony,
sender, and receiver. Participant state and activation evidence already carry
ceremony keys, while derived keyset IDs and transcript hashes retain their
cryptographic natural scope.

The live DKG driver filters loaded setup, contribution, keyset-share, and
activation records to its configured ceremony before planning. A focused
authenticated three-member regression seeds prior-ceremony records at the same
member coordinates, then completes the current ceremony in the unchanged 45
delivery attempts. Six release SQLite DKG persistence tests prove exact replay,
same-ceremony conflict rejection, and cross-ceremony coexistence; SQLite and
Postgres migration parity also passes.

Because federated DKG remains pre-deployment, the base schema was corrected
directly. Disposable databases created by earlier development builds must be
recreated; no compatibility migration or backup workflow is part of Milestone
20A.

### Explicit ceremony abort and replacement

Decision: **Kept** as a liveness and auditability checkpoint, with no speed
claim. Every local database records a ceremony as `active`, `aborted`, or
`complete`, binding its federation, local member, keyset policy, predecessor,
and successor. An unfinished ceremony can change only when configuration names
both a fresh ceremony ID and the exact `replaces_ceremony_id`.

Startup performs replacement as one compare-and-set transaction: the prior row
becomes `aborted` with the new successor and the new row becomes `active` with
the prior predecessor. Implicit replacement, stale expected state, conflicting
policy, and attempts to abort a completed ceremony fail closed. A fresh member
introduced by an explicit roster change may name a predecessor that is absent
from its new local database; members retaining the prior database must match
the durable chain. The new roster must still pass the existing exact all-member
readiness barrier, so replacement cannot silently reduce the dealer set.

The driver compare-and-sets a registered active ceremony to `complete` only
after the all-member activation phase. Startup and status select the unreplaced
active/complete ceremony and ignore all aborted artifacts while retaining them
for audit. This adds no AlephBFT item or DKG message. Focused persistence,
mintd restart/replacement, stale-activation, authenticated completion, and
operator-status tests cover the transitions.

Current verified gate: 50/50 release Axum DKG tests, 19/19 release mintd DKG/status
tests, six release builder DKG tests, seven release SQLite DKG persistence
tests, migration parity, and a successful Postgres lifecycle test compile with
`--no-run`.

### Failure matrix: restart and ambiguous acknowledgement

Decision: **Kept** as the first production failure-matrix checkpoint, with no
speed claim. One authenticated three-member ceremony restarts the same member
at every durable driver boundary: commitment, reveal, secret-share delivery,
local keyset-share persistence, transcript signature, and activation. It still
completes with the same finalized configuration and no regenerated participant
secret state.

A separate test transport delivers one commitment POST to the receiver, waits
for its successful durable response, then discards that response. The sender
records one ambiguous delivery failure and retries; the receiver takes the
exact-duplicate path, all members converge, and the ceremony uses 46 delivery
attempts instead of the 45 baseline while retaining exactly one commitment per
roster member. This directly covers timeout/response loss after durable
persistence without adding a production probe or retry branch.

At this checkpoint, the remaining matrix cases were a deterministic slow-member
deadline and actual SQLite write-failure rollback; both are addressed below.

### Slow-member deadline isolation

Decision: **Kept** as a deterministic liveness checkpoint, with no speed claim.
The test transport delays one member's commitment delivery for 100ms while the
sender uses a 20ms authenticated peer deadline. The two fast members persist
the commitment, the delayed receiver has no partial mutation, and the tick
reports exactly that member as failed. Once the one-shot delay is consumed, the
normal retry catches the slow member up and all three members complete with the
same finalized configuration.

### SQLite write failure and recovery

Decision: **Kept** as the final production failure-matrix checkpoint, with no
speed claim. A disposable file-backed SQLite database installs an engine-level
abort trigger on `federation_member_sequences`. The DKG commitment insert runs
first inside the same transaction, the sequence write then fails, and explicit
rollback leaves both the commitment table and sequence watermark unchanged.
After removing the trigger, the exact same commitment and sequence retry commits
successfully.

This tests the multi-write atomicity and deterministic rollback path used by DKG
routes under a real SQLite statement failure, which is the relevant recovery
boundary for write exhaustion as well. It requires no production fault API and
logs no DKG scalar material. With the phase restart, ambiguous acknowledgement,
partition, slow-member, malformed-share, conflict, transcript, partial
activation, and replacement cases, the focused Milestone 20A production failure
matrix is complete.

### Confidential DKG transport policy

Decision: **Kept** with the simple CDK policy. Production DKG uses the default
HTTP transport: WebPKI certificate verification remains enabled, invalid
certificates are never accepted, and redirects are disabled. Federation config
commits each HTTPS endpoint and identity public key, while every private DKG
request is signed by that configured identity. TLS therefore provides
confidentiality and server endpoint authentication; federation signatures
provide exact application peer authentication.

The initial protocol does not add mandatory mTLS or a second static TLS-key pin
set. Operators may enforce mTLS or SPKI policy at their controlled TLS proxy,
but protocol identity remains the signed federation roster. Explicit plaintext
is confined to the existing all-loopback development exception. The 1 MiB body
cap, per-member in-flight slot, and per-member token bucket remain independent
of TLS termination.

### Fedimint comparison boundary

CDK adopts the safety properties that matter: commit before reveal, all-dealer
participation, immediate share verification, deterministic aggregate keys,
all-peer final-config equality, and fail-closed activation. It simplifies the
mechanics into a restartable planner, durable mint-database records, and signed
private REST using the federation's existing identity keys.

CDK deliberately does not copy Fedimint's setup UI/P2P state machine or require
iroh for setup. It also rejects threshold-only dealer completion, silent roster
shrinkage, non-loopback plaintext shares, persistent volatile delivery
acknowledgements, and an extra consensus phase for setup failures. The retained
design reaches the same relevant safety boundary with fewer moving parts in the
Cashu server architecture.

### Large-process accounting collector

Decision: **Kept** as opt-in benchmark tooling, with one bounded validation
result and no large-acceptance result yet.
`DKG_CAPTURE_METRICS=1` on the generated live-DKG runner now records setup wall
time and one CSV sample stream per process containing member, PID, CPU percent,
RSS, user CPU time, and system CPU time. A second stream probes the primary
member's `/v1/info` HTTP status and latency throughout setup. The runner emits
average/peak CPU, peak RSS, total user/system CPU, average/peak REST latency,
and exact per-tick delivery attempts/successes/failures, timeout counts, and
duplicate successes across interrupted process invocations. User and system CPU
totals take the maximum observed value for each PID and sum those values per
member, so the intentional setup restart does not discard the first process
incarnation's work.

Member logs append across the intentional interruption/restart instead of being
overwritten. Metrics stop at the setup completion boundary before activation
restart and wallet smoke, so process accounting is scoped to DKG. The collector
is disabled by default, contains no ceremony scalars, changes no DKG message or
retry, and generated scripts are syntax-checked with `bash -n` in the focused
generator test. Per the operator's stop request, the 20-member process ceremony
was not rerun; the acceptance measurement remains explicitly open.

The first release collector validation used five members, threshold three, and
the standard eight denominations. It exposed and fixed two harness-only races:
the old smoke waited for participant state while one member was held before the
new all-member readiness barrier, and the background sampler inherited a stale
shell PID array before exiting under `pipefail` when the interrupted PID died.
The corrected smoke crosses readiness with the full roster, catches a durable
pre-keyset checkpoint at 50ms cadence, tracks current PIDs through files shared
with the sampler, takes a final sample before every stop, and waits for every
opt-in driver metric stream to flush. No production ceremony behavior changed.

Verified result:

| Member | Avg CPU | Peak CPU | Peak RSS KiB | User s | System s | Attempted | Successful | Failed | Duplicate successes |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 57.45% | 135.50% | 42,352 | 7.65 | 0.07 | 29 | 25 | 4 | 0 |
| 2, interrupted/restarted | 48.99% | 143.80% | 41,376 | 7.32 | 0.07 | 30 | 30 | 0 | 5 |
| 3 | 71.76% | 112.40% | 41,648 | 7.58 | 0.06 | 29 | 25 | 4 | 0 |
| 4 | 45.79% | 146.80% | 41,712 | 7.22 | 0.05 | 29 | 25 | 4 | 0 |
| 5 | 46.46% | 99.50% | 41,744 | 6.37 | 0.05 | 29 | 25 | 4 | 0 |

Setup wall time was 10 seconds. Aggregate process CPU was 36.14 user seconds
and 0.30 system seconds. Delivery totals were 146 attempted, 130 successful,
and 16 expected connection failures while member 2 was offline, with zero
timeouts. The five duplicate successes are exactly the interrupted member's
commitment broadcast after its intentionally volatile acknowledgement tracker
reset. `/v1/info` produced one pre-listener startup miss followed by nine
successes, zero runtime failures, 98.296ms mean latency, and 499.509ms peak
latency. All members finalized the same transcript/keyset and eight local public
amount shares. Activation and the standard mint/swap/melt/restore smoke then
passed in 5.045 seconds. CPU percentages above 100% represent bounded
multi-threaded cryptographic work, not sampling error.

Current focused release verification: all 50 Axum DKG
tests and all 15 mintd federation-setup/generator tests passed. The generated
scripts also pass `bash -n` through the generator regression.

### Candidate 11: cache finalized config during activation

Decision: **Kept**. The first release 11-of-8, eight-denomination process run
crossed readiness and reached activation, but did not complete. Each receiver
called `FederationDkgFinalizedMemberConfig::from_records` independently for all
11 concurrent activation senders. Those handlers held their per-sender slots
past the ten-second peer deadline; ambiguous retries then received HTTP 429 or
timed out again. A representative member entered activation at 16:37:18,
recorded its first full timeout wave at 16:37:29, and still had no completed
driver metric when the bounded harness stopped after 16:38:50. Its log contained
one 25ms SQLite slow commit, while live info probes remained responsive. This
isolates repeated finalized-config validation as the dominant activation
bottleneck rather than SQLite serialization or general Tokio starvation.

The retained change adds one fallible process-local `OnceCell` to the existing
DKG admission state. The first request after complete keyset-share and transcript
signature records performs the unchanged full public/private validation.
Concurrent requests await and clone that validated immutable result. A lookup
before transcript completion returns its normal error and leaves the cell empty.
Activation request signatures, finalized-config digest binding, conflicting
activation rejection, authenticated sequence advancement, and atomic activation
record persistence remain per request and unchanged.

The identical fresh rerun completed the interrupted/restarted setup in 34
seconds:

| Release 11-of-8 process result | Before cache | After cache |
|---|---:|---:|
| Setup outcome | failed in activation | complete |
| Observed activation interval | over 90s and unfinished | included in 34s total setup |
| Delivery timeouts | repeated full/partial waves | 0 |
| Post-start `/v1/info` failures | 0 | 0 |
| Final transcript/config equality | databases converging; drivers unfinished | all 11 equal |
| Wallet mint/swap/melt/restore | not reached | passed in 7.963s |

After-run resource totals were 345.53 user CPU seconds, 1.92 system CPU
seconds, and 52,864 KiB maximum per-member RSS. The collector recorded 636
artifact attempts, 616 successes, 20 expected connection failures during the
deliberate member-2 interruption, zero timeouts, and 11 exact duplicate
successes after that member's volatile acknowledgement tracker reset. The info
probe recorded one pre-listener miss followed by 31 successes, 12.551ms mean
latency, 90.584ms peak latency, and zero runtime failures. All members exported
the same transcript hash, keyset ID, aggregate keys, and eight local public
amount shares. The full focused release suite is 50/50 Axum DKG tests, including
early-cache-failure recovery and pointer-identical reuse.

## Recommended Work Order

1. Implement immediate post-quote payment observation and benchmark the saving.
2. Add AlephBFT latency instrumentation in a non-production benchmark harness.
   Current checkpoint: single-operation, three-operation, duplicate-fanout, and
   five-member and seven-member live-session stage timing are implemented. Fast
   in-process two-member finality/apply/waiter completion is around 50-80ms per
   operation; the five-member threshold-three probe returned the local waiter
   around 1.0-1.3s, and the seven-member threshold-five probe returned it around
   2.5-3.3s. Default-cadence cold/warm timing now shows the idle-tail
   data-provider wait directly: about 2.0s before the cold operation yields,
   versus about 0.48s for the immediate follow-up.
3. Tune AlephBFT proposal/finality intervals and verify no CPU spin. Completed:
   active work uses 100ms unit creation, duplicate consensus-data gossip cannot
   refresh activity, the data provider waits instead of producing empty idle
   units, and finalized peer activity keeps a one-second propagation tail.
   Fresh idle members use about 0.9% CPU and post-work debug members about
   4-5%; the old 27-32% sustained idle load and unbounded DAG growth are gone.
   Fresh/warm mints complete in about 5-8s, and one-member offline
   restart/catch-up converges exactly. Cold work can still wait on one
   already-scheduled 2s idle interval.
4. Cache validated federation configs and aggregate keysets in the wallet/CLI.
   Current checkpoint: startup activation now builds one federated connector per
   imported federation and reuses it across all active units in that command
   process. Remaining work is cross-process validated-config/digest caching if
   later measurements still show CLI registry validation as meaningful.
5. Re-benchmark two full mints and one receiver-side receive/swap after every change.
   Current checkpoint: two corrected full mints completed in 8.20s and 5.45s;
   receiver-side receive completed in 7.92s with exactly one accepted swap.
6. Only after the above, evaluate whether `mint_quote_payment` and `mint` can be safely merged or linked into a fewer-round design.
