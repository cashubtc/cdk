# Federated CDK Security Review

Date: 2026-07-09

This note records timing-sensitive issues found during a read-only review of
federation-mode minting, wallet behavior, and swap handling. It is intended as
a work queue for follow-up fixes and tests.

## Scope

Reviewed the federation minting paths around:

- Mint quote creation and consensus submission.
- Mint quote payment observation submission and replay.
- Mint issuance after accepted consensus operations.
- Federated swap submission, validation, consensus apply, and persisted share
  response handling.
- Wallet federation fanout, member validation, threshold share collection, and
  state-changing status decisions.
- AlephBFT finality wait/retry behavior.
- Journal status transitions and conflict-key handling.

No code was changed as part of the review.

## Summary

The main risk is that federated mint signing can currently depend on local
backend timing instead of only on consensus-ordered payment evidence. In a
federation, members must sign based on the same ordered state. Any path where a
member independently marks a quote paid before the federation has accepted a
payment-observation quorum can cause divergent signing decisions or rejected
operations that are artifacts of timing.

## Findings

### P1: Mint Signing Can Depend on Local Backend Timing

The federated mint apply path loads local mint quotes and signs when those quote
states are `Paid`.

Relevant code:

- `crates/cdk/src/mint/federation.rs:1533`
- `crates/cdk/src/mint/federation.rs:1579`
- `crates/cdk/src/mint/federation.rs:1630`

However, local quote state can be mutated outside consensus:

- `crates/cdk/src/mint/issue/mod.rs:603` calls `check_mint_quote_paid`.
- `crates/cdk/src/mint/ln.rs:46` checks the backend and mutates quote payment state.
- `crates/cdk/src/mint/mod.rs:1046` handles backend payment notifications and updates local quotes.

This creates a timing window:

1. A quote is accepted by consensus.
2. One member observes backend payment first and locally marks the quote `Paid`.
3. A wallet submits the mint operation.
4. That member signs because its local DB says `Paid`.
5. Another member has not locally observed payment yet and rejects with `UnpaidQuote`.

That violates the federation requirement that signing decisions derive from the
same consensus-ordered state.

Recommended fix:

- In federation mode, `apply_federation_mint` must require a quorum-satisfied
  `MintQuotePayment` observation at or before the mint operation index.
- Do not use raw local quote state as the authority for signing unless it was
  derived from consensus observations at or before that operation.
- Local backend checks should submit observations, but should not make a quote
  mintable before consensus payment evidence allows it.

Tests to add:

- A quote is locally marked `Paid` by backend polling, but no accepted
  payment-observation quorum exists. A federated mint operation must fail before
  signing.
- A payment-observation quorum finalizes after a mint operation index. The older
  mint operation must not become valid retroactively.
- A payment-observation quorum finalizes before the mint operation index. Mint
  signing succeeds deterministically on replay.
- Members with different local backend timing converge to the same journal
  apply result.

### P1: Apply-Time Rejection Can Leave Conflict-Key State Inconsistent

The Axum replay path maps definitive apply errors to `Rejected`.

Relevant code:

- `crates/cdk-common/src/error.rs:714`
- `crates/cdk-axum/src/lib.rs:3388`
- `crates/cdk-axum/src/lib.rs:3691`

`UnpaidQuote` is classified as definitive, so a mint operation that applies
before payment-observation quorum can be marked `Rejected`.

The in-memory federation journal status update only changes the entry status:

- `crates/cdk-common/src/federation/journal.rs:1209`

It does not rebuild or release the conflict-key map. Rejected entries are
modeled specially when they are append-time conflict rejections:

- `crates/cdk-common/src/federation/journal.rs:922`
- `crates/cdk-common/src/federation/journal.rs:1244`

Apply-time rejection is a different case: the operation was accepted first and
its conflict keys were already reserved. Changing its status to `Rejected` after
the fact can leave the conflict-key map inconsistent in memory and may restore
badly after restart.

Recommended fix:

- Separate consensus acceptance status from local materialization/apply status.
- Avoid converting timing-dependent `UnpaidQuote` into a terminal journal
  rejection for accepted operations.
