# TODO: Federated Mint/Melt Quote Consistency

## Status

Open consistency and consensus-validation issue. This document describes the problem and an implementation plan; it does not indicate that the fix has been implemented.

## Summary

Federated melt execution explicitly requires the referenced melt quote to have been applied from an earlier `FederationOperation::MeltQuote` entry in the federation journal. Federated mint execution does not currently enforce the equivalent journal provenance for its mint quote.

The normal mint flow is intended to be ordered as:

```text
MintQuote -> MintQuotePayment observation(s) -> Mint
```

Catch-up normally applies those entries in order, so the missing validation is easy to overlook. The current mint apply path nevertheless relies primarily on materialized database state: it checks that the quote exists and that sufficient qualifying payment observations exist. It does not explicitly prove that the quote and the observations are backed by the required earlier operations in federation consensus.

This creates an architectural asymmetry:

```text
Melt: requires an earlier applied MeltQuote journal entry
Mint: requires a locally materialized quote, without the equivalent explicit journal check
```

Materialized state should be the result of accepted federation history, not a substitute for proving that the required history exists in the correct order.

## How We Found It

Catch-up melt tests began failing after melt quote consensus was canonicalized. Some fixtures inserted a melt quote directly into local storage and then supplied only a later `Melt` journal entry. The runtime correctly rejected those fixtures because `apply_federation_melt` now calls `require_prior_applied_melt_quote`: a matching, applied `MeltQuote` entry must have a lower journal index than the `Melt` entry.

While comparing the equivalent mint path, we found that:

- `apply_federation_mint_quote` materializes a quote from an accepted `MintQuote` operation.
- `apply_federation_mint_quote_payment` records a payment observation and updates the quote if it is already present, but does not explicitly require a prior applied `MintQuote` journal entry.
- `apply_federation_mint` loads the referenced quote from the local database and checks payment quorum using observations at or before the mint operation's journal index.
- `apply_federation_mint` does not explicitly require a matching earlier applied `MintQuote` journal entry.

The ordinary route produces correctly ordered operations, but consensus application and catch-up should independently enforce the invariant. Tests, recovery paths, malformed peer history, or accidentally pre-populated local state must not be able to bypass it.

## Required Invariants

For every quote referenced by a federated mint flow:

1. A `MintQuotePayment` observation may only be applied when a matching `MintQuote` operation was applied at a strictly lower federation journal index.
2. A `Mint` operation may only be applied when a matching `MintQuote` operation was applied at a strictly lower federation journal index.
3. A `Mint` operation may only use payment observations that were themselves applied at strictly lower journal indexes and that satisfy the configured federation payment quorum.
4. A quote merely existing in the local database is not sufficient evidence of consensus provenance.
5. Replayed and caught-up history must enforce the same rules as live consensus application.
6. Batch minting must validate these invariants independently for every referenced quote.

The checks must use journal ordering, not wall-clock time. The referenced prerequisite must have `FederationOperationStatus::Applied` and an index lower than the dependent operation.

## Expected Behavior

Valid history:

```text
index 10: MintQuote(quote A)                  Applied
index 11: MintQuotePayment(quote A, peer 1)  Applied
index 12: MintQuotePayment(quote A, peer 2)  Applied
index 13: Mint(quote A)                       Applied
```

Invalid histories include:

```text
index 10: MintQuotePayment(quote A)           # no earlier MintQuote
```

```text
index 10: MintQuotePayment(quote A)
index 11: MintQuote(quote A)                  # quote appears too late
```

```text
local database: quote A exists
index 10: Mint(quote A)                       # no earlier consensus MintQuote
```

```text
index 10: MintQuote(quote A)
index 11: Mint(quote A)
index 12: MintQuotePayment(quote A)           # payment evidence appears too late
```

## Implementation Plan

### 1. Add failing regression tests first

Add separate tests for live apply and catch-up/replay where practical. Each test should first demonstrate the current gap.

Required cases:

- Reject `MintQuotePayment` when no matching earlier applied `MintQuote` exists.
- Reject `MintQuotePayment` when the matching `MintQuote` occurs later in the journal.
- Reject `Mint` when the quote exists only in local storage and no earlier applied `MintQuote` exists.
- Reject `Mint` when its matching `MintQuote` occurs later.
- Reject `Mint` when otherwise sufficient payment observations occur only after the mint operation.
- Accept the correctly ordered single-quote flow.
- Accept the correctly ordered batch-mint flow.
- Reject a batch mint if even one referenced quote lacks the required earlier quote operation or payment evidence.
- Verify that replaying or catching up the same valid history produces the same result as live application.

