# FROST On-Chain Internal-Settlement Federation Halt

**Status:** Fixed and deployed; coordinated recovery verified  
**Incident:** Nocean development federation, 2026-07-17 22:54:55 UTC  
**Investigated:** 2026-07-18  
**Related TODO:** [federation cleanup item 49](federation-cleanup-todo.md#49-route-terminal-frost-on-chain-internal-melts-around-pending-batching)

> **Follow-up — 2026-07-29:** This incident remains resolved. The cleanup
> crash-boundary matrix in
> [`cleanup-2026-07-29.md`](cleanup-2026-07-29.md) Milestone 13 retained the
> internal-settlement and restart regressions, and the coordinated Nocean update
> preserved a ready, converged federation without reproducing the halt.

## Executive summary

The federation did not halt because transaction batching started too quickly or
because its batching delay was too short. It halted because a valid FROST
on-chain melt was also an **internal settlement**: its destination was the
address of an active on-chain mint quote in the same mint.

Internal settlement correctly completed first. On every member it credited the
mint quote, marked the melt quote `PAID`, spent the eight input proofs, stored
the change, recorded the completed melt, deleted the pending request and saga,
and committed that transaction. The accepted-melt apply path then
unconditionally tried to materialize a **pending external FROST payment
intent**. That helper requires the melt quote to remain `PENDING`; it instead
found the correctly terminal `PAID` quote and returned:

```text
Federation on-chain transaction proposal is invalid: FROST on-chain melt did not remain pending for transaction batching
```

The error is non-retryable under the finalized-payload policy, so AlephBFT
correctly failed closed and halted session 1 on all five members. The journaled
melt was left `failed` even though its separately committed business effects
were already terminal.

This is a generic federation state-machine bug at the intersection of:

- FROST on-chain melts;
- same-mint internal settlement; and
- journal apply/replay.

It is not Nocean-specific and must not be repaired with a deployment-specific
exception, database edit, or delay.

## What stopped, and when

There were two distinct events:

1. At `22:54:55 UTC`, all five running members logged the same non-retryable
   finalized-payload apply error and halted AlephBFT session 1. The HTTP
   processes remained alive but the federation was functionally stopped:
   wallet liveness, broadcasting, payment observation, and new submissions all
   failed closed against the halted member state.
2. A later deployment attempt replaced the mint binary at approximately
   `23:11:36 UTC` and the control script at approximately `23:12:02 UTC`. The
   old member logs end between `23:12:04` and `23:12:08` without a graceful
   mint shutdown record. The control script's restart path kills the old tmux
   session before creating its replacement. No replacement tmux session or
   mint process survived. This later operational teardown explains why no
   processes are running now; it did not cause the earlier consensus halt.

The halt predates the later binary replacement by more than sixteen minutes.
The incident binary and the isolated pre-fix local Linux artifact had the same
SHA-256 digest:

```text
a93d08719b48952edc65537d5f00da8ee97a50b6ff898f2dbd3d8ad12f5b2bdd
```

The source-level investigation was performed at revision `158a9a8db`; its only
change after the deployed code revision is documentation, and the failing code
path is present unchanged.

## Live evidence

### Application logs

Every member logged the same failure within 39 milliseconds. Member 1's
incident window is representative:

```text
2026-07-17T22:54:55.523263Z ERROR ... AlephBFT finalized-payload apply task stopped after a non-retryable error: Federation on-chain transaction proposal is invalid: FROST on-chain melt did not remain pending for transaction batching
2026-07-17T22:54:55.523356Z ERROR ... AlephBFT live session halted session_id=1 reason="stopped after a non-retryable finalized-payload apply failure: Federation on-chain transaction proposal is invalid: FROST on-chain melt did not remain pending for transaction batching"
```

Afterward, each member repeatedly logged that wallet liveness, transaction
broadcast, and payment observation failed closed because the member was
halted. This is the intended response to an unapplyable finalized payload.

The same log files contain an older session-0 halt concerning reorged deposit
evidence. That was the prior incident and is separate from this session-1
FROST/internal-settlement failure.

### Consensus and journal records

AlephBFT did finalize the operation. On member 1, the durable finalized-item
record is:

| Field | Value |
|---|---:|
| accepted item index | 210 |
| session | 1 |
| unit finalization index | 30 |
| submitting member | 2 |
| unit index | 82 |
| item index in unit | 0 |
| finalized-item status | `accepted` |
| operation ID | `25189bb8…28ca` |

That operation ID is journal operation 144. Its journal state is `failed`, with
no application digest. This proves consensus accepted and ordered the melt,
then deterministic application failed; it was not rejected at admission and
was not waiting to enter a batch.

The relevant journal sequence was:

| Journal operations | Kind | Result |
|---|---|---|
| 137–141 | five mint-quote payment observations | paid an earlier 20,000 sat mint quote |
| 142 | mint | issued against that earlier paid mint quote |
| 143 | on-chain melt quote | requested 8,333 sat to a second, unpaid mint quote's address |
| 144 | FROST on-chain melt | credited the second quote internally, then failed during pending-intent materialization |

The melt destination exactly matched the request of a second active on-chain
mint quote. The address itself is intentionally omitted; matching its stored
hash was sufficient to establish the internal-settlement relationship.

### Materialized state on all five members before repair

All five databases agree on the following state after operation 144:

| State | Value |
|---|---:|
| journal operation 144 | `failed` |
| melt quote | `PAID` |
| destination mint quote amount paid | 8,333 sat |
| input proofs | 8 `SPENT` |
| completed melt operation | present |
| completed payment amount | 8,333 sat |
| completed payment fee | 0 sat |
| returned change | 3,478 sat |
| melt saga | absent |
| stored melt request | absent |
| pending FROST payment intent | absent |

The completed operation redeemed 11,811 sat: 8,333 sat was internally
transferred and 3,478 sat was returned as change. No Bitcoin transaction was
needed or broadcast, which is correct for same-mint internal settlement.

## Exact code path

### 1. The melt is recognized and finalized internally

[`melt_federation_with_accepted_time`](../crates/cdk/src/mint/melt/mod.rs)
sets up the melt saga and calls
`finalize_internal_settlement_federation`. Its `Finalized` branch returns a
terminal response immediately.

[`finalize_federation_internal_melt_quote`](../crates/cdk/src/mint/melt/shared.rs)
looks up a mint quote by the melt request string. When the units match, it
performs the complete transfer in one transaction:

- add the payment to the matching mint quote;
- update the melt quote to `PAID`;
- spend the input proofs;
- store signature shares and change signatures;
- delete the melt request;
- add the completed operation;
- delete the saga; and
- commit.

The commit occurs before control returns to the federation journal apply path.

### 2. The caller discards the terminal-vs-pending outcome

[`apply_federation_melt`](../crates/cdk/src/mint/federation/application.rs)
accepts every
successful response through `Ok(_)`, losing the fact that internal settlement
returned a terminal result. It then always opens another transaction, reloads
the quote, and calls `persist_pending_onchain_intent`.

This unconditional follow-up is the primary bug.

### 3. The pending-intent invariant rejects the correct terminal state

[`persist_pending_onchain_intent`](../crates/cdk/src/mint/federation/transaction_state.rs)
correctly requires an external FROST on-chain melt to remain `PENDING` before
it is admitted to transaction batching. The internally settled quote is
already `PAID`, so the helper rejects it.

That helper uses the transaction-proposal `invalid` constructor for all of its
errors. This explains the misleading prefix `Federation on-chain transaction
proposal is invalid` even though the failed journal operation is a `Melt`, not
a `TransactionProposal`.

### 4. The failure cannot roll back the internal settlement

The pending-intent/application-state transaction rolls back on error, but the
internal-settlement transaction has already committed. The result is a split
durable state:

- business state says the melt completed successfully; and
- journal application state says the accepted melt failed.

The split transaction boundary is not by itself fatal if replay is
deterministically resumable. Here it is not: on replay, the paid-quote branch
verifies the exact request and completed operation and returns the terminal
response, after which `apply_federation_melt` again calls the pending-intent
helper and returns the same error.

### 5. The supervisor halts by design

[`is_retryable_federation_apply_error`](../crates/cdk-axum/src/lib.rs) only
classifies concurrent updates and retryable database locking failures as
retryable. `TransactionProposalInvalid` is definitive.

[`aleph_bft_session.rs`](../crates/cdk-axum/src/federation/aleph_bft_session.rs)
therefore turns the finalized-payload apply failure into a generation-scoped
halt, withdraws the live consensus resources, marks readiness halted, and
causes dependent loops and submissions to fail closed. This behavior prevented
members from continuing past an accepted operation they could not mark
applied; it should not be weakened to hide the melt bug.

## Why batching delay was not causal

- No pending FROST payment intent was ever created for this melt.
- Internal settlement intentionally requires no FROST signing, batching, or
  Bitcoin broadcast.
- The wallet liveness loop has a fixed five-second round interval, but the
  failure happened synchronously while applying the accepted melt, before the
  wallet loop could select an intent.
- The deployment had no environment override for an AlephBFT, federation, or
  batching delay.
- `min_delay_time = 0` and `max_delay_time = 0` are fake-wallet settings. The
  deployment used the generic `federated-bdk` on-chain backend, so those values
  do not govern this path.

Increasing a delay could only move unrelated scheduling around. It cannot make
an already-`PAID` internal melt become a pending external payment.

## Local reproduction and confirmation

A permanent focused federation test constructs the exact live combination
against the in-memory SQLite test mint:

1. create a FROST-enabled regtest federation runtime;
2. allocate an on-chain mint-quote address;
3. create an on-chain melt quote targeting that exact address;
4. apply the accepted melt; and
5. inspect terminal state and the transaction-intent projection.

Before the fix, the desired-behavior test failed with the same live error:

```text
apply journal entry: Federation(TransactionProposalInvalid { reason: "FROST on-chain melt did not remain pending for transaction batching" })
```

After the fix, two permanent regressions pass:

```text
accepted_frost_onchain_internal_melt_finalizes_without_pending_intent ... ok
failed_frost_onchain_internal_melt_replays_after_restart ... ok
```

The restart regression reproduces the live split state: it commits internal
settlement, records the journal operation as `Failed`, recreates the mint over
the same database, and reapplies the accepted operation. Replay advances the
journal entry to `Applied` without a wallet intent or duplicate materialized
effect.

The focused regressions verify:

- the destination mint quote and melt quote are `PAID`;
- all inputs are `SPENT`;
- the completed melt operation exists;
- no FROST transaction intent exists;
- the journal operation is `Applied`; and
- exact applied replay is a no-op.

The existing external FROST test continues to verify that a non-internal
on-chain melt remains `PENDING` and materializes one payment intent.

Validation results:

- all 36 federated melt tests passed;
- the federation-enabled `cdk` library run passed 769 tests in the sandbox;
  its only three failures were local-listener binds denied by the sandbox, and
  the complete 12-test affected wallet-repository module passed when rerun with
  listener permission;
- focused Rust formatting and `git diff --check` passed; and
- `cargo clippy -p cdk --lib --features federation` passed with the same nine
  pre-existing warnings and no warning in the changed code.

## Implemented smallest safe fix

The fix keeps the internal-settlement logic, pending-intent invariant, and
fail-closed supervisor unchanged. Only accepted-melt routing changed:

1. `apply_federation_melt` now retains the state of the response returned by
   `melt_federation_with_accepted_time` instead of discarding every success
   through `Ok(_)`.
2. A `PENDING` response follows the existing strict pending-intent path.
3. A `PAID` response must still correspond to a persisted `PAID` quote, then
   marks the journal entry applied without creating an intent.
4. Unexpected successful response states fail closed with `Error::Internal`.
5. The existing `PaymentFailed` behavior and strict
   `persist_pending_onchain_intent` invariant are unchanged.

Arbitrary pre-existing `PAID` quotes are not accepted by this routing change:
the paid replay path still validates the exact input set, change outputs, fee
selection, federation operation ID, and completed-operation kind before it can
return a `PAID` response.

This is generic domain behavior and adds no Nocean check, address allowlist,
database mutation, batching-delay change, or backend-specific exception.

## Validation and deployment checks

- [x] A FROST on-chain melt to an active same-mint on-chain mint quote completes
  internally and the accepted journal operation becomes `Applied`.
- [x] The mint quote is credited once, proofs are spent once, and change and the
  completed operation are stored once.
- [x] No FROST payment intent, signing proposal, Bitcoin transaction, or payment
  observation is created for internal settlement.
- [x] External FROST on-chain melts still remain `Pending` and create exactly one
  batchable intent.
- [x] Crash/restart after the internal commit but before the journal application
  marker replays to `Applied` without double credit or a second completion.
- [x] Restarted members converge on the same journal status and materialized digest.
- [x] A terminal quote with mismatched inputs, outputs, fee selection, or operation
  provenance remains rejected.
- [x] Bolt11 internal settlement and standalone melt behavior remain unchanged.
- [x] Starting the five upgraded members against the existing Nocean databases
  replays operation 144 to `Applied`; all members converge before the deployment
  is declared healthy.

## Deployment result

The state-preserving coordinated restart completed on 2026-07-18 at
approximately 08:03 UTC using revision `6991e83bb`. It reused all five existing
databases and DKG artifacts; no database edit, fresh DKG, address exception, or
deployment-specific workaround was used. The deployed mint binary has SHA-256
digest:

```text
ff90b6fb244e98fc22d3a730f34799a41d5fe2760bb7d5a9b68641394dbe80e0
```

Each member independently replayed operation 144 from `failed` to `applied`.
All five then reported the same operation ID, application digest, order digest,
materialized-state digest, and 145-operation journal length through index 144.
The affected melt remained `PAID`, the destination mint quote remained credited
once with 8,333 sat, and the obsolete melt saga and request remained absent.

The deployment readiness check reported all five members `ready=true`, healthy
sync, a live AlephBFT session, no audit mismatch, no invalid-operation alerts,
and no reorged or quarantined wallet UTXOs. A subsequent live-pane inspection
found five running mint processes and no fresh occurrence of the original
pending-intent error, consensus halt, fatal/panic record, or liability alarm.

The pre-fix binary must not be restarted against this state: it deterministically
reproduces the same terminal-routing failure if this operation is replayed.
