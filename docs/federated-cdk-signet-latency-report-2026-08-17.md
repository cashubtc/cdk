# Federated CDK Signet latency study

Date: 2026-08-17

## Executive summary

The five-member Dublin federation is healthy and can complete all tested flows:

- bidirectional ecash swap;
- incoming Bark Bolt11 mint;
- outgoing Bark Bolt11 melt;
- incoming BDK on-chain mint; and
- outgoing BDK on-chain melt.

The dominant delays are different for each backend:

1. BDK end-to-end latency is dominated by Signet block production. Incoming minting waited about 17 minutes for a block. The outgoing melt broadcast in about 145 seconds, then waited about 16.6 minutes for confirmation.
2. Bark outgoing latency is split between an avoidable coordinator pickup delay of about 15 seconds and about 22 seconds in the external Ark/Lightning settlement path.
3. Bark incoming latency is dominated by the external payer/Ark path. CDK completed its post-payment work in about 6 seconds.
4. Normal Cashu operations spend about 1.0–1.6 seconds in AlephBFT consensus. Most of that interval is proposal-to-finalization, not local application work.
5. BDK recovery attempted FROST finalization far too aggressively while its parent input was not yet locally spendable: 428 commitment rounds, 428 signature rounds, and 212 failed finalizations during one 19-minute confirmation wait.
6. The outgoing BDK test exposed a federated wallet race: a single member's WebSocket notification could carry an unaggregated change-signature share. The payment itself succeeded and wallet saga recovery restored valid change, but the first client call failed proof verification. The wallet now confirms paid federation notifications through the threshold-aggregated HTTP status path.

The highest-value CDK changes are therefore: gate and back off BDK recovery, wake Bark coordination immediately, keep backend observation isolated, and retain phase metrics while disabling verbose traces in normal operation. Tuning AlephBFT below its current interval should come only after sustained load testing because consensus is not the dominant delay in either payment backend.

## Test system

- Five Ubuntu 24.04 VPS members in one Dublin region.
- BFT parameters: five members, four required for consensus safety/liveness under the deployed configuration.
- Client transport: Iroh, not HTTPS.
- Chain: Bitcoin Signet through the configured Esplora source.
- Lightning/Ark: Bark against the configured Signet Ark server.
- Mint process: `cdk-mintd` only; no server-side TUI wallet.
- Client: local `cdk-cli`, with a local Wavelength wallet used as the external payer/recipient.

Nodes are identified only as `cdk-dublin-1` through `cdk-dublin-5`. This report intentionally excludes addresses, payment hashes, quote identifiers, federation invitation material, and server IPs.

## Instrumentation

The mint now emits a low-cardinality Prometheus histogram:

```text
cdk_federation_transaction_phase_duration_seconds
```

Labels are limited to `backend`, `direction`, `phase`, and `outcome`. Correlation identifiers are emitted only to structured trace events so they cannot create unbounded Prometheus cardinality.

The structured phase timer records exact microseconds and milliseconds for:

- Cashu swap, mint, and melt handlers;
- payment observation and backend probes;
- Bark incoming evidence, secret replication, receive claim, and acknowledgement;
- Bark outgoing planning, HTLC cosigning, initiation, and terminal observation;
- BDK address allocation, chain synchronization, fee estimation, PSBT construction, proposal preparation, finalization, broadcast, and confirmation;
- FROST commitment and signature rounds;
- federation submission, notification, and waiter completion;
- AlephBFT data-provider, network-pump, finalization, apply-loop, and application phases; and
- backup probing and recovery.

AlephBFT trace events provide a correlated sequence:

```text
submit queued
  -> data provider yielded
  -> finalized data received
  -> apply loop received
  -> apply completed
  -> operation notified
  -> waiter returned
```

Network-pump events record accepted messages, requeues, failures, and per-pump duration. These traces are useful for a bounded diagnostic window but are intentionally too verbose for continuous production use.

During analysis, a provisional BDK proposal-queue metric treated a consensus sequence number as a Unix timestamp. That produced an obviously impossible queue age. The invalid phase was removed before the final build and is excluded from every result below. No consensus data or wire format was changed.

