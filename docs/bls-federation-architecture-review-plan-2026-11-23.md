# BLS federation architecture review and refactoring plan

Status: package 4 started on 2026-11-23 (first step recorded in the implementation ledger). Decisions in "Decisions needed" are open.

Baseline: `bls-federation` at `3183d04b8`, compared with its fork point from `main` at `87c2bf976`. All `path:line` references are against that baseline.

## Objective and boundaries

Improve the quality of the federation implementation and correct the architectural choices that cause most of its duplication. The federation already works; this plan is about how it is built, and it is not a bug hunt or a security audit.

- Preserve all federation functionality and all ordinary CDK behavior, APIs, defaults and serialization.
- Federation is undeployed. Federation-only formats, schemas and APIs need no compatibility layer; change them in place and regenerate golden vectors.
- Prefer reuse of existing CDK owners (mint sagas, `MintPayment`, `Transport`, `BackoffPolicy`, `MintDatabase` transactions) over new parallel ones.
- Where a finding touches a possible safety gap, this document stays at the level of "these two implementations differ". Details belong in the security channel named in `SECURITY.md`.
- This plan extends the September 5 cleanup and the September 7 maintainability plan. Two of its packages reverse decisions made there; see "Relationship to earlier plans".

## Method and confidence

The branch was read in nine areas (core protocol domain, transport/setup/DKG domain, consensus runtime, custody orchestration, mint integration, wallet/client, storage, crypto/FROST, custody backends and daemon). Nothing was built or run.

Each evidence line carries one of two marks:

- **[V]** re-opened and confirmed in the code after the area read.
- **[R]** reported by an area read with file references, not independently re-opened.

## Size and shape of the branch

760 files changed, about 269,000 lines of Rust added (about a third in test files), 107,000 lines under `docs/`, 19,000 under `scripts/`, in 1,288 non-merge commits. The branch sits on the small `bls` branch (single-signer BLS v3 keysets, 47 files).

| Layer | Location | Content |
| --- | --- | --- |
| Crypto | `cashu`, `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark` | Multi-curve key types, threshold BLS shares, FROST DKG and signing with durable nonces |
| Protocol domain | `crates/cdk-common/src/federation` (about 42k lines) | Operations, envelopes, journal, checkpoints, config, BLS DKG, setup ceremony, signed transport messages, on-chain proposal state |
| Mint integration | `crates/cdk/src/mint/federation` (9.4k + 13.6k tests) | Admission and deterministic application of ordered operations |
| Node runtime | `crates/cdk-axum/src/federation` (37k + 35k tests) | AlephBFT session, sync, catch-up, checkpoints, peer client, payment and custody workers |
| Custody | `cdk-bark`, `cdk-federation-bark`, `cdk-bitcoin-wallet` | Ark-backed Lightning and FROST on-chain custody |
| Storage | `crates/cdk-sql-common/src/mint/federation*` | 38 tables, 37 JSON text columns, 121 trait methods |
| Wallet | `crates/cdk/src/wallet/federation.rs`, `cdk-iroh`, `cdk-cli`, `cdk-ffi` | Connector that fans out to members and aggregates shares |
| Operator | `crates/cdk-mintd/src/federation_*.rs` | Setup ceremony, DKG finalization, status and digest tools |

## Central diagnosis

Federation was added as a second mint beside the standalone one, joined by feature gates and runtime branches. There is no seam for the two things that actually differ between the modes (who signs outputs, and where the active keysets come from), and no crate that owns "a federation node". Most findings below are consequences of those two gaps.

## Findings

### 1. Mint business rules are implemented twice

- [V] Federated swap and mint application are parallel pipelines: `crates/cdk/src/mint/federation/application.rs:2369` and `:2193` against the swap saga and `process_mint_request` in `crates/cdk/src/mint/issue/mod.rs`.
- [V] `verify_federation_transaction_balanced_with_config` at `crates/cdk/src/mint/federation/application.rs:2958` is a line-for-line copy of `crates/cdk/src/mint/verification.rs:286`. [R] The output, input and fee functions are copied the same way.
- [V] Corrected on 2026-11-23: the federated mint does not lack value rules. It bounds issuance by the payment quorum covering the issued amount plus the new amount. The standalone rules it does not run (batch size, NUT-29 methods, NUT-04 limits) read each member's local `mint_info`, so they belong at admission, not in deterministic application; the HTTP handler checks the batch size (`crates/cdk-axum/src/custom_handlers.rs:257`). What was duplicated is the pure quote rules (uniform method and unit, NUT-20 signatures), now shared.
- [V] Share signing was a three-times repeated block (now one). Each application writes the share and a blind signature to two tables; corrected on 2026-11-23: the share record is the provenance authority (restore must ignore a blind signature without one, per `7fdb8d473`), so the removable copy is the blind signature, not the share.
- [R] Admission and application validate separately, so the proposer verifies BLS proofs three times, while stateless rules run only at application.
- [R] Melt has one persisted saga but two drivers; the federated transitions are an if/else over quote state, observation state and saga existence.

Root cause: keysets have two owners (the static config and the rotation-replayed config), and signing is a branch. Every function that touches either was forked.

### 2. The node lives in the HTTP adapter, and dependencies point the wrong way

- [V] 22 of the 44 non-test federation files in `crates/cdk-axum/src` contain no axum reference; 119 functions outside the route files take `MintState`.
- [V] `cdk-axum` depends on `aleph-bft`, `cdk-frost`, `cdk-bark`, `cdk-bdk` and `cdk-bitcoin-wallet`. `cdk-common` depends on `cdk-frost` and `cdk-frost-bip32`. `cdk-sql-common` depends on `cdk-federation-bark`.
- [V] Bark's stateful validator is one 1,412-line function in generic `cdk`: `crates/cdk/src/mint/federation/projection.rs:119`.
- [R] The setup ceremony (`crates/cdk-mintd/src/federation_setup_command.rs`, 2,792 lines) and the derivation of the final consensus config (`crates/cdk-mintd/src/federation_dkg_status.rs:314`) live in the daemon's CLI layer.
- [R] Public Cashu handlers branch inline: 37 cfg gates and 16 runtime branches in `custom_handlers.rs` and `router_handlers.rs`.

### 3. A feature flag changes core public types

- [V] `crates/cashu/src/nuts/nut01/mod.rs:15` swaps the whole `PublicKey` and `SecretKey` modules by feature. `to_bytes()` returns `[u8; 33]` or `Vec<u8>`; `as_secret_bytes()` returns `&[u8]` or `Vec<u8>`.
- [V] `ProofV4.dleq` has different serde attributes per feature (`crates/cashu/src/nuts/nut00/mod.rs:458`), and `KeySetVersion::Version02` exists only with the feature (`crates/cashu/src/nuts/nut02.rs:81`).
- [V] There are 1,548 `feature = "federation…"` gates in the crates, including about 108 in `crates/cdk-mintd/src/lib.rs`, 44 in `crates/cdk/src/mint/builder.rs` and 40 in `crates/cdk/src/mint/mod.rs`.
- [R] BLS v3 keysets are independent of federation but only exist under `federation`, and that feature also pulls FROST into wallets. `scripts/check_standalone_api.py` exists to guard against feature unification.
- [R] 60 gates inside `crates/cashu/src/nuts/nut01/bls.rs` are dead because the module is already gated.

### 4. Custody is a parallel payment pipeline

- [V] The `MintPayment` implementations for federated Bark and BDK are stubs: `crates/cdk-bark/src/adapter.rs:90` and `crates/cdk-mintd/src/federated_bdk_backend.rs:114`.
- [V] The Bark melt driver is one 779-line function in `crates/cdk-axum/src/federation/bark_outgoing.rs`.
- [V] Consensus byte encodings are written twice, by the proposer at `crates/cdk-bark/src/runtime.rs:1321` and by the validator at `crates/cdk/src/mint/federation/projection.rs:2739`. Four domain tags appear in both files. If the copies drift, members disagree.
- [V] The external-application registry is a process global (`crates/cdk-common/src/federation/operation.rs:402`) with one plugin, registered at nine call sites. [R] It costs about 900 lines of mirror enums.
- [R] There is no backend seam: 278 `federation-bdk`/`federation-bark` gates in `cdk-axum`, three actor-rotation schemes, and two near-identical signing coordinators.

### 5. State has several owners and lifecycles are implicit

- [R] The operation envelope is stored in finalized items, journal entries, pending operations and the AlephBFT backup. The journal is also mirrored in memory and re-synced by hand.
- [V] Transitions are guarded by JSON string equality: `crates/cdk-sql-common/src/mint/federation/dkg.rs:116`, and five further sites in `setup.rs` and `bark_projection.rs`.
- [V] `FederationOperationStatus::Applying` is mapped and matched but never written. [R] Operation status is spread over five enums with no transition table.
- [V] `mark_lagging`, `mark_catching_up` and `mark_healthy` overwrite `Halted` without a guard (`crates/cdk-axum/src/federation/state.rs:1035`), while `mark_awaiting_quorum` beside them guards.
- [V] Four consensus state machines are single `state_v1` blobs, loaded, validated and rewritten whole per operation: `crates/cdk/src/mint/federation/transaction_state.rs:30`, `chain_observation.rs:19`, `address_allocation.rs:14`, `application.rs:151`.
- [R] The setup phase of the config is inferred from `!keysets.is_empty() || setup_transcript.is_some()` at 11 sites.

### 6. Work grows with history

- [V] Every mint and melt application loads the entire journal: `crates/cdk/src/mint/federation/application.rs:1684`, `:1707`, `:1716`. The quote row already encodes the fact being checked.
- [R] The Bark projection is read as an all-history snapshot through `serde_json::Value` at about 23 call sites.
- [V] All federation SQL contains six `DELETE` statements, all in `journal.rs`. [R] Checkpoints do not enable truncation.
- [V] The payment scheduler is one JSON blob for all quotes (`crates/cdk-axum/src/federation/payment_schedule.rs:12`). [R] It is rewritten on each transition under a global lock.

### 7. No shared codec, RPC table or error structure

- [V] `canonical_json_bytes` is defined five times in `crates/cdk-common/src/federation` (`operation.rs`, `checkpoint.rs`, `consensus.rs`, `transport.rs`, `transaction_proposal.rs`). [R] Three canonicalization styles coexist, and length framing is `u32` in some copies and `u64` in others.
- [V] 35 message kinds are served by 29 hand-written client methods in `crates/cdk-axum/src/federation/peer_client.rs`. [R] Adding a kind touches about six places.
- [V] `FederationError` has 165 variants. [R] Its free-form `reason` strings are why a 170-line redactor exists.
- [V] `serde_json` features `preserve_order` and `raw_value` were enabled for the whole workspace. `RawValue` has no user.
- [R] Three nonzero-`u16` member id types exist (`cdk-common`, `cdk-frost`, `cashu`).

### 8. Scaffolding and test code ship in production modules

- [V] `crates/cdk-common/src/federation/consensus.rs` still opens with "This is not a production quorum protocol". [R] The pre-AlephBFT append and catch-up paths have only test callers.
- [V] `crates/cdk-mintd/src/federation_smoke.rs` (1,624 lines) compiles under `federation + fakewallet + sqlite`, and the latter two are default features.
- [R] A 310-line test-only recovery implementation, eight fault-injection flags, test fields in session options, a legacy non-checkpoint catch-up path and an unused `/operations` route remain in production modules.

### 9. Workers and locks are ad hoc

- [V] A global Bark-incoming mutex is held across backend waits and peer calls: `crates/cdk-axum/src/federation/bark_incoming.rs:191`.
- [V] Eight peer clients are constructed. [R] Each has its own sequence counter, patched by a process-global static.
- [V] A handler polls the database in a sleep loop: `crates/cdk-axum/src/custom_handlers.rs:424`.
- [V] `BackoffPolicy` (`crates/cdk-common/src/stream.rs`) is unused by federation. [R] Seven hand-rolled backoff mechanisms and two supervision systems exist; the documented lock order covers 4 of 14 locks.

### 10. Wallet and operator surface

- [V] The "group matching responses, pick a group at threshold" pattern is written inline (`crates/cdk/src/wallet/federation.rs:161`). [R] It appears seven times, with four different definitions of equality.
- [R] Federation discovery, validation and registry are reimplemented in `cdk-cli`, `cdk-ffi` and `scripts/fed-cdk-tui`, with different alias rules.
- [V] Corrected on 2026-11-23: the wallet connector's member-validity cache is not vestigial. Connectors built from member URLs start with it filled and skip member validation; connectors built from injected clients validate over the network. Whether both should validate is a design decision. The dead part was the member-map parameter threaded through every helper, now removed.
- [R] BLS verification is repeated at five wallet sites behind 47 cfg gates.
- [R] The operator CLI re-implements the signed peer client because the real one is `pub(crate)`.
- Keep: the `MintConnector` seam held; `Wallet` gained no federation fields.

### 11. Unrelated changes and repository weight

- [V] The bitreq backend now runs each request in `spawn_blocking` (`crates/cdk-http-client/src/backends/bitreq_backend.rs:119`).
- [V] The workspace `rust-version` moved from 1.85.0 to 1.91.0 while `AGENTS.md` still states 1.85.0.
- [V] `docs/` gained six generated presentation directories, a zip archive, 60 benchmark CSV files and `docs/nocean-invite-code.txt`. `scripts/fed-cdk-tui` is a 4.8k-line application with its own 6,115-line lockfile.
- [R] Standalone wallet melt compensation, the fake-wallet repayment delay and saga rollback behavior changed.

## Target architecture

```
cashu                 additive `bls` feature; PublicKey/SecretKey stay secp-only;
                      curve-tagged point type only in keyset keys, B_, C_, C

cdk-common            traits and shared types; no FROST, no custody dependency

cdk (mint)            per operation: validate -> plan -> commit(tx)
                      seams: OutputSigner, KeysetSource
                      federation = a driver calling the same plan/commit

cdk-federation        the node: one owner of consensus state, DB as the only
                      journal, one replay(from, source), one worker runner,
                      one typed RPC table generating router and client,
                      setup ceremony and operator client as library code

custody               one custody trait; impls: MintPayment bridge, Bark, BDK;
                      drivers and consensus codecs live in the backend crates

storage               one consensus log; typed projection rows with a state
                      column and guarded updates; retention at checkpoints

adapters              cdk-axum (routes), cdk-iroh (transport), cdk-mintd
                      (parse arguments, print results)
```

