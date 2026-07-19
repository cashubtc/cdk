# Federated Cashu cleanup todo — 2026-07-19

## Purpose

This document records the cleanup review of the current federation branch against
`docs/federated-cashu-implementation-plan.md`.

The objective is to reduce, unify, and clarify the federation implementation
without losing its required safety or feature set. Standalone CDK behavior and
public Cashu behavior must remain unchanged unless a separate, intentional
protocol change is documented.

This is a cleanup and release-readiness tracker, not a compatibility plan. No
federation protocol, database schema, invitation, or configuration has been
deployed, so the implementation remains under the cut-through compatibility
policy from the main implementation plan.

## Review baseline

The review covered federation code in `cashu`, `cdk-common`, `cdk`, `cdk-axum`,
the SQL backends, `cdk-mintd`, the wallet connector, the AlephBFT runtime, BDK and
FROST support, migrations, tests, scripts, and federation documentation.

The reviewed branch, compared with the local `origin/main`, contained:

- 1,018 commits;
- 421 changed files;
- approximately 253,000 additions and 1,900 deletions;
- 256 added files and 165 modified files;
- merge commits from `main` and multiple federation branches;
- 52 federation/deployment documents totalling approximately 42,000 lines;
- approximately 55,900 additions under `docs`, `scripts`, and `.github`.

Focused verification performed during the review:

- `cargo check -p cdk --no-default-features --features mint,wallet` passed;
- `cargo check -p cdk --no-default-features --features mint,wallet,federation`
  passed;
- `cargo check -p cdk-axum` passed;
- `cargo check -p cdk-axum --features federation` passed;
- standalone `cdk` clippy with `-D warnings` failed;
- federation-enabled `cdk-axum` clippy with `-D warnings` failed with seven
  errors;
- full workspace tests and the multi-node regtest suite were not run as part of
  this static cleanup review.

## Non-negotiable retained feature set

Cleanup work must retain all of the following:

- [ ] Federation keysets remain Cashu v3/BLS-only.
- [ ] Every member verifies blinded outputs and signature shares.
- [ ] Dealer-free DKG retains enrollment, roster agreement, identity binding,
      restartable rounds, transcript signatures, activation, and identical
      public configuration export.
- [ ] Member traffic remains authenticated private REST with replay protection.
- [ ] AlephBFT remains the ordering mechanism for safety-critical operations.
- [ ] Every accepted operation remains durably journaled, canonically encoded,
      deterministic, idempotent, and replayable.
- [ ] Consensus precedes signature-share creation and externally visible
      safety-critical state changes.
- [ ] Mint quote, issuance, swap, melt quote, melt, restore, and check-state
      retain federation behavior.
- [ ] Payment side effects continue to use the replay-safe flow and quorum
      payment evidence.
- [ ] Conflicting payment-observation quorums remain unresolved and fail closed.
- [ ] The wallet connector continues to own member fan-out, response agreement,
      bad-share rejection, aggregation, fallback ordering, and ambiguous-result
      recovery.
- [ ] Restart, catch-up, rotation, configuration mismatch, and readiness failures
      remain fail closed.
- [ ] The durable journal, bounded suffix verification, checkpoint verification,
      Aleph backup recovery, and peer catch-up remain supported.
- [ ] Fakewallet continues to exercise the real observation, DKG, consensus,
      restart, and recovery paths.
- [ ] Standalone CDK behavior remains unchanged when federation features are not
      enabled.

Checkpointing and bounded catch-up are not optional complexity. They implement
explicit recovery and availability requirements from the federation contract and
must not be removed merely to reduce line count.

## Prohibited cleanup shortcuts

- [ ] Do not replace threshold agreement with first-success behavior.
- [ ] Do not introduce single-member production consensus or signing.
- [ ] Do not return usable signatures or success before finality.
- [ ] Do not call payment backends during deterministic replay.
- [ ] Do not weaken authentication, sequence checks, or replay protection.
- [ ] Do not retain experimental federation fallbacks, adapters, dual reads, or
      schema compatibility paths.
- [ ] Do not replace bounded recovery with unbounded scans, queues, retries, or
      timeout increases.
- [ ] Do not remove restart, replay, disagreement, malicious-share, or
      partial-failure tests solely to reduce the test count.
- [ ] Do not build a generic distributed-operation framework, universal quorum
      policy, or speculative phase hierarchy. Prefer concrete operations and
      small ownership boundaries.

## P0 — correctness and safety blockers

### CLEAN-001: separate consensus ordering from wall-clock time

**Status:** Complete (2026-07-19)  
**Severity:** Critical  
**Primary files:**

- `crates/cdk/src/mint/federation/helpers.rs`
- `crates/cdk/src/mint/federation/application.rs`
- `crates/cdk/src/mint/melt/shared.rs`
- `crates/cdk-common/src/federation/operation.rs`
- `crates/cdk-axum/src/federation/payment_observation.rs`
- `crates/cdk/src/mint/federation/tests/mint.rs`
- `crates/cdk/src/mint/federation/tests/quote.rs`

