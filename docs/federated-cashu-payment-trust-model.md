# Federated Cashu Payment Trust Model

This document records the payment-observation policy for federated Cashu before
real funds are enabled. It is part of Milestone 19 in the federation plan.

The central rule is:

> Consensus may order payment observations, but real-backend quote state must
> not finalize from one member's observation alone.

The current implementation keeps fake-wallet testing on the existing
single-observation path and makes real-backend policies explicit in the public
federation config. Durable per-member observation rows now persist ordered
evidence by quote and context. Mint quote payment observations now finalize
only after stored evidence satisfies the configured policy; melt observations
are also persisted and policy-gated before state transitions.

## Policy Surface

`FederationConfig` contains `payment_observation_policy` with separate policies
for:

- `mint_quotes`: incoming payments used before mint issuance.
- `melt_quotes`: outgoing payments used before finalizing melts and change.

Each side can currently use one of two policy families:

- `fake_wallet`: test-only policy. One accepted observation can update quote
  state. This is for the fake wallet backend and local smoke tests.
- `member_quorum`: real-backend policy. A configured number of matching member
  observations is required before quote state can finalize.

The real policy also records a trust model:

- `shared_backend_read_only`: members independently read settlement from the
  same backend with read-only credentials.
- `origin_member_proof`: the origin member proves settlement and other members
  acknowledge the proof.
- `threshold_gateway`: a threshold gateway or payment processor emits the
  observation.
- `onchain_proof`: members verify transaction proof observations against a
  consensus block-height view.

Real quorum policies must require at least the configured consensus threshold
and no more than the member count. This prevents a single malicious member from
marking a quote paid or settled in a real-backend configuration.

Payment observation operations now include the observing federation member ID.
The journal conflict key is scoped by quote, state, and observer, so one member
cannot submit two different observations for the same quote state, while
different members can submit matching evidence for quorum formation. A pure
quorum helper separates matching observers from conflicting observers before the
state machine is allowed to finalize a real-backend observation.

## Mint Quote Observations

Mint quote creation remains a consensus operation. A wallet can ask one member
for a quote, and all members learn the quote through the accepted operation log.

For fake-wallet tests:

- Members may submit a paid observation after the fake backend reports success.
- One accepted observation can increment `amount_paid`.
- Mint issuance is still consensus-gated and tied to the exact output set.

For real backends:

- A single paid observation is not enough to mutate `amount_paid`.
- Matching observations must agree on quote ID, method, state, amount paid,
  issued amount, and backend proof material.
- Conflicting amount or proof observations must be quarantined or rejected by
  the observation accumulator before any mint signatures are produced.

The storage layer now persists per-member observations with quote, observer,
state, context, and typed JSON evidence. The mint state machine evaluates those
stored rows before mutating `amount_paid`: a single real-backend observation is
stored and marked applied but does not mark the quote paid, matching quorum
evidence finalizes the quote, and conflicting evidence remains stored without
silently increasing the paid amount. A shared
`FederationPaymentObservationReport` helper can now summarize the stored rows
for one quote, group matching evidence, mark whether any group reached quorum,
and identify conflicting observers for operator-facing surfaces. Mint quote
reports treat unpaid observations as unresolved progress, while paid or issued
evidence can conflict with other terminal evidence. Signed private REST
`local-status` responses and `cdk-mintd federation status` now expose redacted
report summaries with context, quote ID, status, observer counts, conflicting
members, and quorum flags without returning backend proof material.

## Melt Quote Observations

Melt quote creation and melt reserve are consensus operations. Inputs are
reserved or spent only through the accepted melt flow.

For fake-wallet tests:

- A paid observation finalizes the pending melt saga and signs change shares.
- A failed or unpaid observation rolls back the pending saga and releases inputs.
- Pending observations can survive restart and catch up through the journal.

For real backends:

- A single state transition observation must not finalize payment success,
  rollback, or failure.
- Members need matching evidence before changing from pending to paid, failed,
  or unpaid.
- Outgoing payment attempts need idempotency keys so catch-up and restart cannot
  trigger duplicate external payments.

Melt observations are persisted and evaluated against stored quorum evidence
before quote state transitions. Focused mint-side regressions now prove that a
single real-backend observation leaves a pending melt saga untouched, a matching
paid quorum finalizes the saga and spends inputs, and a matching failed quorum
rolls the quote back to unpaid and releases inputs. The observation report
helper treats pending melt observations as non-terminal progress, while
competing terminal paid, failed, or unpaid observations are marked as
conflicting evidence. The idempotent real payment executor remains future work.

## Pending And Stale Quotes

The policy includes operator-facing limits:

- `max_pending_quotes`: maximum expected number of unresolved payment quotes.
- `pending_quote_ttl_secs`: age after which a pending quote should be inspected.
- `stale_observation_secs`: age after which unresolved observation evidence is
  stale.

These limits are validated as non-zero config values. The current status
surface exposes unresolved and conflicting observations by quote ID, member ID,
state, report status, quote age, stale classification, and a redacted operator
action recommendation. `local-status` and `cdk-mintd federation status` mark old
unresolved evidence as `stale=true` with `action=inspect`, and conflicting
evidence as `action=quarantine`, without logging secrets, invoices, local paths,
private keys, or proof secrets.

## Required Follow-Up

- Add explicit consensus/state-machine quarantine operations for mismatched
  amounts, payment proofs, states, or backend-specific settlement references
  where policy should halt instead of only recommending quarantine.
- Add tests proving stale pending quotes remain visible without blocking
  consensus across live process restart and catch-up scenarios.