Lifecycles follow one pattern, already used well in `crates/cdk-common/src/federation/setup/persistence.rs` and in `cdk-frost`'s nonce store: one state enum, one transition function in the domain, writers that condition on the expected source state.

## Work packages

Packages are ordered so that each one shrinks the surface the next has to move. Each is a series of reviewable changes, not one commit.

### 0. Separate what does not belong

**Approved 2026-11-25:** split the standalone changes out of this branch.

- Move the presentation directories, the zip, the benchmark CSVs and the invite code file out of the branch, or into one clearly marked archive location.
- Split the standalone behavior changes (bitreq, wallet melt compensation, fake-wallet delay, saga rollback, MSRV) into their own changes against `main`, each with its own justification, or revert them here.
- Gate `federation_smoke` on `federation-test-utils` or move it to `cdk-integration-tests`; drop `cdk/wallet` from the daemon's `federation` feature.
- Decide the fate of `scripts/fed-cdk-tui` (decision 7).

Done when the diff against `main` contains only federation and its direct prerequisites.

### 1. Delete superseded code

- Pre-AlephBFT journal append and catch-up paths, the test-only recovery implementation and `*_for_test` constructors, `ModuleFact`, and the stale module headers in `cdk-common`.
- Legacy non-checkpoint catch-up, the non-retrying pump, the finality test flags and the unused `/operations` route in `cdk-axum`.
- `FederationOperationStatus::Applying`, unused trait methods, speculative single-variant version enums and compatibility shims for never-shipped formats.
- The wallet's member-validity cache and its unused constructors; the dead gates in `bls.rs` and `wallet/performance.rs`.
- Retarget tests at the production entry points; inject faults through a test implementation of a trait instead of `cfg(test)` fields.

Done when no federation module carries `allow(dead_code)` and no production struct has a `cfg(test)` field.

### 2. Shared primitives

- One `canonical` module: one canonical-bytes function, one length framing, one domain-hash helper, one macro for the 32-byte and 64-byte newtypes.
- One member id type; one `frost_member()` conversion.
- Reuse `BackoffPolicy` and add one `retry_transient(budget, op)`; one time source.
- Per-module error enums with typed reasons in place of the 165-variant `FederationError` and its redactor.
- Establish why `preserve_order` was enabled workspace-wide; enable it only where needed and drop `raw_value`.

Done when each helper has one definition and golden vectors are regenerated once.

### 3. Additive `cashu` types

- Introduce a `bls` feature that only adds: v3 keysets, pairing verification and share aggregation. `Version02` and the `ProofV4` serde attributes become unconditional.
- Keep `PublicKey` and `SecretKey` secp-only with their original API. Add a curve-tagged type used only in keyset keys, blinded messages, signatures and blinding factors.
- Put keyset-version dispatch behind one set of functions (`verify_proofs`, `verify_blind_signatures`, blind, unblind, sign) so wallet and signatory call sites stop branching.
- Move dealer and commitment functions to a mint-side module; make federation features depend on `bls` rather than define it.

Done when no public type in `cashu` differs by feature and a wallet can enable `bls` without linking FROST. This reverses September 7 package 1 (decision 1).

### 4. Mint seams

- Add `OutputSigner` (signatory or local share) and `KeysetSource` (one owner feeding `Mint.keysets`, including rotations).
- Split swap, mint and melt into `validate`, `plan` and `commit(tx, plan, signatures)`. Standalone and federated application both call them.
- Delete the copied verification, fee and signing functions and the duplicate share table write.
- Share one `validate_stateless()` between admission and application.
- Give the melt saga an explicit driver (local or consensus operation) and one transition table; payment observations become events.
- Replace the full-journal loads with row lookups; hold the effective config in an `Arc`.
- Add a parity test that runs the same request vectors through both drivers.

Done when federated application contains no rule absent from the shared plan and the pre-existing mint files carry no federation gate beyond module registration. This changes shared standalone code (decision 2).

### 5. Extract the node crate

- Create `cdk-federation` and move the transport-independent runtime out of `cdk-axum`: consensus session, sync, catch-up, checkpoints, peer client, payment observer, DKG executor.
- Export `FederationNode` owning `Arc<Mint>`. `cdk-axum` keeps the router, the handlers and error mapping.
- Replace inline handler branches with one frontend seam (standalone or federated) that handlers call unconditionally.
- One peer client per node; one RPC table `(kind, path, request, response, limits)` driving both router and client.
- One `NodePhase` enum with one transition function; session tasks become children of `FederationTaskGroup`; one worker runner with tick, wake and backoff.
- One always-compiled metrics facade in place of interleaved cfg gates.
- Move the setup ceremony, DKG finalization and a typed operator client into library code; the daemon parses and prints.
- Remove the FROST dependency from `cdk-common` (decision 4 covers how far to go).

Done when `cdk-axum` has no dependency on `aleph-bft`, FROST or custody crates.

### 6. Storage and lifecycles

- One consensus log: the journal row references the finalized item instead of repeating it; rolling digests are computed at insert; batch inserts are real batches.
- One status column per lifecycle with `UPDATE … WHERE state = :expected`, checked against the domain transition table. Remove JSON-equality compare-and-set.
- Replace the four `state_v1` blobs with rows per intent, transaction, UTXO and allocation.
- Replace the Bark snapshot and its `serde_json::Value` boundary with typed keyed queries; move the Bark transition table and the DKG config rule out of the SQL crate.
- Define retention: checkpoints become truncation points for the log, shares, observations and history tables.
- Reduce the 121-method trait surface to cohesive ranged or keyed operations.
- Scheduler state becomes per-quote rows holding only local facts; leases stay in memory.
- One apply transaction per operation.
- Write the storage contract per store: purpose, authoritative source, writer, readers, retention, rebuild strategy.

Because federation storage is cut-through, rewrite the creation migration instead of adding migrations.

Done when no federation read is proportional to retained history and every lifecycle has one state column and one guard.

### 7. Custody

- One custody trait (advance mint, advance melt, observe) with three implementations: the `MintPayment` bridge, Bark and BDK. Delete the stub adapters.
- Move the Bark incoming, outgoing and custody drivers into `cdk-bark` behind the host port already used for recovery; move the on-chain spend driver and broadcast beside the BDK wallet.
- Move all Bark request and result codecs and the stateful validator into `cdk-federation-bark`; add per-kind phase enums with one transition table.
- Split the long drivers into one step function per phase.
- One signer fan-out shared by the FROST and Bark coordinators; one rotation function.
- One DKG planner covering BLS and FROST, with one delivery loop.
- In `cdk-frost`, one generic session store and state machine in place of three copies; fold the two adapter crates in as modules.
- Per-quote locking, released before I/O.

Done when no consensus encoding has two definitions and `cdk-axum` contains no custody control flow.

### 8. Wallet and operator surface

- One quorum routine parameterised by a per-response consensus view; one share verifier; no interior per-quote state.
- A federation registry in `cdk` (discover, validate, identity derived from the federation id, member URL aliases), used by CLI, FFI and TUI (decision 6 for persistence).
- Remove the federation mode flag from `HttpClient`; split `cdk-iroh` into client and server features.
- Parse federation config once at startup; derive network and bindings from it; remove the restated bindings and the legacy environment layer.
- Reduce the feature matrix and check the supported combinations in CI.

### 9. Tests

- Break up the fixture files (`cdk-axum` federation `tests/mod.rs` is 6,041 lines; the wallet federation tests are 6,447 lines) into shared builders.
- Keep scenario coverage distinct per trust boundary, as the maintainer walkthroughs require; record retained coverage before deleting a test.

## Relationship to earlier plans

- September 7 package 1 chose feature-selected key implementations to preserve the ordinary secp API when federation is off. Package 3 here replaces that with additive types, because a feature that changes a public type's shape breaks under Cargo feature unification.
- Both earlier plans limited changes in shared files to federation integration sections. Package 4 deliberately changes shared mint code, because the duplication cannot be removed without a seam in it.
- September 7 package 4 added focused Bark action reads. Package 6 here finishes that by removing the snapshot path.
- The September 5 cut-through policy is assumed throughout.

## Decisions needed

1. Replace feature-selected key types with additive ones (package 3)? **Decided 2026-11-23:** yes, following the upstream `bls` branch: one unconditional `PublicKey`/`SecretKey` enum and BLS v3 always compiled, instead of the curve-tagged field type proposed above, so the protocol types do not fork from upstream.
2. Allow the validate/plan/commit split in shared mint code, possibly landed on `main` first (package 4)? **Decided 2026-11-25:** no. Shared standalone mint code stays untouched. The federated commit paths, the duplicate share-table write and the two quote-and-settings adapters stay; the custody trait is not built. Instead, a parity test runs the same request vectors through the standalone and federated swap, mint and melt paths and asserts identical signatures, fees, spent proofs, quote states and errors.
3. Bark as a typed, feature-gated operation variant, or a real trait object injected into the builder? Either way the global registry goes. **Decided 2026-11-23:** a typed, feature-gated variant. **Revised during package 5:** `cdk-common` cannot name the Bark crate's types, so Bark uses the existing generic `Application` variant instead; the global registry is gone either way.
4. Crate layout: only extract `cdk-federation` from `cdk-axum`, or also move the pure domain out of `cdk-common` into a core crate? **Decided 2026-11-23:** only extract `cdk-federation`.
5. Canonical encoding: keep sorted JSON behind one module, or move consensus hashing to one binary encoding? **Decided 2026-11-25:** keep canonical JSON in `canonical.rs`, pinned by golden vectors; revisit only if hashing shows in a profile. `serde_json`'s `preserve_order` also stays enabled workspace-wide.
6. Persist the wallet's federation registry through `WalletDatabase`? This is new persisted information. **Decided 2026-11-25:** persist it, in a federation namespace of the wallet KV store every backend already implements; no `WalletDatabase` trait change and no new migrations. Rediscovery refreshes it.
7. `scripts/fed-cdk-tui`: promote to a crate on the shared registry, or move out of the repository? Same question for the archived documents. **Decided 2026-11-25:** promote the TUI to a workspace crate on the shared registry, built and linted in CI. Under `docs/`, keep only condensed current design documents for shipped behaviour; dated plans, ledgers and reviews move to an archive outside the PR.
8. Keep FROST nonces in redb apart from SQL? If yes, document the reason and a "never restore from backup" rule. **Decided 2026-11-25:** yes. Keep the separate redb file and document, in the operator docs and beside the code, that it is never restored from backup: an operator deletes it and regenerates nonces instead.
9. Added 2026-11-25: what does a member do when it still holds federation state but is below every peer's retention floor? **Decided 2026-11-25:** it halts with a clear status. A `cdk-mintd` reset subcommand, run by the operator, wipes federation and mint rows (keeping key material and the FROST nonce file) and rejoins by state transfer. Nothing is deleted without that explicit action.

## Verification and completion

- Every package keeps the existing federation, standalone and FFI checks green, and adds the parity or boundedness test that demonstrates its claim.
- Boundedness claims are tested by growing history and showing stable examined rows.
- Lifecycle changes test every source and target pair for agreement between the domain table and the storage guard.
- Local regtest payment and restart verification is repeated after packages 4, 6 and 7, as in the earlier ledgers.
- The plan is complete when the central diagnosis no longer holds: one implementation per mint rule, one owner per state, one node crate, and no public type that changes with a feature.

## Worth keeping

- The `MintConnector` seam on the wallet side.
- Prepare, persist, commit on the consensus runtime; application effect and status in one mint transaction.
- No I/O in the `cdk-common` federation modules; the pure BLS DKG planner.
- `cdk-frost` wrapping a FROST library, with an explicit nonce lifecycle and authorization that re-derives the signed message.
- Iroh as a decorator on the existing `Transport`; one `BdkWalletEngine` for standalone and federated.
- `FederationTaskGroup`, the capability types (`VerifiedFederationSuffix`, `VerifiedFederationCheckpointQuorum`, `FederationAcceptedSigningRequest`), and `deny_unknown_fields` with explicit size bounds.

## Implementation ledger

### 2026-11-23: package 4, first step (shared mint rules)

Started with package 4 because finding 1 is the most important; packages 0 to 3 are not yet done.

- **Verification, fees and balance.** `crates/cdk/src/mint/verification.rs` now runs every rule against a `KeysetView`: the mint's signatory keysets, or a federation config through `FederationKeysets`, which also applies the federation's per-output BLS checks. Deleted the federated copies of output, change-output, input, fee and balance verification and the federated change-fee helper.
- **Journal loads.** `require_prior_applied_*` read the whole journal on every mint, melt and payment application. They now read the one entry that reserved the quote's conflict key, an indexed lookup.
- **Signing.** One `sign_accepted_outputs` replaces three copies of the share-signing block, and one `federation_share_signatures` replaces four share-to-signature mappings.
- **Mint quote rules.** `uniform_method_and_unit` and `MintInput::verify_quote_signatures` are shared by standalone issuance and federated application; the two federation wrappers are gone. Standalone now checks NUT-20 signatures after all per-quote state checks, so a request with several faults may report a different first error.
- Net: 9 files, 558 insertions, 716 deletions.

Verification: `cdk` mint tests with federation (412 passed) and without it (258 passed); `cdk-axum` compiles with `federation-bdk,federation-bark`; clippy clean on the changed files. Four unused-import warnings in `crates/cdk/src/wallet/issue/saga/mod.rs` test code predate this work. Not run: the workspace-wide suite, integration tests and regtest.

Not done in this step: the swap and mint commit paths (add proofs, signatures, completed operation) are still written twice; sharing them needs the `validate → plan → commit` split. The duplicate share and blind-signature table writes remain.

### 2026-11-23: package 4, second step (swap plan)

- **Swap plan.** `plan_swap` in `crates/cdk/src/mint/swap/mod.rs` verifies outputs and balance, computes the fee once and builds the operation record. The standalone saga and federated application both call it; the saga previously computed the fee twice.
- **Mint write path left separate, deliberately.** Standalone issuance and federated application write the same rows, but federated application needs deterministic operation ids and idempotent replay rules, and the standalone path does neither. Merging them would change behaviour in both for a few shared calls.
- **Restore reverted.** Switching member-mode restore to the batched blind-signature read broke `restore_ignores_orphan_blind_signature_without_federation_share`, a deliberate provenance rule. Reverted; a batched share lookup would need a new storage method (package 6).
- **Unrelated build fix.** `crates/cdk/src/wallet/npubcash.rs` did not compile with `npubcash` and without `federation`, because the type of `SecretKey::as_secret_bytes()` depends on the feature (finding 3). Fixed by comparing slices. This blocked the integration tests.

