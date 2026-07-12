# Federated Cashu Code Improvement Opportunities

Living list of **code structure and maintainability** improvements… not consensus redesigns. Each milestone is independently shippable. Prefer order as sequenced below when dependencies exist.

**Rules for every milestone**

- Preserve consensus safety, deterministic replay, and standalone (non-federation) CDK behavior.
- Keep production and test metrics separate: test-only moves must not be sold as “prod LOC reduction.”
- Verify premise against current code before implementing; adjacent call sites, features (`federation`), and docs may have drifted.
- Prefer move + table-drive over blind deletes when coverage value is unclear.

**Primary evidence (2026-07)**

- Branch comparison `bls-federation-cleanup` vs `bls` showed mint-dominant prod growth; wallet small.
- `crates/cdk-axum/src/custom_handlers.rs` ~9.8k lines of which **~1.2k prod / ~8.6k tests**.
- Related bulk: `crates/cdk-axum/src/federation/tests.rs` (~21k), federation runtime modules under `cdk-axum` / `cdk-common`.

Related plans: `docs/federation-cleanup-todo.md`, `docs/federated-cashu-refactoring-plan.md`.

---

## Milestone 1 — Extract `custom_handlers` tests from production module

**Status:** Open  
**Goal:** Drop the false impression that value-route handlers are a 10k federated kitchen sink; make prod review surface ~1.2k lines.

### Problem

`crates/cdk-axum/src/custom_handlers.rs` embeds an inline `#[cfg(test)] mod tests` (~8.6k lines, ~89 `tokio::test`s, ~2.5k fixture lines before the first test). Product code ends near line ~1203. Size and complexity stats that include this file as “Axum prod” are misleading.

### Work

- Move `mod tests` out of `custom_handlers.rs` into a dedicated module tree, for example:
  - `crates/cdk-axum/src/custom_handlers/tests.rs` with `mod fixtures;` / route modules, or
  - `crates/cdk-axum/src/custom_handlers/tests/{mod,fixtures,mint,melt,...}.rs`.
- Keep `custom_handlers.rs` as handlers + thin private helpers only.
- Ensure `cargo test -p cdk-axum` still discovers the full suite (feature matrix: `federation` on/off where applicable).

### Acceptance

- [ ] Production `custom_handlers` (or `custom_handlers/mod.rs` + leaf files) has no multi-thousand-line inline test module.
- [ ] Test count and names preserved (or only renamed for module path), and CI-relevant `cdk-axum` tests still pass.
- [ ] File-level prod line count for handlers is amply under ~2k unless further extraction (M2) already landed.

### Non-goals

- Deleting coverage.
- Changing federation protocol or public HTTP contracts.

---

## Milestone 2 — Move federation orchestration out of `custom_handlers`

**Status:** Open  
**Depends on:** Optional; easier after or in parallel with M1  
**Goal:** HTTP stays thin; federation process/submit/observe lives with the federation stack.

### Problem

Production handlers interleave multi-method Cashu HTTP with federation engine work behind `#[cfg(feature = "federation")]` blocks (~26 sites), including:

- `process_federation_mint_input` / `ensure_federation_mint_quotes_paid_for_signing` / `wait_for_federation_mint_quotes_paid_locally`
- `submit_federation_mint_quote_operation` / `submit_federation_melt_quote_operation` / `submit_federation_mint_operation`
- `process_federation_melt_request`
- `federation_melt_response_from_store` / `federation_melt_quote_is_failed_terminal`
- `mint_quote_response_to_string`

Handers already call mint and `MintState` federation APIs; this file is acting as a **second federated edge runtime**.

### Work

- Introduce a clear home, e.g. `crates/cdk-axum/src/federation/public_value_routes.rs` (name bikeshed OK), used only by custom (and later router) handlers.
- Target handler shape:
  1. auth / method validation  
  2. parse body  
  3. `federation_or_local.process_*`  
  4. serialize response / NUT-19 cache  
- Keep pure HTTP helpers (Prefer header, JSON shape per melt variant, method path validation) in the handler module.
- Revisit the 100ms poll in `wait_for_federation_mint_quotes_paid_locally` only if an existing event/watch path is available; do not invent a new payment state machine here.

### Acceptance

- [ ] `custom_handlers` federation-specific helpers are gone or reduced to one-line delegations.
- [ ] No behavior change for standalone mint (feature off) or federated public mint/melt quote and issue/melt paths (feature on).
- [ ] Focused route tests (moved or not) still pass.

### Non-goals

- Redesigning payment observation trust model or Aleph runtime ownership (`docs/federated-cashu-payment-trust-model.md`, runtime cleanup items).

---

## Milestone 3 — DRY NUT-19 cache wrappers in custom handlers

**Status:** Open  
**Goal:** One cache/get/miss/set path for mint, batch mint, and melt.

### Problem

