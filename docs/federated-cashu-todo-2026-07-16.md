# TODO (2026-07-16): Restore standard quote creation

**Status:** Implemented and verified

> **Disposition update — 2026-07-29:** Federation wallets now use ordinary
> mint-generated quote IDs and the standard quote request shapes. The removed
> `post_mint_quote_with_id`, `post_melt_quote_with_id`, and
> `federation_quote_id` surfaces are absent from production code.
> `FederationWalletConnector` performs sequential first-healthy quote creation
> and threshold visibility validation using the returned ID. Current mint and
> Axum federation suites cover standard quote creation, consensus
> materialization, and rejection before readiness. The checklist below is kept
> as the implementation record rather than an open TODO.

## Goal

Restore the standard Cashu wallet API for mint-quote and melt-quote creation by
removing wallet-assigned federation quote IDs. Unify federated mint business
logic with the existing non-federated quote-creation paths instead of
maintaining federation-specific preparation and idempotency behavior in the
mint.

Federation membership and consensus should remain an implementation detail of
the mint. A wallet should be able to submit the same standard Cashu quote
request whether it is talking to a standalone mint or a federation member.

## Intended behavior

For the successful path:

1. The wallet sends a standard quote-creation request to one federation member.
2. The receiving member generates the quote ID, as a standalone mint does.
3. The member creates the quote using the shared mint business logic and
   submits the resulting quote operation to federation consensus.
4. Once accepted, the quote is materialized across the federation.
5. The member returns the standard Cashu quote response containing the
   mint-generated quote ID.
6. The wallet uses that returned ID for normal quote-status requests and may
   verify that the quote is visible to a threshold of members.

Quote creation must not be fanned out concurrently. The wallet selects one
member for each creation attempt. Signing and state operations may continue to
use the existing threshold fan-out behavior.

An ambiguous quote-creation failure has at-least-once semantics. If the member
commits a quote but its HTTP response is not received, the wallet does not know
that quote's ID. A later attempt is allowed to create a new quote with a new
ID. The resulting unused invoice, address, or quote may expire normally. We
explicitly accept this trade-off in order to keep the public API compatible
with the Cashu protocol.

## Required changes

### Wallet and connector API

- Remove `MintConnector::post_mint_quote_with_id`.
- Remove `MintConnector::post_melt_quote_with_id`.
- Remove the corresponding `HttpClient` implementations and the helper that
  appends `federation_quote_id` to quote-creation URLs.
- Change `FederatedMintConnector` quote creation to call the ordinary
  `post_mint_quote` and `post_melt_quote` methods.
- Stop generating quote IDs in `FederatedMintConnector`.
- Remove validation that a quote-creation response matches a wallet-assigned
  ID. Continue validating the payment method and the consistency of the
  returned response.
- Keep threshold visibility checks after a successful response, using the
  quote ID returned by the mint.
- Remove recovery that tries to discover a quote after an ambiguous creation
  failure using a preassigned ID. Retry, if desired, as a new quote-creation
  attempt with a newly generated mint-side ID.

### Public HTTP API

- Remove the `federation_quote_id` query parameter from mint-quote and
  melt-quote creation.
- Remove `FederationQuoteQuery`, `assigned_federation_quote_id`, and the
  federation-only error requiring the query parameter.
- Ensure federation and standalone routes accept exactly the same standard
  Cashu request bodies and URL shapes.

### Mint business logic

- Route federated mint-quote creation through the same quote-ID generation and
  quote-construction business logic used by `Mint::get_mint_quote`.
- Route federated melt-quote creation through the same business logic used by
  ordinary melt-quote creation.
- Generate the quote ID inside the mint before constructing the federation
  consensus operation.
- Remove federation-specific public mint methods whose only purpose is
  accepting a wallet-assigned quote ID, including
  `prepare_federation_mint_quote`,
  `ordered_federation_mint_quote_response`,
  `prepare_federation_melt_quote`, and
  `existing_federation_melt_quote_preparation`, once their remaining behavior
  has been folded into the shared path.
- Consolidate persistence boundaries carefully: an accepted federation
  operation must still be the canonical state replicated to all members.
  Avoid reintroducing proposer-only database state before consensus.
- Preserve federation-specific on-chain address allocation only where required
  by threshold wallet/address derivation. It should not require a wallet-
  assigned quote ID; the proposing mint can generate the ID before requesting
  an allocation.
- Keep consensus conflict detection keyed by the mint-generated quote ID. It
  remains useful for duplicate operation delivery even though distinct HTTP
  retries will use distinct IDs.

### Tests and documentation

- Replace tests requiring `federation_quote_id` with tests proving that an
  ordinary Cashu quote request succeeds against a federation member.
- Test that the returned mint-generated quote ID becomes visible to a
  threshold of members.
- Test mint-quote and melt-quote creation through both standalone and
  federated configurations using the shared business logic.
- Test that retrying after an ambiguous response may create a second quote and
  that both quotes remain internally consistent.
- Retain duplicate-consensus-delivery and same-ID conflict tests at the
  federation operation layer.
- Remove documentation and examples that tell federation wallets or custom
  connectors to provide an assigned quote ID.
- Check whether the connector trait removal affects downstream mock connectors,
  FFI bindings, examples, or integration-test helpers and update them in the
  same change.

## Acceptance criteria

- A standard wallet can create mint and melt quotes against a federation
  member without federation-specific request parameters or connector methods.
- Existing high-level wallet quote APIs have the same signatures and request
  behavior for standalone and federated mints.
- Federated and standalone quote creation share the mint's core business
  logic; federation code is limited to consensus submission, replicated apply,
  and genuinely federation-specific validation.
- A successfully returned quote is visible through the standard GET quote
  endpoint on a threshold of members.
- Losing a creation response does not trigger lookup by a wallet-assigned ID;
  a retry is allowed to create a separate quote.
- No `federation_quote_id` handling remains in the public HTTP routes, wallet
  connector API, or mint quote-creation business logic.
- Standalone mint behavior remains unchanged.
- Relevant unit and integration tests pass for both standalone and federation
  configurations.

## Non-goals

- Providing exactly-once quote creation across ambiguous HTTP failures.
- Deduplicating separate standard Cashu quote requests that happen to have
  identical request bodies.
- Removing threshold fan-out for minting, swapping, melting, restoring, or
  proof-state checks.
- Removing federation consensus operations or replicated quote state.