The original implementation returned `journal_entry.index + 1` as a supposed
logical time. It stored that value in Unix timestamp fields and compared it with
Unix quote expiry values. The review subsequently confirmed that standalone CDK
does not perform an issuance-time quote-expiry check, so federation did not need
a consensus wall-clock protocol to preserve standalone behavior.

Implemented cleanup:

- [x] Removed the federation-only issuance-time quote-expiry check.
- [x] Removed the artificial test that used `expiry = 1` in the journal-index
      domain.
- [x] Mint quote materialization now uses the accepted response's committed
      `updated_at` Unix timestamp as `created_time`.
- [x] Federated melt quote metadata now carries the prepared quote's committed
      `created_time` and materialization uses it directly.
- [x] Bumped the canonical federation operation envelope from v4 to v5 because
      the melt-quote operation payload changed; no compatibility branch was
      retained for the undeployed experimental format.
- [x] Completed mint, swap, and melt operations, and payment records whose
      accepted operation carries no wall-clock evidence, use zero to represent
      an unknown time rather than storing journal order as a Unix timestamp.
- [x] The remaining journal-derived value is named
      `federation_accepted_order` and is used only by the separate on-chain
      batching subsystem as an ordering value.
- [x] Added realistic committed-time tests for mint quote conversion and replay.
- [x] Retained deterministic replay without consulting local time during
      federation application.

Verification completed:

- [x] Federation-enabled production `cdk` check passes.
- [x] Federation-enabled `cdk` unit suite passes: 779 tests passed with only
      three local-listener wallet tests filtered because listener binding is
      prohibited in the workspace sandbox.
- [x] Federation-enabled `cdk-common` unit suite passes: 430 passed and one
      ignored.
- [x] Full federation-enabled `cdk` unit-test target compiles with normal
      features.
- [x] Federation-enabled `cdk-axum` unit-test target compiles.
- [x] Focused quote conversion, quote application, mint replay, swap
      materialization, internal melt, on-chain internal-melt replay, and
      cross-member materialized-state convergence tests pass.

No federation clock, time quorum, compatibility mode, or database migration was
introduced. If a future protocol intentionally requires issuance-time expiry,
that must be specified separately and must not read member-local time during
deterministic replay.

### CLEAN-002: restore immutable standalone migrations

**Status:** Open  
**Severity:** Critical  
**Primary files:**

- `crates/cdk-sql-common/src/mint/migrations/postgres/1_initial.sql`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20240618195700_quote_state.sql`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20250819200000_remove_request_lookup_kind_constraints.sql`

Historical migrations were edited to add the `FAILED` melt quote state. This
changes migration checksums and does not update already-migrated databases.

Required work:

- [ ] Restore all three historical migrations byte-for-byte.
- [ ] Add a new forward PostgreSQL migration for the `FAILED` state.
- [ ] Add a new forward SQLite migration for the `FAILED` state.
- [ ] Test migration from a pre-federation standalone database.
- [ ] Test fresh database creation and upgrade produce identical final schemas.
- [ ] Confirm the migration is justified independently of federation, because
      the state is now used by ordinary payment backends and wallet recovery.

This task is separate from federation-only migration squashing in CLEAN-014.

### CLEAN-003: restore type-safe standalone key behavior

**Status:** Open  
**Severity:** Critical  
**Primary files:**

- `crates/cashu/Cargo.toml`
- `crates/cashu/src/nuts/nut01/mod.rs`
- `crates/cashu/src/nuts/nut01/public_key.rs`
- `crates/cashu/src/nuts/nut01/secret_key.rs`

The `cashu/federation` feature is empty, BLS dependencies are unconditional, and
the ordinary `PublicKey` and `SecretKey` enums now accept both secp256k1 and BLS
values. Several established public methods panic when called with a BLS value
that the same type successfully parsed.

Affected examples include:

- `PublicKey::to_uncompressed_bytes`;
- `PublicKey::x_only_public_key`;
- `PublicKey::x_only_public_key_with_parity`;
- `PublicKey::negate`;
- `SecretKey::x_only_public_key`;
- `SecretKey::to_scalar`;
- `SecretKey::as_scalar`;
- `SecretKey::negate`.

Required work:

- [ ] Prefer preserving the existing secp-only public types and introducing
      explicit v3/BLS point and scalar types at federation boundaries.
- [ ] If mixed enums are retained, make `cashu/federation` a real feature and
      gate BLS dependencies, variants, parsing, conversions, and exports.
- [ ] Replace kind-dependent panics with fallible typed accessors.
- [ ] Audit all NUT, P2PK, HTLC, wallet, FFI, and serialization call sites.
- [ ] Verify standalone serialized forms and accepted inputs remain unchanged.
- [ ] Add feature-disabled tests proving BLS values are not accepted by ordinary
      secp-only APIs.
- [ ] Add federation-enabled tests covering explicit BLS types and v3 rejection
      rules.

