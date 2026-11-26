# Federation payment trust model

Consensus orders payment observations, but a quote's payment state never
changes because one member's backend said so. This document states what the
federation verifies about incoming and outgoing payments, how mint and melt
quotes move through consensus, and what remains trusted. Custody of the
federation's own funds is in [custody](custody.md).

## Observation policy

`FederationConfig.payment_observation_policy` sets a policy separately for mint
quotes (incoming) and melt quotes (outgoing):

- `member_quorum { required }`: a quote changes state only when `required`
  authenticated members submit matching observations. `required` must be at
  least the consensus threshold and at most the member count.
- `development_single_observation`: one accepted observation changes state.
  It is for local development and regtest only, and `cdk-mintd` refuses it
  unless `federation.allow_development_payment_observation_policy = true`.

Observations are `MintQuotePayment` and `MeltQuotePayment` operations. Each
carries the observing member, quote, method, observed state, amounts and the
available payment identity or proof; the signed author must be the observer.
The conflict key is scoped by quote, state and observer, so one member cannot
submit two different observations for the same quote state. Observations are
stored per member (`federation_payment_observations`) and evaluated against the
policy inside the apply transaction.

Matching observations must agree on quote, method, state, amounts and payment
proof material. Mint payment observations use a canonical settlement identity
(the smallest durable payment label), so members that see the same payment set
form matching groups. The conflict rule is minimal: minority disagreement is
kept for diagnostics and does not veto a matching threshold, but if two
incompatible groups both reach the threshold the quote stays unresolved and
produces no spendable proofs and no definitive success. There is no quarantine
operation and no operator quote freeze.

Authorization is temporal: evidence must be accepted at or before the value
operation it authorizes, and a later observation cannot authorize an earlier
operation.

## What `member_quorum` proves

`member_quorum` proves that the required number of authenticated members
asserted the same result. It does not prove independent settlement: CDK does
not verify that members use independent backends, an origin proof, a threshold
gateway or an on-chain proof. Members sharing one backend or provider reduce
the practical independence of the quorum without lowering the threshold. This
model alone is not a production payment-safety claim for an arbitrary
`MintPayment` backend; the custody backends below add their own evidence
rules. The fake wallet exercises the same consensus and recovery paths but
proves nothing about real payments.

## Mint quotes

`MintQuote` is a consensus operation: a wallet asks one member, and every member
learns the quote from the accepted operation. Payment probes run outside
deterministic application and submit only a terminal `Paid` observation. With a
quorum policy, a single observation is stored but does not change
`amount_paid`; a matching quorum does. Issuance (`Mint`) is consensus-gated,
consumes only the consensus-authorized mintable amount, and binds shares to the
exact accepted outputs. A Bolt11 mint must issue exactly the quote amount.

## Melt quotes

`MeltQuote` creates the shared quote. `Melt` is the accepted reserve and
execution request: it validates inputs, witnesses, fees, balance and change
outputs against the federation keysets, moves the inputs to pending, creates
the melt saga and determines which member may attempt the external payment.
`MeltQuotePayment` observations then decide the outcome:

- a paid quorum finalizes the melt in the apply transaction: quote paid, inputs
  spent, change signed, saga and request removed;
- a failed or unpaid quorum rolls the quote back to unpaid and releases the
  inputs;
- anything else keeps the inputs pending.

There are no separate finalize or rollback operations: both are deterministic
consequences of accepted observations, so evidence is not duplicated and
operators can inspect conflicting observation rows.

**Payment actor.** Exactly one member may attempt an outgoing payment at a time.
The actor is the melt's author; if the payment is still unresolved after
`FEDERATION_EXTERNAL_ACTION_ACTOR_LEASE_ITEMS` (8) further journal entries, the
lease moves to the next roster member, and so on. The actor is computed at the
exact index where the decision was ordered, so catch-up agrees on it. Other
members keep the reservation and wait for observations.

**Crash boundaries.** The actor claims the saga (`SetupComplete` to
`PaymentAttempted`) before calling the backend and stores a caller-supplied
recovery identity where the method supports one. Restart and lease takeover
check the same payment identity rather than paying again; an uncertain outcome
stays pending. Late observations after settlement or rollback are idempotent. A
paid melt whose saga is still finalizing after a crash completes from the saga
on a later observation.

Recovery identity is payment-method specific. The staged saga does not make a
backend without a durable recovery identity crash-safe.

## Bark Lightning

For Ark-backed Lightning (`federation-bark`), the Bark projection is the
authority for payment results, and members exchange observations of the Ark
protocol through Bark `MemberObservation` operations. A quorum of matching
observations is certified as an `EvidenceCertificate`, and only certified
evidence advances an action:

- **Incoming.** A `ReceiveIntent` (receive secret commitment, locator, amounts,
  policy) is accepted before the invoice is created, and `ReceivePrepared`
  accepts the exact invoice. The encrypted receive secret is replicated to the
  members' protected receive-secret stores, with signed proofs of durable
  storage, so losing the invoice creator cannot strand a paid invoice. A claim
  plan must contain certified receive evidence, and replay applies that
  evidence from the accepted plan; a standalone HTLC-ready transition cannot
  authorize a claim. The mint quote is paid through the ordinary observation
  path.
- **Outgoing.** An accepted send plan binds the payment identity. Selected
  observers check the exact payment's status before initiating it, including
  the original actor after a restart; only a typed "missing" result permits
  initiation. Pending, a failed check or a timeout is not missing. Certified
  terminal evidence settles the melt.

Accepted Bark payments are observed and settled under the caught-up recovery
gate, so local recovery degradation cannot block settling a payment consensus
already authorized. A wallet-unavailable, invalid-projection or
unavailable-participant state cannot enter that path, and new plans need full
readiness.

## On-chain deposits

On-chain mint quotes are paid to consensus-allocated addresses of the joint
wallet. A deposit becomes a spendable UTXO and counts toward the quote only when
the configured observation quorum reports the exact outpoint, amount, script,
block and allocation at the configured confirmation depth. A member-local
mempool or block observation never creates liability. Reorg handling is in
[custody](custody.md#chain-observation-and-reorgs).

## Operator visibility

Signed `local-status` and `cdk-mintd federation payment-observations` show
redacted reports per quote: context, status, observer counts, conflicting
members and whether a quorum formed, without payment proof material. By default
only unresolved or conflicting reports are listed; `--include-resolved` adds
the rest.

## Open limitations

- Settlement evidence is member assertion, not independently verifiable proof,
  for generic `MintPayment` backends.
- Which members may originate observations in each readiness state is not
  separately restricted beyond readiness gating.
- Every production backend needs a durable, caller-controlled recovery identity
  before external payment effects are safe.
