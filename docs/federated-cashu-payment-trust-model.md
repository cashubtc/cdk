# Federated Cashu Payment Trust Model

This document records the payment-observation policy for federated Cashu before
real funds are enabled. It is part of Milestone 19 in the federation plan.

The central rule is:

> Consensus may order payment observations, but real-backend quote state must
> not finalize from one member's observation alone.

The current implementation models local smoke/regtest single-observation
behavior as an explicit `development_single_observation` policy and makes
production payment policies explicit in the public federation config. Fakewallet
is only one development backend that can be used with that policy. Durable
per-member observation rows now persist ordered evidence by quote and context.
Mint quote payment observations now finalize only after stored evidence
satisfies the configured policy; melt observations are also persisted and
policy-gated before state transitions.

## Policy Surface

`FederationConfig` contains `payment_observation_policy` with separate policies
for:

- `mint_quotes`: incoming payments used before mint issuance.
- `melt_quotes`: outgoing payments used before finalizing melts and change.

Each side can currently use one of two policy families:

- `development_single_observation`: local development/test policy. One accepted
  observation can update quote state. This is for local smoke/regtest setups,
  including fakewallet-backed tests.
- `member_quorum`: production-oriented policy. A configured number of matching
  member observations is required before quote state can finalize.

Federation config accepts only these current policy names; undeployed legacy
aliases are rejected. The real policy deliberately records no descriptive
backend trust-model label. CDK currently enforces authenticated matching member
observations and their required count, but it does not verify that members use a
shared read-only backend, origin proof, threshold gateway, or onchain proof.
Such a model should be added to consensus config only together with its actual
evidence verifier, admission rules, and failure tests.

> **Update 2026-07-18:** settlement identity is now canonical — mint payment
> observations select the smallest durable payment label instead of the last
> payment in backend return order, so exact-match evidence groups agree across
> members given the same visible payment set (milestone `P3` in
> `docs/federated-cashu-consensus-deadlocks-2026-07-18.md` with
> red-before-green proof). The remaining trust-model commitment and the
> proposer-only invoice visibility dead end are tracked as discovered bug
> `DB-005` in that ledger.

Quorum policies must require at least the configured consensus threshold and no
more than the member count. This prevents a single malicious member from marking
a quote paid or settled in a production configuration, but it does not by
itself prove that honest members observed the backend through independent trust
paths.

Payment observation operations now include the observing federation member ID.
The journal conflict key is scoped by quote, state, and observer, so one member
cannot submit two different observations for the same quote state, while
different members can submit matching evidence for quorum formation. A pure
quorum helper separates matching observers from conflicting observers before the
state machine is allowed to finalize a real-backend observation.

## Mint Quote Observations

Mint quote creation remains a consensus operation. A wallet can ask one member
for a quote, and all members learn the quote through the accepted operation log.

For development single-observation tests:

- Members may submit a paid observation after the configured local development
  backend reports success.
- One accepted observation can increment `amount_paid`.
- Mint issuance is still consensus-gated and tied to the exact output set.

For quorum policies:

- A single paid observation is not enough to mutate `amount_paid`.
- Matching observations must agree on quote ID, method, state, amount paid,
  issued amount, and backend proof material.
- Minority disagreement is retained for diagnostics without vetoing a matching
  threshold. Two incompatible threshold-supported results leave the quote
  unresolved.

The storage layer now persists per-member observations with quote, observer,
state, context, and typed JSON evidence. The mint state machine evaluates those
stored rows before mutating `amount_paid`: a single quorum-policy observation is
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
members, and quorum flags without returning backend proof material. Minority
disagreement is diagnostic and does not veto a matching threshold. If two
incompatible evidence groups both reach threshold, deterministic apply leaves
the quote unresolved. Private REST catch-up replays the accepted observations
into the same report and policy path.

## Melt Quote Observations

Melt quote creation and melt reserve are consensus operations. Inputs are
reserved or spent only through the accepted melt flow.

For development single-observation tests:

- A paid observation finalizes the pending melt saga and signs change shares.
- A failed or unpaid observation rolls back the pending saga and releases inputs.
- Pending observations can survive restart and catch up through the journal.

For quorum policies:

- A single state transition observation must not finalize payment success,
  rollback, or failure.
- Members need matching evidence before changing from pending to paid, failed,
  or unpaid.
- Outgoing payment attempts need idempotency keys so catch-up and restart cannot
  trigger duplicate external payments.

Melt observations are persisted and evaluated against stored quorum evidence
before quote state transitions. Focused mint-side regressions now prove that a
single quorum-policy observation leaves a pending melt saga untouched, a matching
paid quorum finalizes the saga and spends inputs, and a matching failed quorum
rolls the quote back to unpaid and releases inputs. The observation report
helper treats pending melt observations as non-terminal progress, while
competing terminal paid, failed, or unpaid observations are marked as
conflicting evidence. The idempotent production payment executor remains future
work.

## Pending And Stale Quotes

Pending-quote capacity and expiry are not currently enforced, so federation
config does not advertise limits for them. The status projection uses one local
15-minute default to classify unresolved observations as stale; this operator
display preference is not committed to consensus config or its digest.

The status surface exposes unresolved and conflicting observations by quote ID,
member ID, state, report status, quote age, stale classification, and a redacted
operator action recommendation. `local-status` and
`cdk-mintd federation status` mark old unresolved evidence as `stale=true` with
`action=inspect`, including conflicting evidence, without logging secrets,
invoices, local paths, private keys, or proof secrets.

## Required Follow-Up

- Add tests proving stale pending quotes remain visible without blocking
  consensus across live process restart and catch-up scenarios.