`cache_post_mint_custom`, `cache_post_melt_custom`, and `cache_post_batch_mint` repeat the same pattern:

```text
validate → calculate_key → get hit → process on miss → set → respond
```

Drift risk (auth, method checks, federation branch) is higher than the line count suggests.

### Work

- Extract a small internal helper (generic or lightly monomorphized) shared by the three routes.
- Preserve cache key namespaces (`mint`, `melt`, `mint_batch`) and existing serialize shapes.
- Keep clippy constraints (`result_large_err` on parse) intentional and documented if still needed.

### Acceptance

- [ ] Three public cache entrypoints remain; bodies share one helper.
- [ ] Method-mismatch and federation share/idempotency tests still pass.
- [ ] Standalone NUT-19 cache behavior unchanged.

---

## Milestone 4 — Collapse multi-method quote parse arms

**Status:** Open  
**Goal:** Shorter `post_mint_custom_quote` / `post_melt_custom_quote` without losing bolt11/12/onchain/custom behavior.

### Problem

Large `match method.as_str()` arms parse parallel request types and call parallel mint APIs. Same pattern repeated on mint and melt quote create (and related). Hard to review federation quote-id assignment soaked through the arms.

### Work

- Add `parse_mint_quote_request(method, value) → MintQuoteRequest` (or equivalent) and melt-side twin.
- Keep response shaping (`melt_quote_*_response_to_json`, mint quote JSON variants) next to handlers or a tiny `response` helper module.
- Federation assigned quote id (`FederationQuoteQuery` / `assigned_federation_quote_id`) stays a single pre-parse step.

### Acceptance

- [ ] Public handlers read as: auth → ready → parse → create → optional federation submit → serialize.
- [ ] Cross-method mismatch tests still pass.
- [ ] Bolt11/bolt12/onchain/custom each still round-trip on quote create.

### Small cleanup (same milestone or follow-up)

- Rename `FederationQuoteQuery._federation_quote_id` to a normal field name if serde rename already supplies the wire key.

---

## Milestone 5 — Shared Axum federation test harness (fixtures)

**Status:** Open  
**Depends on:** Strongly after M1; coordinates with existing `federation/tests.rs`  
**Goal:** One fixture layer for fakes, DB temp files, and federated `MintState` builders.

### Problem

`custom_handlers` tests alone include ~2.5k lines of setup before the first test: multiple full `MintPayment` fakes (`DelayedStatusFakeWallet`, `CountingFakeWallet`, `InstantIncomingPaymentWallet`), temp DB helpers, and several nearly identical:

- `create_federated_test_state*`
- `create_aleph_bft_federated_test_state`
- threshold / direct-route connectors

`crates/cdk-axum/src/federation/tests.rs` (~21k) almost certainly overlaps this investment.

### Work

- Create a shared test-only module (e.g. `crates/cdk-axum/src/test_support/federation_harness.rs` or `federation/tests/support/`).
- Centralize: temp mint DB, fake wallets, federated single- and multi-member constructors, signature/key fixtures as needed.
- Point `custom_handlers` tests and `federation/tests` (incrementally) at the harness; delete duplicate structs as call sites migrate.

### Acceptance

- [ ] No third copy of the same fake wallet lives in a third file without a written reason.
- [ ] Creating a federated route test does not require pasting 100+ lines of wallet impl.
- [ ] Test runtime remains acceptable (no accidental global suite time explosion).

### Non-goals

- Merging all 21k + 8.6k tests into one file.
- Changing Aleph or DKG production code.

---

## Milestone 6 — Relocate swap route tests out of `custom_handlers`

**Status:** Open  
**Depends on:** M1 (mechanical move easier once tests are a package)  
**Goal:** Tests live next to the route ownership.

### Problem

~11 swap-related tests sit under `custom_handlers` tests, but swap is registered from `lib.rs` (`cache_post_swap` / router stack), not `custom_router` / custom method handlers. Reviewers look for swap behavior in the wrong place; custom_handlers appears responsible for swap federation.

### Work

- Move swap-focused tests next to swap handler ownership (`router_handlers` tests or `federation/tests` value-op family).
- Keep mint/melt custom-method tests with custom handlers.

### Acceptance

- [ ] Zero swap-specific tests remain in the custom_handlers test package unless they only use swap as a contemporaneous peer in a multi-op scenario clearly owned by custom_handlers (document if so).
- [ ] Swap fail-closed, idempotency, and share tests still run in CI.

---

## Milestone 7 — Move multi-member wallet fanout tests to wallet/HTTP IT home

**Status:** Open  
**Depends on:** M1; M5 helps  
**Goal:** Stop testing wallet connector aggregation under “custom handlers.”

### Problem

Tests such as concurrent federated wallet mint/swap/melt fanout aggregate behavior across real Axum members exercise **wallet federation client + multi-mint routing**, not a single custom-method handler unit. They bloat axum handler review and obscure true handler surface.

### Work

