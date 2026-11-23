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
2. Allow the validate/plan/commit split in shared mint code, possibly landed on `main` first (package 4)?
3. Bark as a typed, feature-gated operation variant, or a real trait object injected into the builder? Either way the global registry goes. **Decided 2026-11-23:** a typed, feature-gated variant. **Revised during package 5:** `cdk-common` cannot name the Bark crate's types, so Bark uses the existing generic `Application` variant instead; the global registry is gone either way.
4. Crate layout: only extract `cdk-federation` from `cdk-axum`, or also move the pure domain out of `cdk-common` into a core crate? **Decided 2026-11-23:** only extract `cdk-federation`.
5. Canonical encoding: keep sorted JSON behind one module, or move consensus hashing to one binary encoding?
6. Persist the wallet's federation registry through `WalletDatabase`? This is new persisted information.
7. `scripts/fed-cdk-tui`: promote to a crate on the shared registry, or move out of the repository? Same question for the archived documents.
8. Keep FROST nonces in redb apart from SQL? If yes, document the reason and a "never restore from backup" rule.

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
