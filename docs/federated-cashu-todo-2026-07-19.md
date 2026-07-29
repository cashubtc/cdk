# Federated Cashu open bugs and follow-ups — 2026-07-19

> **Archived review snapshot — 2026-07-29:** Counts and statuses in this
> document describe the 2026-07-19 review and are not the current work queue.
> [`cleanup-2026-07-29.md`](cleanup-2026-07-29.md) reassesses the live findings:
> some are fixed and proven, while payment evidence/recovery identity,
> readiness, retry fairness, per-member ingress fairness, broadcaster
> supervision, and external release validation remain deferred. The liar-first
> deposit-field case is fixed and retains focused regression coverage. Use the
> cleanup ledger for the current release impact.

## Scope

This document records federation bugs and incomplete safety/liveness work that
remain after checking the issue reports in `docs/` against the current code.

Current classification after the W1 fix:

- 11 open bugs or design gaps.
- 1 partially addressed crash/replay proof obligation.
- 1 resolved finding retained for verification history.

An item is considered resolved only when the current implementation removes the
reported failure mechanism and, where practical, a focused regression proves
the relevant invariant. A mitigation that narrows the impact but leaves the
reported path possible is recorded as partial or open.

## Summary

| ID | Finding | Status | Impact |
| --- | --- | --- | --- |
| W1 | Member metadata/keyset validation can wait forever | Resolved | Wallet operation liveness |
| W3 | Sequential transaction broadcaster can head-of-line block | Open | On-chain settlement liveness |
| P6 | Pending quote population and observer scans are unbounded | Open | Resource exhaustion/starvation |
| R3 | AlephBFT retry work can starve behind fresh traffic | Open | Network delivery/recovery liveness |
| P13 / A2 | Payment-observation submission bypasses an explicit readiness policy | Open | Consensus admission during degraded states |
| P18 | Raw signing capability lacks an enforceable architecture boundary | Open assurance gap | Future signing-boundary regressions |
| DB-002 | Payment backends do not uniformly honor caller recovery keys | Open | Crash-recovery quote liveness |
| DB-004 | Rebuilt wallet connectors lack original melt change outputs | Open residual | Wallet melt recovery after restart |
| DB-005 | Payment evidence trust model is not committed and verified | Open release blocker | Paid quotes can remain pending indefinitely |
| DB-006 | Shared AlephBFT inbound limits lack per-member fairness | Open residual | Honest-message admission starvation |
| BDK-REORG-1 | Local absence/unconfirmed state can produce false reorg observations | Open residual | Unnecessary reorg/quarantine processing |
| U1 / Issue 13 | Federated quote age mixes logical and Unix timestamps | Open | Incorrect age/staleness reporting |
| P7 | Staged melt commits lack a complete crash-boundary proof | Partially addressed | Melt replay/idempotence assurance |

## Resolved in this pass

### W1 — Member metadata/keyset validation can wait forever

**Status:** Resolved and verified on 2026-07-19.

`validate_member_active_keysets_with_mint_info` now applies the configured
`member_request_timeout` independently to each connector's `get_mint_info` and
`get_mint_keys` request. A timed-out member is recorded as a validation failure,
its fan-out slot is released, and validation continues with later members until
a valid threshold is found or all bounded attempts fail.

**Current evidence:**

- Implementation: `crates/cdk/src/wallet/federation.rs`
- Stalled mint-info regression:
  `federated_mint_connector_times_out_stalled_mint_info_validation`
- Stalled keyset regression:
  `federated_mint_connector_times_out_stalled_mint_keys_validation`

**Verification:** Both regressions pass with `fanout_concurrency = 1`, proving a
never-returning first member cannot prevent two later healthy members from
forming the threshold. The complete `wallet::federation::tests` suite passes
93 tests, and the standalone wallet feature configuration still compiles.

## Open findings

### W3 — Sequential transaction broadcaster can head-of-line block

**Status:** Open, source-proven.

The broadcaster loops through accepted transactions sequentially. Its
`transaction_confirmation`, wallet readiness, broadcast, and observation
submission awaits do not have per-transaction deadlines. One stuck backend can
prevent every later transaction from being inspected. The worker is also a
detached `tokio::spawn`; errors are logged, but panic/exit supervision is not
provided.

