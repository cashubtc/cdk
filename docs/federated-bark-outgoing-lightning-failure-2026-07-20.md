# Federated Bark outgoing Lightning failure report

Date: 2026-07-20

Status: remediated and verified in production. The incident evidence and root
cause sections below were recorded before remediation.

Scope: five-member Nocean federation using the federated Bark BOLT11 payment
backend.

Related documents:

- `docs/federated-cashu-bark-frost-implementation-plan.md`
- `docs/federated-cashu-bark-optimization.md`

This report deliberately omits invoices, payment hashes, action IDs, quote IDs,
preimages, server addresses, and custody material. The retained timestamps,
amounts, phases, operation indexes, and code paths are sufficient to reconstruct
the failure without publishing payment or deployment secrets.

## Executive summary

A 1,000-sat outgoing Lightning payment was accepted by federation consensus but
never reached Lightning payment initiation. The deterministic actor successfully
obtained an Ark-cosigned HTLC package and persisted it locally. The package
contained three HTLC VTXOs totalling 1,025 sats and two change VTXOs totalling
24,799 sats.

The Bark federation facade then rejected its own persisted result because
`validate_send_htlc_evidence` incorrectly required at most one physical change
VTXO. Arkoor accepts multiple inputs by constructing a package of single-input
transactions and may split one logical destination across multiple physical
VTXOs. The two change VTXOs were therefore valid and expected, not malformed
server evidence.

The failure happened after Bark had persisted actor-local side effects but
before it returned evidence to CDK. CDK consequently never submitted the
`SendHtlcsCosigned` transition. Consensus remained at `send_planned`, while the
actor remained locally at `HtlcReceived`. Exact retries reloaded the same two
change VTXOs and failed the same validation deterministically.

The pending action reserved 25,824 sats in the accepted public projection.
Together with older unfinished Bark actions, this left only 92 sats spendable.
Two later wallet quote fan-outs were then rejected by all five members with the
generic `Amount is not what is expected` error before either could create a
durable melt quote.

## Timeline

All timestamps are UTC.

| Time | Event | Durable evidence |
|---|---|---|
| 18:36:45 | A 1,000-sat BOLT11 melt quote was created with a 25-sat fee reserve. | `melt_quote`: amount 1,000, fee 25, state `PENDING`, created time 18:36:45. |
| 18:36:45-18:36:54 | The quote, melt request, and outgoing action plan reached consensus. | Journal operation indexes 147 (`melt_quote`), 148 (`melt`), and 149 (`action_plan`). |
| About 18:36:54 | All five members materialized the accepted outgoing checkpoint. | One `lightning_send` action, actor member 1, phase `send_planned`, attempt 1. |
| 18:36:56.219-18:36:56.246 | The actor completed the Ark HTLC co-sign package and persisted five new VTXOs. | Three locked HTLC VTXOs and two spendable change VTXOs appeared in the actor Bark database. |
| 18:36:56.273 | The actor's movement was updated after persisting the package. | Pending `bark.lightning_send` movement: intended balance -1,000, effective balance -1,025, fee 25. |
| Immediately afterward | Bark rejected the evidence because it contained two change VTXOs. | No `SendHtlcsCosigned` transition exists; the action's latest operation remains index 149. |
| 18:37:45 | The original melt quote expired while still pending. | Quote expiry precedes any payment-initiation record. |
| 18:46:07-18:46:12 | A wallet request fanned out across members 1-5. | Every member logged `Amount is not what is expected`; no quote or action was created. |
| 18:47:10-18:47:15 | A second wallet request or retry fanned out across members 1-5. | The same rejection occurred on every member; again no durable quote or action was created. |

The server logs cannot distinguish whether the two later fan-outs represented
two user payment attempts or one attempt plus a wallet retry. They are distinct
HTTP request waves, but neither crossed quote creation.

## Accepted outgoing plan

The accepted send selected three exact inputs:

| Input | Amount |
|---|---:|
| 1 | 574 sats |
| 2 | 250 sats |
| 3 | 25,000 sats |
| **Total** | **25,824 sats** |

