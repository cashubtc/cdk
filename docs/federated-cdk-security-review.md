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

The four release-blocking findings identified by this review have been fixed on
the cleanup branch. The remaining active findings are availability,
defense-in-depth, wallet-hardening, and policy work rather than confirmed
real-funds safety blockers.

Findings that have since been fixed on the cleanup branch are collected under
`Resolved Findings` rather than mixed into the active work queue.

## Assessment Scale

Each active finding includes a current-branch assessment. `High` findings are
release blockers for real-funds deployment, `Medium` findings are worthwhile
correctness or availability work, `Low` findings are defense-in-depth or UX
hardening, and `Informational` findings are policy choices rather than defects.
The confidence percentage measures confidence that the assigned current
severity is correct; it is not the probability that an exploit will occur.
The current active tally is six Medium, three Low, and one Informational
finding.

## Findings

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

### Resolved: Mint Signing Depended on Local Backend Timing

Former assessment: High severity (94% confidence), worthwhile as a production
release blocker.

Federated mint apply now requires quorum-satisfied `MintQuotePayment` evidence
at or before the accepted mint operation index. Raw local `Paid` state is no
longer sufficient to authorize signing, and later payment evidence cannot make
an older mint operation valid retroactively. Regression coverage includes a
locally paid quote without consensus evidence and a payment quorum ordered
after the mint operation.

### Resolved: Already-Signed Swap Outputs Could Burn Fresh Inputs

Former assessment: High severity (99% confidence), worthwhile as a production
release blocker.

Federated swap apply now rejects cross-operation reuse of persisted output
signatures while inputs are still unspent. Exact replay is tied to the same
completed swap operation; a different operation cannot inherit another
operation's signatures or share provenance. The regression verifies that a
fresh input remains untouched when its requested output was signed previously.

### Resolved: Melt Quote Consensus Did Not Fix Quote Terms

Former assessment: High severity (99% confidence), worthwhile as a production
release blocker.

`FederationMeltQuoteOperation` now carries the canonical
`MeltQuoteCreateResponse` plus only the backend metadata omitted from that
public response. The proposer prepares without persistence, accepted apply does
not call a local backend, and one exhaustive converter validates Bolt11,
Bolt12, onchain, and custom payloads before deterministic materialization using
accepted journal time. Exact replay preserves progressed quote state, while a
conflicting pre-existing quote fails closed without rewrite.

### Resolved: Melt Could Rely on a Local-Only Quote

Former assessment: High severity (97% confidence), worthwhile as a production
release blocker.

Federated quote preparation no longer persists proposer-local quote state
before consensus. Before any melt reservation, saga creation, or payment work,
apply requires a matching earlier `MeltQuote` journal entry whose local
application completed successfully. The check is index-bounded, so a later
quote entry cannot retroactively authorize an older melt on replay.

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

The shared mint-quote preparation path creates the payment request without
persisting or publishing the quote when federation mode invokes it. Durable
quote materialization occurs from the accepted `MintQuote` operation. Pending
AlephBFT quote tests verify that quote state is absent before finality.

### Resolved: Single-Member Quote Status Fallback

The wallet no longer has a single-member mint, batch-mint, or melt quote-status
fallback. All authoritative quote-status reads require a matching threshold
view, unavailable quorum remains ambiguous, and the local saga stays
recoverable. `quote_coordinator_member_id` affects only the first quote-creation
attempt.

### Resolved: Melt Quote Threshold Visibility

Melt quote creation now calls `verify_melt_quote_visible_to_threshold` before
returning a usable quote. If a member's creation response is lost, the wallet
may try another member as a new quote creation; it cannot recover an unknown
quote ID. Visibility timeout occurs before wallet proofs are reserved.

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

1. Verify BLS change shares during melt status recovery before aggregation.
2. Add explicit timeouts to member metadata and keyset validation.
3. Refresh the valid-member cache after insufficient responses, and add a TTL or
   invalidation policy.
4. Restrict state-changing quote-status aggregation to validated members.
5. Add swap input conflict keys so conflicting spends are rejected before
   expensive apply/signing work.
6. Add melt input conflict keys in addition to quote-scoped conflict keys.
7. Ensure non-definitive swap apply failures wake waiters with persisted retry
   status instead of relying on the outer timeout.
8. Add a production threshold-profile warning or explicit safe-profile opt-in
    without treating intentionally weak structural configs as invalid.
9. Synthesize consensus-critical mint info or require a matching threshold view
    for fields that affect wallet behavior.
10. Replace the removed signing, consensus-gating, commit-order, and production
    feature architecture checks with enforceable typed or behavioral boundaries.