**Current evidence:**

- Sequential loop: `crates/cdk-axum/src/federation/broadcast.rs:65`
- Confirmation await: `crates/cdk-axum/src/federation/broadcast.rs:118`
- Broadcast await: `crates/cdk-axum/src/federation/broadcast.rs:155`
- Detached worker: `crates/cdk-axum/src/federation/broadcast.rs:188-210`

**Required correction:** Add per-backend deadlines, isolate transactions with
bounded concurrency or fair scheduling, retain exact-byte retry state, and
supervise/restart the worker. Prove that one permanently blocked wallet does
not delay an unrelated transaction.

### P6 — Pending quote population and observer scans are unbounded

**Status:** Open, source-proven resource hazard.

Each observer round loads all mint quotes and all melt quotes, filters them in
memory, and probes every qualifying row. Per-probe deadlines prevent one
backend call from blocking forever, but no capacity, expiry, cursor, or bounded
work queue limits the durable population or total scan cost. A client can
create a growing set of valid unpaid quotes and increase every observer round's
cost.

**Current evidence:**

- Full mint quote load: `crates/cdk-axum/src/federation/payment_observation.rs:391-405`
- Full melt quote load: `crates/cdk-axum/src/federation/payment_observation.rs:407-419`
- Original finding: `docs/federated-cashu-consensus-deadlocks-2026-07-18.md:3233`

**Required correction:** Introduce an indexed bounded work queue or fair cursor,
enforce an intentional quote capacity/expiry policy, and add load tests proving
bounded round cost under a large unpaid-quote population.

### R3 — AlephBFT retry work can starve behind fresh traffic

**Status:** Open, source-proven.

Failed outbound deliveries are bounded and requeued, but retry work is serviced
only when the current pump tick drains zero fresh messages. Sustained fresh
traffic can therefore postpone recovery traffic indefinitely even though the
queue itself is bounded.

**Current evidence:**

- Fresh delivery loop and requeue:
  `crates/cdk-axum/src/federation/aleph_bft_network.rs:470-505`
- Idle-only retry condition:
  `crates/cdk-axum/src/federation/aleph_bft_network.rs:507-523`

**Required correction:** Reserve a bounded retry quota every tick or use a fair
fresh/retry scheduler. Add a deterministic test that maintains continuous fresh
traffic while proving an older failed delivery receives retry attempts.

### P13 / A2 — Payment-observation submission bypasses an explicit readiness policy

**Status:** Open policy and liveness gap.

Wallet-facing submissions call `ensure_federation_ready_for_public_submit`.
Payment-observation envelopes instead call the lower-level consensus submission
method directly. Allowing lagging members to contribute evidence may be useful,
but the minimum live-session/not-halted rule is not expressed or tested as an
intentional policy.

**Current evidence:**

- Public readiness gate: `crates/cdk-axum/src/lib.rs:1765-1778`
- Lower-level submission without readiness:
  `crates/cdk-axum/src/lib.rs:1781-1790`
- Payment-observation bypass: `crates/cdk-axum/src/lib.rs:1800-1818`

**Required correction:** Define a dedicated payment-observation admission gate,
at minimum rejecting halted, tearing-down, or stale-generation sessions. Test
every reachable sync state and the intended behavior of a lagging observer.

### P18 — Raw signing capability lacks an enforceable architecture boundary

**Status:** Open assurance gap; no current signing bypass demonstrated.

The normal mint `blind_sign` method rejects standalone federation signing, but
`MemberConfigPrivate::sign_blinded_messages` remains a public raw capability.
The removed architecture scanners have not been replaced by a sealed type or
capability boundary that makes misuse difficult to compile.

**Current evidence:**

- Public raw signing method:
  `crates/cdk-common/src/federation/signing.rs:148-170`
- Runtime standalone-path rejection: `crates/cdk/src/mint/mod.rs:1297-1305`
- Original finding: `docs/federated-cashu-code-and-security-review.md:1509`

**Required correction:** Restrict raw signing behind a private/sealed capability
available only to accepted-operation application, or add an equivalent
enforceable boundary with focused tests for all public handlers.

