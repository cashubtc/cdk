# Federation maintainability implementation plan

Status: implementation in progress. See the execution ledger below.

Baseline: `bls-federation` at `d2a5b9965`, compared with main base `bb139eff2`.

## Objective and boundaries

Simplify federation integration, ownership, storage access, and wallet orchestration while preserving all existing federation functionality.

- Preserve ordinary CDK APIs, defaults, serialization, migrations, and behavior.
- Limit changes in shared files to federation integration sections and restoration of the ordinary CDK boundary.
- Reuse established CDK interfaces and patterns; do not introduce a general-purpose orchestration framework.
- Federation is undeployed: obsolete federation-only APIs and representations need no compatibility layer.
- Preserve consensus authorization, independent transaction validation, nonce safety, payment idempotency, checkpoint evidence, and recovery behavior.
- Extend the completed September 5 cleanup; do not reopen completed work without new evidence.

## Implementation work packages

### 1. Restore feature isolation

- Gate BLS dependencies and protocol extensions behind `cashu/federation`; propagate activation explicitly through federation consumers.
- Preserve the original secp256k1 API when federation is disabled, including fixed-size serialization, infallible methods, parsing, and trait implementations.
- Use separate feature-selected key implementations where signatures differ; keep ordinary mint and wallet business logic shared.
- Remove federation from FFI defaults and enable it explicitly in federation binding build recipes.
- Update affected federation callers and FFI conversions together.
- Record any foundational change that cannot meet the boundary as an unresolved prerequisite; do not silently expand scope.

Acceptance: an isolated downstream consumer compiles against ordinary APIs, has unchanged defaults and serialization, and does not acquire BLS dependencies with federation disabled.

### 2. Reduce shared entrypoint complexity

- Move federation admission, notification, synchronization, startup verification, and wallet reconciliation from Axum's root into federation modules.
- Move federation-only mintd configuration and construction into dedicated modules.
- Move federation database trait definitions into focused modules, preserving public re-exports and transaction composition.
- Leave shared entrypoints responsible for construction, route attachment, and lifecycle wiring.
- Separate mechanical relocation from semantic changes.

Acceptance: ordinary startup remains directly readable; federation modules own their implementation without another public abstraction layer.

### 3. Clarify runtime states and service ownership

- Distinguish setup state from active-member context; require essential mint, consensus, sequencing, and notification handles in active-member operation.
- Retain optionality for genuinely unavailable wallets, sessions, and readiness conditions.
- Move task-group ownership from the payment observer to the federation runtime/service root.
- Return prepared routes and services directly instead of using a oneshot handoff.
- Preserve ordinary router constructors, essential-worker failure propagation, bounded shutdown, durable external-action identities, and lock ordering.

Acceptance: construction prevents invalid handle combinations; startup, partial-start failure, shutdown, and restart tests pass without silent service omission.

### 4. Introduce focused Bark storage reads

- Replace complete projection loads used for a single action with action-scoped federation queries.
- Provide typed Bark-facing access without a dependency cycle through `cdk-common`.
- Retain complete snapshots for audits, recovery, and workflows requiring complete state.
- Preserve transactional consistency and deterministic ordering.
- Measure query counts, rows loaded, and latency before and after at identical history sizes.

Acceptance: single-action paths avoid unrelated history reads; SQLite/PostgreSQL results agree; replay and materialized-state digests remain unchanged.

### 5. Simplify wallet collection and aggregation

- Consolidate quote fallback mechanics while retaining explicit mint/melt validation and visibility semantics.
- Return the successful verified aggregate from collection.
- Retain request-scoped verified shares bound to configuration, member identities, keysets, outputs, and validation mode.
- Preserve disagreement handling, invalid-share rejection, restore, partial change, and catch-up behavior.

Acceptance: successful collection does not repeat verification and aggregation; adversarial-response coverage survives.

### 6. Improve errors and internal interfaces

- Replace discarded Bark errors with structured federation errors containing safe phase and operation context.
- Preserve public classifications, redaction, and pending treatment of uncertain payment outcomes.
- Reduce helper visibility to actual callers.
- Remove unused settings and single-behavior abstractions only when they enforce no invariant or external contract.
- Separate local tuning from consensus policy; retain explicit cryptographic domains and consensus limits.

Acceptance: failures remain diagnosable without leaking secrets or changing payment recovery decisions.

### 7. Simplify test support and maintainer documentation

