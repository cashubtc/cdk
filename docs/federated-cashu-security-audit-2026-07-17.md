# Federation Mint Security Audit — 2026-07-17

## Verdict

**CHANGES REQUESTED**

The federation implementation has strong transactional proof-spend controls,
but it is not ready to claim Byzantine safety for client funds or consensus
availability. Three findings are release-blocking:

1. a Byzantine member can replace the intent of an ordinary bearer-proof
   spend;
2. successful off-chain melts charge the full fee reserve instead of the
   backend's actual spend; and
3. one authenticated member can place a semantically invalid operation in an
   AlephBFT unit and halt the finalized-unit apply loop on every honest member.

Three additional findings weaken the configured Byzantine assumptions or
permit resource-exhaustion attacks. The implementation should not be deployed
with Byzantine-security claims until the critical findings are fixed and the
warning findings have either been fixed or explicitly accepted in the threat
model.

The remediation tracker is
[`federated-cashu-code-improvement-todo.md`](federated-cashu-code-improvement-todo.md#security-release-gate--2026-07-17).

## Executive summary

| ID | Severity | Area | Impact |
|---|---|---|---|
| SEC-2026-07-17-01 | Critical | Client spend authorization | One Byzantine member can front-run bearer proofs into attacker-controlled swap outputs. |
| SEC-2026-07-17-02 | Critical | Melt fee accounting | Every successful non-on-chain federated melt can retain the entire unused fee reserve. |
| SEC-2026-07-17-03 | Critical | Consensus finalization | One authenticated member can halt finalized-unit application with a semantically invalid operation. |
| SEC-2026-07-17-04 | Warning | Threshold configuration | Structurally valid but Byzantine-unsafe signing, payment, and checkpoint quorums are accepted. |
| SEC-2026-07-17-05 | Warning | AlephBFT backpressure | Authenticated traffic can accumulate in unbounded internal queues and exhaust memory. |
| SEC-2026-07-17-06 | Warning | Spend conflict detection | Same-proof swaps and cross-quote melts bypass consensus conflict filtering and amplify front-running and DoS. |

No database-level path was found that allows two conflicting swaps or melts to
both commit the same proofs as spent. Proof state and output signatures are
updated transactionally, and the focused replay and input-reuse tests pass.
That protection does not prevent finding SEC-2026-07-17-01: the attack makes
the attacker's spend win and the wallet's spend lose rather than committing
both spends.

## Scope

The audit focused on the federation feature's safety-critical paths:

- AlephBFT admission, ordering, finalization, persistence, session supervision,
  and network queues;
- operation authorization, operation IDs, balance audits, and conflict keys;
- journal persistence, replay, catch-up, and application serialization;
- federated mint, swap, and melt application;
- proof-state transitions and threshold BLS share production;
- payment observations, melt payment execution, and fee/change accounting;
- on-chain transaction policy, fee contributions, Taproot sighashes, and FROST
  authorization; and
- threshold and payment-observation configuration.

Primary files reviewed included:

- `crates/cdk-common/src/federation/consensus.rs`
- `crates/cdk-common/src/federation/journal.rs`
- `crates/cdk-common/src/federation/operation.rs`
- `crates/cdk-common/src/federation/config.rs`
- `crates/cdk-common/src/federation/transaction_proposal.rs`
- `crates/cdk-axum/src/federation/operation_pipeline.rs`
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs`
- `crates/cdk-axum/src/federation/aleph_bft_network.rs`
- `crates/cdk/src/mint/federation.rs`
- `crates/cdk/src/mint/federation/transaction_policy.rs`
- `crates/cdk/src/mint/melt/melt_saga/mod.rs`
- `crates/cdk/src/mint/melt/shared.rs`

## Threat model used for this audit

The review assumes:

- up to `f = floor((n - 1) / 3)` members can be Byzantine when the deployment
  advertises BFT safety;
- Byzantine members possess valid member identity keys and their legitimate
  threshold signing shares;
- Byzantine members can author arbitrary canonical operations, equivocate,
  reorder their own submissions, submit valid private-REST traffic, and race
  honest operations;
- the network adversary can delay honest messages within AlephBFT's asynchronous
  model;
- wallets can use ordinary Cashu bearer proofs unless the protocol explicitly
  requires a proof-bound spending condition; and
- honest members execute the reviewed code and use a converged durable state.

Compromise of a threshold of private BLS or FROST shares, host compromise of an
honest member, malicious payment backends outside the configured observation
assumptions, and cryptographic breaks are outside scope.

## Findings

### SEC-2026-07-17-01 — Member authorization does not authenticate client spend intent

**Severity: Critical**

**Affected code**

- `crates/cdk-axum/src/router_handlers.rs:265-279`
- `crates/cdk/src/mint/federation.rs:323-331`
- `crates/cdk-common/src/federation/operation.rs:1762-1815`

The public swap route receives the wallet's complete `SwapRequest`, including
the bearer proofs, and wraps it in an authorization made by the receiving
federation member. `FederationOperationEnvelope::verify_authorization` proves
that a roster member signed the operation. For `Mint`, `Melt`, and `Swap`, it
does not verify a wallet or proof-owner authorization over the exact operation.

For an ordinary bearer proof, possession of the request reveals all material
needed to construct another valid spend. A Byzantine member can:

1. receive the wallet's swap request;
2. copy its valid input proofs;
3. replace the blinded outputs with attacker-controlled outputs;
4. authorize the replacement as a legitimate federation member;
5. race the replacement operation into AlephBFT; and
6. collect the honest members' persisted shares after the replacement applies.

Under an asynchronous adversarial schedule, the attacker operation can order
first. The honest wallet operation then fails with the proofs already spent.
The database has prevented two committed spends, but the federation has not
preserved client intent or funds against one Byzantine member.

This issue also applies to any melt flow in which a member can reuse revealed
bearer proofs in another quote-scoped operation. Quote conflict keys do not
provide client authorization.

**Required remediation**

- Define a proof-bound client authorization over the canonical unsigned
  federation operation, including every input, output, quote, fee term, and
  relevant witness.
- Require the verification at consensus admission and again at deterministic
  apply.
- For the existing Cashu protocol, evaluate requiring a NUT-11 P2PK condition
  with `SIG_ALL` for federated spends. A detached signing key that was not
  committed when the proof was issued does not solve the attack.
- If ordinary bearer proofs must remain supported, redesign the protocol so no
  individual member learns reusable spend authority, or explicitly document
  that one member remains trusted not to steal wallet proofs. The latter means
  the feature cannot claim Byzantine client-fund safety.

**Required tests**

- A member cannot replace the outputs of an observed wallet swap.
- A member cannot move the same inputs to a different melt quote.
- Client authorization is bound to the operation ID and all outputs.
- Replaying authorization on a different operation, federation, quote, or
  keyset is rejected.
- A Byzantine member's replacement operation is rejected while the original
  operation can still finalize.

### SEC-2026-07-17-02 — Off-chain melt observations discard actual fees

**Severity: Critical**

**Affected code**

- `crates/cdk/src/mint/federation.rs:1294-1331`
- `crates/cdk-common/src/federation/operation.rs:625-646`
- `crates/cdk/src/mint/federation.rs:3207-3242`
- `crates/cdk/src/mint/melt/shared.rs:468-506`

The payment backend returns `MakePaymentResponse::total_spent`, which represents
the amount plus the actual routing fee. `probe_federation_melt_quote_payment`
does not copy that value into `FederationMeltQuotePaymentObservation`. The
canonical observation contains only the quote amount and the maximum fee
reserve.

When a successful non-on-chain observation is converted back into a payment
response, `melt_quote_payment_response` unconditionally calculates:

```text
total_spent = observation.amount + observation.fee_reserve
```

The change calculation then subtracts that inflated total. For example, if the
amount is 100 sat, the reserve is 10 sat, and the backend actually spends 102
sat, federation finalization records 110 sat and withholds 8 sat of change
before considering the normal input fee.

The on-chain path does not have this defect: it uses the receipt's exact
`amount_sat + fee_sat`.

**Required remediation**

- Add exact `total_spent` or `fee_paid` to the canonical melt payment
  observation.
- Bind that field into evidence matching and the operation ID.
- Validate that the value uses the quote unit and satisfies the method-specific
  bounds, including `amount <= total_spent <= amount + fee_reserve` where that
  relationship applies.
- Require a matching observation quorum or authenticated backend receipt for
  the actual value.
- Use the agreed actual spend when persisting finalization data and calculating
  change.

**Required tests**

- Reserve 10, spend 2, and prove that 8 is returned before input fees.
- Conflicting actual-fee observations do not finalize.
- An actual spend above the reserve is rejected.
- Replay and crash recovery preserve the exact spend.
- Bolt11, Bolt12, and custom backends retain their method-specific units and
  fee semantics.

### SEC-2026-07-17-03 — Semantically invalid finalized operations halt the session

**Severity: Critical**

**Affected code**

- `crates/cdk-common/src/federation/consensus.rs:1838-1862`
- `crates/cdk-common/src/federation/journal.rs:1135-1175`
- `crates/cdk-axum/src/federation/operation_pipeline.rs:577-662`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs:1652-1690`

The finalized-unit handler classifies an operation as invalid when its member
authorization fails. It does not perform all pure, deterministic operation
validation before assigning `Accepted` status.

The journal later calls `envelope.operation.balance_audit()?`. An authenticated
member can construct a structurally decodable mint or swap containing amounts
whose sum overflows. The operation has a valid operation ID and member
signature, so AlephBFT can finalize it, but the balance audit returns an error.
That error aborts preparation of the entire finalized unit. The finalized-unit
apply loop treats the error as non-retryable and exits. The session supervisor
then marks the live federation session halted.

Other deterministic semantic errors can reach the application layer and have
the same outcome when they are classified as ambiguous rather than as a
business rejection. For example, an authenticated operation can carry
inconsistent quote terms that produce `InvalidPaymentRequest`; that error is
not currently included among the definitive failures used by federation replay
to continue with a rejected operation.

Every honest member receives the same ordered invalid unit, so one Byzantine
author can halt the whole federation rather than merely its own node.

**Required remediation**

- Perform all pure canonical, bounds, balance, and operation-local validation
  while assigning the finalized item status.
- Convert failures caused by untrusted operation contents into a per-item
  `RejectedInvalidOperation` result. Do not abort the containing unit.
- Make deterministic apply-time business failures persist an auditable
  `Rejected` application result and continue to later entries.
- Reserve session termination for infrastructure failures for which continuing
  could violate local durability or ordering, such as an unrecoverable database
  failure.
- Review `Error::is_definitive_failure` and replace free-form `Custom` errors at
  consensus boundaries with typed deterministic rejection errors.

**Required tests**

- A validly member-signed balance-overflow operation is recorded as invalid.
- A valid operation later in the same unit still applies.
- A valid operation in the next unit still applies.
- Inconsistent mint and melt quote operations are rejected without halting.
- Every honest member records the same rejection response and digest.
- Catch-up and restart reproduce the rejection without retry loops or session
  termination.

**Status update 2026-07-18:** the pure-content mechanism is resolved. Finalized
classification now runs all pure, deterministic, operation-local validation
(`FederationOperation::validate_pure()` in
`crates/cdk-common/src/federation/operation.rs`) when assigning the finalized
item status, so a member-signed balance-overflow or otherwise pure-invalid
operation is recorded as a canonical per-item `RejectedInvalidOperation`
instead of aborting the containing unit. The executed five-member regression
`byzantine_balance_overflow_operation_does_not_halt_finalized_application`
(`crates/cdk-axum/src/federation/tests.rs`) demonstrated the halt pre-fix and
now proves per-item rejection, same-unit/next-unit progress, and
live/restart/catch-up convergence; a cdk-common unit test pins the
classification statuses. The remaining apply-time items — deterministic
stateful business failures that still return ambiguous `Custom`, `Internal`,
`InvalidPaymentRequest`, or federation errors and halt the session, including
the inconsistent-quote case above — are tracked as discovered bug `DB-001` in
`docs/federated-cashu-consensus-deadlocks-2026-07-18.md` and remain open.

### SEC-2026-07-17-04 — Byzantine-unsafe threshold profiles are accepted

**Severity: Warning**

**Affected code**

- `crates/cdk-common/src/federation/config.rs:291-348`

`ThresholdParams::with_bft_consensus` calculates the standard consensus quorum:

```text
f = floor((n - 1) / 3)
consensus_threshold = n - f
```

However, `ThresholdParams::validate` accepts any nonzero signing and consensus
thresholds within the member count as long as the consensus threshold is not
below the signing threshold. A four-member profile with signing threshold 1
and consensus threshold 1 is therefore structurally valid.

With that profile, one Byzantine member can produce valid e-cash signatures
outside consensus. Because payment-observation and checkpoint policies are
bounded against the configured consensus threshold, the same weak profile can
also permit single-member payment finality and checkpoint evidence. Less
extreme undersized quorums can lose the honest-intersection property required
to prevent conflicting certified views.

**Required remediation**

- For production BFT profiles, require `signature_threshold > f`.
- Require a quorum-intersecting consensus threshold, normally `n - f` for the
  advertised fault model.
- Validate payment, checkpoint, catch-up, and signing thresholds as one
  security profile rather than as independent numeric bounds.
- Put weaker configurations behind an explicit, loudly named development-only
  option that cannot be enabled accidentally in production.
- Surface the resulting fault tolerance in mint metadata and wallet import
  warnings.

**Required tests**

- Reject `signature_threshold <= f` for a production profile.
- Reject a consensus threshold below `n - f` for a production profile.
- Preserve explicit regtest/development configurations only with the unsafe
  opt-in.
- Prove that two accepted checkpoint quorums must intersect in an honest
  member under every accepted production profile.

### SEC-2026-07-17-05 — AlephBFT channels do not provide bounded backpressure

**Severity: Warning**

**Affected code**

- `crates/cdk-axum/src/federation/aleph_bft_network.rs:82-138`
- `crates/cdk-axum/src/federation/aleph_bft_network.rs:143-191`
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:654-690`
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs:1271-1279`
- `crates/cdk-axum/src/federation.rs:273-304`

The private-REST semaphore bounds concurrently executing HTTP handlers, but it
does not bound queued AlephBFT messages or enforce a sustained per-member rate.
Each valid handler decodes a message, writes it to an unbounded channel, and
releases its permit. An authenticated Byzantine member can use increasing
sequence numbers and continue enqueueing valid maximum-size messages faster
than the AlephBFT consumer can process them.

The AlephBFT adapter uses unbounded inbound and outbound channels, and finalized
units are also handed to the asynchronous apply loop through an unbounded
channel. A slow database, a prolonged retry, or CPU-heavy application therefore
allows memory growth to become independent of the bounded pending-operation
mempool.

**Required remediation**

- Replace externally driven unbounded channels with bounded queues.
- Use non-blocking bounded admission at synchronous protocol callbacks and
  return explicit backpressure from HTTP routes.
- Enforce per-member request and byte token buckets after authentication.
- Bound finalized-unit backlog and define a fail-closed session response if the
  durable application pipeline cannot keep up.
- Export queue depth, rejected bytes, oldest-item age, and saturation metrics.
- Ensure a stopped or halted session closes and drains all related senders.

**Required tests**

- A member cannot enqueue more than the configured message or byte budget.
- Saturation returns backpressure without dropping an already accepted
  finalized unit.
- One flooding member cannot starve honest member traffic.
- A blocked apply loop does not permit unbounded finalized-unit memory growth.
- Session halt and rotation release queued message memory.

### SEC-2026-07-17-06 — Swap and melt conflict keys omit spent proofs

**Severity: Warning**

**Affected code**

- `crates/cdk-common/src/federation/operation.rs:1440-1503`

Melt operations conflict only on their quote ID. Swap operations return no
conflict keys. A malicious wallet or member can therefore submit:

- many different swaps carrying the same input proofs and different outputs;
  or
- many melts carrying the same proofs but using different quote IDs.

These operations can all consume consensus and journal capacity. The serialized
database application rejects all but the first committed spend, so this is not
an observed database double spend. It is still a Byzantine amplification path:
every duplicate can reach expensive proof verification and application, and
the missing keys make the client-intent race in SEC-2026-07-17-01 cheaper to
execute.

**Required remediation**

- Add one conflict key for each input proof's `Y` to both swap and melt
  operations.
- Retain the melt quote conflict key in addition to proof keys.
- Derive proof conflict metadata only after bounded proof decoding and canonical
  validation. If derivation can fail, make the conflict-key API return a typed
  result or store validated conflict metadata in the consensus item.
- Bound the number of conflict keys by the existing maximum input policy.

**Required tests**

- Two swaps using one common proof conflict before journal application.
- Melts under different quote IDs using one common proof conflict.
- A duplicate operation ID remains idempotent rather than conflicting with
  itself.
- Malformed proof secrets are rejected without panic or unit failure.

## Deadlock and race-condition assessment

### Previously documented Nocean lock cycle

The previously documented status/finalization lock inversion was not found in
the current implementation. The status path now reads the durable checkpoint
frontier rather than acquiring a materialized-state cache while waiting for
the consensus mutex. Mutation and recovery paths consistently serialize around
the application lock before consensus and database mutation.

This removes the observed cycle in which:

```text
status:       materialized cache -> consensus
finalization: consensus -> materialized cache
```

No current mutex cycle with that shape was identified.

### Current serialization safeguards

- Live finalization and checkpoint catch-up share the global application lock.
- Consensus preparation is persisted before its in-memory prepared delta is
  committed.
- Swap proof spending, output signatures, signature-share records, completed
  operation state, and application status commit in one database transaction.
- Melt setup reserves proofs before the external payment actor is allowed to
  pay.
- The external melt actor uses a durable compare-and-set from `SetupComplete`
  to `PaymentAttempted`; the in-process active-payment set is not the sole race
  defense.
- Crash recovery checks the backend after `PaymentAttempted` rather than
  blindly paying again.

### Remaining liveness hazards

SEC-2026-07-17-03 is a logical consensus deadlock: the ordered log contains an
item that honest nodes cannot advance past because invalid operation data is
treated as a session-level failure. SEC-2026-07-17-05 can turn a slow or blocked
consumer into memory exhaustion because the surrounding channels lack bounded
backpressure.

The journal replay loop also retries recognized transient storage contention
without an attempt or elapsed-time bound while the application lock remains
held. Preserving order requires the node not to skip the entry, but the runtime
should expose a bounded health transition and stop accepting unbounded new work
when progress cannot be made.

## Double-spend and issuance assessment

The following controls were verified in the reviewed paths:

- proof states are checked and transitioned under a database transaction;
- duplicate, spent, pending, reserved, and pending-spent proofs fail closed;
- swap output signatures are not persisted independently of the proof-spend
  transition;
- partial signature persistence is detected rather than silently completed;
- mint issuance checks prior accepted quote state and payment-observation
  coverage before producing shares;
- cumulative partial issuance uses the quote's existing `amount_issued` value;
- payment evidence accepted after an issuance operation cannot retroactively
  authorize that earlier issuance;
- replay paths distinguish a complete prior application from partially
  persisted or conflicting state; and
- focused sliding-window input reuse tests pass.

No code path was found that makes two conflicting proof spends both reach a
committed `Spent` transition. The remaining monetary risks are:

- client-intent theft before the atomic gate, described in
  SEC-2026-07-17-01;
- unlimited issuance under a Byzantine-unsafe signing threshold, described in
  SEC-2026-07-17-04; and
- direct user overcharging in SEC-2026-07-17-02.

## On-chain fee and FROST assessment

The reviewed on-chain transaction path has several important safeguards:

- transaction version, locktime, input/output count, weight, and RBF policy are
  validated;
- only Taproot `SIGHASH_DEFAULT` is accepted by policy;
- selected inputs must exactly match accepted UTXOs and derived scripts;
- every output is assigned to a recipient or the expected change derivation;
- the sum of recipient fee contributions must equal the transaction fee;
- input value minus output value must equal that same fee;
- every per-intent contribution is capped by the intent's maximum fee;
- the absolute fee and fee rate are bounded by federation policy;
- replacement transactions preserve recipients, inputs, lineage, and scripts,
  and pay the additional fee by reducing the existing change output; and
- FROST signing authorization binds the accepted proposal, input index,
  derivation, and recomputed sighash.

No on-chain signature mode was found that lets a proposer detach signatures
from the validated outputs. A Byzantine proposer still has discretion within
the configured fee-rate range and each wallet's maximum fee contribution.
Operators should therefore choose narrow production fee policies and monitor
proposals near the maximum, but this audit did not classify bounded policy
discretion as a separate vulnerability.

## Validation performed

The following focused tests passed against the reviewed worktree:

```text
cargo test -p cdk-common --features federation --lib federation::consensus::tests
48 passed; 0 failed

cargo test -p cdk --features federation --lib mint::federation::tests::swap
14 passed; 0 failed

cargo test -p cdk --features federation --lib mint::federation::tests::melt
34 passed; 0 failed
```

The passing tests support the transactional and replay safeguards described
above. They do not contain adversarial reproductions for the six findings, so
they do not reduce those findings' severity.

## Recommended fix order

1. Fix SEC-2026-07-17-03 so an adversarial regression test cannot halt the
   test federation while subsequent work is developed.
2. Fix SEC-2026-07-17-02 before any real-value off-chain melt testing.
3. Resolve the protocol decision in SEC-2026-07-17-01. This can require a
   wallet/protocol migration and should not be treated as a local validation
   patch.
4. Enforce a production-safe threshold profile in SEC-2026-07-17-04.
5. Add proof conflict keys from SEC-2026-07-17-06.
6. Bound the AlephBFT queues and add per-member byte admission from
   SEC-2026-07-17-05.
7. Run multi-member Byzantine, partition, restart, and sustained-load tests
   after all fixes are integrated.

## Release criteria

The security gate can be closed only when:

- all three critical findings have regression tests and verified fixes;
- no accepted production configuration permits a Byzantine coalition within
  the advertised fault bound to forge mint signatures or certify conflicting
  state;
- authenticated Byzantine traffic cannot cause unbounded memory growth;
- same-proof conflicting spends are rejected before application;
- a semantically invalid member-authored operation cannot stop later finalized
  units;
- off-chain melt change is based on exact quorum-verifiable spend;
- the public threat model states precisely which proof types preserve client
  intent against individual members; and
- the full federation unit, integration, restart, and regtest suites pass.

## Limitations

This was a source-level audit with focused unit-test validation. It did not run
a long-lived multi-host federation under Byzantine network scheduling, memory
pressure, a real Lightning backend, or real on-chain funds. DKG cryptographic
proof internals and third-party AlephBFT/FROST implementations were reviewed
only at their CDK integration boundaries.
