# Federated Cashu Minting and Receive Performance Benchmark

Date: 2026-07-09

This note records benchmark results from fresh 5-member, threshold-3 fakewallet
AlephBFT federations and fresh CLI wallets. The first run identified why
federated minting took tens of seconds. The second run verifies the immediate
post-quote payment-observation fix and keeps the remaining delay classified.
The third run verifies pending-work-aware AlephBFT tail pacing after idle gossip
was excluded from the activity hint. The latest run accepts a 100ms active-only
cadence, deduplicates replayed consensus-data wakeups, and verifies both faster
minting and low idle CPU.
The next benchmark target is receiver-side token receive, which redeems a
received token by performing a federated swap.

The measurements below are development-environment measurements. Absolute values will vary by machine, but the stage ordering and relative delays are the important result.

## Benchmark Setup

- Federation: 5 members
- Threshold: 3
- Payment backend: fakewallet
- Consensus backend: AlephBFT
- Wallet: fresh `cdk-cli` work directory
- Operation tested: two full federated mints after adding the federation registry
- Receiver-side token receive/swap benchmark: required, not yet measured
- Instrumentation policy: targeted timing/log instrumentation is allowed when
  it is useful to isolate delays. It must not change protocol behavior, signing
  semantics, payment semantics, consensus ordering, or wallet fanout behavior.
  Temporary instrumentation should be removed before finishing unless it is
  intentionally kept as normal low-noise diagnostics.
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

This closes the observed post-work CPU issue and accepts the active-only cadence
change. The next performance target is receiver-side receive/swap.

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
- Receiver-side receive/swap: not yet measured in this benchmark document. It
  must be measured independently because it should normally require one
  consensus operation instead of three.

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

Status: required next measurement; no authoritative timing has been recorded in
this document yet.

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

Required receive timeline:

| Stage | Timestamp UTC | Delta From Previous | Notes |
| --- | --- | ---: | --- |
| receiver CLI start | TBD | - | command invocation |
| token decoded and federation wallet selected | TBD | TBD | wallet/CLI setup and federation registry lookup |
| swap request received by mint | TBD | TBD | public `/v1/swap` request from receiver wallet |
| swap consensus accepted | TBD | TBD | accepted `swap` operation in AlephBFT journal |
| input proofs spent and output shares persisted | TBD | TBD | deterministic apply/materialization complete |
| shares returned to receiver | TBD | TBD | threshold member responses available |
| receiver aggregation done | TBD | TBD | receiver stores fresh proofs and command exits |

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

This gives a normal warm receive target around 3-4s and a hard target below
10s. If receive is slower than 10s, the benchmark should identify whether the
extra time is before swap submission, inside consensus, or after accepted swap
application.

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
   refresh activity, fresh mints complete in about 8-9s, idle members cool to
   0.3-0.7% CPU, and one-member offline restart/catch-up converges exactly. Cold
   work can still wait on one already-scheduled 2s idle interval.
4. Cache validated federation configs and aggregate keysets in the wallet/CLI.
   Current checkpoint: startup activation now builds one federated connector per
   imported federation and reuses it across all active units in that command
   process. Remaining work is cross-process validated-config/digest caching if
   later measurements still show CLI registry validation as meaningful.
5. Re-benchmark two full mints and one receiver-side receive/swap after every change.
6. Only after the above, evaluate whether `mint_quote_payment` and `mint` can be safely merged or linked into a fewer-round design.