- Consolidate fixtures around roster, transport, storage, and payment backend.
- Preserve distinct admission, application, SQL, network, and restart scenarios.
- Require an old-test-to-retained-coverage mapping before deleting tests; keep test utilities gated.
- Extend the federation guide with the CDK concept map and linked mint, swap, outgoing-payment, and recovery walkthroughs.
- Document authority, transaction boundaries, external effects, and recovery ownership.

Acceptance: reviewers can follow operations without reconstructing architecture from fixtures or historical documents.

## Verification and completion

Run targeted checks per package, followed by the relevant existing feature matrix and integration suites. Required coverage:

- Isolated ordinary-CDK downstream compilation and API/serialization checks.
- Federation feature combinations, FFI, formatting, and applicable lint checks.
- SQLite/PostgreSQL transactions, rollback, focused queries, replay, and digest agreement.
- Invalid shares, conflicting responses, insufficient quorum, partial change, and restore.
- Setup, worker failure, partial startup, shutdown, restart, and catch-up.
- Local regtest incoming/outgoing on-chain, Lightning, and ecash payments.
- Restart/catch-up after payments, consistent member state, and value conservation.

Keep before/after storage and verification measurements. Record commands, results, limitations, and prerequisites below. Completion requires ordinary-CDK isolation and federation payment verification.

## Delivery order

Separate reviewable commits: feature isolation; mechanical relocation; runtime ownership; Bark queries; wallet aggregation; errors/interfaces/fixtures; documentation and final verification.

Preserve federation wire formats and canonical encodings unless a package requires a change. Do not delete local databases, rewrite published history, or deploy services.

## Execution ledger

- Implementation started from the clean baseline. No completion or new payment verification is claimed yet.

### Package 1: feature boundary

- BLS dependencies, curve implementations, version-02 keysets, and wallet verification paths are explicitly gated. Shared curve-aware callers use fallible `try_*` operations; ordinary keys retain their original methods and `Deref` implementations.
- Restored ordinary token encoding and V2 mint defaults. Restored the `KeysetRotation.use_keyset_v2` field; explicit federation keyset selection and rotation are separate builder methods. This also fixes two additional ordinary-CDK API/default changes found during implementation.
- FFI defaults no longer enable federation; Swift/Kotlin manifests already enable it explicitly. Added an isolated downstream conformance check and a CI job using the existing Nix FFI shell.
- `python3 scripts/check_standalone_api.py`: two downstream tests passed; normal dependency graph excludes BLS/FROST and FFI defaults exclude federation.
- `cargo test --offline -p cashu --lib`: 603 passed, 2 ignored.
- `cargo test --offline -p cashu --lib --features federation`: 640 passed, 2 ignored.
- Library checks passed for standalone CDK mint/wallet and combined federation/BDK/Bark Axum.
- `cargo clippy --offline -p cashu -p cdk --lib --no-default-features --features cdk/mint,cdk/wallet,cdk/federation -- -D warnings`: passed. Keyset signature-family dispatch is centralized so callers remain valid under Cargo dependency-feature unification.
- Initial local checks used the available Cargo/Rust 1.95 tools. The project Nix stable shell is available and reports Cargo 1.98; final checks will use it. An all-target Cashu lint run exposed the existing ordinary NUT-16 test's `manual_is_multiple_of` warning; that unrelated test was left unchanged.
- Remaining: final pinned-toolchain matrix, FFI, full integration and payment verification, and packages 2–7. These interim results do not establish overall completion.

### Package 2: mechanical module extraction

- Moved admission, notification, synchronization state, mint integration methods, and startup/wallet reconciliation into Axum federation modules. Shared entrypoints retain construction, routing, and lifecycle wiring.
- Moved daemon federation configuration into its own module and federation database contracts into `database/mint/federation.rs`, preserving public re-exports and transaction composition.
- Retained all existing tests. Removed imports and redundant feature gates made unnecessary by the gated module boundaries.
- `cargo fix --offline --allow-dirty --lib -p cdk-axum -p cdk-mintd -p cdk-common --no-default-features --features cdk-axum/federation-bdk,cdk-axum/federation-bark,cdk-mintd/federation-bdk,cdk-mintd/federation-bark,cdk-mintd/sqlite`: library compilation passed while pruning relocation imports.
- Pinned-toolchain all-target compilation is in progress; final test results will be recorded below. This commit is mechanical and does not claim runtime or payment verification.

### Package 3: member context and lifecycle