- If status changes are allowed to affect conflict-key reservation, make
  `update_status` update the conflict-key map consistently and add restore tests.

Tests to add:

- Accepted mint op applies before payment quorum and is not turned into a
  conflict-key-leaking rejected state.
- Status update from accepted to rejected does not leave the same quote
  permanently conflict-reserved.
- Restart/rebuild from persisted journal with apply-time rejected entries
  preserves the intended conflict semantics.

### P2: Replayed Mint Quotes May Not Carry a Usable Backend Lookup ID

Federated quote replay reconstructs the quote with:

- `PaymentIdentifier::QuoteId(quote_id)` in `crates/cdk/src/mint/federation.rs:2256`

The proposer-created local quote uses the backend-provided lookup ID:

- `crates/cdk/src/mint/issue/mod.rs:337`

Payment polling then checks the backend using:

- `crates/cdk/src/mint/ln.rs:46`

Fake wallet testing may hide this because it can accept simplified identifiers.
Production Lightning backends may not be able to independently verify an invoice
created by another member using only the Cashu quote ID.

Recommended fix:

- Decide the production payment-verification model explicitly.
- If every member independently checks the same backend-visible invoice, the
  consensus quote operation must carry the stable backend lookup data needed for
  all members to check it.
- If only the proposer can check the invoice, then the federation needs an
  explicit trust/quorum policy for proposer observations rather than pretending
  every member independently verifies the backend.

Tests to add:

- Non-proposer member can validate a replayed quote payment using only data from
  the accepted quote operation.
- If lookup data is missing or unusable, payment observation fails safely and
  does not make the quote mintable.
- Fake wallet tests model both success and lookup failure.

### P2: Local Quote Is Persisted Before Quote Consensus Finality

The public quote route creates and persists a local quote before submitting the
quote operation to federation consensus.

Relevant code:

- `crates/cdk-axum/src/custom_handlers.rs:681`
- `crates/cdk/src/mint/issue/mod.rs:363`
- `crates/cdk-axum/src/custom_handlers.rs:687`

Existing tests cover fail-closed behavior before quote creation when AlephBFT is
not ready, but the route can still fail or time out after local invoice creation
and before the quote operation is finalized.

This can leave a local-only quote/invoice that is not in the accepted federation
order.

Recommended fix:

- Treat quotes created before consensus finality as provisional.
- Do not let provisional/local-only quotes become mintable or emit payment
  observations that can authorize signing.
- Prefer materializing durable federation quote state only from the accepted
  quote operation, or explicitly track local quote provenance and finality.

Tests to add:

- Quote creation succeeds locally, but consensus submission times out. The quote
  must not become mintable.
- A local-only quote paid by the backend must not authorize a mint operation.
- Retrying quote creation/submission either finalizes the same quote cleanly or
  produces a clearly rejected/provisional state without leaking mintable value.

## Wallet Perspective Findings

These findings focus on whether malicious, stale, offline, or slow federation
members can cause incorrect wallet state, failed recovery, or avoidable race
conditions.

### P1: Single-Member Quote Status Fallback Can Corrupt Melt Recovery

The federated wallet connector can fall back to a single primary member for melt
quote status:

- `crates/cdk/src/wallet/federation.rs:718`
- `crates/cdk/src/wallet/federation.rs:1274`
- `crates/cdk/src/wallet/federation.rs:1704`

Melt recovery and execution paths treat that status as authoritative:

- `crates/cdk/src/wallet/melt/saga/resume.rs:95`
- `crates/cdk/src/wallet/melt/saga/resume.rs:344`
- `crates/cdk/src/wallet/melt/saga/mod.rs:955`
- `crates/cdk/src/wallet/melt/saga/mod.rs:1204`

This can create incorrect wallet state during a partition or malicious-primary
scenario:

1. The wallet submits a melt.
2. Threshold federation members may have accepted the melt or marked inputs
   pending/spent.
3. Status quorum is unavailable because members are offline or slow.
4. The primary member returns `Unpaid` or `Failed`.
5. The wallet releases proof reservations and deletes the saga.
6. The wallet now believes proofs are spendable even though the federation may
   still consider them pending or spent.

