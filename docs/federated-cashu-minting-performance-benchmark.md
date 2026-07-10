# Federated Cashu Minting and Receive Performance Benchmark

Date: 2026-07-09

This note records benchmark results from fresh 5-member, threshold-3 fakewallet
AlephBFT federations and fresh CLI wallets. The first run identified why
federated minting took tens of seconds. The second run verifies the immediate
post-quote payment-observation fix and keeps the remaining delay classified.
The third run verifies pending-work-aware AlephBFT tail pacing after idle gossip
was excluded from the activity hint. Later runs accept a 100ms active-only
cadence, deduplicate replayed consensus-data wakeups, stop empty-unit creation
when no operation is active, and remove an O(history) finalized-item scan from
peer status. A receiver-side token receive benchmark is also complete.

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