- Required mint, consensus, sequencing, notification, admission, and peer-policy handles now live in one member context. Membership distinguishes standalone, setup, and active operation. Setup cannot enable payment workers through a separate boolean.
- The federation runtime owns the shared task group. Payment observation, DKG, synchronization, checkpoint work, Bark work, and readiness monitoring use that group; cancellation, failure propagation, durable identities, and bounded shutdown are preserved.
- Prepared routers return their services directly. Ordinary public router constructors keep their existing signatures and automatic lifetime behavior.
- Pinned Nix stable check passed: `cargo check --offline -p cdk-axum -p cdk-mintd --all-targets --no-default-features --features cdk-axum/federation-bdk,cdk-axum/federation-bark,cdk-mintd/federation-bdk,cdk-mintd/federation-bark,cdk-mintd/sqlite`. The restricted daemon test feature set reports existing unused-helper warnings; no Axum errors or warnings remain.
- `RUST_MIN_STACK=16777216 cargo test --offline -p cdk-axum --lib --no-default-features --features federation-bdk,federation-bark federation::services::tests -- --test-threads=1`: 4 passed, including drain, timeout/abort, explicit failure, and panic propagation.
- Same configuration with filter `service_startup_failure_cancels_workers_already_started`: 1 passed.
- The test build also verified the module extraction. Moved test imports were made explicit. Two federation test helpers used outside metrics tests had an unnecessary metrics feature gate; removing those gates restores the no-metrics test combination. No tests were deleted.
- Full setup/restart/catch-up and payment verification remain part of final validation.

### Package 4: focused Bark action reads

- Added an action-scoped read to the federation storage interface and a typed mint accessor. Certificate-to-payment lookup and transition-certificate lookup now fetch one action. Validation, audits, and recovery retain complete snapshots. No migrations or canonical encodings changed.
- Both SQL implementations share the same keyed query. The shared test checks absence, rollback, repeated application, and exact equality with the complete snapshot at 1, 64, and 256 accepted receive intents.
- Pinned Nix stable: `cargo test --offline -p cdk-sqlite --lib --features federation-bark federation_bark_action_reads -- --nocapture --test-threads=1`: 1 passed. The corresponding `cdk-postgres` command with `CDK_MINTD_DATABASE_URL` set to the local fixture: 1 passed.
- Measurements below are elapsed microseconds for 20 paired reads per history size, including decoding. These are local workload measurements, not production latency predictions. PostgreSQL statement logging was enabled equally for both reads.

| Backend | History | Snapshot rows/read | Focused rows/read | Snapshot µs/20 reads | Focused µs/20 reads |
| --- | ---: | ---: | ---: | ---: | ---: |
| SQLite | 1 | 2 | 1 | 6,061 | 3,711 |
| SQLite | 64 | 128 | 1 | 184,727 | 6,445 |
| SQLite | 256 | 512 | 1 | 696,368 | 10,672 |
| PostgreSQL | 1 | 2 | 1 | 88,261 | 18,175 |
| PostgreSQL | 64 | 128 | 1 | 360,561 | 39,598 |
| PostgreSQL | 256 | 512 | 1 | 1,638,454 | 146,651 |

- PostgreSQL statement logs confirm 60 SELECTs for each of the six projection tables (360 total), compared with 60 action-keyed SELECTs for the paired reads: six queries versus one per read. Fixture setup and rollback queries were excluded. Raw logs contain local fixture data and are not committed.
- Final Bark-filter rerun: SQLite 3 passed (including sustained workload), PostgreSQL 2 passed. Canonical materialized-state digests match across both backends at all three history sizes. At 256 intents both produce `fcb7003dfd459b573eb0ef7f90d4b94b793bedaaae326696e7a6ae0f27b1df1d`. Full payment and restart verification remain pending.

### Lifecycle follow-up

- The final worker inventory found a detached BDK sync loop still starting during wallet construction. It now starts explicitly with active member services, uses the runtime task group, observes cancellation between syncs, and retains bounded in-flight shutdown. Existing retry timing and latency events are preserved.
- Combined Axum/daemon all-target compilation passed with BDK and Bark enabled. The complete runtime suite and live restart validation are pending below.

### Package 5: quote fallback and verified aggregation