Recommended fix:

- Quote statuses that mutate wallet state or drive saga recovery must require
  threshold agreement from validated federation members.
- Single-member status fallback may only be used for non-authoritative display
  state and must not release proofs, delete sagas, mark melts failed, or mark
  quotes final.
- If threshold status is unavailable, leave the local melt pending and retry
  later.

Tests to add:

- Primary returns `Failed`, quorum is unavailable, and the wallet keeps melt
  proofs reserved/pending.
- Primary returns `Unpaid`, threshold members later return `Paid`, and wallet
  recovery completes without losing the saga.
- A malicious primary cannot cause `compensate_melt` to run without threshold
  status agreement.
- Threshold `Pending` or unavailable status never mutates proofs to `Unspent`.

### P1: Federation Member Validation Needs Explicit Timeouts

Federation keyset/config validation fetches member metadata without an explicit
wallet-level timeout:

- `crates/cdk/src/wallet/federation.rs:498`
- `crates/cdk-http-client/src/transport/mod.rs:88`
- `crates/cdk-http-client/src/backends/reqwest_backend.rs`

Signing requests use bounded member request behavior, but validation can wait on
transport behavior. With some transports, this may be very long or effectively
unbounded. If threshold cannot be reached and one member hangs, mint, swap,
melt, restore, or check-state operations can stall instead of returning a clear
insufficient-valid-members error.

Recommended fix:

- Wrap federation metadata validation calls in `member_request_timeout` or a
  dedicated metadata timeout.
- Treat timeout as a non-definitive member failure.
- Return a clear error when fewer than threshold members validate before the
  timeout budget is exhausted.

Tests to add:

- One member hangs during `get_mint_info`; threshold is still reachable and the
  operation proceeds.
- One member hangs during `get_mint_keys`; threshold is not reachable and the
  wallet returns a bounded error.
- Validation timeout does not mutate wallet proofs, quotes, or sagas.

### P1: Valid-Member Cache Can Pin Wallet to a Dead Threshold Set

The wallet caches the first threshold-valid member set:

- `crates/cdk/src/wallet/federation.rs:498`
- `crates/cdk/src/wallet/federation.rs:585`

Mint, swap, melt, restore, and check-state paths then use that cached set:

- `crates/cdk/src/wallet/federation.rs:1603`
- `crates/cdk/src/wallet/federation.rs:1653`
- `crates/cdk/src/wallet/federation.rs:1731`
- `crates/cdk/src/wallet/federation.rs:1752`
- `crates/cdk/src/wallet/federation.rs:1813`

If the cache contains exactly threshold members and one later goes offline, the
wallet may fail even when other federation members are healthy and valid.

Recommended fix:

- Add cache invalidation or a TTL for the validated member set.
- On insufficient responses from the cached set, rebuild validation from the
  full federation roster and retry once before failing.
- Do not let a stale cache reduce the effective federation size.

Tests to add:

- Cache members 1, 2, 3 in a 3-of-5 federation, take member 1 offline, and
  confirm the wallet revalidates and succeeds with members 2, 3, 4.
- A stale cached member advertising old keysets is removed from the cache before
  signing.
- Cache refresh failure does not release reservations or delete sagas.

### P2: Melt Quote Creation Does Not Require Threshold Visibility

Mint quote creation verifies that the quote is visible to threshold members, but
melt quote creation currently uses first-healthy-member behavior:

- `crates/cdk/src/wallet/federation.rs:1578`
- `crates/cdk/src/wallet/federation.rs:868`
- `crates/cdk/src/wallet/federation.rs:1674`

A malicious or stale member can return a melt quote that is not known by the
federation quorum. Later melt execution may fail, and status fallback can make
recovery worse.

Recommended fix:

- Add a threshold visibility check for melt quotes before the wallet stores or
  uses them.
- Ensure the quote was accepted by the federation consensus path before a wallet
  reserves proofs against it.
- Validate the selected member against the federation config before quote
  creation.

Tests to add:

- Primary creates a melt quote but fewer than threshold members can see it; the
  wallet rejects the quote.
- Threshold members see the quote; the wallet accepts and can melt with it.
- Timeout during melt quote visibility does not reserve proofs.

### P2: Quote Status Aggregation Should Use Validated Members

Status collection currently uses all configured connectors, while signing paths
first validate member config and aggregate keysets.

Relevant code:

- `crates/cdk/src/wallet/federation.rs:718`
- `crates/cdk/src/wallet/federation.rs:756`
- `crates/cdk/src/wallet/federation/validation.rs:49`
- `crates/cdk/src/wallet/federation/validation.rs:69`

A member that would fail federation metadata or keyset validation can still
contribute status responses. That is less dangerous than invalid signing shares,
but it can still mislead wallet state transitions.

Recommended fix:

- Run status aggregation over the validated member set.
- Alternatively, validate a member's federation metadata before accepting a
  status response from it.
- Require status responses used for wallet state transitions to be threshold
  matching and from currently valid federation members.

Tests to add:

- A member with mismatched federation config returns `Paid`; wallet ignores it.
- A member with non-v3 or mismatched aggregate keysets returns status; wallet
  ignores it for state-changing decisions.
- Threshold matching status from valid members still updates wallet state.

### P2: Insecure Threshold Configurations Are Accepted Structurally

The public config validation checks threshold bounds, but does not enforce a
production safety profile:

- `crates/cdk-common/src/federation.rs:1488`
- `crates/cdk-cli/src/sub_commands/federation.rs:190`

A multi-member federation with `signature_threshold = 1` can be structurally
valid. In that configuration, one malicious member can produce complete
signatures.

Recommended fix:

- Add a wallet/CLI production profile that rejects obviously unsafe federation
  threshold settings.
- Keep explicit dev/test overrides for local fake-wallet federation tests.
- Show a clear warning when importing or joining a federation whose threshold
  profile is unsafe.

Tests to add:

- CLI federation join rejects a multi-member production federation with
  `signature_threshold = 1`.
- Dev/test mode can opt in to weak thresholds explicitly.
- Wallet connector refuses to silently downgrade threshold assumptions.

### P3: Single-Member Mint Info Can Mislead Wallet Feature Decisions

After validation, `get_mint_info` still returns a single member's mint info:

- `crates/cdk/src/wallet/federation.rs:1783`

This should not bypass signature verification, but it can mislead display,
feature detection, or payment-method selection if that member is stale.

Recommended fix:

- Synthesize federation mint info from public config where possible.
- For fields that can affect wallet behavior, require threshold-agreed metadata
  or treat the data as non-authoritative.

Tests to add:

- One valid member advertises stale optional features; wallet does not enable a
  federation feature based only on that member.
- Threshold-agreed mint info is stable across member response order.

### Positive Wallet-Side Properties Observed

The signing path is substantially stronger than the status path:

- Federation mint, swap, melt-change, and restore outputs are validated against
  the public federation config before signing.
- Non-v3 or non-BLS aggregate keysets are rejected on federation signing paths.
- Member BLS shares are verified against public member shares before
  aggregation.
- Aggregation only succeeds with threshold-compatible shares for the same output
  metadata.
- Swap recovery uses threshold check-state responses, which is safer than the
  current melt status fallback.
- Local wallet sagas reserve proofs or quotes before network calls and store
  enough information to replay or recover exact mint/swap requests after a
  crash.

Remaining reliability requirement:

- Wallet state transitions that release proofs, delete sagas, or mark quotes
  final must be based on threshold-authoritative federation evidence, not on a
  single member's observation.

## Federated Swap Findings

These findings focus on dead ends, deadlocks, race conditions, and Byzantine
behavior around federation-mode swap operations.

### P1: Already-Signed Swap Outputs Can Burn Fresh Inputs

`apply_federation_swap` checks whether blind signatures already exist for the
requested output blinded messages:

- `crates/cdk/src/mint/federation.rs:1723`
- `crates/cdk/src/mint/federation.rs:1727`