### CLEAN-004: delete unverified melt-change aggregation fallback

**Status:** Open  
**Severity:** Critical  
**Primary file:** `crates/cdk/src/wallet/federation.rs`

The wallet verifies member melt-change shares against the original blinded
outputs only while those outputs remain in the connector's in-memory
`melt_change_outputs` map. After rebuilding the connector, it falls back to
interpolating parseable shares without verifying them against the original
blinded messages.

Required work:

- [ ] Persist original blinded change outputs in the wallet saga/database.
- [ ] Recover them after process restart or connector reconstruction.
- [ ] Use the same verified aggregation path before and after restart.
- [ ] Delete the scalar/interpolation fallback that lacks output verification.
- [ ] If durable context is unavailable, return an ambiguous/recoverable result
      and fail closed.
- [ ] Add restart tests with one malformed share, one valid minority response,
      and exactly-threshold honest responses.
- [ ] Add a test proving no aggregate change is returned without verification
      against the requested blinded outputs.

The federation cut-through policy explicitly rejects preserving this fallback
for compatibility.

### CLEAN-005: narrow the raw threshold-signing capability

**Status:** Open  
**Severity:** High  
**Primary file:** `crates/cdk-common/src/federation/signing.rs`

`MemberConfigPrivate::sign_blinded_messages` is public and can produce shares for
an arbitrary operation ID and blinded outputs without requiring an accepted
journal entry. The current application path may call it correctly, but the API
does not enforce the invariant that consensus precedes signing.

Required work:

- [ ] Make raw signing private to the federation application/signing module, or
      require an accepted-entry capability/evidence type.
- [ ] Ensure all usable signing paths require an accepted journal operation.
- [ ] Add a compile-time or focused unit test demonstrating that ordinary callers
      cannot invoke raw member signing.
- [ ] Retain validation of federation ID, keyset, signer identity, operation ID,
      amount, output, and share provenance.

## P1 — reduce federation v1 scope

### CLEAN-006: remove BDK/FROST on-chain federation from the core v1 feature

**Status:** Open  
**Severity:** High  
**Decision:** Recommended  
**Primary files and crates:**

- `crates/cdk-bdk/`
- `crates/cdk-frost/`
- `crates/cdk-frost-bip32/`
- `crates/cdk-common/src/federation/address_allocation.rs`
- `crates/cdk-common/src/federation/chain_observation.rs`
- `crates/cdk-common/src/federation/frost_config.rs`
- `crates/cdk-common/src/federation/liveness.rs`
- `crates/cdk-common/src/federation/transaction_proposal.rs`
- `crates/cdk-axum/src/federation/frost_signing.rs`
- `crates/cdk-axum/src/federation/wallet_liveness.rs`
- `crates/cdk-axum/src/federation/broadcast.rs`
- `crates/cdk/src/mint/federation/address_allocation.rs`
- `crates/cdk/src/mint/federation/chain_observation.rs`
- `crates/cdk/src/mint/federation/transaction_policy.rs`
- `crates/cdk/src/mint/federation/transaction_state.rs`
- `crates/cdk-mintd/src/federated_bdk_backend.rs`
- `crates/cdk-mintd/src/federation_wallet_operator.rs`

The main federation plan scopes v1 around BLS issuance, Aleph ordering,
mint/swap/melt, payment-observation quorum, recovery, and fakewallet. The branch
also implements a second distributed protocol for address allocation, chain
observation, FROST DKG/liveness, transaction proposal, signing, broadcast intent,
broadcast observation, replacement, and reorg handling.

The directly identifiable BDK/FROST/on-chain surface is approximately 34,500 raw
lines before associated documentation and integration tooling.

Required work:

- [ ] Remove BDK, FROST, and FROST-BIP32 from the core `federation` feature.
- [ ] Remove address allocation, chain observation, transaction proposal,
      transaction signing, broadcast intent, and broadcast observation from the
      core `FederationOperation` enum.
- [ ] Remove on-chain wallet configuration and persistence from core federation.
- [ ] Remove on-chain workers from the core federation runtime.
- [ ] Remove on-chain DKG phases from the core dealer-free DKG implementation.
- [ ] Remove on-chain operator commands and deployment requirements from the
      core federation review path.
- [ ] Keep BLS threshold blind signatures; Bitcoin FROST is unrelated to that
      required feature.

If this functionality is retained for future work, place it behind a separate
`federation-onchain` feature and a later, independently reviewable PR. It must not
be transitively enabled by `cdk-mintd/federation` or `cdk-axum/federation`.

Expected effect:

- substantially smaller operation and configuration enums;
- fewer dependencies and feature combinations;
- simpler DKG and recovery state;
- removal of broadcaster and wallet-liveness workers;
- removal of reorg, replacement, descriptor, and FROST safety surfaces;
- a federation v1 that matches the stated implementation contract.

## P2 — isolate federation from standalone CDK

### CLEAN-007: move federation orchestration out of the standalone melt saga