Verification: `cdk` mint tests with federation (412 passed) and without (258 passed); `test_swap_flow` integration test on the memory database (17 passed); `cdk` builds with `npubcash` with and without federation; `cdk-axum` compiles with `federation-bdk,federation-bark`. Not run: workspace-wide suite, other integration tests, regtest.

### 2026-11-23: package 1, first step (dead code)

- **Deleted, no callers:** in `cdk-common`, the consensus runtime's `catch_up` and `catch_up_checked`, the journal's `append_catch_up_entry` and `into_rows`, `into_quorum`, `conflicting_count`, `signer_set_count` and `has_same_committed_state`, plus two constants and two imports only they used.
- **`cdk-axum` dead-code allowances:** removed all 14 blanket `allow(dead_code)` markers in the federation runtime. Eight items the compiler then flagged are test entry points, now `#[cfg(test)]` so they leave production builds. FROST signing items that only custody backends use carry an allowance naming that feature condition. All four CI feature combinations build without warnings.
- **Wallet connector:** removed the member-map parameter that every caller filled with the full member set, and folded the two `*_from` wrappers into their implementations.
- **Always-true gates:** removed 60 from `crates/cashu/src/nuts/nut01/bls.rs` and 8 from `crates/cdk/src/wallet/performance.rs`, both inside modules already gated on `federation`, plus one constant that could never compile.
- Net: 13 files, 71 insertions, 301 deletions.

Verification: `cashu` with federation (665 passed), `cdk-common` with `federation,test` (491), `cdk` with `mint,wallet,federation` (1,036), `cdk-axum` with `federation-bdk,federation-bark` (598). Not run: `cdk-mintd`, FFI, integration tests, regtest.

Still to do in package 1, each needing test retargeting or a decision rather than plain deletion:

- the legacy journal appends (`append`, `append_with_consensus_evidence`, `from_entries`), now used only by tests;
- the test-only recovery path `reconstruct_retained_items` and its `*_for_test` constructors;
- `ModuleFact`, which only tests construct;
- `FederationOperationStatus::Applying`, which appears in the SQL `CHECK` constraint;
- the unauthenticated runtime constructors;
- `cfg(test)` fields in runtime structs;
- the legacy non-checkpoint catch-up in `cdk-axum`.

### 2026-11-23: packages 0 and 1, test code out of production builds

- **`cdk-common/test` leaked into production.** `crates/cdk-sql-common/Cargo.toml` enabled it in `[dependencies]` (on `main` it did not), so everything gated `cfg(any(test, feature = "test"))` in `cdk-common` compiled into every mint build. It now comes only through `cdk-sql-common`'s own `test` feature, which the SQLite and Postgres test builds already enable.
- **Smoke tool** (`crates/cdk-mintd/src/federation_smoke.rs`) now compiles only with `federation-test-utils` and `sqlite`, instead of with the default `fakewallet` and `sqlite`. The daemon's `federation` feature no longer enables `cdk/wallet` and `cdk-sqlite/wallet`; `federation-test-utils` does. Every recipe that runs the smoke command already builds with it.

Verification: `cdk-mintd` builds with `sqlite,cln,federation`, `sqlite,federation-bdk,federation-bark`, `sqlite,federation-test-utils` and `sqlite,cln,fakewallet,federation`; its tests pass with `sqlite,federation-test-utils` (291) and the production-CLI gate tests pass without the test utilities; `cdk-sql-common`, `cdk-sqlite` and `cdk-postgres` test builds compile with `federation-bark`. Unused-function warnings in `cdk-mintd` test builds without the fake wallet predate this work.

### 2026-11-23: package 1, test-only paths out of production

- **Gated as test-only** (`cfg(any(test, feature = "test"))`): the legacy journal appends (`append`, `append_with_consensus_evidence`, `append_finalized_with_consensus_evidence`, `from_entries`) and their private helpers, and the consensus runtime constructors `new`, `for_members` and `with_limits`, which leave operation authorization unset. All 16 production builds the checks cover compile without them, which confirms production never used them. Since the previous step, `feature = "test"` no longer reaches production builds.
- **Explicit test dependencies:** `cdk-federation-bark`'s tests and the fuzz crate now request `cdk-common/test` themselves instead of receiving it through feature unification.
- **Already test-only, left in place:** the legacy non-checkpoint catch-up in `cdk-axum` and `reconstruct_retained_items` were already `cfg(test)`. Moving them into test modules is cosmetic.
- **Deferred:** `ModuleFact` is constructed only by tests, but about 25 test sites use it as a variable-size stand-in consensus item; deleting it means rewriting those tests. `FederationOperationStatus::Applying` is never written but appears in the SQL `CHECK` constraint; it goes with the storage lifecycle work in package 6.

Verification: the 16 production builds in the check script; unit tests of `cdk-common` (491), `cdk-federation-bark` (36), `cdk-sqlite` with `federation-bark` (194), `cdk-bark` (19); `cdk` and `cdk-axum` federation test binaries build.

### 2026-11-23: package 2, first step (canonical encoding)

- **One canonical module.** `crates/cdk-common/src/federation/canonical.rs` holds `canonical_json_bytes` and `append_hash_bytes`. It replaces six copies of the first (five in `cdk-common`, one in `cdk-federation-bark`) and five byte-identical copies of the second, plus the hand-written recursive key sorter in `consensus.rs`. The output is unchanged: the operation-ID and setup-transcript golden vectors pass.
- **Left as is:** `transaction_proposal.rs` frames lengths as `u64` rather than `u32`. Unifying the framing changes those hashes and needs regenerated vectors.
- **`serde_json`:** dropped the unused `raw_value` feature. `preserve_order` stays until package 6, because the Bark projection's JSON-string compare-and-set may depend on key order.
- Net: 12 files, 19 insertions, 107 deletions, plus the new 55-line module with two tests.

Verification: `cdk-common` with `federation,test` (493 unit tests and both vector suites), `cdk-federation-bark` (36), and the 16 production builds.

### 2026-11-23: package 2, second step (byte newtypes)

- **One macro for byte newtypes.** `impl_fixed_bytes!` in `crates/cdk-common/src/federation/canonical.rs` generates `from_bytes`, `from_slice`, `from_hex`, `to_bytes`, `to_hex`, `Display`, `FromStr` and hex serde. It replaces the hand-written copies on 13 digest and ID types and 3 signature types, which had identical bodies apart from name, length and size-error variant. Each type keeps its own size-error variant, so errors and wire format are unchanged. The signature types gain `from_bytes`.
- **Not converted:** four types with different behaviour (the recovery-barrier ID, the FROST application ID, the setup correlation nonce, and the secret-bearing invitation capability), and the two older macros in `transaction_proposal.rs` and `setup/mod.rs`, whose size errors are free-form strings. They go with the per-module error enums.
- Net: 8 files, 157 insertions, 1,154 deletions.

Verification: `cdk-common` with `federation,test`, including the golden-vector suites; `cdk-federation-bark` (36); `cdk` federation (1,036); `cdk-axum` with `federation-bdk,federation-bark` (598); `cdk-mintd` with `federation-test-utils` (291); `cdk-sqlite` with `federation-bark` (194); the 16 production builds.

### 2026-11-23: package 2, third step (transient storage retry)

- **One retry helper.** `retry_transient_storage` in `crates/cdk-axum/src/federation/state.rs` owns the doubling delay, the retry budget and the recovery, retry and exhaustion logs. It returns either the value or a typed `Permanent` / `Exhausted` failure, which each caller maps to its own error. It replaces three copies of the loop: journal replay apply (`node_runtime.rs`), finalized-unit apply (`aleph_bft_session.rs`) and backup persistence (`aleph_bft_runtime.rs`). Error messages are unchanged; the logs carry the operation or session in the message rather than as a separate field.
- **`main`'s `BackoffPolicy` does not fit.** It only configures supervised-stream reconnects and computes no delays, so it could not replace these loops as the plan assumed. The other backoff code (payment observer, scheduler, chain sync, setup) has different policies and stays.
- **Pre-existing test build failure fixed.** `crates/cdk-axum/src/federation/tests/iroh_transport.rs` did not compile with `iroh` at the original branch tip `3183d04b8`, because `Transport::http_post` now returns a raw response. The test now decodes it with `json_or_status_error`.

Verification: `cdk-axum` with `federation-bdk,federation-bark,iroh` (602 passed, including the Iroh tests that did not compile before) and the 16 production builds.

### 2026-11-23: package 3 (key types no longer depend on a feature)

- **One key implementation.** `cashu` no longer swaps `PublicKey` and `SecretKey` by feature. The curve-aware enums the upstream `bls` branch (`eb34616e8`) introduced are the only definitions, and `cashu` has no `federation` feature: BLS12-381, v3 keysets and threshold share verification and aggregation are always compiled. 63 plain gates and 8 special cases were removed. `TokenV4` omits an empty DLEQ field in every build, as upstream `bls` does; `main` still writes `"d": null`, and decoders accept both.
- **Standalone BLS v3 now works.**
  - Without `federation`, the wallet returned `UnknownKeySet` for v3 signatures; it now verifies them in every build.
  - Without `federation`, the signatory checked v3 proofs with the secp256k1 verifier; it now uses the BLS check. `cdk-signatory` lost its `federation` feature.
  - The standalone BLS issue tests and the v3 signatory rotation tests now run in ordinary builds.
- **Wallet performance log:** replaced the five-variant stage enum with a fixed label string.
- **Standalone API guard:** `scripts/check_standalone_api.py` and its fixture asserted the retired feature-dependent API. They now assert that ordinary builds expose the same key types as federation builds, including v3, and that the federation custody crates (`cdk-frost`, `cdk-federation-bark`) stay out of an ordinary dependency tree. `bls12_381` is no longer forbidden there.
- **API change against `main`:** `PublicKey::to_bytes` returns `Vec<u8>`, the key types no longer `Deref` to secp256k1 types, and `KeySetVersion` has a `Version02` variant. This is the upstream `bls` branch's API.
- Net: 25 files, 79 insertions, 276 deletions (the two duplicate key modules removed).

Verification:
- unit tests: `cashu` (665), `cdk` standalone (790) and with federation (1,035, one fewer because the stage-label test was removed), `cdk-common` with `federation,test` (515 including integration targets), `cdk-sqlite` (154) and with `federation-bark` (194), `cdk-mintd` with `federation-test-utils` (291), `cdk-axum` with `federation-bdk,federation-bark,iroh` (602);
- `test_swap_flow` on the memory database (17);
- builds: the workspace with default features except `cdk-ldk-node`, which was not built; the 16 production builds;
- the standalone API guard.

Not checked: the WASM target is not installed with this toolchain.
- **Follow-up, one v3 proof check in the wallet:** `verify_bls_proofs` in `crates/cdk/src/wallet/blind_signature.rs` batch-verifies v3 proofs, logs the stage and maps the error. It replaces three copies in `Wallet::verify_token_signatures`, the receive saga and the issue saga (4 files, 45 insertions, 67 deletions). `cdk` tests pass standalone (790) and with federation (1,035).

### 2026-11-23: package 4, third step (one keyset owner for members)

- **Problem:** in member mode the mint's synchronous keyset accessors (`pubkeys`, `keysets`, `keyset`, `keyset_pubkeys`, `get_active_keysets`, `get_keyset_info`) read the static configured keysets and ignored applied keyset rotations. Separate async `effective_federation_*` accessors read the rotation-aware config, and the HTTP handlers branched between the two. Any synchronous caller after a rotation saw stale keysets. The rotation-aware config was also cloned in full, secret shares included, on every use.
- **Change:**
  - The effective config is one atomically swapped `Arc` snapshot. It is built when a member mint is constructed and rebuilt after a rotation commits, rather than cleared and lazily rebuilt.
  - The synchronous accessors read it through `with_federation_keysets`.
  - The three async keyset accessors and the handler branches are deleted.
  - `effective_federation_config` returns the shared `Arc` instead of a clone.
- **Not done:** the signatory's own keyset store, which its subscription refreshes, still exists beside the federation snapshot. A member's keysets now have one owner, but the two modes still keep keysets in two fields.
- **Test change:** the rebuild-count test now asserts that construction builds the config and an unchanged config is never rebuilt. The rotation tests now check the ordinary accessors, before and after a restart.

Verification: `cdk` with federation (1,035) and standalone (790); `cdk-axum` with `federation-bdk,federation-bark,iroh` (602); `cdk-mintd` with `federation-test-utils` (291); the 16 production builds.

### 2026-11-23: package 6, first step (remove the unreachable `Applying` status)

- `FederationOperationStatus::Applying` and `FederationApplicationState::Applying` were mapped, matched and allowed by the SQL `CHECK` constraint, but no writer ever set them. Removed from both enums, every match arm, the SQL mappings, and the `CHECK` constraint in both creation migrations. The federation migrations are not checksum-locked, consistent with the cut-through policy; only the standalone history is.
- Tests that enumerated legal states drop it. The persistence test now picks its consensus-rejected entry by state rather than by position.
- Net: 13 files, 11 insertions, 62 deletions.

Verification: `cdk-common` with `federation,test` (515), `cdk-sqlite` with `federation-bark` (194), `cdk` with federation (1,035), `cdk-mintd` with `federation-test-utils` (291), `cdk-axum` with `federation-bdk,federation-bark,iroh` (604 of 605 in the full run). The one failure, `aleph_bft_live_session_latency_probe_records_seven_member_bft_operation`, passed twice when rerun alone. It drives a seven-member live session against timing limits and looks load-sensitive, not related to this change.

### 2026-11-23: package 8, first step (one quorum routine in the wallet)

- **Two helpers.** `group_by_view` groups member responses by the part that must agree, failing on an invalid response where the caller does. `first_quorum` picks the first group that reaches the threshold. Together they replace seven hand-written copies of grouping and selection in `crates/cdk/src/wallet/federation.rs`: the signature-share sets, mint quote status, batch mint quote status, check-state, restore, melt quote status (which keeps its state-priority selection) and the mint-quote early-stop check.
- **Bug fixed.** The mint-quote early-stop check compared raw JSON, while the aggregator compared a view that ignores `updated_at`. Members whose quotes differed only in update time never triggered early stop, so the wallet always waited for every member. Both now use the same view; regression test `mint_quote_status_quorum_ignores_member_update_times`.
- Net: 2 files, 98 insertions, 159 deletions.