If a new swap uses fresh unspent inputs but reuses blinded outputs that were
already signed by an earlier operation, the code can continue, mark the fresh
inputs as `Spent`, and then skip signing because signatures are already present:

- `crates/cdk/src/mint/federation.rs:1762`
- `crates/cdk/src/mint/federation.rs:1764`

The HTTP route then tries to return persisted federation signature-share rows
for this new operation:

- `crates/cdk-axum/src/router_handlers.rs:273`
- `crates/cdk/src/mint/federation.rs:914`

Those rows may not exist for the new operation, because the signatures belonged
to an earlier operation. The result is a dead-end swap: fresh inputs can be
spent while the wallet cannot obtain usable signature shares for the operation.

This also weakens operation provenance. BLS blind signature shares are bound to
the blinded message, not to the federation operation ID; the operation ID is
metadata in the share record. A later operation must not be allowed to inherit
already-persisted output signatures unless it is an exact idempotent replay of
the same operation.

Recommended fix:

- Reject swap outputs whose blinded messages already have persisted signatures
  unless the operation is proven to be the exact same accepted operation.
- Prefer matching the standalone swap saga behavior, where duplicate blinded
  outputs are detected before final spend.
- Require persisted signature-share records, blinded messages, completed
  operation rows, and output signatures to agree on the same operation ID before
  treating an operation as idempotent.

Tests to add:

- First swap signs output `B`; second swap uses fresh input `A2` and reuses
  output `B`. The second swap must fail before spending `A2`.
- Same spent input and same outputs submitted under a different operation ID
  must be rejected or treated as a terminal duplicate, not applied as a new
  operation.
- Persisted response lookup after a rejected duplicate-output swap must not
  return shares from another operation.
- Replay of the exact same operation ID returns the original persisted response
  without additional spending or signing.

### P2: Swap Operations Have No Consensus Conflict Keys

Swap operations currently do not declare conflict keys:

- `crates/cdk-common/src/federation/operation.rs:1418`

Correctness is mostly protected later by database state: only one conflicting
swap can ultimately spend a proof. However, malicious wallets or malicious
members can still submit many conflicting swaps into the consensus stream. Those
operations can become finalized and only fail during apply/materialization.

This creates avoidable risks:

- Journal and consensus bloat from finalized operations that could have been
  rejected earlier.
- Extra finalizer work during replay and catch-up.
- More route timeouts or confusing rejected states for clients.
- Easier denial-of-service by repeatedly spending the same proof in distinct
  swap operations.

Recommended fix:

- Add a swap input conflict key derived from each input proof `Y`.
- Hash the `Y` value if the conflict-key namespace should avoid storing raw
  proof identifiers directly.
- Reject overlapping swaps before signing or materialization whenever consensus
  already contains an accepted operation with the same input conflict key.
- Consider applying the same pattern to other spend-like operations if they do
  not already have input-level conflict keys.

Tests to add:

- Two finalized swap operations spending the same proof: one succeeds, the
  other is rejected before signing.
- A batch of conflicting swaps does not grow materialized proof/signature state
  beyond the single accepted winner.
- Restart/replay rebuilds swap input conflict keys and preserves the same winner
  and loser statuses.
- A malicious member cannot include many same-input swaps that all reach
  expensive signing or materialization work.

### P2: Wallet Swap Fanout Stops Before Share Validation

The wallet federation collector stops once it has threshold successful HTTP
responses:

- `crates/cdk/src/wallet/federation.rs:611`
- `crates/cdk/src/wallet/federation.rs:628`
- `crates/cdk/src/wallet/federation.rs:632`

Share metadata and BLS share correctness are validated later:

- `crates/cdk/src/wallet/federation.rs:922`
- `crates/cdk/src/wallet/federation.rs:1012`

A fast malicious or stale member can return a syntactically successful but
invalid swap response. If the wallet stops after threshold raw responses before
a slower honest member replies, aggregation can fail even though enough honest
members are online. That is a liveness vulnerability under Byzantine members or
misbehaving networks.

Recommended fix:

- For signing operations, collect until threshold valid compatible shares are
  available, not merely threshold successful HTTP responses.
