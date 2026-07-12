# Federated CDK Security Review

## Implementation Guardrails

Keep remediation changes lean and reuse established code paths, abstractions,
and functions wherever possible. Follow the existing federation, journal,
transaction, error-handling, and testing patterns. Every fix must be clean,
minimal, deterministic, replay-safe, and idempotent, and must not change or
repair standalone/non-federated CDK behavior.

Add focused federation regression coverage for each fix, run formatting and
the relevant tests, and update this security review and the implementation plan
with verified progress. Commit coherent milestones separately using
conventional commit messages.

Date: 2026-07-09
Current-branch assessment updated: 2026-07-13

This note records timing-sensitive issues found during a read-only review of
federation-mode minting, wallet behavior, swap handling, and melt handling. It
is intended as a work queue for follow-up fixes and tests.

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

Findings that have since been fixed on the cleanup branch are collected under
`Resolved Findings` rather than mixed into the active work queue.

## Assessment Scale

Each active finding includes a current-branch assessment. `High` findings are
release blockers for real-funds deployment, `Medium` findings are worthwhile
correctness or availability work, `Low` findings are defense-in-depth or UX
hardening, and `Informational` findings are policy choices rather than defects.
The confidence percentage measures confidence that the assigned current
severity is correct; it is not the probability that an exploit will occur.
The current active tally is four High, six Medium, three Low, and one
Informational finding.

## Findings

### P1: Mint Signing Can Depend on Local Backend Timing

Assessment:

- **Status:** Confirmed, with substantial route-level mitigation already in
  place.
- **Current severity:** High (94% confidence).
- **Decision:** Worth addressing before production. Consensus apply itself must
  enforce the observation-at-or-before-index invariant rather than relying on
  how current callers caused the quote to become `Paid`.

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

### P2: Architecture Tests Were Removed Before Enforceable Boundaries Existed

Assessment:

- **Status:** Partially valid defense-in-depth concern; no current signing
  bypass was found.
- **Current severity:** Low (86% confidence), downgraded from P2.
- **Decision:** Worth addressing as pre-production hardening, but not ahead of
  confirmed state-machine defects. Existing behavioral, runtime-guard, and
  production-feature tests already cover much of the former source checks.

Cleanup item 41 removed source-based architecture tests that guarded several
security-sensitive invariants:

- Public federation handlers must not call raw signing functions directly.
- Mint, swap, melt, restore, and quote paths must pass through federation
  readiness and consensus admission before returning signatures or mutating
  authoritative state.
- Pub/sub notifications must occur only after the surrounding transaction
  commits.
- Production federation builds must not acquire fake-wallet or development
  smoke-test dependencies.

Relevant removals:

- `crates/cdk-axum/src/custom_handlers.rs` previously checked public-handler
  signing and consensus-gating call order.
- `crates/cdk/src/mint/mod.rs` previously maintained a direct-signing call-site
  allowlist and checked commit-before-publish ordering.
- `crates/cdk/src/mint/federation/tests/runtime.rs` previously checked that
  federation payment handling remained backend-agnostic.
- `docs/federation-cleanup-todo.md:2619` requires source inspection to be
  replaced by module visibility, feature-compilation checks, typed APIs, or
  behavioral integration tests.

The removed checks were formatting-sensitive and were not strong proofs of
runtime behavior, so deleting them is a reasonable end state. However, the
replacement primarily relies on existing route behavior tests plus focused CLI
and schema tests. Those tests show that reviewed paths behave correctly today,
but they do not prevent a future handler from gaining access to a raw signatory,
a new notification from being published before commit, or a new production
feature edge from exposing development-only code.

Recommended fix:

- Restrict raw signing capabilities behind module-private or sealed typed APIs
  that public handlers cannot call; expose only the federation consensus-submit
  and persisted-share response interfaces they require.
- Encapsulate transaction commit and notification publication so callers cannot
  publish an authoritative event before a successful commit.
- Add production feature-matrix compilation checks that exclude fake-wallet and
  smoke-test surfaces.
- Until those boundaries exist, retain narrowly scoped checks for the
  direct-signing allowlist and commit-before-publish invariant rather than
  removing all enforcement at once.

Tests to add:

- A compile-fail or visibility test proving public handlers cannot access raw
  signing APIs.
- A transaction failure test proving no mint, melt, swap, proof-state, or quote
  notification is published when commit fails.
- A production federation feature build proving fake-wallet and smoke commands
  are absent from the compiled surface.
- Behavioral route tests for every public signing path proving no signature or
  authoritative mutation occurs before consensus finality.

## Wallet Perspective Findings

These findings focus on whether malicious, stale, offline, or slow federation
members can cause incorrect wallet state, failed recovery, or avoidable race
conditions.