The accepted payment accounting was:

| Component | Amount |
|---|---:|
| Invoice | 1,000 sats |
| Maximum outgoing fee | 25 sats |
| Required HTLC value | 1,025 sats |
| Expected change | 24,799 sats |

## Actor-local Ark result

The Ark co-sign request succeeded and produced the following locally persisted
outputs on the actor:

| Output state | Amount |
|---|---:|
| Locked HTLC | 201 sats |
| Locked HTLC | 250 sats |
| Locked HTLC | 574 sats |
| Spendable change | 129 sats |
| Spendable change | 24,670 sats |

The HTLC outputs total exactly 1,025 sats. The change outputs total exactly
24,799 sats. Together they conserve the complete 25,824-sat input value.

The actor contained no row in `bark_lightning_send`. That table is populated
only after Lightning initiation, so the invoice was never handed to the Ark
server for payment. No Lightning routing attempt occurred.

Members 2-5 remained at the accepted start checkpoint. This actor/follower
asymmetry is normally resolved by submitting the exact actor evidence to
consensus and replaying it on followers. The validation error prevented that
reconciliation step.

## Root cause

The outgoing path creates one logical HTLC destination and, when necessary,
one logical change destination in:

```text
../bark-cdk/bark/src/actions/lightning/pay.rs
```

It passes those destinations and all selected inputs to
`ArkoorPackageBuilder::new_with_checkpoints`. The package builder explicitly
supports splitting destinations across inputs:

```text
../bark-cdk/lib/src/arkoor/package.rs
```

Arkoor transactions have one input, so a logical destination may materialize
as multiple VTXOs carrying the same policy. Dust isolation can introduce
additional fragments as well. `ArkoorDestination` documents this behavior in:

```text
../bark-cdk/lib/src/arkoor/mod.rs
```

The federation facade nevertheless applied this check to the materialized
evidence:

```rust
ensure!(
    !evidence.htlc_vtxos.is_empty() && evidence.change_vtxos.len() <= 1,
    "HTLC evidence has an invalid output shape",
);
```

That check confuses one logical change destination with one physical change
VTXO. It was introduced with the original federated-send execution commit and
has no documented protocol rationale or regression coverage.

The implementation plan requires canonical full HTLC VTXOs and canonical
change VTXOs, in the plural. The code following the faulty check already
validates the correct invariants:

- HTLC and change IDs are canonical, distinct, and disjoint;
- every VTXO is valid for the configured Ark server;
- every HTLC has the accepted key, payment hash, expiry, and policy;
- every change VTXO has the accepted change key and policy;
- aggregate HTLC value equals payment plus fee; and
- aggregate change equals accepted inputs minus payment and fee.

Multiple valid change VTXOs do not weaken any of these properties.

## Side-effect ordering that made the failure persistent

`request_lightning_send_htlcs` performs the following work:

1. Register the accepted input VTXOs with Ark.
2. Construct the exact Arkoor package.
3. Obtain and validate server co-signatures.
4. Store the HTLC VTXOs as locked.
5. Mark the selected inputs spent in the actor-local Bark database.
6. Store the change VTXOs as spendable.
7. Update the existing outgoing movement.
8. Return the stored HTLC/change IDs to the federation facade.

The facade then reloads the persisted VTXOs and runs
`validate_send_htlc_evidence`. The incorrect output-count check occurs at this
point, after steps 4-7.

The accepted checkpoint was advanced locally to `HtlcReceived`, so retries do
not contact Ark or duplicate the co-sign operation. They reload the same five
VTXOs, reconstruct the same evidence, and reject its two change VTXOs again.
This is why ordinary retrying cannot heal the action.

## Why the causal error was absent from production logs

The Bark facade returns the specific `HTLC evidence has an invalid output
shape` error. The CDK Bark runtime maps every error from that call to the
generic `SendExecutionFailed` variant. The Axum federation layer maps that to
`cdk::Error::Internal`. Finally, the payment observer catches the error and
logs only a `debug` message saying that send planning left the quote pending.