- Relocate to a home owned by wallet federation or a multi-mint integration layertheir natural API (`cdk` wallet federation tests, `cdk-integration-tests`, or axum `federation/tests` multi-member section with a clear name).
- Keep only handler-level contracts (status codes, share JSON, method validation) in handler suites if still needed in isolation.

### Acceptance

- [ ] Fanout/aggregation tests are discoverable from wallet federation docs or module README paths.
- [ ] Producer path coverage (mint/melt/swap fanout once per member) remains green.

---

## Milestone 8 — Table-drive templated recovery / idempotency families

**Status:** Open  
**Depends on:** M1; stronger after M5–M6  
**Goal:** Same invariants, fewer near-copy test bodies.

### Problem

Named families repeat the same shape for mint / melt / (swap) × recovery modes:

- `duplicate_federation_*_after_{cache_eviction,sqlite_restart,catch_up}_returns_persisted_*`
- `duplicate_post_*_...`
- concurrent pending join-one-queue-item suites
- concurrent wallet fanout per operation

Body length dominates readable intent; parameter matrix is small.

### Work

- Introduce table-driven (or macro-generated) tests parameterized by operation kind and recovery mode.
- Keep distinct literals only where peer behavior truly differs (e.g. melt terminal failure vs mint shares).
- Prefer one well-named suite with cases over nine pasted functions.

### Acceptance

- [ ] At least the three largest copy-paste families are parameterized.
- [ ] Failure messages still name operation + recovery mode clearly.
- [ ] No intentional loss of coverage for restart / cache eviction / catch-up.

---

## Milestone 9 — Deduplicate Axum federation E2E vs handler-route suites

**Status:** Open  
**Depends on:** M5 recommended  
**Goal:** One source of truth per invariant; delete true duplicates only after mapping.

### Problem

Two large suites target overlapping federated public-route safety:

| Suite | Scale | Focus (approx.) |
|---|---:|---|
| `custom_handlers` tests | ~8.6k / ~89 tests | Value routes, NUT-19 cache, mint/melt/swap idempotency, melt terminal, fanout |
| `federation/tests.rs` (+ submodules) | ~21k+ | Aleph, DKG routes, private ops, peer paths, related E2E |

Without a matrix of invariants, cleanup either deletes safety or never starts.

### Work

- Inventory invariants: fail-closed when not ready, share idempotency, pay-once melt, terminal failed observation, queue coalescing, catch-up resume, etc.
- Map each invariant to primary suite; secondary suite either drops or keeps a single smoke.
- Document the ownership matrix in this file or in `federation/tests` module docs.

### Acceptance

- [ ] Written invariant → test ownership matrix exists.
- [ ] Measurable reduction in redundant wall-clock test time or line count without coverage gaps called out intentionally.
- [ ] No consensus-critical path left untested “because the other file had it.”

---

## Milestone 10 — Prod metric hygiene for federation reviews

**Status:** Open  
**Goal:** Future size/complexity reviews never double-count tests as product.

### Problem

Branch stats that treat `custom_handlers.rs` as +9.8k product Loc spuriously inflate Axum “prod” cost (~+549% narratives). Tests are valuable but must be scored separately (this project already prefers that policy).

### Work

- When reporting federation cost vs `bls` (or main), always split:
  - production `.rs` (exclude `/tests`, `#[cfg(test)]` modules when measurable, benches, examples, fuzz)
  - test LOC separately
- Prefer tree totals (`HEAD` vs baseline line counts) over addition-only diffs when discussing %↑.
- Optional: a small script under `scripts/` or a `just` recipe that prints the side/layer prod table used in reviews.

### Acceptance

- [ ] At least one documented command or recipe produces mint/wallet/shared prod Δ and %↑ with tests excluded.
- [ ] This document’s headline numbers stay updated when a major extract lands (M1–M2).

---

## Suggested sequencing

```text
M1  extract custom_handlers tests          (immediate, low risk)
M3  DRY cache wrappers                     (small, high clarity)
M4  quote parse collapse                   (small)
M2  federation orchestration extraction    (medium, behavior-sensitive)
M5  shared test harness                    (unlocks M6–M9)
M6  move swap tests
M7  move wallet fanout tests
M8  table-drive recovery families
M9  dedupe large E2E suites
M10 metrics recipe / hygiene               (anytime; amend after M1)
```

---

## Out of scope here (tracked elsewhere)

- Payment observation trust model and melt election redesign → `federation-cleanup-todo.md`, payment trust docs.
- Aleph session supervision / runtime collapse → remaining runtime cleanup todos.
- DKG ceremony productization, encryption, backups → DKG plan docs.
- Docs-only federated RFC/html progress trees → not code structure work.

---

## Changelog

| Date | Note |
|---|---|
| 2026-07-13 | Initial milestones from `custom_handlers` review and `bls-federation-cleanup` vs `bls` prod/test analysis. |