- Validate member responses incrementally during collection where practical.
- Alternatively, continue collecting beyond threshold raw responses until
  aggregation succeeds, all members fail, or the bounded timeout budget is
  exhausted.
- Treat invalid shares as Byzantine failures that do not count toward the
  threshold.

Tests to add:

- In a 3-of-5 federation, two honest members respond quickly, one malicious
  member responds quickly with an invalid share, one honest member responds
  slowly, and one member is offline. Swap must still succeed with the three
  honest shares.
- Same scenario with no third honest response must fail with a bounded
  insufficient-valid-shares error.
- Invalid but syntactically successful responses must not prevent waiting for
  slower honest responses while timeout budget remains.
- The same valid-share collection rule is applied to mint, swap, melt-change,
  and restore signing responses.

### P2: Apply-Time Swap Failures Can Leave Callers Waiting for Timeout

In the AlephBFT path, finalization applies an entry before notifying HTTP
waiters:

- `crates/cdk-axum/src/lib.rs:1958`
- `crates/cdk-axum/src/lib.rs:1959`

If a finalized swap fails during materialization, for example because another
conflicting swap already spent the input, the route waiter can depend on timeout
or retry behavior instead of receiving a prompt terminal rejected/failed status.
That is not a safety failure by itself, but it is a liveness and UX dead end.

Recommended fix:

- Notify waiters after every finalized apply attempt, including rejected or
  failed materialization.
- Store enough terminal error/status information for duplicate retries to return
  a deterministic response instead of waiting again.
- Keep accepted consensus status and local materialization status distinct so
  route responses can explain whether consensus accepted the operation but local
  apply rejected it.

Tests to add:

- A swap accepted by consensus but rejected during apply wakes the waiting HTTP
  route promptly.
- Retrying the same failed swap returns the persisted terminal status without
  resubmitting or waiting for another timeout.
- Conflicting swaps under AlephBFT produce one successful response and bounded
  terminal failures for the losers.

### Swap Non-Issues Observed

The reviewed code and existing tests already cover several important swap
properties:

- Pending AlephBFT swaps do not appear to mutate proof state before finality.
- Duplicate pending swap requests join one queue item rather than producing
  multiple local mutations.
- Input proof content, uniqueness, signatures, spending conditions, output
  validity, and balance are checked before apply-time state mutation.
- Concurrent conflicting swaps spend only one input in existing tests, and loser
  outputs do not receive signatures.
- Wallet aggregation verifies member share metadata and BLS share correctness
  before constructing final signatures.

## Non-Issues Observed

### AlephBFT Waiters Are Bounded

I did not see an unbounded HTTP waiter deadlock in the AlephBFT submit path.

Relevant code:

- `crates/cdk-axum/src/lib.rs:1873`
- `crates/cdk-axum/src/lib.rs:505`

The route wait is bounded by `operation_submission_timeout`, and waiter
registration checks cached outcomes so the common "accepted before waiter
registration" race appears covered.

### SQL Payment Observation Ordering Is Deterministic

The SQL storage path orders payment observations by consensus index and observer:

- `crates/cdk-sql-common/src/mint/federation.rs:1393`

That addresses the initial concern that replay could apply payment observations
in backend-dependent row order for SQL backends.

## Suggested Fix Order

1. Make federated mint signing explicitly check payment-observation quorum at or
   before the mint operation index.
2. Prevent non-consensus local payment state from making federation quotes
   mintable.
3. Clarify and fix accepted-operation apply failure status semantics.
4. Add restart/rebuild tests for apply-time rejection and conflict keys.
5. Decide and encode the production backend lookup model for replayed quotes.
6. Add provisional quote handling for quote creation before consensus finality.
7. Reject federated swaps that reuse already-signed outputs under a different
   operation before spending inputs.
8. Add swap input conflict keys so conflicting spends are rejected before
   expensive apply/signing work.
9. Change wallet signing fanout to wait for threshold valid compatible shares,
   not threshold raw successful HTTP responses.
10. Ensure swap apply failures wake waiters and persist terminal retry status.