The production deployment runs with `RUST_LOG=info`, so the final message and
its already-erased cause are not visible. The observable logs therefore showed
only unrelated maintenance failures and the later quote rejections.

Relevant code paths:

- `../bark-cdk/bark/src/federation.rs`: build and validate outgoing evidence;
- `crates/cdk-bark/src/runtime.rs`: map Bark execution into a semantic transition;
- `crates/cdk-axum/src/federation/bark_outgoing.rs`: drive one accepted outgoing phase;
- `crates/cdk-axum/src/federation/payment_observation.rs`: catch advancement errors.

## Effect on later outgoing requests

At the time of the two later request waves, every member had the same accepted
projection:

| State | VTXO count | Total |
|---|---:|---:|
| `spendable` | 1 | 92 sats |
| `reserved` | 6 | 36,147 sats |
| `pending_spend` | 1 | 10,000 sats |
| `spent` | 6 | 36,139 sats |

The non-terminal reservations were:

| Action kind and phase | Count | Total |
|---|---:|---:|
| Outgoing send, `send_planned` | 3 | 25,824 sats |
| Refresh, `refresh_planned` | 2 | 10,223 sats |
| Incoming claim, `claim_planned` | 1 | 100 sats |
| Incoming claim, `receive_preimage_disclosed` | 1 | 10,000 sats pending spend |

The federated Bark quote adapter sums only accepted `spendable` VTXOs and
requires that amount to cover the invoice plus maximum fee. Any ordinary
payment requiring more than 92 sats therefore fails before external I/O.

Several quote-validation cases are intentionally collapsed into
`AmountMismatch`, and request invoices are not logged. The production logs
alone cannot select the exact branch. For normal amount-bearing, whole-sat
invoices within configured limits, the accepted 92-sat available balance is
the deterministic cause.

## Impact and recovery constraints

- The 1,000-sat invoice was not paid over Lightning.
- The original Cashu melt remains pending rather than reaching a paid or failed
  terminal state.
- The actor locally holds the Ark-cosigned HTLC and change VTXOs, while
  consensus still reserves the original inputs.
- The original quote is expired and must not be resumed as a fresh payment.
- Recovery must use the accepted failed-send/revocation protocol so every
  member converges on the same public VTXO state.
- Database rows must not be deleted or manually rewritten to release funds.
- The older stuck refresh and incoming actions are separate incidents but must
  also be recovered before their reserved value becomes spendable.

## Corrective action and regression criteria

The immediate code correction is to remove the single-change-VTXO assumption
while preserving bounded evidence size and all existing cryptographic,
canonicality, policy, and amount-conservation checks.

Regression coverage must prove:

1. A federation send accepts one logical change destination materialized as
   multiple physical change VTXOs.
2. Multiple change VTXOs must be canonical, distinct, valid, and carry the
   exact accepted change policy.
3. Their aggregate amount must equal the accepted change amount.
4. An exact retry after actor-local persistence reconstructs the same evidence
   without a second Ark co-sign request.
5. CDK can encode, submit, accept, and replay the complete multi-change
   `SendHtlcsCosigned` transition.
6. Invalid extra, duplicate, unsorted, wrong-policy, or value-imbalanced change
   VTXOs remain rejected.

Future observability work should retain a bounded, operator-visible Bark error
classification so a phase failure cannot remain silently pending. Raw external
error strings must remain redacted because they can contain endpoints or local
paths.

Deployment verification must separately confirm:

- the deployed binary contains the corrected validator;
- the expired production action enters recovery rather than Lightning
  initiation;
- its reservation is released only through accepted consensus transitions;
- all five member projections converge; and
- a fresh outgoing payment can progress beyond `send_planned` without creating
  duplicate Ark side effects.

## Remediation

The immediate Bark fix replaced the physical-output assertion with a bounded
shape validator. It now requires at least one HTLC VTXO and permits up to 1,024
HTLC and 1,024 change VTXOs per class. The existing canonicality,
distinctness, Ark-server, policy, expiry, and value-conservation validation is
unchanged.