**Status:** Open  
**Severity:** High  
**Primary files:**

- `crates/cdk/src/mint/melt/melt_saga/mod.rs`
- `crates/cdk/src/mint/melt/shared.rs`
- `crates/cdk/src/mint/melt/mod.rs`
- `crates/cdk/src/mint/start_up_check.rs`
- `crates/cdk/src/mint/saga_recovery.rs`
- `crates/cdk/src/mint/federation/application.rs`
- `crates/cdk/src/mint/federation/projection.rs`

Federation operation IDs, accepted-time handling, internal/external federation
settlement outcomes, share provenance, and federation recovery currently modify
the ordinary melt saga and startup path.

Required work:

- [ ] Restore the standalone melt saga's original state model where possible.
- [ ] Move the mapping from journal operation to side-effect recovery into a
      federation-owned projection or auxiliary saga record.
- [ ] Extract only small shared domain helpers that genuinely eliminate
      duplicated validation or transaction behavior.
- [ ] Keep proof verification, quote validation, fee calculation, and ordinary
      finalization in shared mint domain code.
- [ ] Keep ordering, journal identity, replay, accepted evidence, and quorum
      decisions in federation code.
- [ ] Remove federation-specific branches and imports from feature-disabled
      standalone compilation wherever possible.
- [ ] Cover both standalone and federation-enabled recovery after the refactor.

### CLEAN-008: make the Axum federation boundary explicit

**Status:** Open  
**Severity:** High  
**Primary files:**

- `crates/cdk-axum/src/lib.rs`
- `crates/cdk-axum/src/custom_handlers.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`
- `crates/cdk-axum/src/federation/operation_pipeline.rs`

`MintState` contains federation forwarding logic and generic handlers contain
federation construction, submission, waiting, recovery, and terminal-response
handling. The runtime has many `pub(crate)` optional fields, so it is not an
opaque ownership boundary.

Required work:

- [ ] Keep public handlers limited to parsing, authentication, method selection,
      serialization, and delegation.
- [ ] Move federation public-value workflows into one federation service/adapter.
- [ ] Store `Option<Arc<ActiveFederationRuntime>>` at the standalone integration
      seam.
- [ ] Make active-runtime fields private and required rather than optional.
- [ ] Separate setup/DKG ownership from active consensus runtime ownership.
- [ ] Replace `MintState` federation forwarding algorithms with a minimal runtime
      API.
- [ ] Suggested runtime API surface: public operation submission, payment
      observation submission, authoritative reads, status, and shutdown.
- [ ] Ensure feature-disabled handlers compile without federation imports or
      branches.