### P1: Federation Member Validation Needs Explicit Timeouts

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (98% confidence), downgraded from P1 because the
  demonstrated impact is bounded availability rather than signature safety.
- **Decision:** Worth addressing. Apply an explicit per-member timeout to every
  metadata and keyset validation call.

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

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (97% confidence), downgraded from P1.
- **Decision:** Worth addressing. Refresh from the full roster after an
  insufficient cached response set and add an invalidation or TTL policy.

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

### P2: Quote Status Aggregation Should Use Validated Members

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (88% confidence).
- **Decision:** Worth addressing before production because quote status can
  drive wallet state transitions even though it does not produce signatures.

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

Assessment:

- **Status:** Configuration-policy concern, not a protocol implementation
  defect.
- **Current severity:** Informational (94% confidence), downgraded from P2.
- **Decision:** Do not reject these configurations in the core structural type.
  Add a CLI warning or explicit production safety profile while preserving
  intentional development and specialist configurations.

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

Assessment:

- **Status:** Confirmed low-impact metadata weakness.
- **Current severity:** Low (91% confidence).
- **Decision:** Worth backlog hardening. Consensus-critical behavior must not
  depend on a single response; display-only and advisory fields may remain
  best-effort.

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
- Swap recovery uses threshold check-state responses, and mint, batch-mint, and
  melt quote status decisions now require matching threshold views.
- Local wallet sagas reserve proofs or quotes before network calls and store
  enough information to replay or recover exact mint/swap requests after a
  crash.

Remaining reliability requirement: threshold-authoritative status responses
should also come only from currently validated federation members.

## Federated Swap Findings

These findings focus on dead ends, deadlocks, race conditions, and Byzantine
behavior around federation-mode swap operations.

### P1: Already-Signed Swap Outputs Can Burn Fresh Inputs

Assessment:

- **Status:** Confirmed.
- **Current severity:** High (99% confidence).
- **Decision:** Release blocker. Reject cross-operation reuse of already-signed
  blinded outputs before any fresh input proof is marked spent.

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

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (94% confidence).
- **Decision:** Worth addressing as consensus/replay DoS hardening. Database
  proof state currently protects monetary safety, but only after finalization
  and apply work has already been accepted.

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

### P2: Apply-Time Swap Failures Can Leave Callers Waiting for Timeout

Assessment:

- **Status:** Partially resolved. Definitive business failures are persisted and
  wake waiters; non-definitive apply failures can still return before
  notification.
- **Current severity:** Low (90% confidence), downgraded from P2.
- **Decision:** Worth backlog improvement for deterministic retry UX. The outer
  operation timeout already bounds the wait, so this is not a safety blocker.

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

## Federated Melt Findings

These findings focus on federation-mode melt quote creation, accepted melt
apply, outgoing payment execution, payment observation convergence, change
signing, and wallet recovery behavior.

### P1: Melt Quote Consensus Does Not Fix the Quote Terms

Assessment:

- **Status:** Confirmed.
- **Current severity:** High (99% confidence).
- **Decision:** Release blocker. Consensus must carry canonical quote terms and
  members must materialize those exact terms without independently regenerating
  them from local backends during apply.

`FederationMeltQuoteOperation` carries only the quote ID and wallet request:

- `crates/cdk-common/src/federation/operation.rs:468`

During apply, each member independently reconstructs and persists the melt quote
from its own payment backend:

- `crates/cdk/src/mint/federation.rs:1790`
- `crates/cdk/src/mint/melt/mod.rs:307`

This is materially weaker than mint quote consensus, where the accepted
operation includes the quote response. Melt quote generation can differ across
members in amount, fee reserve, expiry, onchain fee options, request lookup ID,
or backend-specific metadata. Payment observations later require local quote
amount and fee reserve to match the observation:

- `crates/cdk/src/mint/federation.rs:2370`

Honest members can therefore accept the same melt quote operation but persist
different quote terms, then reject each other's payment observations or compute
different change. Fake-wallet tests can hide this because the fake backend
returns deterministic quote terms.

Recommended fix:

- Put the full melt quote response or canonical quote terms into
  `FederationMeltQuoteOperation`, including amount, fee reserve, expiry,
  payment method, request string, selected fee options, and stable lookup
  metadata required for later status checks.
- Materialize exactly the accepted consensus quote terms on every member.
- Treat local backend quote generation by the proposing member as provisional
  until the exact terms are accepted by consensus.
- Reject replay if an existing local quote with the same ID does not match the
  accepted terms.

Tests to add:

- Two members whose backends return different fee reserves for the same payment
  request still materialize the exact consensus fee reserve.
- A non-proposer member does not call quote generation in a way that can alter
  amount, expiry, or lookup ID after consensus.