### DB-002 — Payment backends do not uniformly honor caller recovery keys

**Status:** Open backend-contract and standalone-flow gap.

Federation code now persists a recovery identifier before invoking an external
payment side effect, fixing the original ordering hazard. That does not provide
recovery when a backend cannot resolve the caller's identifier. The fake
Bolt12/custom backend still generates random `CustomId` values for quote and
payment lookup handles instead of preserving the caller-supplied quote ID.

**Current evidence:**

- Random quote lookup ID: `crates/cdk-fake-wallet/src/lib.rs:641-644`
- Random payment lookup IDs: `crates/cdk-fake-wallet/src/lib.rs:808-827`
- Original finding: `docs/federated-cashu-consensus-deadlocks-2026-07-18.md:460`

**Required correction:** Define and enforce a payment-backend contract under
which the caller-supplied recovery key always resolves the attempt. Add
per-backend crash-after-side-effect tests, including standalone melt flows.

### DB-004 — Rebuilt wallet connectors lack original melt change outputs

**Status:** Open residual restart gap.

The live `FederatedMintConnector` retains original blinded change outputs in a
bounded in-memory map and uses them to verify member shares. The wallet saga
also stores the outputs durably. After rebuilding the connector, however, the
connector cannot read the saga data and falls back to interpolation without
verifying shares against the original blinded outputs.

**Current evidence:**

- In-memory retention and bound: `crates/cdk/src/wallet/federation.rs:65-72`
- Verified in-process aggregation: `crates/cdk/src/wallet/federation.rs:1775-1804`
- Rebuilt-connector fallback: `crates/cdk/src/wallet/federation.rs:1806-1808`
- Durable saga data: `crates/cdk/src/wallet/melt/saga/mod.rs:833-853`

**Required correction:** Give recovery aggregation durable access to the
original outputs, either through a wallet-database-backed connector store or a
wallet-side verified recovery path. Test a full process restart with one
well-formed but invalid member share.

### DB-005 — Payment evidence trust model is not committed and verified

**Status:** Open; release blocker before real funds.

Federation payment observations are authenticated and require matching quorum,
but each member probes only its local processor. With independent backends, an
invoice can be visible only to its creator, leaving the quote unable to gather
the configured evidence quorum. No concrete shared-backend, origin-proof,
gateway, or on-chain evidence verifier is committed and enforced by
configuration.

**Current evidence:**

- Detailed finding: `docs/federated-cashu-consensus-deadlocks-2026-07-18.md:613`
- Trust model: `docs/federated-cashu-payment-trust-model.md`
- Unfinished real-funds requirement:
  `docs/federated-cashu-implementation-plan-completed.md:2860-2861`

**Required correction:** Select and enforce at least one concrete evidence
model, reject configurations whose processors cannot satisfy it, and prove
payment completion with genuinely independent member backends.

### DB-006 — Shared AlephBFT inbound limits lack per-member fairness

**Status:** Open residual fairness gap.

The inbound AlephBFT channel and byte count are bounded, fixing the unbounded
memory hazard. Both limits remain global to the session. One authenticated
member can consume the shared budget, causing honest members' requests to be
rejected with backpressure until the queue drains.

**Current evidence:**

- Shared byte counter: `crates/cdk-axum/src/federation/aleph_bft_network.rs:80-87`
- Global budget admission: `crates/cdk-axum/src/federation/aleph_bft_network.rs:187-218`
- Global budget constant: `crates/cdk-axum/src/lib.rs:130`

**Required correction:** Add per-member accounting/reservations while retaining
the global hard bound. Test that a flooding authenticated member cannot prevent
an honest member from reserving and delivering messages.

### BDK-REORG-1 — Local absence/unconfirmed state can produce false reorg observations

**Status:** Resolved 2026-07-29.

Reorg observations now require quorum and conservative state transitions, so
one member can no longer unilaterally reorg or permanently quarantine accepted
funds. However, the observer still emits a reorg whenever its local scan does
not show the output confirmed in the exact recorded block. Temporary local
absence, an unconfirmed representation, or an incomplete scan is not proof that
the block was disconnected from the active chain.

**Current evidence:**