### CLEAN-009: retain focused database subtraits and split on-chain persistence

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/federation/`
- `crates/cdk-sqlite/src/mint/mod.rs`
- `crates/cdk-postgres/`

The existing focused federation database traits are the correct direction, but
the federation-enabled top-level database trait also requires FROST wallet
persistence and backend implementations contain a large federation method
surface.

Required work:

- [ ] Retain separate journal, application, DKG, and checkpoint persistence
      traits rather than placing methods directly in ordinary quote/proof traits.
- [ ] Remove `FederationFrostWalletDatabase` from core federation when
      CLEAN-006 is completed.
- [ ] Keep federation persistence implementations in federation-specific backend
      modules.
- [ ] Keep ordinary database APIs and source paths unchanged wherever possible.
- [ ] Avoid a new generic storage framework unless it demonstrably removes more
      code than it adds.

## P3 — runtime and operational cleanup

### CLEAN-010: introduce one owned worker supervisor

**Status:** Open  
**Severity:** High  
**Primary files:**

- `crates/cdk-axum/src/federation/payment_observation.rs`
- `crates/cdk-axum/src/federation/sync_loop.rs`
- `crates/cdk-axum/src/federation/broadcast.rs`
- `crates/cdk-axum/src/federation/wallet_liveness.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`

Workers currently self-spawn or have separate lifecycle ownership. The runtime
stores worker objects without owning a unified cancellation and join policy.

Required work:

- [ ] Workers expose `run` futures instead of detaching themselves.
- [ ] One supervisor owns named tasks, cancellation, joins, start-once behavior,
      and health/status.
- [ ] Add explicit deadlines around awaited backend and peer operations.
- [ ] Define shutdown behavior and test it.
- [ ] Define panic handling without silently restarting an unsafe task forever.
- [ ] If on-chain federation is removed, delete broadcaster and wallet-liveness
      supervision entirely.

### CLEAN-011: prevent Aleph retry starvation

**Status:** Open  
**Severity:** High  
**Primary file:** `crates/cdk-axum/src/federation/aleph_bft_network.rs`

Fresh outbound traffic can consume the complete per-tick budget. Retry traffic is
processed only when a tick drains no fresh messages, so a sustained stream can
starve failed-member delivery indefinitely.

Required work:

- [ ] Reserve a bounded portion of every tick for retries, or alternate fairly
      between fresh and retry traffic.
- [ ] Retain total message, byte, attempt, and queue bounds.
- [ ] Preserve local delivery progress independently of remote retry failures.
- [ ] Add a saturation test with continuous fresh traffic and a recovering peer.
- [ ] Add a bounded-queue eviction test and expose useful metrics.

### CLEAN-012: replace full quote scans with bounded pending queries

**Status:** Open  
**Severity:** High  
**Primary files:**

- `crates/cdk-axum/src/federation/payment_observation.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/quotes.rs`
- SQL backend implementations

Every observer round currently loads all mint quotes and all melt quotes, then
filters and batches in memory. The database load itself is unbounded.

Required work:

- [ ] Add stable, ordered, limited database queries for quotes requiring
      observation.
- [ ] Use a cursor or watermark so every pending quote eventually receives work.
- [ ] Bound per-round database rows, probes, consensus submissions, and elapsed
      time.
- [ ] Preserve event-driven fast paths.
- [ ] Add saturation and fairness tests with a large historical quote set.
- [ ] Keep issued on-chain quote handling out of core federation if CLEAN-006 is
      completed.

### CLEAN-013: enforce readiness at the submission boundary

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- `crates/cdk-axum/src/lib.rs`
- `crates/cdk-axum/src/federation/payment_observation.rs`
- `crates/cdk-axum/src/federation/node_runtime.rs`

Current observer call paths check authoritative-read readiness before calling a
lower-level consensus submission method, but the submission API itself does not
encode that policy.

Required work:

- [ ] Define the exact readiness policy for public requests, authoritative
      reads, internal observations, catch-up, and recovery.
- [ ] Expose one payment-observation submission method that enforces its policy.
- [ ] Avoid relying on every caller to remember a preceding readiness call.
- [ ] Test halted, catching-up, no-live-session, and awaiting-quorum states.

## P4 — undeployed schema, test, and production-surface cleanup

### CLEAN-014: squash federation-only migrations

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260712000000_create_federation_schema.sql`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260713000000_allow_federation_payment_observation_revisions.sql`
- `crates/cdk-sql-common/src/mint/migrations/sqlite/20260714000000_chunk_federation_aleph_bft_backups.sql`
- corresponding PostgreSQL migrations

Federation has not been deployed. Its three experimental schema stages therefore
should not become permanent compatibility history.

Required work:

- [ ] Produce one final SQLite federation schema migration.
- [ ] Produce one final PostgreSQL federation schema migration.
- [ ] Remove obsolete intermediate federation migrations.
- [ ] Regenerate experimental databases and fixtures.
- [ ] Verify fresh SQLite and PostgreSQL schemas match the persistence traits.
- [ ] Do not add dual reads, adapters, or upgrades for the removed federation
      schemas.

### CLEAN-015: remove trusted-dealer support from production APIs

**Status:** Open  
**Severity:** High  
**Primary files:**

- `crates/cdk-common/src/federation.rs`
- `crates/cdk-common/src/federation/trusted_dealer.rs`
- `crates/cdk/src/mint/builder.rs`
- `crates/cdk-mintd/src/federation_setup/`

Production requires dealer-free DKG, while trusted-dealer generation is publicly
re-exported from `cdk-common` and used broadly by test fixtures.

Required work:

- [ ] Move trusted-dealer generation into `federation-test-utils` or an equivalent
      explicitly test-only feature/crate.
- [ ] Remove public production re-exports.
- [ ] Ensure normal production federation builds cannot provision keys through a
      trusted-dealer API.
- [ ] Keep deterministic fixture generation available to tests that explicitly
      opt in.
- [ ] Retain runtime rejection of trusted-dealer production configurations.

### CLEAN-016: remove test-only Aleph adapters from production builds

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs`
- `crates/cdk-axum/src/federation/aleph_bft_network.rs`
- `crates/cdk-axum/src/federation/aleph_bft_session.rs`
- `crates/cdk-axum/src/federation/tests/`

The in-memory `CdkAlephBftBackup`, backup reader/writer, transport-injected startup
helpers, and pump wrappers are compiled into production or hidden with broad
dead-code allowances even though production uses the database-backed adapter and
the full pump path.

Required work:

- [ ] Gate genuinely test-only adapters with `#[cfg(test)]` or move them to test
      fixtures.
- [ ] Test the production pump function directly and delete redundant wrappers.
- [ ] Remove blanket `cfg_attr(not(test), allow(dead_code))` annotations.
- [ ] Investigate every remaining dead-code allowance and either use or delete
      the item.
- [ ] Keep the production database-backed, size-bounded Aleph backup path.

### CLEAN-017: consolidate DKG phase ownership

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- `crates/cdk-common/src/federation/dkg.rs`
- `crates/cdk-common/src/federation/frost_dkg.rs`
- `crates/cdk-axum/src/federation/dkg.rs`
- `crates/cdk-mintd/src/federation_setup/`

Common and Axum layers currently contain overlapping DKG phase concepts. The
FROST phase adds a second ceremony and materially expands the state machine.

