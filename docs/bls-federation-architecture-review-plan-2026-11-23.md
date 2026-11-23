# BLS federation architecture review and refactoring plan

Status: proposed on 2026-11-23. No work package has started. Decisions in "Decisions needed" are open.

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
- [V] The copies have drifted. The batch-mint size limit is checked in `crates/cdk/src/mint/issue/mod.rs:888` and in the HTTP handler `crates/cdk-axum/src/custom_handlers.rs:257`, but not in federated application. [R] Three further standalone mint rules have no counterpart there.
- [R] Share signing is a three-times repeated block, and each application writes the share and a blind signature to two tables.
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

1. Replace feature-selected key types with additive ones (package 3)?
2. Allow the validate/plan/commit split in shared mint code, possibly landed on `main` first (package 4)?
3. Bark as a typed, feature-gated operation variant, or a real trait object injected into the builder? Either way the global registry goes.
4. Crate layout: only extract `cdk-federation` from `cdk-axum`, or also move the pure domain out of `cdk-common` into a core crate?
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