- Local exact-block test and reorg emission:
  `crates/cdk-axum/src/federation/payment_observation.rs:212-236`
- BDK unconfirmed positions become `confirmation=None`:
  `crates/cdk-bdk/src/federation.rs:544`
- Open follow-up: `docs/federated-cashu-bdk-reorg-problem.md:18`

**Resolution:** `FederatedBdkWallet::block_membership` now distinguishes an
exact persisted checkpoint, a different known checkpoint at the same height,
and an unknown height. The Axum observer submits reorg evidence only for the
second case. Missing outputs, unconfirmed positions, stale/incomplete scans,
and checkpoint gaps remain pending. The BDK membership test includes restart
persistence; the Axum policy test covers connected, unknown, disconnected, and
still-confirmed cases.

### U1 / Issue 13 — Federated quote age mixes logical and Unix timestamps

**Status:** Open; locally confirmed semantics mismatch, production-backend
impact still requires validation.

Federated quote application uses the accepted journal index as logical
`created_time`. Payment reporting later subtracts that value from current Unix
time. This can report an age near the Unix epoch even for a fresh quote and may
misclassify staleness if the value is used directly.

**Current evidence:**

- Logical time assigned to federated quotes:
  `crates/cdk/src/mint/federation.rs:2705-2712`
- Logical time stored as `created_time`/`updated_at`:
  `crates/cdk/src/mint/federation.rs:3283-3295`
- Unix subtraction: `crates/cdk-axum/src/federation/payment_observation.rs:599-625`
- Recorded investigation: `docs/federated-cashu-performance-and-issues.md:1278-1302`

**Required correction:** Store a real wall-clock timestamp for reporting or
represent age as unknown when only logical time exists. Add fresh, stale, and
unknown-age tests across persistence and restart.

## Partially addressed proof obligation

### P7 — Staged melt commits lack a complete crash-boundary proof

**Status:** Partially addressed; no current reproduced runtime defect.

The implementation has restart/replay coverage for pending and terminal melts,
including no-duplicate-payment behavior. These tests reduce the original risk,
but they do not form the complete crash matrix requested by the deadlock ledger:
crash after each evidence, claim, backend-call, lookup-handle, quote, saga,
proof, and application commit boundary.

**Current evidence:**

- Restart replay test:
  `crates/cdk/src/mint/federation/tests/melt.rs:2745`
- Catch-up finalization without duplicate payment:
  `crates/cdk/src/mint/federation/tests/melt.rs:2928`
- Open proof requirement:
  `docs/federated-cashu-consensus-deadlocks-2026-07-18.md:60`

**Required correction:** Build a deterministic fault-injection matrix for every
durable boundary and prove after restart that external payment occurs at most
once, proofs are neither lost nor double-spent, and every accepted operation
converges to the correct terminal state.

## Verification performed

The following focused tests passed while validating this list:

```text
cargo test -p cdk-common --features federation \
  reorg_is_conservative_and_raises_post_liability_alarm --lib

cargo test -p cdk-axum --features federation \
  aleph_bft_inbound_network_queue_rejects_flood_beyond_bounded_budget --lib

cargo test -p cdk --features federation \
  failed_frost_onchain_internal_melt_replays_after_restart --lib

cargo test -p cdk --features federation \
  catch_up_melt_payment_observations_finalize_without_duplicate_payment --lib
```

These tests confirm that conservative reorg handling, global inbound memory
bounds, and the covered melt replay paths are implemented. They do not close
the residual detection, fairness, or complete crash-boundary findings above.

## Findings intentionally excluded as resolved or superseded

The following previously recorded findings are not open TODO items here:

- The old P11 catch-up-certificate soft-skip was superseded by the checkpoint
  cutover; production recovery now uses checkpoint candidates and quorum.
- P4's recovery lookup identifier is persisted before the external payment
  side effect. DB-002 tracks the remaining backend contract.
- P5's single-observer reorg/quarantine authority is removed by quorum-backed
  conservative reorg handling. BDK-REORG-1 tracks only false observation
  detection.
- A1's unbounded AlephBFT queues are bounded. DB-006 tracks only per-member
  fairness.
- DB-003's minority-fixed immutable deposit fields were corrected by adopting
  quorum-backed fields.