Required work:

- [ ] Remove the FROST ceremony from core DKG with CLEAN-006.
- [ ] Keep one BLS DKG phase vocabulary and state machine in common/domain code.
- [ ] Limit Axum to authenticated transport, persistence orchestration, and
      message delivery.
- [ ] Limit mintd to configuration and lifecycle wiring.
- [ ] Do not replace the concrete DKG with a generic effect framework.

## P5 — build, dependency, and source hygiene

### CLEAN-018: restore strict clippy for standalone and federation builds

**Status:** Open  
**Severity:** High  
**Primary examples:**

- unconditional `PaymentIdentifier` import in
  `crates/cdk/src/mint/melt/melt_saga/mod.rs`;
- undocumented panic in
  `crates/cdk/src/mint/federation/configuration_and_admission.rs`;
- needless `?` in `crates/cdk/src/mint/federation/application.rs`;
- complex tuple return in `crates/cdk/src/mint/federation/helpers.rs`;
- oversized federation settlement enums;
- nine-argument federation melt helpers;
- oversized `Mint::new_internal` argument list.

Required work:

- [ ] Fix feature-disabled imports instead of allowing them.
- [ ] Return structured errors instead of adding panic allowances where
      practical.
- [ ] Replace large or complex settlement structures through the orchestration
      cleanup, not broad lint suppression.
- [ ] Run strict clippy for every supported standalone and federation feature
      combination.

Required commands include:

```bash
cargo clippy -p cdk --no-default-features --features mint,wallet --lib -- -D warnings
cargo clippy -p cdk --no-default-features --features mint,wallet,federation --lib -- -D warnings
cargo clippy -p cdk-axum --lib -- -D warnings
cargo clippy -p cdk-axum --features federation --lib -- -D warnings
```

### CLEAN-019: audit new dependency features

**Status:** Open  
**Severity:** Medium  
**Primary files:**

- root `Cargo.toml`
- federation-enabled crate manifests

New dependencies such as `aes-gcm` and `aleph-bft` do not declare
`default-features = false`. Internal FROST dependencies also omit explicit
default-feature control.

Required work:

- [ ] Set `default-features = false` for new dependencies unless a documented
      reason requires defaults.
- [ ] Enable only explicitly required features.
- [ ] Inspect the federation feature graph for duplicate runtimes, TLS stacks,
      platform support, and crypto implementations.
- [ ] Remove BDK/FROST dependencies from core federation with CLEAN-006.
- [ ] Keep federation dependencies inactive in standalone builds.

### CLEAN-020: make formatting and diff checks clean

**Status:** Open  
**Severity:** Low  

`git diff --check` reports blank lines at EOF and whitespace findings across
federation tests, implementation files, documents, and the mobile FFI patch.
Some Markdown two-space line endings are intentional, but source EOF and patch
whitespace are not.

Required work:

- [ ] Remove blank lines at EOF in Rust source and tests.
- [ ] Clean patch-file whitespace.
- [ ] Retain intentional Markdown hard line breaks only where they are useful.
- [ ] Run `cargo fmt --all -- --check` and `git diff --check`.

## P6 — tests, documentation, tooling, and branch reviewability

### CLEAN-021: consolidate tests without reducing the safety matrix

**Status:** Open  
**Severity:** Medium  
**Primary areas:**

- `crates/cdk/src/mint/federation/tests/`
- `crates/cdk-axum/src/federation/tests/`
- inline tests in `crates/cdk-axum/src/custom_handlers.rs`
- federation wallet tests
- federation SQL backend tests

The test suite contains very large scenario files and repeated construction,
restart, duplicate-operation, disagreement, and recovery fixtures. Much of
`custom_handlers.rs` is inline test code.

Required work:

- [ ] Extract handler tests from the production handler file.
- [ ] Centralize member, journal, quote, output, and transport fixtures.
- [ ] Convert genuinely duplicated cases to table-driven invariant tests.
- [ ] Map every removed test to a retained invariant and replacement coverage.
- [ ] Preserve explicit tests for consensus-before-signing, deterministic replay,
      restart, catch-up, duplicate operations, invalid shares, minority
      disagreement, conflicting quorums, pending melt recovery, and bounded
      resources.
- [ ] Do not serialize tests or increase timeouts merely to hide liveness bugs.

### CLEAN-022: consolidate federation documentation

**Status:** Open  
**Severity:** Medium  

The branch has 52 federation/deployment documents with overlapping plans,
completed checklists, incident records, performance logs, and generated HTML
copies.

Retain a small authoritative documentation set:

- [ ] `federated-cashu-implementation-plan.md` as the contract and scope.
- [ ] One current architecture document covering ownership, ordering, journal,
      checkpoints, catch-up, and recovery.
- [ ] One security/threat-model and open-risk document.
- [ ] One production operator guide for DKG, rotation, backup, recovery, upgrade,
      and incident response.
- [ ] One current cleanup/release tracker, this document or its successor.

