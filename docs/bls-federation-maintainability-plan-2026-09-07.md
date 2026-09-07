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
