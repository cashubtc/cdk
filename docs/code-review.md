# CDK Code Review Guidelines

Review CDK changes for observable correctness, Cashu interoperability, safe upgrades, and maintainability. Use the current repository's [AGENTS.md](../AGENTS.md), [CODE_STYLE.md](../CODE_STYLE.md), [DEVELOPMENT.md](../DEVELOPMENT.md), and [SECURITY.md](../SECURITY.md) for project conventions. Apply the checks relevant to the change; do not turn this guide into a requirement to redesign unrelated code.

## Review approach

- Establish the intended behavior, base and current head, affected callers, and supported configurations. Read existing review threads and replies before repeating feedback. Recheck the code even when a thread is marked resolved.
- Inspect surrounding code and unchanged consumers as needed. Findings should concern defects introduced or exposed by the change, or requirements its stated fix still fails to satisfy. Clearly distinguish an incomplete fix from a new regression; keep unrelated existing issues separate.
- Prefer evidence over speculation: identify the triggering conditions, affected behavior, and practical consequence. Check the premise before proposing an alternative.
- Prioritize correctness and fund/accounting integrity, protocol compatibility, persistence and upgrades, runtime recovery, public APIs, then performance and readability.
- Ask concise questions for design choices and suggest concrete alternatives. State confirmed defects directly. Do not downgrade a correctness finding merely to make it sound polite.
- Keep the PR focused. Distinguish blockers from optional improvements and reasonable follow-ups. Prefer existing abstractions and shared implementations over new frameworks without demonstrated need.

## Errors, arithmetic, and validation

- Flag production `unwrap()`. Tests, including integration tests, may use it. Permit `expect()` only when an invariant actually guarantees success and the message explains why; input, configuration, network, and database failures should return structured errors.
- Prefer existing domain errors and `thiserror`. Preserve errors needed by callers for recovery; do not hide failures with empty strings, defaults, or success responses. An intentional optional fallback is acceptable when it matches the contract.
- Check amount arithmetic, fees, units, narrowing conversions, and signed database boundaries. Use checked conversions/arithmetic where failure must be reported. Saturation is suitable only where clamping is intended, not as a way to hide financial overflow.
- Validate configuration through every public construction path, not only the daemon's setup. Check defaults, environment/file precedence, zero values, duration conversions, and actionable error messages.

## Protocol and compatibility

- Check the applicable NUT and its status, wire field names, error codes, encodings, and test vectors. Do not equate an unmerged proposal with an established interoperability contract. Consult the relevant external specification for non-Cashu protocols too.
- Preserve exact wire names when using ergonomic Rust names. For example, a Rust field named `proofs` corresponding to wire `inputs` needs `#[serde(rename = "inputs")]`.
- Keep business operations out of serialization. Custom serializers/deserializers may validate, normalize, or preserve supported legacy representations through shared `FromStr`, `TryFrom`, or constructors.
- Distinguish CDK mint implementation choices from what wallets may assume about other mints. Validate against other implementations or established vectors when behavior changes.
- Consider old wallet/new mint and new wallet/old mint combinations. Describe the rollout, compatibility mechanism, or intentional breaking change; passing tests between two updated CDK components alone is insufficient.
- Check public functions, traits, re-exports, feature flags, config/env names, RPC schemas, and deployed platform support. Intentional breaking changes still need appropriate migration guidance. Preserve unrelated API surface during focused fixes.

## Storage and state transitions

- Do not edit migrations that existing installations may already have applied. Add new migrations in the relevant backend/domain directory. New migrations introduced by the current change may be revised before adoption.
- Locate migrations in the current tree. SQL migrations are under `crates/cdk-sql-common/src/{mint,wallet,mint/auth}/migrations/{sqlite,postgres}/`; follow the applicable backend's existing ordering and naming. Check helper recipes before relying on their paths. Do not prescribe the old sqlx-cli workflow.
- Test upgrades from populated prior schemas, including related rows and enabled foreign keys, as well as fresh initialization and restart. Account for all affected domains/backends and migration/import tooling.
- Consider atomicity, interrupted upgrades, data preservation, locking duration, and realistic data volume. Do not promise a reversible migration when information has been discarded.
- For Redb and other serialized storage, verify old-record compatibility against the actual stored representation. Optional fields may avoid a migration, but do not assume every schema change does.
- Follow transaction ownership through the entire operation. Avoid acquiring another connection while holding a SQLite transaction or keeping database locks across unbounded external work.
- Check atomic transitions, duplicate handling, affected-row checks, and transaction-wide lock ordering on PostgreSQL. Process-local coordination alone does not establish correctness for multiple instances.
- Review explicit commit/rollback and cancellation paths. Preserve accounting and proof/quote state invariants through retries, uncertain external outcomes, and restart. Ensure repeated completion or recovery remains idempotent.

## Async services and privacy