A production-shaped regression constructs the exact Arkoor split from this
incident. It proves that three inputs of 574, 250, and 25,000 sats materialize
as HTLC outputs of 201, 250, and 574 sats plus change outputs of 129 and 24,670
sats, and that this shape is accepted. Separate boundary cases reject an empty
HTLC set and either output class above the hard bound.

End-to-end recovery exposed four follow-on replay and sequencing defects. Each
was corrected without weakening accepted-state validation:

1. Applying `payment_failed` no longer degrades the runtime before the normal
   accepted revocation can be proposed. A genuine revocation execution or
   replay error still marks recovery required.
2. Bark accepts exact replays of earlier `send_htlcs_cosigned` and
   `payment_initiated` evidence after the local checkpoint has advanced into
   `revocable_htlcs` or `revocation_stuck`. The stored HTLC IDs, change IDs,
   mailbox, and payment-initiation state must still match exactly.
3. New send proposals still require a `PENDING` Cashu quote, while startup
   replay of an already accepted send also permits its quote to have reached
   `PAID` or `FAILED`. This prevents terminal quote resolution from making a
   valid historical plan unreplayable.
4. A rotated AlephBFT session is considered progressed only when its
   per-session unit cursors have advanced. The lifetime-wide finalized-item
   cursor no longer makes an empty session demand nonexistent backup bytes on
   restart.

The first accepted revocation execution also found a short application race at
the FROST boundary. A signer could see the applied journal entry just before
its Bark projection exposed the corresponding reservations. Custody validation
now waits for up to two seconds, polling every 20 milliseconds, for that exact
already-accepted projection to become visible. No nonce or Ark request is
created during this wait. A permanent locator, amount, state, reservation, or
action mismatch still fails closed after the bound.

The split-revocation regression now uses three physical HTLC inputs of 201,
250, and 574 sats that share the one accepted logical locator and recover into
the sole preallocated revocation locator. It proves aggregate recovery of 1,025
sats while retaining all action, predecessor, request, and locator bindings.

## Deployment and production recovery

The corrected CDK and sibling Bark builds were deployed with the existing
update procedure. Deployment verification reported all five processes alive,
all five federation members globally ready, all five Bark runtimes ready,
identical operation and application state, live AlephBFT networking, and no
audit, payment-conflict, or invalid-operation alerts.

The original action then advanced through the following accepted operations:

| Operation index | Accepted event |
|---:|---|
| 171 | The original three-HTLC/two-change Ark result was accepted as `send_htlcs_cosigned`. |
| 172 | Lightning payment initiation was recorded. |
| 173-177 | All five members recorded matching payment observations. |
| 178 | The already-expired invoice reached `payment_failed`; it was not retried as a new payment. |
| 212 | The exact three-HTLC failed-send revocation plan was accepted. |
| 213 | Ark revocation completed and the exact recovered VTXOs were accepted. |
| 214-218 | All five members recorded matching failed-result observations. |
| 219-223 | All five members recorded `FAILED` melt-quote payment observations. |

Final database and operator-status checks showed:

- the action at `revocation_completed`;
- the 1,000-sat quote with its 25-sat fee reserve at `FAILED`;
- recovery-required action count and value both zero;
- three formerly locked HTLC VTXOs, totalling 1,025 sats, moved to `spent`;
- three accepted recovery outputs, also totalling 1,025 sats, became
  `spendable`;
- all five Bark projections at the same accepted frontier; and
- no `local_rate_limited` error or repeated revocation failure in the final
  deployment window.

One Ark connection was made for the accepted revocation. No second outgoing
HTLC package was created. The original Cashu proof reservation was released
only after revocation completion and the threshold of failed observations.

The deployment still contains older unfinished refresh and incoming-receive
actions documented elsewhere. Their maintenance and expired-HTLC warnings are
not part of this outgoing failure, and their reserved values were not modified
as part of this recovery.
