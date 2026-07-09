# Federated Cashu Minting Performance Benchmark

Date: 2026-07-09

This note records benchmark results from fresh 5-member, threshold-3 fakewallet AlephBFT federations and fresh CLI wallets. The first run identified why federated minting took tens of seconds. The second run verifies the immediate post-quote payment-observation fix and keeps the remaining delay classified.

The measurements below are development-environment measurements. Absolute values will vary by machine, but the stage ordering and relative delays are the important result.

## Benchmark Setup

- Federation: 5 members
- Threshold: 3
- Payment backend: fakewallet
- Consensus backend: AlephBFT
- Wallet: fresh `cdk-cli` work directory
- Operation tested: two full federated mints after adding the federation registry
- Mint operation sequence confirmed in the journal:
  - `mint_quote`
  - `mint_quote_payment`
  - `mint`

Each mint currently requires three serialized consensus operations. For two mints, all members converged on six accepted journal operations in the same order:

1. `mint_quote`
2. `mint_quote_payment`
3. `mint`
4. `mint_quote`
5. `mint_quote_payment`
6. `mint`

Finalized item count was higher than accepted operation count because wallet/member fanout causes duplicate finalizations. The duplicates did not change the accepted journal order.

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

- Add a non-production benchmark harness for AlephBFT operation latency:
  - one operation
  - three sequential operations
  - duplicate fanout submissions
  - 5-member and 7-member federations
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

## Recommended Work Order

1. Implement immediate post-quote payment observation and benchmark the saving.
2. Add AlephBFT latency instrumentation in a non-production benchmark harness.
3. Tune AlephBFT proposal/finality intervals and verify no CPU spin.
4. Cache validated federation configs and aggregate keysets in the wallet/CLI.
5. Re-benchmark two full mints after every change.
6. Only after the above, evaluate whether `mint_quote_payment` and `mint` can be safely merged or linked into a fewer-round design.