Do not weaken existing quote validity, payment quorum, expiry, replay, or signature-share assertions to make these tests pass.

### 2. Generalize the existing melt prerequisite check where useful

The existing `require_prior_applied_melt_quote` captures the right core rule. Prefer extracting a small shared journal-query helper if it makes the invariant clearer, for example a helper that answers whether a matching applied prerequisite operation exists before a supplied index.

Possible structure:

```text
require_prior_applied_quote(kind, quote_id, max_operation_index)
```

or two thin, domain-specific wrappers over one shared predicate:

```text
require_prior_applied_mint_quote(...)
require_prior_applied_melt_quote(...)
```

The wrappers may be easier to reason about because they preserve explicit protocol language at call sites. Consolidate only the journal scanning and ordering mechanics; do not create a generic dependency framework unless additional operation families actually need it.

### 3. Enforce the prerequisite for mint payment observations

Before applying a `MintQuotePayment` observation, require a matching earlier applied `MintQuote` operation. Perform this check against accepted journal history using the observation entry's index.

The check should happen before persisting the observation or mutating quote state. Failure must leave both the materialized quote state and payment-observation store unchanged.

Use the existing domain error that best represents a missing quote if it remains unambiguous. Introduce a more precise federation ordering/dependency error only if callers or tests need to distinguish "unknown quote" from "quote exists locally but lacks prior consensus provenance."

### 4. Enforce the prerequisite for mint operations

Before applying a `Mint`, require a matching earlier applied `MintQuote` for every quote ID in the request. This must not be satisfied merely by `get_mint_quote` returning a record.

Keep the existing payment-quorum check, which already limits qualifying observations by operation index. Confirm with regression tests that it strictly excludes observations at or after the `Mint` entry. If that behavior is already correct, do not rewrite it.

For batch minting, validate all quote dependencies before creating any blind signatures or updating any quote. Preserve transaction atomicity so one invalid quote rejects the complete operation without partial effects.

### 5. Keep admission validation side-effect free

Request admission currently performs structural validation, while accepted-state apply is authoritative for checks that depend on ordered quote and proof state. Preserve that architecture.

Do not add database or journal reads to the pure envelope-validation functions merely to reject these operations earlier. The authoritative dependency checks belong in ordered apply, where the journal index and finalized predecessor state are known. Earlier rejection may be added only as an optional optimization if it cannot disagree with apply.

### 6. Update fixtures to model complete consensus history

Any test that applies or catches up a mint payment observation or mint operation must include the prerequisite `MintQuote` entry. Tests must not directly insert a quote into local storage unless their explicit purpose is to prove that local state cannot replace consensus provenance.

Use shared fixture helpers to construct complete ordered flows and reduce accidental omissions. Keep negative fixtures explicit so the missing or misordered dependency remains visible in the test.

### 7. Prove the fix

Run the focused federation mint and catch-up tests, followed by the complete federation test suite. The proof should demonstrate:

- Previously accepted malformed histories are rejected.
- Correctly ordered histories still succeed.
- Live application and catch-up/replay converge.
- Batch minting remains atomic.
- Existing melt behavior is unchanged after any helper consolidation.
- Existing payment quorum and signature-share test surface remains intact.

## Architectural Guidance

Keep the implementation close to the current melt design. A pair of explicit mint/melt prerequisite functions backed by a small shared journal predicate is likely the clearest balance: shared mechanics without hiding protocol semantics.

Avoid adding caches, dependency registries, new synchronization loops, or generalized operation graphs for this fix. The issue is an ordered-state validation gap, and the minimal solution is to verify that the required applied predecessor exists before mutating dependent state.

If repeated full-journal scans become measurably expensive, address indexing separately with evidence from benchmarks. Correctness should be established first without coupling this fix to speculative performance plumbing.

## Definition of Done

- `MintQuotePayment` cannot apply without a matching earlier applied `MintQuote`.
- `Mint` cannot apply without matching earlier applied `MintQuote` operations for all referenced quotes.
- Only earlier qualifying payment observations contribute to mint authorization.
- Local database state alone cannot satisfy consensus provenance.
- Valid live, replay, and catch-up flows behave identically.
- Melt's existing prerequisite enforcement remains covered and unchanged semantically.
- Regression tests cover missing, late, valid, replayed, and batch dependencies without reducing existing test coverage.