- Check startup, stop, restart, reconnect, and shutdown behavior. Spawned tasks need clear ownership and termination; cleanup must work for library users as well as mintd.
- Inspect cancellation at await/select boundaries and timeout recovery. A timeout that only releases a permit may still leave the client unable to progress. State updates and notifications must agree with committed data.
- Check cache initialization/update ordering, stale snapshots, event delivery, backpressure, and recovery from missed events. Use state snapshots when only the latest value matters and event delivery where individual transitions matter.
- Use bounded retries/backoff where appropriate and verify recovery after a successful but idle connection. Optional dependencies should retain their documented degradation behavior.
- Consider background network activity, cache refreshes, configured transports, and privacy-sensitive diagnostics. Prefer behavior consistent with the user's selected privacy settings.
- Follow SECURITY.md for security findings. Keep public summaries high-level until release and coordinated disclosure; prepare detailed reports for the security contact through an authorized private channel. A review request alone does not authorize sending email or publishing findings.

## Wallet API and bindings

- Keep changes to the CDK Wallet API synchronized with `crates/cdk-ffi/src/wallet.rs`, `wallet_trait.rs`, and the conversions under `types/`.
- Keep protocol/business logic in the canonical Rust implementation. Bindings should adapt that implementation rather than maintain a second version in another language or a binding-specific Rust crate.
- Validate generated bindings and the target-language call path when conversions, ownership, errors, async behavior, or packaging change. Rust compilation alone does not verify a usable foreign-language API.

## Dependencies, performance, and style

- Disable default dependency features unless deliberately needed; enable the required features explicitly. Check inherited workspace settings before flagging an omitted local declaration. Reuse workspace dependencies and existing utilities.
- Check relevant minimal/optional features, WASM, and the project's current MSRV. Avoid relying on features accidentally enabled by unrelated workspace members. Consider transitive runtime/TLS dependencies and binding size.
- Prioritize repeated database/network operations, lock contention, and substantial allocations. Do not trade understandable APIs for excessive generics merely to avoid a small allocation. Performance claims need evidence.
- Follow CODE_STYLE.md and automated formatting/linting. Do not mechanically suggest `fold` over `map().sum()`, or invent performance differences between equivalent iterator forms.
- Use fully qualified `tracing::*` for diagnostics. Preserve intentional CLI/user-facing output. Prefer comments explaining invariants and reasons, and avoid redundant comments or dead commented-out code.
- Keep feature work rebased rather than merging main, following DEVELOPMENT.md. Use JJ-native local operations in JJ workspaces. Treat history cleanup as workflow feedback, not a runtime defect.

## Tests and evidence

- Ask for regression tests tied to the behavior at risk. Exercise the real entry point, not a helper that the production path bypasses. Test a claimed failure fix independently of unrelated validation failures.
- For races, coordinate tasks with barriers or explicit signals so the relevant interleaving is exercised. Prefer deterministic fixtures/state control to arbitrary sleeps.
- Put shared database-contract tests in the existing reusable test suite and verify the affected backend, including PostgreSQL-specific behavior where SQLite cannot establish it.
- Preserve meaningful coverage when moving or deleting tests. Avoid duplicate cases that exercise the same branch without proving an additional contract.
- Check that new tests are actually included in the relevant CI/feature configuration. State which tests ran and which did not; do not infer execution from the presence of a test file or approval from an unrelated green job.
- Separate a known unrelated CI flake from a regression. A proposed flake fix must demonstrate that it addresses the flaky behavior.

## Builds and releases

- Prefer the existing pinned Nix tooling; consolidate duplicated setup. Use Nix builds for reproducible build artifacts where appropriate and development shells for checks requiring external services.
- Ensure release artifacts, generated bindings, manifests, and lockfiles correspond to the intended source ref. Check individual and aggregate release entry points for consistent inputs and behavior.
- Gate final release side effects on successful required publishing. Make partial failures and retries explicit; serialize attempts to publish the same immutable version. Do not silently succeed when required publishing cannot run.
- Check that packaged native libraries match advertised platform/architecture tags and can be consumed without the build toolchain. Keep unsupported-platform claims aligned with actual tests and maintenance commitments.

## Reporting

Follow the caller's output contract. By default, list actionable findings first, ordered by impact, with a precise file/line reference, triggering condition, consequence, and suggested fix where useful.

- `critical`: demonstrated severe correctness, safety, data/accounting integrity, or release-blocking failure.
- `warning`: a concrete defect, compatibility problem, or missing requirement with a stated consequence.
- `nit`: optional style or maintainability feedback; normally omit low-value nits and never present them as correctness blockers.

Choose severity from impact and reachability, not the presence of a particular method name. Separate unresolved questions from confirmed findings.

Use `CHANGES_REQUESTED` for confirmed blockers, `COMMENT` for nonblocking warning-level findings or material uncertainty, and `APPROVE` when neither applies. Optional nits alone do not prevent approval. These are descriptive verdicts, not permission to submit a GitHub review. A submission adapter must map them to GitHub's API events.

Use valid inline anchors only when the finding belongs on a line accepted by the review API. Put findings without such anchors in the review body or the caller's supported unanchored channel. Do not invent line numbers, default to the wrong diff side, or suppress a missing update merely because its file is unchanged.

Conclude with a short account of scope, validation performed, and material remaining uncertainty. Do not repeat findings in the summary. If automation requests JSON, follow its documented schema and return JSON only. Unless the caller specifies another schema, read and use the [automated review JSON format](code-review-json.md); check consumers before changing its structure.