## Method and interpretation

- Wall-clock time was measured at the CLI boundary.
- Backend timings came from correlated member traces.
- CPU values are process CPU deltas sampled around each operation. They include normal background work and the tracing overhead, so they are upper bounds rather than pure transaction cost.
- Per-member phase distributions are not additive. Concurrent members measure the same critical path from different viewpoints.
- “Consensus” below means submission to the operation waiter returning. “Provider to finalized” isolates the AlephBFT ordering interval. “Apply” is local deterministic application after finalization.
- Signet block time is external to CDK. Both end-to-end and pre-confirmation timings are reported so CDK latency is not confused with block production.

The diagnostic TRACE configuration increased idle CPU materially. It was enabled only for measurement and removed after collection.

After the corrected build was deployed, a 100-sat ecash round trip was run as a clean smoke test. The forward receive completed in 1.91 seconds and the reverse receive in 2.05 seconds. Both wallets returned exactly to their starting balances, confirming that the deployed members served Iroh requests and that the resulting threshold proofs remained spendable.

## Results overview

| Flow | Amount | Client wall time | Principal delay | Result |
|---|---:|---:|---|---|
| Ecash send | 1,000 sat | 0.29 s | Local token selection/serialization | Success |
| Ecash receive | 1,000 sat | 2.86 s | Swap consensus | Success |
| Ecash receive | 500 sat | 2.21 s | Swap consensus | Success |
| Reverse ecash receive | 500 sat | 2.35 s | Swap consensus | Success |
| Bark incoming mint | 2,000 sat | 55.60 s | External payment plus manual handoff | Success |
| Bark outgoing melt | 1,000 sat | 47.75 s | Coordinator pickup plus Ark/LN | Success |
| BDK incoming mint | 10,000 sat | 1,076.92 s | Signet block wait | Success |
| BDK outgoing melt | 2,000 sat | 1,156.80 s | Signet confirmation wait | Backend success; client recovered after initial verification error |

The Bark incoming wall time includes roughly 15 seconds of manual handoff. The external Wavelength payment itself took 26.61 seconds and paid a 1-sat fee.

## Ecash swap

Three transfers exercised offline send plus online receive, including a reverse transfer so both wallets proved that their received proofs were spendable.

### First transfer: 1,000 sat

- Offline send: 0.29 seconds.
- Online receive: 2.86 seconds.
- Successful member handler duration: median 1.470 seconds, maximum 1.746 seconds.
- One member handler was cancelled at 1.697 seconds after the threshold result was already available. This is expected threshold fan-out behavior, not a failed swap.
- Consensus: 1.593 seconds.
- Data-provider to finalized: 1.324 seconds.
- Local apply: median 267 ms, maximum 319 ms.
- Submit to waiter: 1.589 seconds.
- Network pump: median 55.2 ms, p95 86.3 ms, maximum 560.6 ms; four requeues and no failures.

### Second transfer: 500 sat

- Online receive: 2.21 seconds.
- All five member handlers succeeded: median 1.249 seconds, maximum 1.333 seconds.
- Consensus: 1.167 seconds.
- Data-provider to finalized: 1.003 seconds.
- Local apply: median 136.6 ms, maximum 159.4 ms.
- Submit to waiter: 1.164 seconds.
- Three network-pump requeues and no failures.

### Reverse transfer: 500 sat

- Online receive: 2.35 seconds.
- All five member handlers succeeded: median 1.249 seconds, maximum 1.291 seconds.
- Consensus: 1.139 seconds.
- Data-provider to finalized: 1.004 seconds.
- Local apply: median 138.4 ms, maximum 166.3 ms.
- Submit to waiter: 1.136 seconds.
- No network-pump requeues or failures.

Conclusion: a normal ecash receive currently has a stable roughly one-second AlephBFT floor plus client/Iroh and handler overhead. Local application is a smaller contributor.

## Bark incoming Lightning mint

The measured mint was 2,000 sat.