- Mint/melt quote creation shares ordering, timeout, failure accumulation, and definitive-error handling. Each operation retains its own validation and threshold-visibility step. Visibility failure never initiates another quote.
- Mint, melt, and restore collection return the successful aggregate directly. Verified shares are retained only within one request, bound to the immutable connector configuration, member identity, exact blinded outputs, response content, and validation mode. Ready responses drained after a rejection retain their chance to produce a valid result.
- Pinned Nix stable: `RUST_MIN_STACK=16777216 cargo test --offline -p cdk --lib --no-default-features --features mint,wallet,federation-bark wallet::federation::tests -- --nocapture --test-threads=1`: 103 passed. This covers malicious shares, incompatible groups, insufficient quorum, partial change, fallback, restore, and catch-up.
- The repeated-pass comparison verifies the same aggregate with 12 individual share verifications using three fresh passes versus 4 with request-local retention. Changed outputs, member signatures, and amount metadata are rejected. This measures cryptographic work for that workload, not end-to-end latency.
- The final ready-response preservation adjustment will also run through the complete CDK suite. No wallet public methods changed, and no tests were deleted.

### Package 6: diagnostics and internal interfaces

- Bark execution/reconciliation errors retain their typed, redacted backend reason with phase and accepted-operation context. Database/host projection failures retain a redacted reason while preserving their prior ambiguous classification. Public Cashu and private HTTP responses remain generic internal failures.
- Special custody-signer recovery, uncertain external outcomes, degraded-state decisions, and evidence requirements remain unchanged. Diagnostic conversion never authorizes initiation or retry.
- Removed the maintenance wrapper whose only caller always supplied one action. Reduced visibility of maintenance startup and the local wallet-liveness configuration; retained the configuration's timing invariants and explicit consensus signing lease.
- `cargo test --offline -p cdk-common --lib --features federation federation_backend_failure -- --test-threads=1`: 1 passed, verifying public redaction and ambiguous failure classification before/after wire conversion. The existing private HTTP diagnostic test now also covers backend errors; the full Axum run is pending.
- Fixture consolidation uses the complete member-context constructor and shared cross-backend Bark conformance helper. No existing tests were deleted: admission, application, SQL, network, restart, and cryptographic scenarios remain separate. The old-to-retained coverage mapping is identity for every existing test.

### Final verification: runtime

- Pinned Nix stable: `RUST_MIN_STACK=16777216 cargo test --offline -p cdk-axum --lib --features federation-bdk,federation-bark,iroh,prometheus -- --test-threads=1`: 584 passed, 2 failed, 5 existing manual/stress tests ignored, in 1,109.68 seconds.
- Both failures came from the shared custom-handler fixture constructing BLS outputs for an ordinary mint after ordinary keyset defaults were restored. The fixture now calls the established `hash_to_curve_for_version` helper. Only the two ordinary cached-method tests use that fixture with a standalone state; their focused rerun passed both tests in 0.33 seconds (`cached_url_method_quote_method_mismatch -- --nocapture --test-threads=1`, otherwise identical feature flags). All 586 non-ignored Axum tests are now verified across the full run and this focused rerun.
- All runtime scenarios passed: partial startup, essential-worker failure/panic, bounded shutdown, DKG restart at every durable phase, multiple process-session restart patterns, four/five-member offline value windows, catch-up without duplicate payment, checkpoint paging/replay, state digests, and public error redaction. The ignored cases are four manual benchmarks and the pre-existing process stress regression; no tests were removed or newly ignored.

### Focused-read call-site follow-up

- Evidence-attestation routing and the operator's post-retry phase lookup now use the same action-scoped accessor. Together with the two certificate paths, four single-action consumers avoid unrelated history reads.
- Retained complete reads for recovery/replay, phase-history reconstruction, auditing, and independent custody validation against reserved VTXOs. The latter needs the action and its associated reservation state; replacing only its action lookup would not remove the snapshot or improve its validation boundary.
- A remaining send-plan materialization failure now logs the already-redacted Bark error while preserving the existing degraded-state outcome. The typed accessor retains the action-ID predicate previously enforced by snapshot lookup. Follow-up tests passed: 2 SDK Bark tests, 19 Axum Bark tests, and 4 Axum service-lifecycle tests, using the full-run feature flags above and filters `bark` and `federation::services::tests`.

### Feature-matrix follow-up

