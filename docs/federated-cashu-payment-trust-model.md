# Federated Cashu Payment Trust Model

This document records the payment-observation policy for federated Cashu before
real funds are enabled. It is part of Milestone 19 in the federation plan.

The central rule is:

> Consensus may order payment observations, but real-backend quote state must
> not finalize from one member's observation alone.

The current implementation keeps fake-wallet testing on the existing
single-observation path and makes real-backend policies explicit in the public
federation config. Durable per-member observation rows now persist ordered
evidence by quote and context. Until the DB-backed quorum finalizer is wired
into quote state transitions, real policies fail closed instead of silently
using fake-wallet finality.

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
state, context, and typed JSON evidence. The next implementation step is to
finalize the quote only after the configured quorum is satisfied by stored
evidence.

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

The current guard rejects single-observation state changes for real policies.
The next implementation step is to evaluate the stored quorum evidence before
finalizing melts, while the idempotent real payment executor remains future
work.

## Pending And Stale Quotes

The policy includes operator-facing limits:

- `max_pending_quotes`: maximum expected number of unresolved payment quotes.
- `pending_quote_ttl_secs`: age after which a pending quote should be inspected.
- `stale_observation_secs`: age after which unresolved observation evidence is
  stale.

These limits are validated as non-zero config values. The next admin work should
expose unresolved observations by quote ID, member ID, state, and age without
logging secrets, invoices, local paths, private keys, or proof secrets.

## Required Follow-Up

- Use the persisted per-member payment observation rows and deterministic
  matching rules from the pure helper inside the DB-backed finalizer.
- Add conflict quarantine for mismatched amounts, payment proofs, states, or
  backend-specific settlement references.
- Add admin tooling to inspect unresolved, conflicting, and stale observations.
- Add tests proving fake-wallet and real-backend policies use separate code
  paths, single real observations do not mark quotes paid, conflicting amounts
  do not silently update state, and stale pending quotes remain visible without
  blocking consensus.