Remove or archive from the core branch:

- [ ] superseded implementation plans;
- [ ] completed consolidation plans after their final requirements are captured;
- [ ] duplicate generated HTML copies;
- [ ] environment-specific incident transcripts and performance dumps;
- [ ] stale improvement and todo files superseded by this tracker;
- [ ] deployment-specific Nocean records that do not define product behavior.

### CLEAN-023: remove generated and environment-specific artifacts

**Status:** Open  
**Severity:** High  
**Primary examples:**

- `docs/nocean-invite-code.txt`
- deployment scripts and generated configs under `scripts/`
- mobile-binding patch artifacts
- benchmark and stress-test output records

Required work:

- [ ] Remove the committed generated invite artifact.
- [ ] Generate invitations and environment configs at operator runtime.
- [ ] Ensure no private shares, credentials, secrets, machine-specific paths, or
      personally identifiable environment data are committed.
- [ ] Move reusable deployment tooling to a separate follow-up or clearly scoped
      operator-tooling area.
- [ ] Keep core federation review independent of a specific hosted environment.

### CLEAN-024: reconstruct an independently reviewable branch series

**Status:** Open  
**Severity:** High  

The current branch includes merge commits from `main` and multiple federation
branches, more than one thousand commits, and large unrelated documentation and
tooling changes.

Required work:

- [ ] Rebase onto current upstream `main`; do not merge `main` into the feature
      branch.
- [ ] Reconstruct coherent, independently testable commits with conventional
      commit messages.
- [ ] Keep standalone shared-domain changes separate from federation runtime
      changes.
- [ ] Keep migrations, tests, and documentation with the change they validate.
- [ ] Move on-chain federation, deployment tooling, mobile bindings, dashboards,
      and historical incident records to separate follow-ups.
- [ ] Ensure every commit builds in the feature combinations it affects.

## Recommended target architecture

### `cashu`

- Preserve existing secp-only public protocol behavior.
- Add explicit, federation-gated v3/BLS point and scalar types.
- Keep v3 encoding, aggregate keys, blinded points, and signature verification
  explicit rather than hidden in a universal key enum.

### `cdk` mint

- Keep Cashu validation, proof checks, quote handling, fee calculation, and
  database transactions in shared domain helpers.
- Keep one federation application service responsible for accepted-journal
  application and projection.
- Keep federation operation identity, consensus evidence, replay, and recovery
  orchestration out of standalone saga state where possible.

### `cdk` wallet

- Keep one federation connector for fan-out, response grouping, share
  verification, aggregation, fallback ordering, and ambiguous outcomes.
- Persist all request context required to verify delayed or post-restart
  responses.
- Return ordinary Cashu responses to existing wallet sagas.

### `cdk-axum`

- Keep public and private handlers thin.
- Delegate federation workflows to an opaque active runtime.
- Separate DKG/setup ownership from active consensus ownership.
- Supervise all long-lived workers from the runtime.

### Storage

- Keep focused feature-gated federation subtraits.
- Keep journal, checkpoint, application, DKG, and backup persistence explicit.
- Remove on-chain/FROST storage requirements from core federation.
- Keep ordinary standalone database APIs untouched where possible.

### Consensus operations

The core v1 operation set should contain only operations necessary for:

- mint quote creation and payment observation;
- issuance;
- melt quote creation and payment observation;
- melt;
- swap;
- keyset/session rotation and other required consensus metadata.

Address allocation, chain observation, Bitcoin transaction proposal, FROST
signing, broadcast intent, and broadcast observation belong to a later on-chain
feature, not core federation v1.

## Proposed implementation sequence

### Phase 1: restore correctness baseline

- [x] CLEAN-001 consensus ordering and timestamp separation.
- [ ] CLEAN-002 immutable migrations.
- [ ] CLEAN-003 type-safe key boundary.
- [ ] CLEAN-004 verified melt-change recovery.
- [ ] CLEAN-005 narrow signing capability.
- [ ] CLEAN-018 strict clippy baseline.

### Phase 2: cut core scope

- [ ] CLEAN-006 remove/split BDK and FROST on-chain federation.
- [ ] CLEAN-009 remove FROST persistence from the core database trait.
- [ ] CLEAN-017 simplify core DKG to the required BLS ceremony.
- [ ] CLEAN-019 reduce the feature and dependency graph.

### Phase 3: restore thin standalone integration

- [ ] CLEAN-007 move federation orchestration out of standalone sagas.
- [ ] CLEAN-008 complete the Axum/runtime boundary.
- [ ] CLEAN-013 encode readiness policy at the runtime API.

### Phase 4: simplify runtime operations

- [ ] CLEAN-010 worker supervisor.
- [ ] CLEAN-011 fair Aleph retry scheduling.
- [ ] CLEAN-012 bounded pending-quote queries.

### Phase 5: delete undeployed and test-only baggage