- CLI wall time: 55.60 seconds, including roughly 15 seconds of manual delay.
- External Wavelength payment: 26.61 seconds.
- Payment start to ecash availability: approximately 33 seconds.
- Incoming preparation: 38.1 ms.
- Secret replication: 308.7 ms.
- External-evidence pending polls: median 444.9 ms, p95 866 ms, 13 samples.
- HTLC observation: pending median 29.3 ms; successful observation 254 ms.
- Bark receive claim: 1.287 seconds.
- FROST commitment: 393.5 ms.
- FROST signature: 149.1 ms.
- Acknowledgement: 1.4 ms.
- Cashu mint handler: median 1.338 seconds, maximum 1.544 seconds.
- Federation consensus: median 991.9 ms, p95 3.460 seconds across all related operations.
- Data-provider to finalized: median 929 ms, p95 3.385 seconds.
- Submit to waiter: median 989.7 ms, p95 3.457 seconds.
- Local apply: median 79 ms, p95 199 ms.
- Network pump: median 54.5 ms, p95 105 ms, maximum 600.8 ms; 30 requeues and no failures.

The external payer path is the largest component. Once the mint can prove settlement, Bark custody/FROST and Cashu issuance account for approximately six seconds of the remaining critical path.

## Bark outgoing Lightning melt

The measured payment was 1,000 sat.

- CLI wall time: 47.75 seconds.
- Fee reserve charged/used: 25 sat.
- Initial Cashu melt handler: approximately 1.063 seconds median; four threshold responses succeeded and one was cancelled.
- Durable coordinator pickup delay: approximately 15 seconds.
- Bark planning: 99.8 ms.
- FROST commitment: 204.5 ms.
- FROST signature: 148.2 ms.
- HTLC cosigning: 1.798 seconds.
- Payment initiation: 45.9 ms.
- External Ark/Lightning initiation-to-success observation: approximately 22.4 seconds.
- Final payment observation across members: 0.17–2.93 seconds.
- Federation consensus: median 1.090 seconds, p95 1.686 seconds.
- Data-provider to finalized: median 983 ms, p95 1.532 seconds.
- Local apply: median 72 ms, p95 390 ms.
- Network pump: median 56.1 ms, p95 125.5 ms, maximum 385.8 ms; 35 requeues and no failures.

This reproduces the earlier roughly 45-second Bark melt. Bark-specific work is a substantial part of it, but the clean split is:

- about 15 seconds waiting for the durable coordinator to pick up work;
- about 22 seconds waiting for external Ark/Lightning settlement;
- about 2.2 seconds for Bark planning, FROST, and cosigning;
- about 1–3 seconds for final federation observation/consensus; and
- the remainder in initial request, Iroh, client handling, and polling alignment.

## BDK incoming on-chain mint

The measured mint was 10,000 sat.

- CLI wall time: 1,076.92 seconds (17 minutes 56.92 seconds).
- External block wait: about 17 minutes 10 seconds, or 95.6% of wall time.
- Address allocation: 960.4 ms.
- BDK scan: median 1.063 ms, p95 1.576 ms, maximum 1.232 seconds, 1,300 samples.
- Wallet synchronization: median 386.8 ms, p95 465.5 ms, maximum 605.8 ms, 390 samples.
- Backend probe: median 0.701 ms.
- Cashu mint handler: median 1.511 seconds, maximum 1.653 seconds.
- Consensus: median 1.101 seconds, maximum 1.773 seconds.
- Data-provider to finalized: median 1.006 seconds, maximum 1.474 seconds.
- Submit to waiter: median 1.098 seconds, maximum 1.770 seconds.
- Local apply: median 30.5 ms, p95 336.3 ms.
- Network pump: median 53.3 ms, p95 74.0 ms, maximum 353.4 ms; 37 requeues and no failures.

The actionable CDK cost is well below the end-to-end Signet time. The repeated 0.39-second wallet synchronizations are more important for resource use than for critical-path latency.

## BDK outgoing on-chain melt

The measured output was 2,000 sat.