- The disabled-feature build found BLS-only storage fixtures imported by the shared `test` feature. BLS fixture definitions and registration are now gated at `cdk-common`'s feature boundary. This retains BLS tests for every backend when federation is enabled, without requiring each backend to invent its own federation feature or registering no-op tests when it is disabled.
- The narrow consensus-only lint check found one relocated error import used only by custody integrations. Its gate now matches those callers.
- BDK-only lint additionally found two Bark-only imports in the daemon configuration module; those imports now carry the Bark gate. The sync worker explicitly clones its owned handle inside the loop, preserving its required task lifetime while satisfying Clippy.
- Corrected disabled-feature check passed: `cargo check --offline -p cdk-mintd -p cdk-cli -p cdk-ffi --no-default-features --features cdk-mintd/sqlite,cdk-mintd/cln,cdk-ffi/bip353`.
- Strict lint passed for all three custody combinations: `cargo clippy --offline -p cdk-mintd --no-default-features --features sqlite,federation -- -D warnings`, then the same command with `sqlite,federation-bdk` and `sqlite,federation-bark`.

### Final verification: SDK, daemon, and storage

All final commands use the repository's Nix stable shell (Rust/Cargo 1.98). Federation test commands set `RUST_MIN_STACK=16777216`; PostgreSQL commands set `CDK_MINTD_DATABASE_URL` to the local fixture. Tests run serially to avoid unrelated consensus timing pressure.

| Command | Result |
| --- | --- |
| `python3 scripts/check_standalone_api.py` | 2 passed; independent ordinary dependency graph and API/serialization checks passed |
| `cargo test --offline -p cashu --lib` | 603 passed, 2 existing ignored |
| `cargo test --offline -p cashu --lib --features federation` | 640 passed, 2 existing ignored |
| `cargo test --offline -p cdk --lib --no-default-features --features mint,wallet,federation-bark -- --test-threads=1` | 950 passed, no ignored; includes all 103 federation wallet tests |
| `cargo check --offline -p cdk-ffi --features federation` | Passed, including default binding features |
| `cargo check --offline --workspace --all-targets` | Passed, including binding crates and test targets |
| `cargo test --offline -p cdk-sqlite --lib --features federation-bark federation -- --test-threads=1` | 36 passed |
| `cargo test --offline -p cdk-postgres --lib --features federation-bark federation -- --test-threads=1` | 30 passed |
| `cargo test --offline -p cdk-mintd --lib --features federation-bdk,federation-bark,federation-test-utils,iroh -- --test-threads=1` | 365 passed, no ignored |

The SDK run took 927.01 seconds and the daemon tests took 152.59 seconds, excluding compilation. Storage runs cover federation transactions, rollback, checkpoints, replay, and materialized-state agreement. The focused-read call-site and narrow feature corrections are being verified separately below; these results do not yet establish fresh live payment verification.

- Backend fixture follow-up: SQLite and PostgreSQL each passed both BLS storage tests (`cargo test --offline -p <backend> --lib --features federation-bark bls -- --test-threads=1`). A Redb-only run exposed that canonical federation operations already require shared mint quote/payment types but `cdk-common/federation` did not declare that dependency. The feature now explicitly enables `cdk-common/mint`; this selects shared types, not the SDK mint feature, and has no effect when federation is disabled. `cargo check --offline -p cdk --no-default-features --features wallet,federation` passed; `cargo test --offline -p cdk-redb --lib --features cdk-common/federation bls -- --test-threads=1` passed its existing BLS transaction test.

- Ordinary SQLite tests exposed an unconditional Bark dev-dependency that enabled federation types without the SQL implementation feature. Both SQL backends now request fixture support through `cdk-sql-common/test`, whose weak optional dependency enables Bark test utilities only when `federation-bark` is active. Existing test names and scenario bodies are unchanged. `cargo test --offline -p cdk-sqlite --lib --no-default-features --features mint,wallet -- --test-threads=1`: all 149 ordinary tests passed. The isolated downstream dependency check now includes SQLite to guard this boundary too.

- After changing fixture access, the `bark` filter passed 5 SQLite tests and 3 PostgreSQL tests, including ordinary-to-Bark migration coverage. No tests were deleted or renamed; their helper now comes from the conditionally enabled SQL test support re-export. Production federation builds do not enable this fixture feature.

- Final expanded isolated check (`python3 scripts/check_standalone_api.py`) passed both API/serialization tests with ordinary SQLite in the dependency graph; the graph excludes BLS/FROST/Bark and FFI defaults remain ordinary.
- `cargo build --offline -p cdk-mintd -p cdk-cli --features cdk-mintd/federation-bdk,cdk-mintd/federation-bark,cdk-mintd/iroh,cdk-cli/federation` passed. The live-test binaries retain ordinary defaults and explicitly enable federation; `federation-test-utils` is not enabled. `cargo fmt --all -- --check` passed.