- [ ] CLEAN-014 squash federation migrations.
- [ ] CLEAN-015 trusted dealer becomes test-only.
- [ ] CLEAN-016 test-only Aleph adapters leave production builds.
- [ ] CLEAN-020 formatting and diff hygiene.
- [ ] CLEAN-021 consolidate tests.
- [ ] CLEAN-022 consolidate docs.
- [ ] CLEAN-023 remove generated/environment artifacts.
- [ ] CLEAN-024 reconstruct the branch series.

## Final verification matrix

### Standalone compilation and lint

- [ ] `cargo check --workspace --all-targets`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo check -p cdk --no-default-features --features mint,wallet`
- [ ] `cargo clippy -p cdk --no-default-features --features mint,wallet --lib -- -D warnings`
- [ ] `cargo check -p cdk-axum`
- [ ] `cargo clippy -p cdk-axum --lib -- -D warnings`
- [ ] `cargo check -p cdk --target wasm32-unknown-unknown`

### Federation compilation and lint

- [ ] `cargo check -p cdk --no-default-features --features mint,wallet,federation`
- [ ] `cargo clippy -p cdk --no-default-features --features mint,wallet,federation --lib -- -D warnings`
- [ ] `cargo check -p cdk-axum --features federation`
- [ ] `cargo clippy -p cdk-axum --features federation --lib -- -D warnings`
- [ ] Confirm core federation does not enable BDK or FROST.
- [ ] Confirm standalone dependency trees do not include BLS/Aleph/federation
      dependencies unnecessarily.

### Formatting and migrations

- [ ] `cargo fmt --all -- --check`
- [ ] `git diff --check`
- [ ] Upgrade a pre-federation SQLite database.
- [ ] Upgrade a pre-federation PostgreSQL database.
- [ ] Compare upgraded and fresh final schemas.
- [ ] Verify all historical standalone migration checksums remain unchanged.

### Domain and protocol tests

- [x] Mint quote creation timestamps and standalone-aligned issuance semantics.
- [ ] Issuance only after accepted payment evidence and consensus.
- [ ] Swap only after consensus and deterministic proof-state transition.
- [ ] Melt quote creation, internal settlement, external settlement, failure, and
      change.
- [ ] Restore and check-state threshold-authoritative reads.
- [ ] Keyset v3/BLS-only startup, signing, restore, and wallet enforcement.
- [ ] No signature-share creation before accepted journal application.

### Distributed and adversarial tests

- [ ] Dealer-free DKG with restart at every phase.
- [ ] One unavailable member while thresholds remain available.
- [ ] Minority inconsistent responses.
- [ ] Malformed and cryptographically invalid signature shares.
- [ ] Two incompatible payment-observation groups reaching threshold.
- [ ] Duplicate, reordered, and conflicting operations.
- [ ] Sustained fresh Aleph traffic while retrying a recovering peer.
- [ ] Bounded inbound, retry, backup, waiter, notification, and quote-observation
      queues at saturation.
- [ ] Restart from exact checkpoint and checkpoint tail.
- [ ] Catch-up from a lagging member.
- [ ] Corrupt or mismatched checkpoint, backup, transcript, roster, and keyset.
- [ ] Wallet restart between melt request and delayed change response.
- [ ] Pending melt crash/recovery matrix across every durable transition.

### End-to-end and operational tests

- [ ] Multi-member fakewallet mint, send/receive, restore, swap, and melt.
- [ ] Member restart during pending issuance and pending melt.
- [ ] Multi-host deployment and upgrade drill.
- [ ] Sustained partition and recovery test.
- [ ] Performance test with a large historical quote/journal set.
- [ ] Backup and operator-only materialized-state audit recovery.
- [ ] Production operator documentation reviewed against actual commands and
      failure modes.

## Completion criteria

This cleanup is complete only when:

- [ ] all P0 findings are fixed with regression tests;
- [ ] core federation matches the scope in the main implementation plan;
- [ ] BDK/FROST on-chain federation is absent or independently feature-gated;
- [ ] standalone CDK public behavior and builds remain unchanged;
- [ ] no successfully parsed federation value can reach an ordinary public-key
      panic;
- [ ] every delayed wallet share is verified against durable original request
      data;
- [ ] all historical standalone migrations are restored and upgrades are tested;
- [ ] strict standalone and federation clippy pass;
- [ ] runtime tasks are owned, bounded, observable, and stoppable;
- [ ] federation-only undeployed schemas and compatibility fallbacks are removed;
- [ ] production builds contain no trusted-dealer shortcut or test-only Aleph
      implementation;
- [ ] the branch is rebased and split into a reviewable series;
- [ ] the retained documentation describes the implementation that actually
      ships;
- [ ] independent security/protocol review and remaining release work from the
      main implementation plan are complete.

## Review verdict at creation

`CHANGES_REQUESTED`

The core BLS/Aleph federation design should be retained, but the reviewed branch
contains correctness blockers, an unnecessarily bundled on-chain protocol,
standalone API and saga coupling, and substantial test/deployment/compatibility
scaffolding that should be removed or isolated before release review.