- CLI wall time: 1,156.80 seconds (19 minutes 16.80 seconds).
- Request to transaction broadcast: approximately 145 seconds.
- Broadcast to confirmation: approximately 16 minutes 48 seconds.
- Quoted fee reserve: 4,967 sat.
- Actual total debit beyond the output: 638 sat.
- Unused reserve restored: 4,329 sat.
- Address allocation: 1.147 seconds.
- Fee estimation: 102.9 ms and 118.9 ms.
- PSBT construction: 3.7 ms.
- Proposal preparation: 2.349 seconds.
- Successful transaction finalization: 1.077 and 1.783 seconds.
- Broadcast calls: median 108.9 ms, p95 183.1 ms across 15 successful calls.
- Confirmation observations: 1,120 pending and five successful polls, reflecting independent five-second member loops.
- On-chain synchronization: median 384.4 ms, p95 482.4 ms across 376 samples.
- Cashu outgoing observation: 122 pending and two successful observations.
- Federation consensus: median 1.062 seconds, p95 1.308 seconds across 44 operations.
- Data-provider to finalized: median 1.005 seconds, p95 1.502 seconds.
- Submit to waiter: median 1.060 seconds, p95 1.305 seconds.
- Local apply: median 36.6 ms, p95 60.7 ms.

The backend payment succeeded and the destination detected the value. The first client completion returned a proof-signature verification error. A restart-safe wallet saga recovery then obtained the threshold-aggregated change, restored the balance, and subsequent bidirectional swaps spent those recovered proofs successfully.

Root cause: the wallet's WebSocket was connected to one federation member. That member could publish `Paid` with its own change-signature share before the client had collected a threshold. Treating that share as an aggregate signature failed validation. The connector now marks such notifications as requiring authoritative confirmation; the wallet then obtains the threshold-aggregated status response before finalizing.

### Recovery retry storm

While the transaction's parent input was not yet available in each local BDK wallet, the liveness loop repeatedly attempted recovery/CPFP finalization:

- 428 FROST commitment rounds;
- 428 FROST signature rounds;
- 212 transaction-finalization errors; median 476.7 ms, p95 734.6 ms; and
- repeated errors stating that an accepted input was absent from the local BDK wallet.

The retries stopped after confirmation made the input locally available. This is avoidable work, not useful progress. It explains much of the on-chain test's CPU overhead and log volume.

## CPU and memory

With diagnostic TRACE enabled, a 30-second idle sample consumed 1.29–1.45 CPU-seconds per member, equivalent to 4.3–4.8% of one core. RSS stayed essentially flat, within 0.61 MB.

Raw operation-window CPU deltas were:

- Bark incoming: 5.99–7.47 CPU-seconds per member over roughly 70 seconds.
- Bark outgoing: 5.21–6.45 CPU-seconds per member over 47.75 seconds.
- BDK outgoing: 72.8–80.4 CPU-seconds per member over 1,156.8 seconds.

Subtracting the TRACE-enabled idle baseline gives approximate incremental costs:

- Bark incoming: about 2.9–4.4 CPU-seconds per member.
- Bark outgoing: about 3.1–4.3 CPU-seconds per member.
- BDK outgoing: about 20–30 CPU-seconds per member.

The BDK retry storm is the main avoidable CPU consumer. Member 1 RSS grew by roughly 29 MB during the long on-chain test; the other members grew by roughly 4–5.5 MB. This warrants a longer steady-state memory test after retry suppression, but the single sample is not enough to classify a leak.

## Findings and recommended optimizations

### Priority 0: correctness fixes completed

1. **Aggregate federated melt change before client finalization.** Paid member WebSocket events now trigger the authoritative threshold status path. This removes the observed client verification race without changing consensus.
2. **Route observations only to their backend.** Bark advancement is now limited to Bolt11 quotes. BDK on-chain observations no longer invoke Bark planning merely because Bark is enabled in the same mint.
3. **Remove the invalid provisional queue-age metric.** Consensus order is not wall-clock time and is no longer interpreted as such.

### Priority 1: largest avoidable latency/resource wins

