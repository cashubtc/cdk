# Fedimint Consensus Review

## Summary

Fedimint uses AlephBFT as an ordering engine. AlephBFT does not decide whether
a transaction is valid and does not directly mint ecash. Its job is to make all
honest guardians agree on one shared order of submitted consensus items.

Fedimint then executes those ordered items deterministically. For transactions,
that execution is where inputs are spent, outputs are created, and mint/ecash
outputs receive each guardian's blind-signature share.

The useful mental model is:

> AlephBFT decides "process these envelopes in this exact order." Fedimint opens
> each envelope, validates it, and updates the database.

## AlephBFT's Role

Each guardian gathers pending items it wants ordered. These items include:

- User transactions.
- Module consensus items.
- Session-signing data used while finishing a session.

Fedimint wraps these into AlephBFT unit data, gossips signed units among
guardians, and lets AlephBFT finalize a shared order. Once units are finalized,
Fedimint receives ordered units through its finalization handler and processes
the contained data.

In Fedimint's code, the integration boundary is mainly under:

- `fedimint-server/src/consensus/aleph_bft/data_provider.rs`
- `fedimint-server/src/consensus/aleph_bft/finalization_handler.rs`
- `fedimint-server/src/consensus/aleph_bft/network.rs`
- `fedimint-server/src/consensus/aleph_bft/keychain.rs`
- `fedimint-server/src/consensus/engine.rs`

## Responsibility Split

AlephBFT handles BFT agreement on ordering. Fedimint business logic does not
count how many guardians "committed" to each individual transaction or output
before processing it.

The split is:

- AlephBFT decides the finalized order of units.
- Fedimint receives ordered units and processes their contents.
- Fedimint modules decide whether each ordered item is valid against current
  state.
- Fedimint stores a signed `CatchUpCertificate` after the session is complete.

This means one guardian can propose a valid transaction. If AlephBFT finalizes
the unit containing that transaction, all honest guardians will process the
transaction in the finalized order. There is no extra rule requiring a quorum of
guardians to have proposed the same transaction.

That does not let one guardian force arbitrary state. A guardian can only
propose data. All honest guardians still run the same deterministic validation
rules before accepting any state transition.

Threshold counting still exists, but at different layers:

- AlephBFT uses threshold assumptions internally to finalize order.
- Fedimint validates threshold signatures on completed catch-up certificates.
- Clients wait for threshold API responses.
- Mint clients collect threshold blind-signature shares.

## Fault Model and Thresholds

Fedimint uses the standard Byzantine fault tolerant assumption:

- With `n` guardians, up to `(n - 1) / 3` can be faulty or malicious.
- The threshold required for consensus/signatures is `n - f`, where `f` is the
  maximum faulty count.

For example, in a 4 guardian federation:

- `f = 1`
- threshold is `3`

For example, in a 5 guardian federation:

- `f = 1`
- threshold is `4`

This is why client queries often wait for a threshold of matching or valid
responses rather than trusting one guardian.

AlephBFT handles offline or faulty guardians up to this fault threshold. If too
many guardians are offline, safety should still hold, but new consensus progress
stalls until enough guardians return. Fedimint adds operational handling around
this, such as session timeouts, persisted AlephBFT units for crash recovery, and
signed catch-up certificates for catching up, but it does not bypass the BFT
threshold requirement.

## Sessions and Rounds

Fedimint consensus is organized into sessions. A session collects and orders
items, produces a `CatchUpCertificate`, gathers a threshold of guardian signatures
over that outcome, stores it, and then moves to the next session.

There is not a simple "block every N seconds" rule.

There is a round pacing parameter:

- `broadcast_round_delay_ms`

That controls how quickly AlephBFT creates later rounds/units. Round 0 has no
delay. Later rounds use the configured delay with jitter. A session also has:

- `broadcast_rounds_per_session`

After that many rounds, Fedimint stops collecting more transaction batches for
the session and finishes the catch-up certificate.

So transaction processing is closer to continuous BFT ordering with round
pacing than fixed-interval block production.

## What Happens When a Wallet Submits a Transaction

A Fedimint wallet submits a transaction through the federation API. The API
client sends requests to all guardians and uses a query strategy that waits for
a threshold of compatible responses.

However, submission success is not the same as finality.

The flow is:

1. The client finalizes a transaction locally.
2. The transaction is submitted through the federation API.
3. Each guardian that receives it does a local validation pass against current
   state.
4. If the transaction appears valid, the guardian queues it as
   `ConsensusItem::Transaction`.
5. AlephBFT orders that transaction relative to other submitted items.
6. Fedimint processes the ordered transaction during consensus execution.
7. If accepted, the transaction is written into consensus state and output
   outcomes become queryable.

The important split is:

- Submission does a dry-run validation.
- Consensus execution commits the actual state changes.

On the server side this is visible in:

- `fedimint-server/src/consensus/api.rs`
- `fedimint-server/src/consensus/transaction.rs`
- `fedimint-server/src/consensus/engine.rs`

`submit_transaction` uses a read-only/non-committing database transaction and
calls transaction processing in `TxProcessingMode::Submission`. The writes made
during this pass are ignored. If the transaction passes, it is sent into the
consensus submission channel.

Later, after AlephBFT orders it, the consensus engine processes it in
`TxProcessingMode::Consensus` and commits the result.

## No Primary Transaction Receiver

There is no primary federation member for transaction submission. The wallet
sends the same transaction request to all guardians through the federation API.

The submit path is:

1. The wallet calls `submit_transaction(tx)`.
2. The federation API sends that request to all known guardians.
3. Each reachable guardian dry-runs validation.
4. Each guardian that considers the transaction valid queues
   `ConsensusItem::Transaction(tx)`.
5. The wallet waits for a threshold of compatible submission responses.
6. The wallet separately waits for the transaction to appear as accepted in
   consensus.

This means many guardians can try to enter the same transaction into consensus
at the same time. That is expected.

Duplicate proposals are harmless because the transaction has a deterministic
`txid`:

- Each guardian's AlephBFT data provider tracks transaction IDs it has already
  submitted in the current session and avoids submitting the same transaction
  twice locally.
- Across guardians, the same transaction can still appear more than once in the
  finalized order.
- The first accepted copy commits `AcceptedTransactionKey(txid)`.
- Later copies see that the transaction is already accepted and are rejected as
  duplicates.

Duplicate proposals may waste some bandwidth, but they do not mint twice, spend
twice, or create duplicate outputs.

## Ecash Issuance and Blind Signatures

For mint/ecash outputs, guardians do not hand back final spendable ecash
directly from the transaction submission call.

Instead:

1. The wallet creates blinded note issuance requests.
2. These blinded nonces are included in mint outputs in a transaction.
3. The transaction is submitted and ordered by consensus.
4. During consensus execution, each guardian processes the mint output.
5. Each guardian creates its own blind-signature share and stores it as the
   output outcome.
6. The client later queries guardians for output outcomes.
7. The client verifies the signature shares, collects a threshold, aggregates
   them, unblinds the aggregate signature, and obtains spendable ecash notes.

The mint server stores the blind-signature share during `process_output` in:

- `modules/fedimint-mint-server/src/lib.rs`

The mint client waits for output outcomes and verifies/aggregates shares in:

- `modules/fedimint-mint-client/src/output.rs`

For mint v2, signature shares are exposed through explicit signature-share
endpoints. The important property is the same: the share is stored during
consensus execution and later returned by lookup.

So asking for blind signatures is not a fresh issuance decision. It is a lookup
for the guardian's already-created share for a consensus-accepted output.

## Waiting and Timing

The wallet does not need to be online at the exact moment the transaction is
processed by consensus. Waiting works through persisted state and retries.

On the server side, long-running API calls wait on database keys. The pattern is:

1. Register for notification on the relevant DB key.
2. Check whether the key already exists.
3. If it exists, return immediately.
4. If it does not exist, wait for a notification.
5. Re-check the DB.

This avoids races:

- If the transaction was already accepted, the request returns immediately.
- If the transaction is accepted later, the request wakes up and returns.

For transaction acceptance, the server waits for `AcceptedTransactionKey(txid)`.
For output outcomes, the server first waits for the transaction to be accepted
and then reads the module's output status. For mint outputs, that status contains
or points to the guardian's stored blind-signature share.

On the client side, transaction and output flows are state machines:

- The transaction submission state machine keeps submitting the transaction with
  backoff.