Verification: `cdk` with `mint,wallet,federation` (1,036, including the wallet's adversarial federation tests), clippy clean, the 16 production builds.

### 2026-11-23: package 6, second step (lifecycle transitions without JSON compare-and-set)

- **Six transitions changed.** They guarded concurrent updates with `WHERE payload_json = :expected` (serialized-string equality), which depends on byte-identical serialization and checks no transition rule:
  - DKG ceremony insert and transition: lock and read the row, treat a row equal to the update as an idempotent retry, reject anything else not equal to the expected record (compared as decoded records), and require the new `FederationDkgCeremonyState::can_transition_to` before writing;
  - setup session transition, invitation transition and invitation replacement: the same locked read and record comparison, after the existing domain `validate_transition_to` checks;
  - Bark allocation and VTXO updates: the readers now lock (`FOR UPDATE`) the rows they return inside the projection transaction, so the update no longer needs the JSON guard.
- **Left as is:** about 25 idempotent inserts of immutable records still accept "an identical row already exists" by comparing JSON. That is replay detection for values that never change, not a lifecycle guard. The Bark action head update already guards on a typed column (the expected latest operation).
- **Not yet removed:** `serde_json`'s `preserve_order`. These transitions no longer depend on it, but the idempotent inserts above compare serialized JSON.

Verification: `cdk-sqlite` with `federation-bark` (194, the storage conformance suite including ceremony, setup and projection transitions), `cdk-common` with `federation,test` (515), `cdk` with federation (1,036), `cdk-mintd` with `federation-test-utils` (291), `cdk-axum` with `federation-bdk,federation-bark,iroh` (601 of 602; the same timing-bound latency probe as above), and a `cdk-postgres` test build. The Postgres suite itself needs a running server and was not run.

**Latency probe follow-up.** At the original branch tip `3183d04b8`, `aleph_bft_live_session_latency_probe_records_seven_member_bft_operation` took 7.6 s in isolation; after these changes it takes 5.3 s. Its 10 s wall-clock bound is tight when about 600 tests share the machine, so its failures in full-suite runs predate this work and are not a regression.

### 2026-11-23: package 8, second step (Iroh client without the server)

- `cdk-iroh` gained a `server` feature that gates `IrohServer`, `IrohConnectionInfo`, `axum` and `tower`. `cdk-axum` and `cdk-mintd` enable it with their `iroh` features; the wallet (`cdk`), FFI and CLI use only the client.
- A wallet-only build with Iroh (`cdk` with `wallet,iroh`) no longer compiles `axum`. `tower` remains only through `iroh`'s own `reqwest` dependency.
- The process-global default Iroh node the review flagged was not changed.

Verification: `cdk-iroh` tests with `server` (21); builds of `cdk-iroh` with and without `server`, `cdk` with `iroh`, `cdk-ffi` with `federation`, `cdk-axum` with `federation,iroh`, `cdk-mintd` with `sqlite,iroh,federation`, and `cdk-cli` with `federation,iroh`.

### 2026-11-23: package 5 (federation node crate)

- **New crate `cdk-federation`** holds the federation node: the AlephBFT session, network and runtime, peer sync and catch-up, checkpoints, the peer client, DKG execution, FROST signing, and the payment, Bark and on-chain workers. 34 modules moved from `crates/cdk-axum/src/federation/`, keeping their history. The runtime helpers formerly in `federation.rs` (checkpoint verification metrics, local peer status) form the crate root.
- **Node type:** `FederationNode { mint, federation }` replaces `MintState` throughout the runtime. Its 51 methods formerly on `MintState` are now methods on the node.
- **`cdk-axum` keeps the HTTP surface:**
  - the router, private-request admission and error responses;
  - the route modules, plus three new ones holding the handlers split out of the FROST signing, Bark custody and Bark secret files;
  - the invalid-operation alert middleware;
  - the setup coordinator service, an HTTP service in its own right.
  - `MintState` holds a `FederationNode` and dereferences to it, so handlers needed few changes.
- **Dependencies:** `cdk-axum`'s only federation dependency is `cdk-federation`. AlephBFT, FROST and the custody crates are now dev-dependencies, used only by its tests. `cdk-federation` re-exports the four names the HTTP layer needs (the DKG RNG, the two member-local config types, the Bark degraded reason and error).
- **Visibility:** modules and items `cdk-axum` uses are `pub` and `#[doc(hidden)]`, which marks them as this pair's working surface rather than a stable API. Test hooks moved from `cfg(test)` to a `test-utils` feature that `cdk-axum` enables for its tests. The runtime gained `FederationNodeRuntime::new`, so test-only fields and their initializers stay in one crate.
- **Public API unchanged:** `cdk-axum` re-exports the same items for `cdk-mintd`, which needed no changes. `Cargo.lock` and `Cargo.lock.msrv` gain only the new workspace crate; no external version changed.
- **Decision 3 resolved differently.** A typed Bark variant in `cdk-common` would require `cdk-common` to name `cdk-federation-bark`'s types, but that crate depends on `cdk-common`. Doing it means moving about 10k lines of Bark protocol code into `cdk-common`, against decision 4. Package 7 therefore uses the existing generic `Application` variant, which also removes the global registry.
- **Not done:** federation runtime construction (`prepare_federation_router` and the router configuration) is still in `cdk-axum`'s `lib.rs`, and most of the setup ceremony is still in `cdk-mintd`.

Verification:
- unit tests: `cdk-federation` with `federation-bdk,federation-bark,iroh,prometheus` (131) and `cdk-axum` with `federation-bdk,federation-bark,iroh` (474), together the 602 tests this suite had before the split plus the three prometheus label tests now counted in `cdk-federation`; `cdk-axum` standalone (41); `cdk-mintd` with `federation-test-utils` (291);
- clippy with `-D warnings` on both crates, federated and standalone;
- the 16 production builds.

### 2026-11-23: package 7, first and second steps (Bark as a generic application operation; shared send encodings)

- **One way to carry an application operation.** Bark operations now travel as the generic `FederationOperation::Application` variant, the same path any registered application uses. The parallel `ExternalApplication` variants of the operation, its conflict key and its materialized key are gone, along with the process-global codec registry, the external payload types and the hand-written mirror enums that existed only to give those variants serde support. All three enums now derive their serde implementations.
- **Bark conflict keys** become application conflict keys in the `bark` namespace; Bark rows become application materialized keys owned by the Bark application ID, with the same collection names as before (`bark_action`, `bark_derivation_allocation`, and so on).
- **No start-up registration.** The eight `register_federation_bark_wire_codec()` calls in `cdk-mintd` and the test helpers are removed: decoding no longer depends on whether a process remembered to register a codec first.
- **Wire change under cut-through.** A Bark operation's wire tag changes from `bark` to `application`, and its conflict keys change shape accordingly. Federation formats are undeployed, so no migration is provided. The two Bark wire tests now pin the new form.
- **Test build fix:** `cdk`'s wallet Iroh tests start an `IrohServer`, which needs `cdk-iroh`'s `server` feature since package 8's second step. `cdk` now enables it as a non-wasm dev-dependency. The same tests also gained the `json_or_status_error` fix already applied to `cdk-axum`.
- **One definition of each Lightning-send result encoding.** The executing member (`cdk-bark`) and every member's projection (`cdk`) each had their own copy of the payment-succeeded result hash, the payment-failed result and its check, and the revocation request bytes with their locator layout. These now live once in `cdk-federation-bark`'s new `encoding` module; both sides call it and map its error to their own. The projection's success check gains the proof-hash comparison the runtime already made.
- Net change: about 1,400 lines removed.
- **Found, not fixed: debug-build stack depth.** At the previous commit, one `cdk-axum` melt test (`duplicate_federation_melt_after_cache_eviction_returns_persisted_response`) already needs slightly more than the 2 MiB default test-thread stack in debug builds (it passes at 2.5 MiB). Earlier plans on this branch ran federation tests with `RUST_MIN_STACK=16777216`, which hides this. Large async futures on the request path are the likely cause; package 9 should find and box them so the suite runs with the default stack.

Verification, with `RUST_MIN_STACK=16777216` as on the rest of this branch:
- unit tests: `cdk-common` federation (493), `cdk-federation-bark` (36), `cdk-bark` (19), `cdk-sqlite` Bark (194), `cdk` federation and Bark (1,053), `cdk` wallet Iroh (2), `cdk-federation` with all features (131), `cdk-axum` with `federation-bdk,federation-bark,iroh` (474), `cdk-mintd` with `federation-test-utils,federation-bark` (298);
- clippy with `-D warnings` on `cdk-federation-bark`, `cdk-bark` and `cdk`;
- the 16 production builds.


### 2026-11-23: package 7, third step (Bark state validator in the Bark crate)

- **Validator moved.** The 13 pure checks of Bark transitions against the projection snapshot (maintenance, refresh, exit, revocation, receive-claim and send headers, locator allocation, custody signers) moved from a module inside `cdk`'s projection to `cdk_federation_bark::validation`. They depend only on Bark and `cdk-common` types, so other crates can use them too. `cdk` keeps the stateful projection that reads storage and calls them.
- **Error classification unchanged.** The checks now return `FederationError::ApplicationOperationInvalid` with the same message. Like the previous `Error::Custom`, the apply path treats it as neither a definitive rejection nor a retryable storage error.
- **One effective-phase rule.** `signing_retry_phase.unwrap_or(phase)` was written out in ten places across `cdk`, `cdk-bark`, `cdk-federation-bark` and `cdk-federation`, plus a helper in the projection. It is now `FederationBarkActionRecord::effective_phase()`. The SQL crate's raw-column copy stays until package 6 removes the transition logic from storage.
- Net change: about 560 lines removed.
- **Not done in package 7:**
  - The two quote-and-settings adapters (`cdk-bark`'s `FederatedBarkPaymentAdapter`, `cdk-mintd`'s `FederatedBdkPaymentAdapter`) are not only stubs. They supply payment settings and melt fee quotes to the mint, and only their execution methods are consensus-only. Removing them needs a quote-only seam in the shared mint payment registry, which is standalone code and part of decision 2.
  - The Bark drivers (about 4,200 lines in `cdk-federation`) have not moved into `cdk-bark` behind a host port.

Verification, with `RUST_MIN_STACK=16777216`: the same suites and counts as the previous step, clippy with `-D warnings` on the four touched crates, and the 16 production builds.

### 2026-11-23: package 6, third step (Bark lifecycle out of the SQL crate)

- **The Bark action lifecycle lives in the Bark crate.** The SQL projection held the transition table, the plan-kind-to-phase map, the signing-retry rule and the refresh-to-exit escalation rule as private functions. They are now domain methods:
  - `FederationBarkSemanticPhase::may_advance_to` and `allows_signing_retry`;
  - `FederationBarkActionKind::planned_phase`;
  - `FederationBarkActionRecord::may_transition_to` and `is_refresh_exit_escalation`.
  The SQL crate calls them and keeps only storage.
- **Duplicate removed.** `FederationBarkOperation::phase()` already mapped a plan's kind to its phase, and the SQL crate checked a plan's phase against its own copy of that map, a check that could not fail. Both now use `planned_phase`, and the redundant check is gone.
- **Tests moved with the rules:** the lifecycle and outgoing-result table tests and the escalation test now live in `cdk-federation-bark`, plus a new test of signing-retry transitions.
- **Remaining in package 6:** the row-per-entity storage, retention, the trait surface and the single apply transaction. The Bark snapshot's typed keyed queries also remain.

Verification, with `RUST_MIN_STACK=16777216`: the same suites as package 7's third step, with `cdk-federation-bark` now at 40 tests. Also clippy with `-D warnings` on `cdk-federation-bark` and `cdk-sql-common`, and the 16 production builds.

### 2026-11-23: package 9, first step (tests run on the default stack)

- **Cause.** In debug builds an async function's poll frame reserves stack for every child future it awaits, and nothing between the HTTP handler and the SQL layer was boxed, so each level's future contained the whole apply chain below it. A federated melt nests submission, local finalization, journal apply, the mint's operation dispatch and the melt saga. About a dozen frames used 80 KB to 210 KB each, 2.1 MB in total. This affects a debug `cdk-mintd` as well as tests, since Tokio worker threads also default to 2 MiB.
- **Production boundaries now return boxed futures from plain functions**, so callers hold a pointer instead of the callee's state machine:
  - `Mint::apply_accepted_federation_operation`, plus one dispatch function that boxes each operation kind's apply future;
  - `FederationNodeRuntime::submit_envelope`, `apply_finalized_unit` and `apply_journal_entries`;
  - `FederationNode::submit_federation_envelope_to_consensus`, matching the existing `submit_federation_envelope`.
  `reconcile_federation_bark_runtime` is boxed at its four call sites.
- **Test fixtures.** The five fixture builders whose futures made most test futures large are boxed at their call sites. `RUSTC_BOOTSTRAP=1 cargo rustc … -- -Zprint-type-sizes` lists each future's largest awaited child, which is how these were found.
- **Result.** Every federation suite passes on the default 2 MiB test stack; `cdk-axum` also passes on 1.5 MiB, about 25% headroom. `RUST_MIN_STACK` is no longer needed.
- **Not done in package 9:** splitting the fixture files into shared builders.

Verification, without `RUST_MIN_STACK`: the same nine suites and counts as the previous step, the 16 production builds, and clippy with `-D warnings` on `cdk`, `cdk-federation` and `cdk-axum`.

### 2026-11-23: package 2, fourth step (one byte-length error)

- **One variant instead of seventeen.** Each fixed-length federation value had its own `Invalid…Size { expected, found }` variant of `FederationError`. They are now one `InvalidByteLength { kind, expected, found }`, and `impl_fixed_bytes!` takes the value's name instead of a variant. Display text is unchanged.
- **Dead variant removed:** `OperationFederationMismatch` had no constructor.
- `FederationError` goes from 162 variants to 145. **Not done in package 2:** splitting the rest into per-module enums with typed reasons, and with it the operator-message redactor.

Verification: `cdk-common` federation tests (493), compile checks of all targets in the dependent crates, and the 16 production builds.

### 2026-11-23: package 6, fourth step (typed Bark store with keyed reads)

- **No JSON boundary.** `cdk-common`'s database trait no longer has `get_federation_application_projection_snapshot` and `get_federation_application_action`, which returned `serde_json::Value` because `cdk-common` cannot name Bark types. In their place, `cdk-federation-bark` defines a `FederationBarkStore` trait with typed reads, and `cdk-sql-common` implements it for SQLite and Postgres. The mint receives it through `MintBuilder::with_federation_bark_store`. `cdk-mintd` now returns one `MintDatabases` struct (localstore, keystore, KV, configuration store, Bark store) instead of a four-handle tuple.
- **Keyed reads on the apply path.** Bark operation validation and payment-record derivation read the whole projection on every Bark operation. They now use indexed queries:
  - one action;
  - one action's transitions or observations;
  - VTXOs by id, by state, or by reserving action;
  - the highest allocated locator index.
  The pure validators in `cdk-federation-bark` take the records they check (input VTXOs, the action's observations) instead of the snapshot. The Bark payment adapter's balance check reads spendable VTXOs only.
- **Still whole-snapshot:** the Bark workers and recovery driver in `cdk-bark` and `cdk-federation`, operator status, and checkpoint materialization. These run outside the apply path and need every live row; bounding them belongs with retention.
- **Ordering.** Per-action transitions and observations come back in consensus order (`operation_index`). The snapshot sorted them by operation id; where validation picks the first match, it now picks the earliest accepted one.
- **Test.** The shared storage test, which runs on SQLite and Postgres, now checks every keyed read against the same filter over the full snapshot after 256 actions.
- **Remaining in package 6:** row-per-entity storage for the `state_v1` blobs, retention, the rest of the trait surface, and the single apply transaction.

Verification: the nine federation suites, with the same counts as the previous step. Clippy with `-D warnings` on each touched crate alone, and the 16 production builds. One `cdk-axum` latency test (`aleph_bft_live_session_latency_probe_records_seven_member_bft_operation`) asserts a 10-second bound. It failed once while other builds were running and passed when rerun alone.

### 2026-11-24: package 7, fourth step (Bark drivers in the Bark crate)

- **Drivers moved.** The incoming, outgoing, evidence, observer, receive-secret replication, custody-attempt and maintenance code (about 3,700 lines) moved from `cdk-federation` into a `node` module of `cdk-bark`. So did the projection readiness and replay code from `cdk-federation`'s bootstrap. `cdk-bark` now owns Bark phase selection, replay, evidence and the construction of every Bark operation, and depends on `cdk`'s mint for accepted state.
- **One node port.** The drivers reach the node only through `FederatedBarkNode`:
  - the mint;
  - the Bark runtime, receive-secret store and incoming lock;
  - envelope submission, ordered group submission, admission and readiness checks;
  - wallet-publisher ordering;
  - an authenticated peer client for receive-secret replication and evidence delivery.

  `cdk-federation` implements it in `bark_node.rs`.
- **Recovery host unified.** The recovery driver's `FederatedBarkRecoveryHost` is now implemented once, in `cdk-bark`, for any `FederatedBarkNode`. The node no longer has its own Bark-specific recovery host.
- **Shared types moved down.** The latency timers and their label domain moved to `cdk_common::federation::latency`, and `FederationPaymentAdvancement` to `cdk_common::federation`. Both are shared by the node and the backend. The tracing target is unchanged, so existing log filters still match.
- **Stays in the node:**
  - `bark_custody.rs`, the FROST signing fan-out behind the existing `FederatedBarkCustodyClient` port. It shares the signing coordinator with BDK, and the shared signer fan-out is a separate package 7 item.
  - The payment scheduler that decides when to call the drivers.
- **No behaviour change intended.** One difference is visible: a failed operator reconciliation's log line now carries the `cdk::Error` wrapper text around the same federation error.
- Net change: about 220 lines added, mostly the port trait, its implementation and the shared replay source.

Verification: the nine federation suites pass. `cdk-bark` now has 36 tests and `cdk-federation` 114; together that is the same 150 as before. The other suites are unchanged. Clippy with `-D warnings` is clean on `cdk-common`, `cdk-bark`, `cdk-federation` and `cdk-axum` alone, and the 16 production builds pass.

### 2026-11-24: package 2, fifth step (checkpoint errors in their own enum)

- **One enum per module, starting with checkpoints.** `FederationCheckpointError` in `crates/cdk-common/src/federation/checkpoint.rs` takes the 11 checkpoint variants of `FederationError` (signature, version, quorum, audit, startup suffix and replay, catch-up size and the generic invariant failure). `FederationError::Checkpoint` wraps it transparently, so callers that hold a `FederationError` or `cdk::Error` still see the same text.
- **Fixed reasons.** `Invalid` and `StartupSuffixMismatch` take `&'static str` instead of `String`. The six sites that formatted values into a reason now use named variants with typed fields: `CursorRegressed`, `RangeOutsideRetainedItems`, `RetainedOperationMismatch`, `RetainedOperationMissing` and `ScheduledSequenceMismatch`. The recovery-barrier ID length check uses the shared `InvalidByteLength`. Message text is unchanged apart from that length message.
- **Construction stays short where the type allows it.** `From` conversions into `FederationError` and `cdk::Error` let sites that return either type name only the inner variant. Functions that return `anyhow::Result` (the REST peer client, sync loop and startup verifier) still wrap the value in `FederationError::Checkpoint`: callers such as the DKG driver classify peer failures by downcasting to `FederationError`, and an unwrapped value would bypass that.
- `FederationError` goes from 145 variants to 135. Net change: about 190 lines added, mostly the documented enum and the longer wrapped form.
- **The redactor stays; the plan was wrong about why it exists.** Most of its callers scrub text that does not come from `FederationError`: database, `anyhow` and other `cdk::Error` messages in HTTP handlers, node halt reasons, backend broadcast details and DKG driver status. Fixed reasons make federation messages safe without it, but those boundaries still carry foreign text. The remaining work is to split the other reason-carrying families the same way (DKG, AlephBFT recovery, application operations, address allocation, chain observation) and then narrow the redactor to the foreign-error boundaries.

### 2026-11-24: package 2, sixth step (DKG errors in their own enum)

- **`FederationDkgError`** in `crates/cdk-common/src/federation/dkg.rs` takes the ten DKG variants of `FederationError`: limits, commitment, commitment round, transcript, transcript signatures and activations. `FederationError::Dkg` wraps it transparently.
- **Fixed reasons.** The 65 sites that formatted values into a reason now use typed variants. Generic cases carry a fixed reason plus the one public value they name (`TranscriptKeyset`, `TranscriptCeremony`, `TranscriptMember`, `CommitmentForAmount`, `CommitmentForReceiver`). Specific cases get their own fields: ceremony transitions, version and count mismatches, coefficient counts, receiver mismatches, conflicting activations and transcript-hash mismatches.
- **FROST errors are kept as a source.** FROST validation failures were formatted into a string; they are now `Frost(#[source] cdk_frost::Error)`.
- **Text changes.** Messages that named a keyset, ceremony or member in mid-sentence now end with `: keyset <id>`, `: ceremony <id>` or `: member <id>`. Two messages drop a secondary value: the transcript hash on a conflicting transcript signature, and the receiver on a contribution that does not match the public commitments. Nothing parses these messages.
- **Tests.** Tests that matched a substring of a formatted reason now match the typed variant.
- `FederationError` goes from 135 variants to 126. Net change: about 240 lines added. The enum is larger than the variants it replaces, because each formatted message became a documented variant.

### 2026-11-24: package 2, seventh step (AlephBFT errors in their own enum)

- **`FederationAlephBftError`** in `crates/cdk-common/src/federation/consensus.rs` takes the 14 AlephBFT variants of `FederationError`: units, sessions, ordered groups, network admission and finalized-item recovery. `FederationError::AlephBft` wraps it transparently.
- **Fixed reasons.** The 25 sites that formatted values into a reason now use typed variants:
  - ordered groups that are too large;
  - the inbound byte budget;
  - non-contiguous cursors and session regressions;
  - journal, operation, item-index and count mismatches;
  - unit replay mismatches.

  `aleph_bft_catch_up_recovery_mismatch` and `finalized_unit_recovery_mismatch` now take `&'static str`.
- **Two values no longer travel in the error.** The SCALE decoder's debug text was copied into `InvalidAlephBftNetworkMessage`; it is still logged where decoding fails, and the error carries a fixed reason. Unit body hashes are typed hashes instead of hex strings.
- **One predicate replaces a variant.** "Any recovery mismatch" used to be one variant and now spans nine. `FederationAlephBftError::is_recovery_mismatch` keeps the HTTP status mapping (500) and the tests that asked that question in one place.
- `FederationError` goes from 126 variants to 113. Net change: about 265 lines added.

### 2026-11-24: package 2, eighth step (fixed reasons everywhere)

- **No free-form reasons left.** The twelve remaining `reason: String` fields of `FederationError` become `&'static str`, and every helper that built one takes `&'static str`: application operation, address allocation, chain observation, transaction proposal, operation state, FROST configuration and signing, wallet liveness, backup manifest, setup and invite. About 60 sites formatted values or foreign errors into a reason. They now use:
  - shared shapes: `VersionMismatch { kind, expected, found }`, `TooLarge { kind, actual, max }` and the existing `InvalidByteLength`;
  - variants that name the value: `InvalidSetupField`, `InvalidSetupMember`, `ApplicationInvalid { context, reason }`, `ApplicationFieldInvalid` and `ApplicationLabelInvalid`;
  - source-carrying variants for FROST library errors: `FrostConfiguration`, `FrostDerivation` and `FrostSigning`;
  - fixed reasons naming the step that failed, for parser, secp256k1 and BIP32 errors whose text added little.
- **Bark messages unchanged.** The Bark crate's "Bark:" and "invalid federated Bark state:" prefixes are kept by `ApplicationInvalid`'s context.
- **Projection errors.** `OperationStateInvalid` marks a deterministic rejection, so it gets no sibling variant. The two Bark projection sites that named a column use a fixed reason and log the column instead.
- **Redactor narrowed.** Every `FederationError` message is now fixed text plus public protocol values. The exceptions are `BackendFailure` and `NodeHalted`, which carry foreign text and are redacted when they are built. `federation_error_response` therefore no longer redacts. The redactor stays where foreign text is logged or replicated: `anyhow`, database and other `cdk::Error` messages, halt reasons, broadcast details and DKG driver status. The remaining `String` fields hold public IDs and hashes in text form.
- `FederationError` has 123 variants: 145 at the start of package 2, minus 35 moved into the checkpoint, DKG and AlephBFT enums, plus their three wrappers and the ten general shapes added in this step.

Package 2 is done.

### 2026-11-24: package 7, fifth step (one signer fan-out)

- **One fan-out.** `crates/cdk-federation/src/signer_fan_out.rs` runs one request per selected FROST signer. This member's request goes to the local signer runtime; every other member's goes over authenticated REST. The FROST signing coordinator (commitment, signature-share, aggregate-result and abort rounds) and the Bark custody coordinator (virtual MuSig2 commitment and partial, threshold DH and abort rounds) now share it. They also share `LocalFrostSigner`, `signer_members` and `concurrency_limit`, which replace a duplicated local-signer struct, `member_for_signer` and `parallel_limit`.
- **One behaviour, the stricter of the two.** FROST waited for every signer but had no concurrency bound. Bark was bounded but stopped at the first error, dropping requests still in flight before it sent the abort. The shared fan-out is bounded by the private REST outbound limit, waits for every request before returning (so an abort never races an in-flight request), keeps signer order, and returns the first error in that order. FROST keeps its one transport retry inside its remote calls.
- Net change: about the same size. The 300 lines of repeated dispatch become one 110-line module.
- **Still to do in package 7:**
  - The custody trait with three implementations and deleting the stub adapters both need decision 2's quote-only payment seam.
  - The on-chain spend driver (`wallet_liveness.rs`, `broadcast.rs`) depends on the node's FROST coordinator and peer client. Moving it beside the BDK wallet means giving `cdk-bitcoin-wallet` or `cdk-bdk` a node port, as `cdk-bark` now has.
  - The DKG planner and the `cdk-frost` session-store unification are not started.

### 2026-11-24: package 8, finding (the HTTP client's federation flag stays)

- The plan proposed removing the federation mode flag from `HttpClient`. It is `preserve_error_responses`, set only by the federation wallet connector. With it, the client returns a mint's exact `ErrorResponse` for 4xx replies, so the threshold aggregator can compare members' answers. Ordinary wallets map the same replies to typed `Error` variants. Removing the flag means giving the aggregator a second, lower-level connector API that returns raw responses, which trades one boolean for a parallel trait. It stays.

### 2026-11-24: package 6, fifth step (payment scheduler per quote)

- **One stored entry per quote.** The payment observation scheduler kept every quote's retry schedule in one KV blob (`payment_quote_backoff_v2`) and rewrote it on each transition. Each quote now has its own entry in `payment_quote_schedule_v5`, and the two scan cursors have their own keys. The key is `mint_`/`melt_` plus a SHA-256 digest of the schedule key, because base64 quote IDs contain characters the KV store rejects. The jitter seed is unchanged, so retry timing is the same.
- **Per-quote locks instead of one global lock.** `claim`, `finish` and `remove` lock only their own quote's key (`KeyedLocks`, whose entries are dropped when unused), so attempts on different quotes no longer serialise behind one mutex and one blob write.
- **Reads.** `candidates` and `active_lease_until_ms` read only the quotes they are given. `next_attempt_at_ms` lists the stored schedules, which holds only quotes still being observed, because a completed quote's entry is deleted.
- **Leases stay durable.** The plan suggested keeping leases in memory. The persisted lease still coalesces concurrent wakes (`concurrent_wakes_share_one_persisted_attempt_lease`), so it stays in the per-quote entry.
- **Cache semantics kept.** An entry that no longer decodes is dropped and rebuilt, as the old version check did; the test now covers a corrupt entry. The old blob is no longer read; federation storage is cut-through.

### 2026-11-24: package 7, sixth step (one session state machine in `cdk-frost`)

- **One state machine instead of six copies.** FROST signing nonces, virtual MuSig2 nonces and threshold-DH requests share one participant lifecycle: activate an attempt for an authorization slot, reserve one record, advance it by exact compare-and-swap, list nonterminal records, and compact terminal detail into a permanent replay tombstone. Each kind had its own in-memory and redb implementation. `crates/cdk-frost/src/session.rs` (`SessionRecord`, `MemorySessionTable`) and `session_redb.rs` (`RedbSessionTable`) now implement the six operations once. `MemoryNonceStore` and `RedbNonceStore` keep the public `DurableNonceStore` and `DurableProtocolStore` traits and delegate each method.
- **Only real differences remain as hooks:**
  - whether a kind is keyed by session;
  - key encoding, context and terminal state;
  - tombstone and burn-for-replacement;
  - per-kind validation of new records and transitions;
  - per-kind error variants;
  - the redb table layout and the two locks.

  Accidental differences (validation order, slot-release order, claim-check style, load and compaction order, and wording that differed by backend) became one rule, choosing the stricter behaviour each time.
- **Bugs fixed in the in-memory store.** A failed activation could release a slot without burning its record or advancing the attempt, and a failed compaction could drop terminal-order entries after compacting some records. Both now leave the table unchanged, like redb's rolled-back transaction. Regression tests: `failed_memory_activation_leaves_table_unchanged`, `failed_memory_compaction_leaves_table_unchanged`. The in-memory store is for tests and embedding prototypes; production uses redb, which was already all-or-nothing.
- **Nonce activation now checks the protocol** like the other kinds. `cdk-federation`'s only path to it already rejected other protocols first.
- **Unchanged:** public API, redb table names and on-disk encodings. In-memory error messages now use the redb wording. Error variants are unchanged.
- **Inconsistencies recorded, not changed.** The same failure maps to `InvalidCommitment` for one kind and `InvalidSigningSession` for another: key collision, compacted session, and load of a compacted key. A wrong-protocol nonce record fails activation and reservation with different variants. Callers may match on these variants, so aligning them is a separate change.
- Net change: about 70 lines fewer in the stores (1,402 added, 1,509 removed for the unification, then 231 added and 193 removed for the follow-up, about 110 of which are the regression tests).

### 2026-11-24: package 6, sixth step (consensus state as rows, loaded whole)

Decision: rows loaded whole, not lazy keyed queries. The validation logic keeps working on the in-memory projections; only storage changes.

- **One row per entity.** The four `state_v1` blobs are now one key-value row per entity in their existing namespaces:
  - `transaction`: one row each per payment intent, accepted transaction, batch lineage head and UTXO reservation;
  - `chain_observation`: one row per wallet UTXO;
  - `address_allocation`: one row per allocation;
  - `bark_recovery`: one row per reserved recovery input.
- **Derived indexes are not stored.** The quote-to-intent index and the per-scope next-index counters are rebuilt from the rows on load. Validation already required them to match exactly.
- **One table abstraction.** `FederationStateTable` (`crates/cdk-common/src/federation/state_table.rs`) gives each projection `to_rows` and `from_rows`.
  - `FederationStoredState` loads a projection inside the apply transaction and writes back only the rows that changed, removing rows that no longer exist.
  - `read_federation_state` reads a projection outside a transaction.
  - Every row key must be the key its content would be stored under. Unknown row kinds are rejected.
- **One query per load.** The KV traits gained `kv_entries`, which reads a whole namespace. SQL backends answer it with one `SELECT`, so a read outside a transaction is still one consistent read, as the single blob was. Other backends fall back to listing keys and reading each one.
- **Blob encoding deleted.** The four state types no longer have `from_bytes`, `to_bytes`, a `version` field or serde derives. Rows are their only stored form. Tests that seeded or tampered with the blob now write or edit rows.
- **One test tightened.** `unavailable_or_non_spendable_utxos_and_non_pending_intents_fail_closed` built reorged and quarantined UTXOs by editing JSON and decoding without validation, so the policy rejected them only because it re-validates the state. It now asserts that such a state cannot be loaded at all, and checks the policy only for reserved and spent UTXOs, which can be stored validly.
- **What changed for cost:** each apply writes only the changed rows. Each apply still loads the whole projection, so reads still grow with retained history. Truncation is the retention question, which is still open.
- **Remaining in package 6:** retention, the rest of the trait surface, the single apply transaction, and bounded snapshot reads.

### 2026-11-24: package 6, retention (finding and design, nothing deleted)

Decision: checkpoints may truncate logs, history and terminal entities. The investigation found that no deletion is safe yet, so this step is a design: `docs/federation-storage-retention-2026-11-24.md`.

- **Why not yet.** There is no state transfer, so peers must keep journal bodies for any member behind them. Envelopes have no expiry, so journal rows and conflict keys are the replay guard. Terminal entities are replay guards and are hashed into every signed checkpoint, so pruning by local timing would split the members' digests.
- **Design.** A retention floor fixed by the scheduled checkpoint sequence; terminal entities below the floor excluded from the digest by definition and deleted in the background; spent-UTXO tombstones; Bark readers off the full journal; state transfer verified against the signed materialized-state digest.
- **Storage contract.** The design document has the per-store table (purpose, authority, readers, retention target) that this package asked for.
- **Open decisions:** the replay guard (permanent compact index or envelope validity window) and whether to build state transfer.

### 2026-11-24: package 6, retention prerequisite 1 (operation validity window)

Decision: the replay guard is a validity window, not a permanent index.

- **Envelope stamp.** `FederationOperationEnvelope` carries the AlephBFT `session` its author stamped it in. The stamp is outside the operation id, so client retries keep their id, and inside the authorization signature (transcript `v2`), so only the author can re-stamp. Envelope version 7.
- **Stamped at submission.** The submission funnel stamps every envelope this member authored with the active session and signs it again before it enters the mempool, so the ~40 creation sites are unchanged.
- **Enforced at finalization.** An envelope is `RejectedInvalidOperation` unless the unit's session is within `[session, session + 16]`. Startup recovery checks stored statuses with the same rule.
- **Tests.** `session_stamp_is_signed_but_outside_the_operation_id` and `finalization_rejects_envelopes_outside_their_validity_window` (expired, future, and oldest-valid stamps in one unit).
- **Nothing is deleted yet.** The design document has the per-kind table of what guards each operation once ids outside the window are forgotten; each row needs a replay test before journal rows are deleted.

### 2026-11-24: package 6, retention prerequisite 3 (Bark readers off the full journal) and the state-transfer design

- **One reconcile function with a durable marker.** Startup, readiness, the operator reconcile route and the post-apply hook each read the whole journal (or, after apply, a slice) into the Bark replay. They now call `reconcile_federation_bark_runtime(mint, runtime)`, which reads from a per-scope marker of the first unreplayed index, replays, and moves the marker past every finished entry. A failed replay leaves the marker in place. The post-apply hook therefore retries an earlier failed entry instead of skipping past it.
- **Not covered by a dedicated test.** The marker runs under the existing Bark suites (`cdk-axum` with `federation-bark`, `cdk-federation`), which pass; there is no test that restarts a member and checks that only the suffix is read.
- **Spent-UTXO records (prerequisite 2) move to the pruning step.** They only have a job once spent UTXOs are deleted; adding them now would be unused code.
- **State transfer designed, not built.** The signed digest leaves out fields a transferred member would install (quote public keys, payment identifiers, blind signatures), and a live database cannot export the state at an earlier checkpoint. The design in `docs/federation-storage-retention-2026-11-24.md`: full-fidelity digest rows, snapshots frozen at scheduled checkpoint capture, a paged transfer verified against the quorum-signed digest, and pairing checks for blind signatures.

### 2026-11-24: package 6, state transfer step 1 (digest covers what a transfer installs)

- **Quotes.** Mint quote rows add the payment lookup id and NUT-20 public key; melt quote rows add expiry, options, creation time, extra fields and fee options. Each is fixed by the accepted operation and checked equal on insert. Fields that can differ on the originating or paying member (mint creation time and extra fields, the melt payment lookup id) stay out.
- **Projections.** The four stored projections hash their stored rows (now canonical JSON) through a default `FederationStateTable::materialized_rows`, replacing four hand-written field lists (about 190 lines removed). The digest and the transfer use one encoding.
- **Proofs.** Proof rows add the full stored proof, which every member stores from the same operation inputs, so an installer can write the spent-proof rows that block double spends.
- **Blind signatures stay out by design**: each member stores its own share and will recompute it on install.
- **Verification.** Multi-member value-flow tests compare every member's digest after real flows and pass. One full `cdk-axum` run had six timing-sensitive failures under machine load; each passed alone, and a second full run passed (474/474).
- **Remaining for state transfer:** check completed operations, keysets and FROST application configs; snapshot storage at scheduled capture; the paged route, client and installer; an end-to-end transfer test.

### 2026-11-24: package 6, state transfer steps 2–4 (snapshot, paged transfer, installer)

- **Remaining digest tables checked.** Completed operations gain the melt payment amount and fee; keysets and FROST configs already carried everything. Blind-signature rows gain the quote id and the member share's operation, output position and accepted index; proof rows gain the quote id and stored operation; melt rows gain the request as JSON, estimated blocks and selected fee index. All are fixed by the accepted operation, and the multi-member digest comparisons still agree. The design document's "Digest coverage" table records each table.
- **One source, one row builder.** A single storage read (`get_federation_materialized_state_source`, on a connection or inside a transaction) returns every row the digest covers, and one builder (`cdk/src/mint/federation/state_transfer.rs`) turns it into sorted snapshot rows. The live digest, the frozen snapshot and the installer's in-transaction check share it. The Bark-only remainder of `projection.rs` is now compiled only with `federation-bark`.
- **Snapshot storage.** Scheduled capture stores its rows in the candidate's transaction (`federation_state_snapshots`, `federation_state_snapshot_rows`, in the creation migrations) and keeps the latest two scheduled snapshots.
- **Paged transfer.** `POST /federation/v1/state-transfer` (REST kind `StateTransfer`, unsigned response, 500 rows by default, 8 MiB pages) and `FederationRestPeerClient::{fetch_state_transfer_page, download_state_snapshot_from_member, transfer_state_from_member}`, capped at 1 GiB per transfer.
- **Installer.** `Mint::install_federation_state_transfer` checks the rows against the signed digest, writes them through the apply path's own storage calls in one transaction (payments rebuilt from observations, the member's shares recomputed from its key share), recomputes the digest in that transaction, commits only on equality, and installs the checkpoint as trusted frontier and state-transfer base. Checkpoint digest lookups and the prior-quote check understand the base.
- **Tests.** `cdk-axum` `state_transfer::wiped_member_installs_checkpoint_state_then_catches_up_and_restores` (paged download over the real route, tampered row and attachment refused with nothing written, install, restart, suffix catch-up including a mint of a quote created below the checkpoint, equal digests, NUT-09 restore and check-state on both sides, recomputed shares aggregate correctly) and `scheduled_capture_keeps_the_latest_two_snapshots`.
- **Validity window (follow-up the same day).** The transfer also carries every finalized item and journal entry ordered in the `FEDERATION_OPERATION_VALIDITY_SESSIONS` sessions before the checkpoint, plus one older item that proves the window complete (`POST /federation/v1/state-transfer/window`). The installer checks them before writing: items chain to the signed finalized-item digest, entries are exactly the accepted items and chain to the signed order and application digests. It stores them as history with their conflict keys, so finalization's duplicate and conflict checks decide as on peers. The test re-orders a window operation and a conflicting quote on all three members and compares journals and digests; a tampered and a truncated window are refused with nothing written. Without the window rows the installed member forks; the test was run that way and fails.
- **Left out (design document, "Left out, and what must follow").** Duplicates and conflicts older than the window still differ while peers keep full history; the fix belongs with deletion (window-scoped lookups on every member). Also refused or not restored: in-flight melts, unsigned melt blinded messages, per-keyset fee totals, Bark journal lookups below the base. Nothing triggers a transfer in production yet.

### 2026-11-24: package 7, DKG planner (one delivery path for BLS and FROST)

- **FROST planning without network I/O.** `run_frost_dkg_tick` mixed deciding the next step with sending. `plan_frost_dkg` now decides the phase and the messages to send from stored records and the delivery tracker, and advances durable local state only when a round is complete: every member acknowledged this member's messages and every other member's were received.
- **One delivery path.** Five BLS broadcast and per-receiver blocks and three FROST blocks become two generic helpers, `deliver_broadcast` and `deliver_unicast`, both driven by `FederationDkgDeliveryTracker`, which now tracks the FROST rounds too.
- **Behaviour change:** FROST messages are no longer re-sent to members that already acknowledged them. A three-member ceremony takes 70 deliveries instead of 97; the tests pin the new counts. A FROST round now advances on acknowledgements gathered across ticks instead of requiring every send in one tick to succeed.
- **Not moved to `cdk-common`.** The FROST planner stays in `cdk-federation` because it calls the `cdk-frost` DKG functions, and decision 4 is about removing that dependency from `cdk-common`.

### 2026-11-24: package 1, test hooks out of production structs (part 1)

- **AlephBFT session.** Seven `cfg(test)` fields on `FederationAlephBftLiveSessionOptions` (task panics, pre-spawn failure, apply failure, rotation-start failure, publication pause, activation observer) become one `faults: Arc<dyn AlephBftSessionFaults>`. Production uses `NoAlephBftSessionFaults`; tests pass a `TestAlephBftSessionFaults` through `with_faults`. The apply loop no longer carries a production branch on a constant `false`.
- **FROST signing.** The runtime's test-only map that bypassed authorization in two validators becomes a `SigningAuthorizer` trait object: `MintSigningAuthorizer` runs the mint's checks; the test constructor installs fixed authorizations that fall through to those checks for any other session. The two validators are now free functions.
- **Setup coordinator.** The `cfg(test)` delay and storage-failure fields become a `SetupOperationFaults` trait object; the test fixture owns the test implementation.
- **Remaining:** the two finality test flags in `node_runtime.rs` (changed on the state-transfer branch, so done after its merge) and the feature-conditional `allow(dead_code)` markers on the FROST signing path.

### 2026-11-24: package 1, test hooks out of production structs (part 2) and dead-code markers

- **Finality mode.** The node runtime's two test booleans (`local_finality_test`, `manual_finality_test`) become one `finality: FederationFinalityMode`. Production compiles only `Live`; `Local` and `Manual` exist in test builds. The impossible both-true state is gone, and the wallet publisher check no longer needs a `cfg` block.
- **FROST signing dead code.** Ten `cfg_attr(not(custody), allow(dead_code))` markers on the signing coordinator, its peer-client methods and the signer fan-out module become `cfg(any(feature = "federation-bdk", feature = "federation-bark"))`, with their imports gated to match. The aggregate signature's package hash, read only by on-chain signing, compiles only with `federation-bdk`. Clippy passes with both custody features, with each alone, and in all 16 production builds.
- **Package 1 done criterion met:** no federation module carries `allow(dead_code)`, and no production struct has a `cfg(test)` field.

### 2026-11-24: package 1, `ModuleFact` out of production builds

- **Test-only variant.** `FederationConsensusItem::ModuleFact` and `FederationConsensusModuleFact` now compile only with `cfg(any(test, feature = "test"))`. Production builds can neither encode nor accept the variant. About 45 test sites use it as a variable-size item without an operation id, so they keep it rather than being rewritten.
- **No downstream matches on it.** Because Cargo unifies features, a downstream crate can be compiled against a `cdk-common` that has the variant while its own test feature is off. The two production matches in `cdk-federation` (`checkpoint_manager.rs`) now use a new `FederationConsensusItem::as_recovery_barrier()` instead of matching every variant. Inside `cdk-common`, the arms for the variant carry the same `cfg`.
- **Verification.** All 16 production builds, clippy and the suites pass. One full run had `byzantine_balance_overflow_operation_does_not_halt_finalized_application` fail a convergence assertion under load; it passed alone and in every earlier full run.

### 2026-11-24: package 9, fixture files split into modules

- **`cdk-axum` federation tests.** `tests/mod.rs` (6,060 lines) keeps only imports and module declarations (about 165 lines). The helpers move into seven fixture modules: transports, route state, signed requests and DKG, envelopes and proofs, AlephBFT waits and synthetic history, live sessions, and Prometheus telemetry (built only with `prometheus`). The 25 tests it held move into five test modules: recovery barriers, sync config, startup quorum, error statuses and service startup.
- **Wallet federation tests.** `wallet/federation/tests.rs` (6,466 lines) becomes `fixtures.rs` (1,327 lines) plus nine test modules by behaviour: connector config, keyset discovery, signature aggregation, mint quotes, threshold responses, quote fallback, quote status, restore and check-state, and admission.
- **Moved unchanged.** A brace-aware scanner moved whole items. The only edits are `pub(super)` visibility on fixtures, `super::` paths one level deeper in moved wallet tests, and two helpers placed with the fixtures that other modules use. Test counts are unchanged (`cdk` 1,053, `cdk-axum` 476).
- **Found while splitting.** The FROST-signing test module did not build for `cdk-axum` with `federation` alone after the earlier feature gating of the signing coordinator; it is now gated the same way, and the verification script lints that configuration too. The byzantine-overflow test compared journals at one instant and failed under machine load; it now waits for the journals to converge.
- **Retained coverage** is the same tests in new files; no scenario was deleted.

### 2026-11-24: package 6, audit of "one apply transaction per operation"

- **Met for most operations.** Mint quotes, mints, swaps, melt quotes, melts, keyset rotations, mint-quote payment observations, address allocations, chain observations, transaction states and Bark operations without melt evidence each apply in one transaction that also marks the journal entry applied (`finish_federation_apply_transaction`).
- **Not met for melt payment evidence.** `apply_federation_melt_quote_payment_record` adds the observation in one transaction, may run the melt saga's finalization (`resume_paid_federation_melt_finalization`, its own transactions), and marks the entry applied in a final transaction. A Bark operation carrying melt payment evidence first commits its application rows and observations in a separate transaction, then takes the same path. A melt-quote state observation also commits through its own transaction.
- **What fixing it needs.** Melt finalization has to run inside the apply transaction, which means one melt driver instead of the saga plus the federated branch (package 1, "melt has one persisted saga but two drivers"). Not started.

### 2026-11-25: package 6, melt payment evidence in one transaction

- **One federated finalization.** `finalize_federation_melt_in_transaction` (`cdk/src/mint/melt/shared.rs`) does all federated finalization work inside a transaction the caller supplies. It locks the quote, loads the melt request, checks amounts (`finalize_melt_core_in_transaction`, shared with the standalone `finalize_melt_core`), sets the quote Paid, spends the inputs, stores the locally computed change shares and signatures (`add_federation_melt_change`), records the completed operation and deletes the melt request and saga. The already-Paid, missing-request and missing-input branches are kept. It returns a `FederationMeltFinalization` (spent Ys, paid quote, payment proof, change), which is published only after commit. `finalize_federation_melt_quote` is now begin, in-transaction finalize, commit, publish. Its callers are startup recovery, the saga's paid finalize, saga recovery (which keeps its `Finalizing` handoff) and `resume_paid_federation_melt_finalization`. The federated branch of `finalize_melt_quote_inner` is gone. The standalone two-transaction path is unchanged, because its change signing may call a remote signatory.
- **In-transaction failure and state paths.** `rollback_melt_quote_in_transaction`, `persist_permanent_payment_failure_in_transaction`, `recover_recorded_payment_failure_in_transaction`, `process_federation_melt_saga_outcome_in_transaction`, `finalize_paid_federation_melt_outcome_in_transaction` and `update_melt_quote_payment_observation_state_in_transaction` write only through the caller's transaction. They return `MeltNotifications` instead of publishing. The standalone rollback and failure functions now wrap the same code in their own transaction, with unchanged behaviour.
- **Apply.** `apply_federation_melt_quote_payment_record` opens one transaction and takes the quote lock. Inside it, it reads the quote, validates on-chain evidence (transaction state and saga read in the transaction), adds the observation, computes `can_finalize` and makes the same branch decisions as before. It then finalizes, rolls back or updates state, marks the entry applied (`finish_federation_apply_transaction`) and commits, and publishes only after that. A Bark operation with melt payment evidence writes its application rows, its observations and the melt outcome in that same transaction.
- **Storage.** The transaction traits gain `get_melt_saga_by_quote_id`, `get_blinded_secrets_by_operation_id` and `get_federation_journal_entry_by_operation_id`. These reads previously went through the database handle, which an open transaction blocks on the single-connection in-memory SQLite.
- **Tests.** `paid_federation_melt_evidence_applies_in_one_transaction` injects a failure (`FEDERATION_MELT_PAYMENT_APPLY_BEFORE_COMMIT`) after the observation and the finalization are written. It then checks that none of their effects remain: the quote and inputs stay Pending, the saga and melt request are unchanged, there is no observation row, change signature, share or completed operation, the entry stays Accepted and no quote event is published. Replay then commits everything: the quote is Paid, the inputs Spent, the change signature and share stored, the completed operation recorded, the saga and melt request gone, the observation stored, both entries Applied, and the Paid event carries the change. The former crash-between-transactions test (`FEDERATION_MELT_FINALIZE_BEFORE_CLEANUP_COMMIT`) is now `paid_federation_melt_failure_before_commit_leaves_melt_pending_and_replays`. It fails inside the single finalization transaction, shows the same absence of effects, replays after restart and stays terminal after a second restart. `failed_federation_melt_rolls_back_atomically_and_replays` now expects the observation and the `PaymentFailed` record to roll back with the failed apply. A storage test covers the new saga read inside a transaction.
- **Behaviour differences, all from atomicity.** On an apply, an error while checking for internal settlement is returned and the entry is retried. Previously the error was logged and the entry still marked applied. A failure to delete the saga on the already-completed branches is returned instead of logged. A melt saga without a federation operation id found by the federated apply is an error instead of a standalone finalization.
- **Left out.** The Bark melt-evidence branch has no unit test. Reaching it needs a full Bark action plan, a send-succeeded transition and a member observation quorum, and no fixture builds those in `cdk`. It shares the in-transaction helper that the tests above cover. `finalize_federation_internal_melt_quote` already commits in one write transaction and was not changed, apart from loading the config before it prepares change.

### 2026-11-25: package 6, retention stage 1 (window-bounded duplicate and conflict detection)

- **One rule on every member.** An entry guards duplicates and conflicts for a unit finalized in session `S` only if it was ordered in session `S - FEDERATION_OPERATION_VALIDITY_SESSIONS` or later (`federation_operation_guards_session`). The finalization handler keeps each accepted id with its ordering session and drops ids that no longer guard when a session begins; the in-memory journal takes an id or conflict key as held only while its latest holder guards. The finalization path and startup suffix verification register only stored entries that guard. A member holding only the window (state transfer, or deleted history) and a member holding everything now decide alike.
- **Storage.** `federation_journal_entries.operation_id` is no longer unique; lookups and application-state updates use the latest entry. `federation_journal_conflict_keys` keeps every holder by `(conflict_key_json, operation_index)`. New `ordered_session` and `ordered_item_index` columns carry the evidence; the state-transfer window start uses them instead of joining on operation id, which a repeated id would make ambiguous. New lookups `get_federation_journal_entry_by_{operation_id,conflict_key}_before` serve startup verification.
- **Replays change nothing.** Payment observations (`ON CONFLICT DO NOTHING`), identical signature shares and materialized-state keys re-recorded by the same operation keep their first position.
- **Per-kind replay tests** (`cdk`, `mint::federation::tests::replay_window`, 13 tests): every operation kind is applied, then re-stamped in the first session after its window by a member allowed to stamp it and applied again at a later index. Each ends `Applied` with no materialized-state change, or (`Mint`, a superseded broadcast attempt) in a definitive rejection; none ends `Failed`. The design document's table lists the guard per kind. Three guards were missing and are added: a replayed reorg is matched before the block-anchor check, a replayed broadcast intent or observation returns `Existing` once the lineage moved on (and stale or conflicting observations are now definitive rejections), and a Bark operation already recorded in the projection is marked applied before phase validation.
- **Consensus test.** `duplicate_and_conflict_guards_cover_only_the_validity_window_on_every_member` (`cdk-common`): inside the window a replay is a duplicate and a conflicting quote is consensus-rejected; one session later a runtime with full history and a checkpoint runtime with none append the same entries with the same digests.
- **Moved.** `is_definitive_federation_apply_rejection` now lives in `cdk::mint` so the apply tests use the production rule.
- **Not covered.** Bark `ReceivePrepared`, `ActionPlan`, `ActionTransition` and `EvidenceCertificate` replays take the same Bark no-op path but no `cdk` fixture builds them.

### 2026-11-25: package 6, retention stage 2 (retention floor and deletion of consensus history)

- **Floor.** `prune_federation_history` advances a `FederationRetentionFloor`: the scheduled checkpoint before the latest one the trusted checkpoint covers, capped at the latest one's validity-window start, lowered to the earliest journal entry live state reads (melt sagas, unfinished Bark actions, unfinished outgoing transactions via `Mint::federation_transaction_retention_pin`), never moving back. It records the order, finalized-item and application digests before it, and the `federation_*_digest_at` lookups fall back to them. A state-transfer install records its window start as its floor.
- **Deletion.** After every promotion, `FederationNodeRuntime::prune_history_below_retention_floor` deletes journal entries with their conflict keys, finalized items, and scheduled checkpoints older than the floor checkpoint (not the trusted or audited one), up to 64 batches of 500 rows, each in its own transaction.
- **Readers.** Catch-up and the state-transfer window route answer a request below the floor with `BelowRetentionFloor` (HTTP 410), which the peer client reads as `PeerHistoryPruned`. `require_prior_applied_quote_operation` accepts a stored quote created below the floor. Startup starts at the trusted checkpoint, above the floor. Bark replay reads from its marker.
- **Trigger.** A sync round in which every peer answers `PeerHistoryPruned` installs a peer's latest scheduled checkpoint state (`install_state_transfer_below_peer_floors`), restarts the consensus runtime at it, and catches up the suffix in later rounds. A member with no stored next checkpoint probes the peer toward the latest known quorum so it sees the answer. Only an empty member can install; a member holding state below every floor is logged as needing a reset.
- **Tests** (`cdk-axum`, `federation::tests::retention`): `pruning_keeps_the_floor_checkpoint_and_the_validity_window`, `unfinished_melt_saga_pins_the_retention_floor`, `member_below_every_peer_floor_installs_a_transfer_then_catches_up`.
- **Not done.** Payment observations of terminal quotes are digest rows, so they move to stage 3 with the digest exclusion rule. No automatic reset of a non-empty member below every floor. The transaction pin has no test. Apply-time reads of journal entries of already final entities are not pinned (design document, "Not deleted yet").

### 2026-11-25: decision 8, FROST nonces stay in redb

- **Documented, not moved.** The nonce stores stay in their own redb files under the FROST protocol storage root (`frost-nonces/root-<epoch>.redb`). The rule and its reason (a restored store offers consumed nonces again, and a nonce used for two messages reveals the key share) are in `cdk-frost`'s README, the `nonce_redb` module docs, `FederationFrostSigningRuntime::open`, the `frost_protocol_storage_root` config field and the operator workflow in `docs/federation.md`. A lost or suspect file is deleted, never restored.
- **Also fixed:** `docs/federation.md` still named envelope version 6; it is 7.

### 2026-11-25: package 4, parity test

Decision 2 keeps two implementations of the swap, mint and melt rules, so this step adds the parity test that guards them. Test code only; no production code changed.

- **Test.** `crates/cdk/src/mint/federation/tests/parity.rs` (`cdk`, `mint::federation::tests::parity`, 15 tests). Each vector runs on a standalone mint and on a federation member with the same amounts (powers of two to 128), the same input fee (100 ppk) and the same scripted payment backend. The standalone side uses `process_swap_request`, `process_mint_request`, `get_mint_quote`, `get_melt_quote` and `melt`. The federated side uses `prepare_mint_quote`, `prepare_melt_quote`, the payment probes and `apply_federation_journal_entries` on raw envelopes, ordered directly so the application rule decides rather than admission or consensus conflict keys.
- **Compared per step.** Signatures by output position (signed or not, amount, keyset by activation order, and whether the unblinded proof verifies; bytes and keyset ids differ because the keys differ, and the federation uses threshold 1 so a member's share is the aggregate signature). Every completed operation's amounts, fee, payment amount and payment fee. Input proof states. Mint quote state, amount, paid and issued; melt quote state, amount, fee reserve and payment proof. Error variant, message and whether a member would record it as a definitive rejection.
- **Vectors.** Valid swap; swap without the fee; insufficient inputs; overpaid fee; double spend; duplicate outputs; duplicate inputs; outputs on an unknown keyset; a keyset rotation (outputs on the inactive keyset, inputs from it); mint on an unpaid quote; mint on a paid quote, then on the issued quote; outputs below and above the quote amount; melt with fee reserve and change; melt with insufficient inputs.
- **Excluded, by design.** Idempotent replay of the same operation, consensus duplicate and conflict keys, admission checks, deterministic operation ids and completion time zero, DLEQ proofs, and batch mints (a standalone mint needs NUT-29 configured; batch limits run at admission in federation mode).
- **Divergences found**, each pinned by an `expected_divergence_*` assertion that also checks every other dimension agrees:
  - Outputs on an unknown keyset: both reject without state change, but federated application returns a federation error instead of `UnknownKeySet`, and the two are classified differently by the apply-rejection rule.
  - Outputs on an inactive keyset: both reject definitively without state change; the federated error kind differs from `InactiveKeyset`.
  - A Bolt11 mint whose outputs are below the quote amount: a standalone mint rejects it (`TransactionUnbalanced`); federated application signs it and leaves the remainder mintable.
  - A Bolt11 mint above the quote amount: both reject without state change; federated application reports `UnpaidQuote` instead of `TransactionUnbalanced`.
- **Not fixed.** These are business-rule and error-kind differences in federated application; fixing them changes production code and is left for a follow-up decision.

Verification: `cargo fmt --all`; `cargo clippy -p cdk --features mint,wallet,federation,federation-bark --all-targets -- -D warnings`; `cdk` lib tests with `mint,wallet,federation,federation-bark` (1,082 passed) and with default features (790 passed); `cdk` tests build with `mint,federation` and no default features.

### 2026-11-25: package 4, divergences found by the parity test fixed

- **Keyset errors.** Federated output validation now reports `Error::UnknownKeySet` and `Error::InactiveKeyset`, the errors a standalone mint uses, instead of the federation's own variants (`FederationKeysets::check_output`). Every federated output-validation error (unknown keyset, unsupported amount, wrong keyset version, non-BLS blinded secret) is now a definitive rejection. The keyset view used at apply time is rebuilt from applied keyset rotations in journal order, so these outcomes are the same on every member.
- **Bolt11 mint amount.** A federated Bolt11 mint must issue exactly the quote amount, single or batch, as `process_mint_request` requires (`expected_federation_mint_amounts`). Below or above the quote amount is `TransactionUnbalanced` with the standalone message. Other payment methods keep partial single mints.
- **Tests.** The four `expected_divergence_*` assertions in `parity.rs` are now plain parity assertions; all 15 vectors agree on every compared dimension. `cdk-common` gains `federation_output_validation_errors_are_definitive`.

### 2026-11-25: package 8, third step (one federation registry in the wallet)

- **One registry.** `cdk::wallet::FederationRegistry` (`crates/cdk/src/wallet/federation/registry.rs`) discovers a federation from any member URL (`GET /federation/v1/config` checked against that member's `GET /v1/info` advertisement), validates it, and stores it under its federation ID. Every member's public mint URL and the wallet's local mint URL are aliases of that entry, and an alias belongs to at most one federation. API: `import` / `import_with_transport` (from a `FederationSource`: member URL, invite or public config, with `FederationImportOptions` for the local mint URL and the member-call policy), `inspect_with_transport` (validate without storing), `refresh_with_transport` (rediscover through the members, local URL first), `discover`, `validate_discovered`, `list`, `get`, `find_by_mint_url`, `set_connector_policy`, `remove`. An entry (`RegisteredFederation`) exposes the config, local mint URL, aliases, member lookup by URL, active units, config digest and the connector built over the default or a given transport. Errors are one `FederationRegistryError` enum. `FederationConnectorPolicy` is the persisted form of `FederatedMintConnectorConfig`.
- **Identity.** The key is the federation ID, which the config derives from its roster. Importing the same federation again, through any member, an invite or a file, refreshes the one entry and keeps its local mint URL; asking for a different local mint URL fails (`LocalMintUrlConflict`), as does a URL already held by another federation (`AliasConflict`). Rediscovery that returns another federation ID is refused (`FederationChanged`). The wallet still stores proofs under the local mint URL: tokens carry it, so a URL derived from the ID would not be resolvable by receivers.
- **Persistence (decision 6).** Entries are JSON in the wallet KV store, primary namespace `federation`, secondary `registry`, key = federation ID. No `WalletDatabase` change, no migration. Entries are checked on load (format version, key matches the ID, `validate_wallet_runtime_metadata`, BFT-safe thresholds, an active keyset, a valid policy); full validation (threshold keys, setup proofs) runs once at import.
- **Removed duplicates.**
  - `cdk-cli`: the `federations.json` sidecar (`FederationWalletRegistry`, `StoredFederationWallet`, `StoredFederationConnectorPolicy`), its own discovery (`discover_public_config`, `validate_discovered_public_config`), invite and file loaders, and the default-URL rule. The CLI now reads the registry from its wallet database at startup and keeps it in the existing process-wide lookup.
  - `cdk-ffi`: its discovery and validation (`discover_public_config`, `fetch_json`, `parse_federation_invite`, `federation_connector`) and the in-memory URL map (`FederationRegistration`). `WalletRepository` now reactivates persisted federations at construction, so `new_with_federations` is removed; `list_federations` and `refresh_federation` are added. `discover_federation` and `inspect_federation_invite` use `inspect_with_transport`.
- **Alias rules unified.** The CLI matched by local URL and member URL but replaced entries by local URL, so the same federation imported through two members became two entries; the FFI kept one entry per federation and refused URLs held by another federation; the TUI kept one federation per directory. All three now use the registry's rule. Each caller keeps its own wallet-key choice: the CLI still opens a wallet under the URL a token names (receive requires the token's mint URL), the FFI resolves aliases to the local mint URL.
- **Stricter.** All front ends now refuse thresholds that are not BFT-safe and configs without an active keyset; before, only the CLI did.
- **Tests.** `cdk` `wallet::federation::tests::registry` (7): discovery through a member URL, validation failures with nothing stored (digest mismatch, URL not on the roster, unreachable member, tampered federation ID, unknown coordinator, bad invite), configs the wallet cannot use (no active keyset, unsafe thresholds, a non-v3 keyset, a stale wallet protocol version, roster metadata that does not match), alias mapping and conflicts, persistence across a reopened SQLite wallet database including a policy update, an entry stored under the wrong key, and refresh on rediscovery (through another member while the local one is down, by re-import, and refusal of a changed federation). `cdk-ffi`: `imported_federation_is_reactivated_after_restart`. The CLI's 19 registry-file tests are replaced by 3 command tests; the rules they covered are in the `cdk` registry tests.

Verification: `cargo fmt`; clippy with `-D warnings` for `cdk-common` (`federation,test`), `cdk` (`mint,wallet,federation,federation-bark`), `cdk-federation`, the three `cdk-axum` configurations, and `cdk-cli` and `cdk-ffi` with and without `federation`; the 15 production builds; `cdk-common` with `federation,test` (496 lib, 496 integration), `cdk-federation-bark` (40), `cdk-bark` (36), `cdk-sqlite` with `federation-bark` (195), `cdk` with `mint,wallet,federation,federation-bark` (1,073 before the seventh registry test), `cdk-federation` (115), `cdk-mintd` with `sqlite,federation-test-utils,federation-bark` (298), `cdk-axum` with `federation-bdk,federation-bark,iroh` (478 of 479; the timing-bound seven-member latency probe noted above, which passes alone), `cdk-cli` (27; 43 with `federation,iroh`) and `cdk-ffi` (110; 111 with `federation`).

### 2026-11-25: package 8, fourth step (the TUI as a workspace crate, decision 7a)

- **Moved.** `scripts/fed-cdk-tui` is now `crates/cdk-fed-tui` (crate `cdk-fed-tui`, `publish = false`, binary still `fed-cdk-tui` so the deploy and smoke scripts keep their paths to the binary). It is a workspace member on workspace dependencies and workspace lints; its own 6,115-line `Cargo.lock` is deleted and its packages are in the root `Cargo.lock` and `Cargo.lock.msrv`. `build_dist.sh`, the crate README, the ignore list for `dist/`, `deploy_nocean_multi_operator_federation.sh`, `AGENTS.md` and the root README use the new path.
- **On the registry.** The TUI's `federation.json`, its discovery and validation (`discover_public_config`, `fetch_json`, `validate_discovered_public_config`), its stored-config checks and its conflicting-federation check are gone. `WalletRuntime` keeps the federation in the wallet database's registry. The one-federation-per-directory rule is kept: a join that would add a second federation is refused and its entry removed. A first join stores the dashboard's 90-second member-call policy with the joined member as quote coordinator.
- **Primary switching no longer moves the wallet.** Switching the primary member used to rewrite the local mint URL, which is the wallet's key for proofs and quotes, so proofs stored under the previous primary dropped out of view. It now changes only the stored quote coordinator; the wallet keeps its local mint URL.
- **Drift fixed.** Outside the workspace the TUI had stopped compiling against current CDK (`http_get_raw` no longer exists; `Transaction` gained `status`). Both are fixed.
- **Tests.** 24 (`cargo test -p cdk-fed-tui`). The three discovery-validation tests moved to the `cdk` registry tests; the stored-config conflict and `with_primary` tests are replaced by `join_rejects_a_different_federation_over_an_existing_wallet`, `first_join_uses_the_dashboard_policy_and_rejoin_keeps_it` and `primary_policy_accepts_roster_members_and_rejects_strangers`.
- **Not done.** `build_dist.sh` needs macOS, Docker and code signing and was only syntax-checked. An existing `~/.fed-cdk-tui` directory's `federation.json` is not imported; rejoining through the same member restores the same local mint URL and its proofs.

Verification: `cargo fmt`; `cargo clippy -p cdk-fed-tui --all-targets -- -D warnings` under the workspace lints, plus the clippy, build and test runs listed for the third step, with the same results (`cdk` 1,074 with the seventh registry test; `cdk-axum` again 478 of 479 with only the latency probe failing, which passes alone); `cdk-fed-tui` 24; `cargo metadata --locked`.

### 2026-11-25: package 8, fifth step (supported feature matrix in CI)

- **Supported set.** These crate and feature combinations are supported; each is a `flake.nix` clippy-and-test entry (the `federation-*` groups of `clippyAndTestChecks`, which the CI `clippy-and-test` matrix runs on pushes, releases and manual runs; pull requests keep their smaller subset):

  | Group | Combination |
  | --- | --- |
  | `federation-domain` | `cdk-common` `federation,test` |
  | | `cdk-federation-bark` (default) |
  | | `cdk-sql-common` `federation-bark` (added) |
  | | `cdk-sqlite` `federation-bark` |
  | | `cdk-bark` `federation-bark` (added) |
  | | `cdk` `--no-default-features` `mint,wallet,federation` |
  | | `cdk` `mint,wallet,federation,federation-bark` (added) |
  | | `cdk` `--no-default-features` `wallet,iroh` (added) |
  | `federation-custody` | `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark`; `cdk-bitcoin-wallet` all features |
  | `federation-runtime` | `cdk-federation` `test-utils,federation-bdk,federation-bark,iroh,prometheus` (added; its tests ran nowhere in CI) |
  | | `cdk-axum` `--no-default-features` with `federation`, `federation-bdk`, `federation-bark`, and `federation-bdk,federation-bark,iroh` |
  | `federation-binaries` | `cdk-mintd` `--no-default-features` with `sqlite,cln,federation`, `sqlite,federation-bdk,federation-bark`, `sqlite,federation-test-utils` |
  | | `cdk-mintd` `--no-default-features` `sqlite,info-page,federation-bdk,federation-bark,iroh,prometheus` (added; the production build in `deploy_nocean_multi_operator_federation.sh`) |
  | | `cdk-mintd` `--no-default-features` `sqlite,federation-test-utils,federation-bark` (added) |
  | | `cdk-cli` `--no-default-features` `federation,iroh` |
  | | `cdk-ffi` `--no-default-features`, and with `federation` |
  | | `cdk-fed-tui` (added) |

  `cdk-axum` with `prometheus` is built and tested through the production `cdk-mintd` entry and clippy-checked locally, but has no entry of its own: with `prometheus` on, the seven-member AlephBFT latency probe takes 10.3 to 10.7 s against its 10 s bound even when run alone, so a test entry would fail. The standalone `cdk` default build stays in `core-sdk-baseline`. `checkall.sh` and `testall.sh`, the local scripts these entries come from, are a subset of this table.
- **TUI in CI.** `cdk-fed-tui` is a workspace member, so the pull-request `quick-check` (workspace clippy, all targets) lints it, `pr-unit-tests` now runs its tests (`cargo test --bins -p cdk-fed-tui`), and `federation-binaries` runs clippy and its tests. `mkClippyAndTest` runs `--bins` tests for it as for `cdk-cli`, since it has no library target. The MSRV dependency build excludes it, as it does `cdk-cli`.
- **Removed.** `cdk-integration-tests/federation-bdk`: no code, recipe, script or CI job enabled it.
- **Fixed.** The production `cdk-mintd` build (`sqlite,info-page,federation-bdk,federation-bark,iroh,prometheus`) failed clippy: the `MetricsMintPayment` import is used only by the Lightning, fake-wallet and gRPC backends, none of which that build enables. The call sites now use the full path.
- **Kept, with reasons.**
  - `cdk-sqlite/federation` and `cdk-sql-common/federation` in the wallet front ends' features (`cdk-cli`, `cdk-ffi`, `cdk-fed-tui`). The wallets do not use federation storage, but `cdk-common/federation` enables `cdk-common/mint`, and `cdk-sqlite` builds its mint store by default, so without these features the mint store fails to implement the federation traits. Building the TUI without them reproduced that failure.
  - `cdk-postgres/federation` and `federation-bark`. No supported combination reaches them (they follow `cdk-mintd`'s `postgres` feature), but they gate the Postgres federation storage itself; removing them removes a storage backend, which this plan does not do. They are not in CI because the Postgres tests need a server. Flagged for a decision.
  - `cdk-bark/federation-test-utils`, `cdk-federation-bark/test-util`, `cdk-federation/test-utils`: test-only switches used by the supported test entries.
- **Not done.** Pull requests still run only the core feature groups plus workspace clippy and unit tests; the federation groups run on pushes and releases, as before. The groups grew (`federation-runtime` now also runs the `cdk-federation` suite); their 45-minute job limit was not measured in CI.

Verification: every added entry was run as `mkClippyAndTest` runs it (clippy with `-D warnings`, then `cargo test --lib`, or `--bins` for the TUI): `cdk-sql-common` `federation-bark` (15), `cdk-bark` `federation-bark` (36), `cdk` `wallet,iroh` (522), production `cdk-mintd` (239), `cdk-mintd` `sqlite,federation-test-utils,federation-bark` (298), `cdk-fed-tui` (24); `cdk-federation` (115) and `cdk` with `mint,wallet,federation,federation-bark` (1,074) ran in the suite below. Then the full set of the third step again, all passing (`cdk-axum` 479 of 479 this time), workspace clippy with all targets (with `clippy::manual_is_multiple_of` allowed: the local clippy flags it in `cashu` and `cdk-ldk-node` code this change does not touch), `cargo metadata --locked`, and the flake evaluates.
- **Postgres federation features kept.** No supported combination reaches `cdk-postgres`'s federation features, but removing them would remove Postgres federation storage, which is not a feature-matrix reduction. They stay; adding a Postgres federation check to CI needs a running Postgres and is left to the existing Postgres job setup.