1. **Gate BDK recovery before FROST.** Check that every required accepted input exists and is spendable in the local BDK wallet before requesting commitment/signature shares. Missing inputs should produce a cheap deferred state, not a FROST attempt.
2. **Use exponential backoff with jitter for unchanged recovery state.** A five-second retry is too aggressive during a 10–20 minute Signet confirmation interval. Reset the backoff when chain height, wallet state, or accepted proposal state changes.
3. **Wake Bark coordination on accepted work.** Signal the durable coordinator immediately when a melt plan becomes actionable. Retain periodic scanning only as crash recovery. This can remove the observed approximately 15-second pickup delay.
4. **Wake payment observation on backend events.** Chain or Bark state changes should trigger focused quote observation. Periodic scans remain a safety net.

### Priority 2: efficiency and predictability

1. **Coalesce BDK synchronization per chain tip.** Five members must remain independent, but each member can share one in-flight sync result among its quote observers and liveness tasks.
2. **Separate broadcast from confirmation polling.** Return and persist broadcast success promptly, then use an event-driven confirmation watcher. End users should see both “broadcast” and “confirmed” rather than one opaque 19-minute wait.
3. **Improve fee-reserve estimation.** The measured reserve was 4,967 sat versus 638 sat actual overhead, about 7.8 times higher. Derive reserve from the selected UTXO set and current fee estimate, then preserve a bounded safety margin.
4. **Deduplicate equivalent broadcasts and probes.** Fifteen successful broadcasts and 1,120 pending confirmation polls are safe but noisy. Cache accepted broadcasts by transaction ID and poll once per member per chain tip.
5. **Keep Prometheus labels bounded.** Never add quote IDs, operation IDs, payment hashes, or transaction IDs as metric labels. Use trace correlation only during sampled diagnostics.

### Priority 3: consensus tuning after load tests

Normal operations show roughly 1.0 second from AlephBFT data-provider yield to finalization. Local apply is usually tens to hundreds of milliseconds. The active-unit interval and pump size could lower latency, but changing them may increase bandwidth, CPU, and instability under loss.

Before tuning, run a matrix with sustained concurrent swaps and injected member/network faults. Compare p50/p95/p99 finalization, requeues, unit rate, bandwidth, CPU, and recovery behavior. The present data does not justify trading safety margin for a sub-second improvement while payment backends still contribute tens of seconds or block-scale waits.

## Suggested performance acceptance targets

These are engineering targets, not claims about current behavior:

- Offline ecash send: p95 below 500 ms.
- Online ecash receive: p95 below 2.5 seconds with all members reachable.
- Cashu consensus waiter: p95 below 1.5 seconds under normal load.
- Bark accepted-plan pickup: p95 below 1 second.
- Bark CDK post-settlement processing: p95 below 4 seconds, excluding Ark/LN.
- BDK request-to-broadcast: p95 below 15 seconds when inputs are already confirmed and the chain source is healthy.
- BDK recovery: zero FROST rounds while any accepted input is locally unavailable.
- Idle mintd with normal logging: below 2% of one core per member over a 10-minute sample.
- No client proof-verification failure after a backend has reached threshold-paid state.

## Follow-up benchmark design

After implementing recovery gating and event-driven wakeups:

1. Run at least 30 operations per flow for distributions instead of single samples.
2. Separate request-to-broadcast, broadcast-to-confirmation, and confirmation-to-ecash timings.
3. Test one offline member and one delayed member.
4. Run 1, 5, and 20 concurrent swaps to expose serialization and queueing.
5. Repeat with normal INFO logging and a short sampled TRACE window to quantify instrumentation overhead.
6. Track process CPU, RSS, network bytes, DB growth, FROST rounds, AlephBFT units, and chain-source calls.
7. Assert that Bark-only and BDK-only builds and deployments continue to work independently.

## Conclusion

The federation's cryptographic and consensus work is not the main source of end-to-end payment latency. AlephBFT contributes a consistent roughly one-second floor to normal Cashu state changes. Bark latency is dominated by coordinator scheduling and the external payment path. BDK end-to-end latency is overwhelmingly block-bound, while its current recovery loop creates avoidable FROST and CPU load during that wait.

The immediate correctness fixes preserve backend independence and require no new consensus messages. The next optimization should be BDK recovery gating/backoff, followed by event-driven Bark coordination. Consensus parameter tuning should remain last and evidence-driven.