- A separate transition waits until the transaction is accepted in consensus.
- Mint output state machines wait for transaction acceptance, then fetch
  persisted signature shares/output outcomes.
- If the wallet restarts, the state machines can resume from local state.

The practical rule is:

- Asking too early waits or retries.
- Asking after consensus processing is a lookup.
- Disconnecting during consensus processing does not lose the signature shares.

## Are Guardians Processing in Synchrony?

They are synchronized on final consensus order, not on API response timing.

Individual guardians can receive and pre-check a submitted transaction at
different times. The transaction becomes final only after it is ordered by
AlephBFT and accepted during deterministic consensus execution.

After that point, honest guardians should have equivalent consensus state for
the session. The client can then collect a threshold of valid output outcomes,
such as blind-signature shares for ecash issuance.

The wallet is not trying to stand in the same moment as every guardian. It uses
stable identifiers, such as transaction IDs, outpoints, and blinded messages, to
ask whether a threshold of guardians has reached the state containing its
transaction and outputs.

If a guardian has already processed the transaction, it can answer from stored
state. If it has not processed it yet, the request waits or the client retries.

## Client Connectivity Limits

Client-to-guardian availability is separate from guardian-to-guardian consensus
availability.

If the guardians can communicate with each other, a transaction can be accepted
by consensus even if the wallet initially reaches only one guardian. That one
guardian can propose the transaction to AlephBFT.

However, the wallet still needs enough API connectivity to finish client-side
flows:

- To observe accepted federation state, it waits for threshold API responses.
- To finalize ecash, it needs threshold valid blind-signature shares.

In a 5 guardian federation, the threshold is 4. If the wallet can only reach one
guardian forever, it cannot assemble spendable ecash, even if consensus accepted
the transaction. The shares are not lost; they remain stored by guardians. The
wallet can complete once it can reach enough guardians.

## Idempotency

Submitting the exact same transaction is intended to be idempotent.

If the wallet submits a transaction to one guardian, consensus accepts it, and
the wallet later submits the same transaction to another guardian that has
already processed it, that guardian:

1. Computes the same `txid`.
2. Finds `AcceptedTransactionKey(txid)` in its DB.
3. Returns success with that `txid`.
4. Does not process the transaction or create outputs again.

Blind-signature retrieval is also effectively idempotent for the same accepted
output. Once a guardian has processed the mint output, it stores its signature
share. Later requests return the stored share by outpoint or blinded message,
depending on the mint version and endpoint.

This idempotency only applies to the same transaction/output identifiers. A
different transaction that tries to spend the same input or reuse invalid data is
handled by normal validation and may be rejected.

## Implementation Invariants

An implementation that wants to preserve this behavior should keep these
invariants:

- Transaction IDs must be deterministic and stable.
- Submission must do validation without committing state.
- Consensus execution must be the only path that commits transaction effects.
- Output signature shares must be persisted during consensus execution.
- Signature-share APIs must return persisted shares, not independently decide to
  issue new ecash.
- API waits must handle both "already happened" and "will happen later" cases.
- Client state machines must persist enough information to resume waiting after
  disconnect or restart.
- Duplicate transaction submissions must converge to the same accepted `txid`
  or the same deterministic rejection.
- Duplicate ordered copies of an already accepted transaction must not reapply
  effects.

## Key Takeaways

- A wallet submits transactions to the federation API, which queries guardians
  and waits for threshold agreement.
- Guardians do immediate local validation, but this is not final processing.
- Transactions are queued for consensus as `ConsensusItem::Transaction`.
- There is no primary transaction receiver; all guardians may receive the same
  transaction.
- Multiple guardians may propose the same transaction into consensus.
- Duplicate proposals are deduplicated or rejected after the first accepted
  copy.
- AlephBFT orders batches of consensus items.
- Fedimint executes the ordered items deterministically.
- Mint blind-signature shares are created during consensus execution, not during
  the initial submit call.
- Signature share requests are lookups of persisted consensus results.
- The client waits for output outcomes, gathers threshold shares, verifies them,
  and aggregates them into final ecash.
- If the wallet cannot reach threshold guardians, it cannot complete some
  client-side flows, but accepted shares remain stored for later recovery.
- Exact transaction resubmission is idempotent after acceptance.
- There is round pacing via `broadcast_round_delay_ms`, but no simple fixed
  "transaction processing every N seconds" block time.