- A mismatched pre-existing local quote is rejected before any melt can reserve
  inputs.
- Payment observations using the accepted consensus amount and fee reserve
  finalize consistently on all members.

### P1: Melt Can Rely on a Local-Only Quote

Assessment:

- **Status:** Confirmed.
- **Current severity:** High (97% confidence).
- **Decision:** Release blocker. A melt must prove that its quote was accepted
  earlier in the ordered journal before reserving proofs or initiating payment.

The public melt quote route creates and persists a local quote before submitting
the melt quote operation to federation consensus:

- `crates/cdk-axum/src/custom_handlers.rs:917`
- `crates/cdk/src/mint/melt/mod.rs:284`

The accepted melt path then loads the local quote and validates local state:

- `crates/cdk/src/mint/melt/mod.rs:1275`

I did not see an explicit check that the quote has a prior accepted
`MeltQuote` journal entry before a `Melt` operation can reserve inputs or
attempt payment. If quote consensus times out after local quote creation, or a
member receives a malicious private operation for a locally known but
non-consensus quote, a melt can be driven by state that is not part of the
federation order.

Recommended fix:

- `apply_federation_melt` must require an accepted `MeltQuote` entry for the
  quote ID at or before the melt operation index.
- Local-only melt quotes must remain provisional and non-spendable in
  federation mode.
- The quote operation's accepted terms should be the only authority for melt
  quote materialization.

Tests to add:

- Local quote exists but no accepted `MeltQuote` operation exists; federated
  melt fails before reserving inputs.
- Accepted `Melt` appears before accepted `MeltQuote`; replay rejects or parks
  it deterministically without payment execution.
- Quote creation times out after local persistence; paying/melting that quote
  remains impossible until quote consensus finalizes.

### P2: Melt Conflict Keys Are Quote-Scoped Only

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (94% confidence).
- **Decision:** Worth addressing as consensus and payment-path DoS hardening.
  Keep the quote key and add keys derived from every input proof `Y`.

Federation `Melt` operations conflict by quote ID:

- `crates/cdk-common/src/federation/operation.rs:1410`

That prevents two different melt requests from spending the same quote, but it
does not prevent the same input proofs from being submitted across different
quotes. As with swaps, database proof state should prevent double spend, but
conflicting operations can still enter consensus and only fail during apply.
That increases journal bloat, replay work, route timeouts, and denial-of-service
surface.

Recommended fix:

- Add input proof conflict keys derived from each melt input proof `Y`.
- Keep the quote conflict key as well, because one quote should still map to one
  exact melt request.
- Reject same-input different-quote melts before payment execution or change
  signing.

Tests to add:

- Two accepted melts with different quotes but the same input proof: one wins,
  the loser is rejected before payment execution.
- Restart/replay rebuilds quote and input conflict keys and preserves winner
  and loser statuses.
- A malicious wallet cannot flood many same-input melt operations that all
  reach backend payment attempts.

### P2: Wallet Melt Status Change Aggregation Does Not Verify Shares

Assessment:

- **Status:** Confirmed.
- **Current severity:** Medium (99% confidence).
- **Decision:** Worth addressing before production. Verify each change share
  against the original blinded output and member public share, and continue
  collection until a valid compatible threshold is available.

Normal mint/swap/melt-change aggregation validates each member's BLS share
against the member public share before aggregation:

- `crates/cdk/src/wallet/federation.rs:1012`

Melt quote status recovery aggregates change shares directly after matching
metadata:

- `crates/cdk/src/wallet/federation.rs:1274`
- `crates/cdk/src/wallet/federation.rs:1345`

This path checks amount/keyset metadata but does not appear to verify each
member's blind signature share against the change blinded message and member
public share before interpolation. A malicious or stale member can therefore
poison recovered change aggregation and cause wallet recovery failure even when
enough honest members are online.

Recommended fix:

- Reuse `aggregate_member_responses` or the same `valid_member_share_set`
  validation for melt status change signatures.
- Status recovery must know or retrieve the original change blinded outputs
  before accepting change shares.
- Continue collecting responses until threshold valid compatible change shares
  are available, not merely threshold matching metadata responses.

Tests to add:

- Melt status returns two valid change shares and one invalid share in a 2-of-3
  or 3-of-5 setup; wallet ignores the invalid share and succeeds if enough
  valid shares remain.
- If not enough valid shares remain, wallet keeps the melt pending and returns a
  bounded recovery error without releasing proofs.
- A member returning a valid-looking amount/keyset but invalid BLS share cannot
  corrupt stored wallet change proofs.

### Melt Non-Issues Observed

The reviewed code and existing tests already cover several important melt
properties:

- Inputs are reserved before an outgoing payment attempt.
- Under real quorum policies, non-elected members reserve without attempting the
  external payment.
- `Unknown` backend payment status remains transient and does not become a
  durable melt quote state.
- Paid and failed observations are quorum-gated under real policies.
- Duplicate same-quote melts are conflict-keyed by quote ID.
- Pending AlephBFT melts do not appear to reserve inputs, mutate quote state, or
  sign change before finality.

## Resolved Findings

These findings were valid when recorded but are resolved on the current cleanup
branch. They remain here as an audit trail and are no longer part of the active
work queue or suggested fix order.

### Resolved: Apply-Time Rejection Conflict-Key State

Consensus rejection and post-consensus business rejection are now represented
separately as `FederationApplicationState::ConsensusRejected` and
`FederationApplicationState::Rejected`. Business rejections retain their
accepted operation's conflict-key reservation, while append-time consensus
conflicts do not acquire one. Restart reconstruction applies the same rule, and
`business_rejection_keeps_consensus_conflict_reservation_after_replay` covers
the invariant.

### Resolved: Mint Quote Backend Lookup ID Replay

`FederationMintQuoteOperation` now carries the backend-provided
`request_lookup_id`. Accepted quote replay materializes that exact identifier
instead of synthesizing `PaymentIdentifier::QuoteId`, so non-proposer members
probe the backend using the consensus-carried lookup data.

### Resolved: Mint Quote Persistence Before Consensus Finality

`prepare_federation_mint_quote` creates the payment request without persisting
or publishing the quote. Durable quote materialization now occurs from the
accepted `MintQuote` operation. Pending AlephBFT quote tests verify that quote
state is absent before finality.

### Resolved: Single-Member Quote Status Fallback

The wallet no longer has a single-member mint, batch-mint, or melt quote-status
fallback. All authoritative quote-status reads require a matching threshold
view, unavailable quorum remains ambiguous, and the local saga stays
recoverable. `quote_coordinator_member_id` affects only the first quote-creation
attempt.

### Resolved: Melt Quote Threshold Visibility

Melt quote creation now calls `verify_melt_quote_visible_to_threshold` before
returning a usable quote. Ambiguous creation recovery also requires a matching
threshold-visible response, and visibility timeout occurs before wallet proofs
are reserved.

### Resolved: Signing Fanout Stopping Before Share Validation

Mint, swap, melt-change, and restore collectors now continue until aggregation
finds a cryptographically valid compatible threshold, a threshold business
rejection is reached, or the bounded member set is exhausted. Invalid shares no
longer count merely because their HTTP responses succeeded.

### Resolved: Outgoing Payment Execution in Consensus Apply

Accepted melt apply now performs deterministic reservation only: it persists
the quote/proof/saga state as pending without calling the external payment
backend. The elected member initiates payment through the federation payment
observer after apply, and terminal observations drive quorum finalization or
rollback.

### Resolved: Delayed Melt Payment Observation Submission

The public federation melt path immediately asks the payment observer to probe
and submit a terminal observation after accepted melt processing. The periodic
observer also scans pending melt sagas, providing repair when the immediate
probe is pending or fails. Duplicate observation suppression keeps both paths
idempotent.

### Resolved: Free-Form Melt Failure Evidence

`FederationMeltQuotePaymentObservation` no longer carries or matches on
`failure_reason`. Failed observations match only backend-neutral structured
evidence: quote, method, state, amount, fee reserve, payment ID, and payment
proof. Different terminal states remain conflicting and quarantinable.

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
3. Reject federated swaps that reuse already-signed outputs under a different
   operation before spending inputs.
4. Put full melt quote terms into consensus and materialize those exact terms
   on every member.
5. Require an accepted prior `MeltQuote` operation before any federated melt can
   reserve inputs or attempt payment.
6. Verify BLS change shares during melt status recovery before aggregation.
7. Add explicit timeouts to member metadata and keyset validation.
8. Refresh the valid-member cache after insufficient responses, and add a TTL or
   invalidation policy.
9. Restrict state-changing quote-status aggregation to validated members.
10. Add swap input conflict keys so conflicting spends are rejected before
   expensive apply/signing work.
11. Add melt input conflict keys in addition to quote-scoped conflict keys.
12. Ensure non-definitive swap apply failures wake waiters with persisted retry
    status instead of relying on the outer timeout.
13. Add a production threshold-profile warning or explicit safe-profile opt-in
    without treating intentionally weak structural configs as invalid.
14. Synthesize consensus-critical mint info or require a matching threshold view
    for fields that affect wallet behavior.
15. Replace the removed signing, consensus-gating, commit-order, and production
    feature architecture checks with enforceable typed or behavioral boundaries.
