# BLS federation cleanup plan

Status: code cleanup, local payment verification, and isolated-source build verification completed. Bark is pinned to the reviewed remote revision as of 2026-09-07.

Baseline: `bls-federation` at `3367b77c1`, after rebasing onto main `bb139eff2`.

## Objective and scope

Make the undeployed federation implementation smaller, clearer, and easier to extend. Remove obsolete federation representations, development scaffolding in production surfaces, duplicate authority, and unnecessary work. Preserve the actual safety properties of consensus, custody, and recovery.

The user has explicitly authorized a cut-through design: old federation deployments, database contents, configuration files, wire messages, and source APIs do not require compatibility. Implementation was authorized on 2026-09-05, including local regtest verification of incoming/outgoing on-chain, Lightning, and ecash payments.

The initial survey covered the branch diff and federation/BLS/FROST references across protocol types, mint and wallet code, SQL storage, Axum, mintd/CLI, Bitcoin/Bark adapters, bindings, tests, fuzzing, scripts, and CI. The broad search selected 340 files containing approximately 349,000 lines; these numbers include tests and unrelated portions of shared files. This is a repository-wide inventory and targeted source inspection, not a claim that every line has already received a correctness review. Each work package below includes the remaining call-site and invariant audit.

Scope includes:

- BLS protocol additions in `cashu`; federation domain, transport, DKG, journal, checkpoint, and database contracts in `cdk-common`.
- `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark`, `cdk-bitcoin-wallet`, `cdk-bdk`, `cdk-bark`, and `cdk-federation-bark` where they participate in federation.
- Mint admission/application/recovery and wallet connector behavior in `cdk`.
- Axum routes, peer transport, AlephBFT, worker lifecycle, status, and application adapters.
- SQLite/PostgreSQL federation schema, projections, operator commands, configuration, HTTP/Iroh, management RPC, and federation FFI integration.
- Federation tests, benchmarks, fuzz targets, `justfile`, development/deployment scripts, and documentation.

Shared standalone behavior is a boundary: federation-only formats can change directly, while existing standalone migrations and public Cashu behavior must remain valid. Standard Cashu clients must still work with the federation's public endpoints.

## Evidence and proposed disposition

Line references below refer to the baseline and will move during implementation. “Confirmed” describes inspected source; “candidate” means the disposition needs call-site or behavioral evidence before making the change.

### 1. Remove federation format compatibility — confirmed, high priority

- `cdk-common/src/federation/transport.rs:3007`: `FederationLocalStatusResponse` has separate borrowed and owned representations, custom serialization, and legacy `<application>_operator` flattening. It accepts both flattened and nested operator maps, while serialization still writes the flattened shape. Use one current `application_operators` field and derived serialization with explicit field policy. Update CLI, metrics, scripts, and fixtures together; reject obsolete shapes.
- `cdk-sql-common/src/mint/federation.rs:459`: an untagged `StoredFederationAlephBftPendingEnvelopeCompat` decodes both current storage and a bare legacy envelope. Delete the legacy decoder and require the current storage record. Keep optional ordered groups if current ungrouped operations need them.
- `cdk-federation-bark/src/operation.rs:30`: Bark operation versions 1 and 2 are both accepted, with different reservation conflict behavior around line 1575. Keep the current semantics and remove the old version branch and acceptance tests. Keep an explicit current-version check; cut-through does not mean accepting unknown versions.
- `cdk-federation-bark/src/projection.rs:166`: allocation records retain `vtxo_id` alongside `vtxo_ids` for compatibility. SQL projection code updates both around `cdk-sql-common/src/mint/federation/bark_projection.rs:892`. Make the collection and aggregate amount authoritative; remove the redundant field and SQL column after tracing every reader.
- `cdk-federation-bark/src/operation.rs:835`: optional certified receive evidence explicitly supports replay of an earlier receive workflow. Trace live producers, then choose the current certified receive path for claim plans. Remove the old path only after proving any still-needed intermediate transition has a current role. Other action kinds legitimately lack receive evidence.

Acceptance: one supported representation per affected record/message, current round trips and canonical vectors pass, fresh replay converges, and old forms fail clearly. No dual readers or conversion migrations for old federation data.

### 2. Consolidate the federation schema — confirmed, high priority

Both SQL dialects have federation-specific migrations that create and later rebuild backup storage and replace indexes. For example, `20260714000000_chunk_federation_aleph_bft_backups.sql` renames a legacy table, copies its rows, and drops it.

Fold federation-only schema evolution into clean creation SQL for the final tables, constraints, and indexes. These federation files are absent from the inspected main tree. Preserve every upstream/standalone migration and its checksum. Account for all federation SQL, including Bark projections, generic KV records, checkpoints, and stored configuration, rather than squashing only the visibly named backup migration.

The normal review guidance forbids changing deployed migrations. The user's explicit instruction that federation storage is undeployed authorizes this narrowly scoped federation reset. Do not build an old-federation upgrade path or delete anyone's local database as part of implementation.

Acceptance: fresh SQLite and PostgreSQL databases produce equivalent logical schemas; keys, uniqueness, rollback, restart, projection digests, and backup/restore are verified. A pre-existing standalone database can acquire the final federation tables without rewriting its history.

### 3. Fix concrete boundary and startup defects — confirmed, first implementation batch

- `cdk-sql-common/src/mint/federation.rs:448`: `sql_member_id` casts `u64` to `u16` before validation, so oversized values can truncate to a valid ID. Use checked conversion and retain the member-ID domain validation. Test zero, boundaries, and values that previously wrapped.
- `cdk-bitcoin-wallet/src/chain/bitcoin_rpc.rs:17`: `initial_checkpoint` constructs an RPC client separately from the configured, timeout-aware builder at line 54. Route bootstrap through the same validated constructor; verify timeout and error behavior at bootstrap as well as sync.
- `justfile:994`, `1216`, `1270`, `1343`, `1566`, `1640`, and `1669`: federation launch paths still pass the removed top-level startup `--config`. Several also depend on old runtime environment overrides for slow/mismatched-member fixtures. Consolidate process initialization/start/restart around `config init`/`config apply` and database-backed startup. Preserve ceremony subcommand `--config`, which is a different, valid argument.

Acceptance: the actual generated CLI smoke, slow-member, and mismatched-member scenarios start and exercise their intended behavior. Compilation and shell syntax alone are insufficient for these launchers.

### 4. Isolate development-only capabilities — confirmed surface, disposition audit required

- The mintd trusted-dealer command is already gated by `federation-test-utils`; preserve that boundary.
- `cdk-common/src/federation.rs:29` and `256` compile and publicly export trusted-dealer generators with ordinary federation support. The implementation describes itself as the mocked development DKG path. Trace library, example, fuzz, and integration consumers; move fixture generation behind explicit test support and keep production setup dealer-free.
- `cdk-common/src/federation/consensus.rs:667` includes a generic `Default` consensus item documented for future compatibility tests. The inspected construction sites are tests. Verify there are no production producers, then remove this speculative production variant; use a real bounded item or a test adapter in its tests.
- `cdk-bitcoin-wallet/src/federation.rs:334` suppresses dead-code warnings for a path field read only by a test-only accessor. Remove production retention or gate the field and initialization for tests.
- `cdk-bdk/src/lib.rs:203` retains a `sync_config` field that is stored but not read. Trace callers and previous behavior before deciding whether to remove the dead field or reconnect an intended setting; do not silently discard working configuration.

Acceptance: production feature graphs exclude trusted-dealer/test helpers and simulated finality; no unexplained dead-code suppression remains in the touched surfaces. Keep legitimate regtest tools available through explicit test features.

### 5. Consolidate operator helpers and make status decisions typed — confirmed

`federation_bark_operator.rs:160` and `federation_wallet_operator.rs:152` repeat config loading, member selection, and sequence creation. The loaders have already drifted: one validates the private/public pairing locally while the other only parses it. Member-selection and timestamp-sequence bodies are effectively duplicated.

Introduce a small operator client/context that loads and validates once and owns signed request construction. Resolve wall-clock failure explicitly instead of converting it to sequence zero. Verify sequence uniqueness and replay behavior across concurrent commands before changing the sequence scheme.

`federation_status.rs:772` and `federation_digest_compare.rs:507` infer digest faults partly by searching human-readable text. Use one typed health/reason field and one policy helper. Keep redacted presentation separate from decisions. Consolidate duplicated display/setup labels where they have identical meaning.

Acceptance: malformed config fails consistently, signed requests remain authenticated and replay-protected, and changing error wording cannot change health classification.

### 6. Own and supervise federation workers — confirmed, high impact

The payment observer (`payment_observation.rs:1125`), transaction broadcaster (`broadcast.rs:249`), and wallet-liveness worker (`wallet_liveness.rs:862`) spawn tasks without returning retained join handles. The observer has local panic recovery, but that does not provide whole-service shutdown and failure ownership. Broadcaster and liveness loops have no explicit cancellation branch.

Integrate these workers into the current prepared/running daemon lifecycle with an explicit cancellation signal, retained handles, startup error propagation, and bounded join behavior. Inventory nested per-quote tasks and other federation workers as well. Use the existing task-management conventions where sufficient; introduce one small owner only if none covers the need.

Define behavior separately for worker failure, transient round failure, and shutdown. Preserve durable retry/exact-byte broadcast semantics. A failed worker must become observable; automatic restart must not repeat an uncommitted external payment or signing action.

Acceptance: repeated start/stop leaves no live tasks; injected failure is visible; shutdown during RPC, signing, and pending observation is bounded; recovery resumes safely from durable state. Do not “simplify” this by aborting arbitrary tasks while they hold stateful operations.

### 7. Replace implicit namespaces with explicit ownership — confirmed, staged refactor

- `cdk/src/mint/federation/mod.rs:13` uses three production `include!` files sharing an implicit namespace.
- `cdk-axum/src/federation/tests/mod.rs:102` includes fifteen scenario files into one scope.
- `cdk-axum/src/custom_handlers.rs` has 1,294 lines before its approximately 9,000-line inline test module.
- `cdk-bark/src/runtime.rs` holds roughly 6,100 production lines spanning send, receive, refresh, exit, CPFP, projection readiness, and reporting.

First identify ownership and the smallest required interfaces. For tests, build an explicit fixture API before converting includes. An earlier attempt documented in `cleanup-2026-07-29.md` produced 1,556 visibility/name-resolution errors; blanket visibility widening would recreate the problem.

Then move one domain at a time: mint admission/application/response recovery, public quote and mint/melt orchestration, Bark send/receive/maintenance, and transport message families. Keep HTTP extraction/authentication/response formatting at routes and business coordination in the relevant domain. Separate mechanical moves from semantic consolidation.

Acceptance: ordinary Rust modules with deliberate visibility; fixtures do not rely on sibling lexical scope; handlers expose a readable sequence; no generic manager, callback framework, or public re-export layer exists solely to make a move compile. File size is a review signal, not an arbitrary pass/fail limit.

### 8. Simplify DKG, signing, and transaction orchestration — candidates requiring deeper review

The Axum DKG driver has its own `FrostDkgPhase` mapping and a large FROST tick function (`dkg_driver.rs:37`, `838`), in addition to common durable driver plans and FROST local phases. Map each state's evidence and authority. Consolidate only duplicated phase decisions; the transport executor should deliver/persist a domain plan without becoming a second source of ceremony truth.

For BLS/FROST signing, checkpoint/catch-up, Bark operations, and on-chain proposals, trace construction, validation, persistence, execution, and recovery. Remove branches that accept states made unreachable by the chosen current schema. Use domain enums or validated values where they eliminate several impossible combinations, rather than adding wrappers to every value.

Keep independent validation of peer-proposed transactions separate from transaction construction. Preserve signer binding, nonce consumption, exact transaction bytes, accepted-entry authority, quorum evidence, authenticated sequencing, and halt-on-divergence behavior. Similar-looking checks at distinct trust boundaries are often required.

Acceptance: one documented owner per transition; every removed branch has proof of unreachability or a replacement rule; crash/replay and malicious-input coverage survive.

### 9. Reduce wasted work with measurements — one confirmed candidate plus profiling

`cdk-common/src/federation/operation.rs:2605` gives melts quote-only conflict keys; swaps return no conflict keys at line 2646. Prior review identified overlapping-spend ordering as resource amplification, not a database double-spend. It was deferred because keys are persisted in old journals. Cut-through removes that compatibility prerequisite.

Reassess proof-Y conflict keys for swap/melt, including canonical ordering, duplicate submissions, overlapping proof subsets, legitimate retries, and invalid submissions attempting to reserve honest proofs. Change admission, live ordering, persistence, suffix verification, and replay together only after this invariant is settled.

Profile representative federation workloads before further optimization: database query counts and rows scanned, allocations/serialization, lock wait/hold time, network fanout, idle polling, and status request cost. One inspected conversion worth removing is the typed Bark status -> JSON -> cloned typed status round trip used for metrics in `status_routes.rs:461`; record metrics before erasing the type at the response boundary.

Audit unbounded collection defaults and retained maps, repeated config/digest computation, full projection scans, and repeated request setup. Change only measured or directly demonstrated unnecessary work. Record before/after results at the same roster and workload; retain independent validation even when it costs CPU.

### 10. Rationalize constants, defaults, and API surfaces — mixed confirmed/candidate

Classify values by authority:

| Kind | Treatment |
| --- | --- |
| Cryptographic domain tags, encodings, quorum rules, derivation boundaries | Keep explicit and versioned; do not turn into operator knobs. |
| Consensus transaction fee/size limits | Keep one agreed policy authority. Local settings must not make members validate different rules. |
| Local timeouts, concurrency, retry intervals, queue limits | Centralize in validated runtime policy where operational tuning is useful; keep documented defaults. |
| Test ports, fixed rosters, fake delays, demo endpoints | Keep in fixtures or named deployment profiles. Generic runtime must not depend on a demo environment. |
| Dead settings and single-variant future placeholders | Remove after verifying callers and actual behavior; retain types that enforce a useful contract. |

Examples to reassess: `transaction_policy.rs:29` mixes consensus limits with local estimation behavior; wallet connector visibility fields at `wallet/federation.rs:101` use mint-specific names for reusable polling policy; `FederationWalletConnector` in `wallet/federation/validation.rs:10` is a zero-state public namespace for validators rather than a connector. Breaking federation-only APIs is allowed, so clearer naming and private functions are available without compatibility facades.

Acceptance: each retained setting has a reader and an explained effect; each safety constant has one authority; meaningful local limits are bounded; no configuration is added solely because a literal exists.

### 11. Reduce redundant tests while preserving distinct failure coverage — evidence-led

Inventory tests by the behavior they protect, layer, fixture cost, and dependencies. Coverage families include cryptographic vectors, malicious peer inputs, replay/nonces, SQL rollback, value conservation, concurrency, threshold completion, lifecycle failure, restart/catch-up, and operator contracts.

Immediate deletion candidates are acceptance tests for formats explicitly removed above, such as legacy Bark conflict behavior in `cdk-federation-bark/src/operation/tests.rs:602`. Replace them with one clear old-version rejection assertion where useful.

Review tiny wrapper tests such as acknowledgement-boolean checks in `federation_wallet_operator.rs:238` and `federation_bark_operator.rs:209`. Delete only when command/route coverage proves enforcement at the actual side-effect boundary. Consolidate repeated CLI parsing/formatting cases into tables when they test the same contract; preserve redaction and admission tests.

Move bulky fixtures into test support and parameterize true duplicates across backends or failure cases. Maintain an old-test -> retained-test mapping for deletions, including the assertion/failure each retained test detects. Use targeted mutation or fault injection for critical consolidations. Never delete a test just because it is long, slow, passes, or resembles another test at a different boundary.

Use virtual time for scheduler tests where clock passage is the subject; retain real process/network tests where scheduling, transport, or service shutdown is the subject. Fix redundant setup before removing valuable scenarios.

### 12. Finish tooling, dependency, binding, and documentation cleanup — proposed scope

`cdk-bark/Cargo.toml:43` explicitly calls its sibling-checkout dependency temporary. Resolve a reviewed, immutable, remotely fetchable Bark source and pin the dependency/lockfiles. The required revision must actually contain the federation integration; do not replace it with an arbitrary release. Verify a clean checkout can build without that sibling repository. This dependency decision may require the user's preferred upstream/fork if repository history does not resolve it.

`scripts/mobile-bindings/federation.rs` and `federation-ffi.patch` carry a second integration route outside normal FFI source. Determine whether the patch is still the supported path; integrate maintained federation exports into `cdk-ffi` or remove the stale overlay. Update binding checks for every affected Wallet API and conversion.

Consolidate common setup/start/status/restart logic across scripts and `justfile`; retain useful operator actions and named deployment profiles. Inventory docs, historical TODO/audit reports, rendered presentations, archives, benchmark outputs, and generated material. Establish a current architecture/operator guide and a small decision index, preserving still-relevant security rationale. Propose explicit archive/removal lists before bulk deletions; Git history can retain obsolete iterations.

Audit feature combinations and test utilities in Cargo and CI. The existing security fuzz workflow is manually dispatched; retain it and add deterministic decoder/corpus regressions to normal checks if absent. Use the existing Nix tooling and avoid introducing a second dependency/runtime setup.

## Execution sequence and review units

| Batch | Work | Depends on | Main validation |
| --- | --- | --- | --- |
| 0 | Record source/test/feature inventory, safety matrix, current warnings, representative timings | Plan approval | Reproducible baseline and explicit remaining uncertainties |
| 1 | Fix SQL conversion, RPC bootstrap, stale launchers; remove proven dead fields | 0 | Boundary regressions and actual CLI smoke |
| 2 | Current-only federation wire/storage/Bark forms and clean federation SQL | 0 | Both SQL backends, strict decoding, fresh replay/digests, backup/restore |
| 3 | Gate test helpers; remove unused consensus placeholder; simplify operator/status interfaces | 2 where wire shape changes | Production/test feature graphs, CLI/route enforcement |
| 4 | Explicit test fixture modules and redundant-test consolidation | 0–3 | Test ownership mapping; existing failure scenarios retained |
| 5 | Supervised worker lifecycle | 1 and fixture support | Failure, cancellation, restart, no duplicate external effects |
| 6 | Domain module boundaries, thin routes, focused Bark/DKG cleanup | 4–5 | Move-only comparisons followed by focused state-machine checks |
| 7 | Conflict-key improvement and measured performance work | 2, 6 | Invalid-input reservation tests, live/replay equality, controlled benchmarks |
| 8 | Final policy/API cleanup, reproducible dependency/bindings, tooling/docs | Throughout; finish after interfaces settle | Clean-checkout build, feature matrix, operator flow |

Use small conventional commits with one understandable purpose. Keep mechanical moves separate from changed semantics. Each batch reports what was removed, what became simpler, evidence from checks, and remaining items; update this plan's dispositions rather than creating another competing TODO document. Do not publish, deploy, or reset local state as part of cleanup.

## Validation matrix

Previous rebase verification reported a passing workspace check and 1,355 selected tests. That is useful history, not evidence that this whole cleanup is already tested. The broader federation surface, actual launch scripts, PostgreSQL behavior, and production feature isolation require their own checks.

| Surface | Required evidence when touched |
| --- | --- |
| Protocol/BLS/FROST | Existing vectors, negative decoding, signer/nonce/authorization tests, canonical bytes; bounded fuzz/corpus checks |
| Domain and consensus | Acceptance/rejection, duplicate/overlap, threshold behavior, current-version replay and identical final state |
| Database and recovery | SQLite + PostgreSQL, atomicity/rollback, concurrency, restart/catch-up, checkpoint and backup/restore invariants |
| Mint/wallet integration | Mint/swap/melt/restore, ambiguous failure handling, change recovery, value conservation, standalone behavior |
| Runtime | Worker startup failure, bounded shutdown, panic/error visibility, stopped/restarted members, no duplicated payment/broadcast authority |
| Transport | HTTP and Iroh, authentication/sequencing, bounded requests, timeout/cancellation, quorum behavior with bad/stalled peers |
| Features/bindings | Federation disabled; core federation; BDK; Bark; combined features; production without test utilities; supported FFI and WASM builds affected by changes |
| Tooling | Fresh config generation/init/start; slow and mismatched peers; operator commands; shell syntax plus real local process smoke |
| Quality/performance | Formatting, applicable clippy, workspace all-target check, no new warnings, dependency/feature inspection, controlled before/after measurements |

Run relevant checks per batch and a full selected integration matrix at the end. Avoid recompiling the entire workspace after every file move. Do not waive a failed check or weaken an assertion to make a refactor pass; report environmental prerequisites separately from code failures.

## Completion criteria

- Every inventoried area has a recorded disposition: changed, retained with a reason, or blocked by a named external prerequisite.
- No old-federation compatibility reader, duplicate persisted representation, or obsolete launch path remains in the chosen current design.
- Development helpers and simulated behavior are absent from ordinary production builds.
- Worker ownership, transition authority, and module interfaces are explicit and reviewable.
- Removed tests have a coverage explanation; distinct safety and real-boundary tests remain.
- Fresh databases and current-version restart/replay work across both SQL backends; standalone upgrades remain supported.
- Retained constants/settings have clear authority and use. Performance claims include measurements.
- Supported builds and operator flows work from a reproducible checkout, with current documentation.
- Report production/test LOC, dead API/branch removals, test runtime and retained coverage, and warning changes separately. No arbitrary deletion quota or claim of being bug-free.

## Review decisions

The cut-through compatibility policy and implementation, including tooling/document cleanup, were authorized by the user. The ledger below records the resulting dispositions.

Useful operator tools, security rationale, and historical reports/presentations are retained. No bulk archive/deletion was performed. The user authorized publishing the tested Bark revision to a new branch in the existing fork on 2026-09-07; existing remote branches remain intact.


## Implementation ledger

Baseline remains `3367b77c1`; all results below describe the cleanup relative to that baseline, not a deployment. The current architecture/operator guide is [federation.md](federation.md). Local payment validation is complete; the final remote-dependency build verification is recorded below.

| Package | Disposition |
| --- | --- |
| 1: Formats | Changed: strict current status and pending-envelope records, Bark v2 only, required certified receive claims, one allocation collection, one v6 envelope for every operation, explicit chain-observation epochs. Removed legacy receive hashes and standalone HTLC-ready replay. |
| 2: Schema | Changed: folded federation-only backup/index evolution into both creation schemas and removed the single-VTXO column. Upstream migration contents/checksums are preserved. |
| 3: Boundaries/startup | Changed: checked member-ID conversion, shared timeout-aware RPC bootstrap, effective shared sync policy, generated-member config init/apply launcher. The wider SQLite suite also found and fixed a missing upstream `last_checked` quote column. Normal, slow-peer, and mismatched-member process scenarios pass. |
| 4: Test isolation | Changed: trusted dealer requires explicit test support; unused consensus Default item removed; test-only wallet path retention gated. Production feature compilation also exposed and corrected narrow BDK import and test-output-helper gates. |
| 5: Operators/status | Changed: shared validated config loading, target selection, fallible monotonic process-local nanosecond sequences, typed digest policy, metrics before JSON conversion. Receiver authentication/replay checks remain mandatory. |
| 6: Workers | Changed: tracked worker/child ownership, observed panic/failure, cancellation between rounds, bounded graceful shutdown, startup-error cleanup, and AlephBFT shutdown. No automatic external-action restart. |
| 7: Modules | Changed: ordinary mint modules and Axum test modules; extracted custom-handler tests; Bark incoming/outgoing/maintenance/on-chain-recovery modules. Shared fixtures stay private to the test parent. |
| 8: DKG/signing | Changed: removed duplicate FROST phase enum and obsolete receive branches. Profiling found activation validation repeating whole-config crypto per member and status reconstructing an already-validated final config; reuse now stays within that same trust boundary. Independent signer proposal validation, nonce discipline, quorum evidence, and replay checks are retained. |
| 9: Efficiency | Changed: removed the Bark status JSON clone/decode round trip; bounded default wallet fanout by the supported roster maximum; shared sync configuration now has a reader. Retained proof conflict behavior: finalizer authorization/pure validation cannot safely reserve proof-Y identifiers before proof verification. The controlled DKG-status timing comparison is recorded below. |
| 10: Policy/API | Changed: generic quote-visibility naming across wallet/CLI/TUI/smoke, removed the zero-state public validator facade. Retained versioned fee/weight policy: it is a deterministic contract, including proposer estimate/fallback behavior; splitting it into competing local authorities would add complexity. Named fixture/deployment values remain in their profiles. |
| 11: Tests | Changed: obsolete compatibility acceptance/conversion tests removed or converted to rejection cases; explicit fixture ownership; simple compiler-proven redundant borrows/clone/wrapper code reduced. Distinct malicious-input, value, recovery, and concurrency scenarios retained. Ack tests retained because current route tests do not substitute for command-side acknowledgement enforcement. |
| 12: Tooling/docs | Changed: integrated federation FFI source and removed overlay patch/copy, added Nix feature groups and normal CI decoder smoke, current guide and history index. Historical reports/presentations are retained; no bulk deletion. Bark is pinned to the reviewed immutable revision in the existing fork; isolated-source workspace and combined binary builds pass without a sibling checkout. |

### Test deletion/replacement map

| Removed/changed coverage | Retained coverage and reason |
| --- | --- |
| SQLite `federation_aleph_bft_backup_chunk_migration_preserves_existing_bytes` | Fresh chunked append/read/restart, backup restore, standalone table preservation, and transaction rollback tests. Old federation upgrade conversion is unsupported. |
| Bark v1 acceptance/conflict assertions | Current v2 conflict tests plus explicit v1 rejection. |
| Legacy receive hash acceptance | Current certified package validation, missing-certificate rejection, claim admission, accepted-plan replay and evidence mismatch tests. |
| Flattened local-status format compatibility | Derived current-map round trip, explicit old-form/unknown-field rejection, real status routes including empty wallet lists. |
| Future Default consensus item version test | Future module-fact version rejection and canonical consensus/checkpoint vectors, using an actual bounded item. |
| `legacy_mint_schedule_migrates_into_the_shared_namespace`; `version_two_schedule_migrates_without_losing_backoff` | Current persisted backoff/lease/restart/scan-cursor tests and unsupported-version cache rebuild table. No old schedule migration. |
| Legacy epoch omission assertions | Explicit epoch-zero serialization plus differing-epoch conflict key and operation-ID assertions; current state/digest round trips. |

No acknowledgement or crypto/nonce/replay safety tests were deleted solely for size, similarity, or runtime. Fault injection covers worker panic, failure visibility, stalled work at shutdown, and startup failure after workers begin. Ordered spent/pending proof conflicts now have an explicit classification distinct from wallet proof-release policy; losing swaps reject and replay continues.

### Verification ledger

- Bitcoin wallet all features: **40 passed**, including bootstrap timeout and sync-policy boundaries.
- Common protocol: **484 passed, 1 ignored** after current envelope/epoch changes. Current operation/journal vectors and unchanged genesis/materialized vectors are checked.
- SQL common: **19 passed**, including upstream migration checksum lock. SQLite core federation: **185 passed**; with Bark: **187 passed**. PostgreSQL with Bark: **171 passed**. The final next-candidate lookup assertions passed separately on both backends.
- Crypto/custody crates: **91 passed** across Bark, federation Bark, FROST, FROST Bark, and FROST BIP32.
- Main CDK combined federation suite: **957 passed**. Mint daemon: **160 passed**.
- Final Axum suite with BDK/Bark/Iroh/Prometheus: **586 passed, 0 failed, 5 ignored** in **1,055.10 seconds**, run serially without competing regtest/build load. The original seven-member latency bound passes unchanged. A cross-test global metrics assertion was replaced with a direct assertion of zero historical-signer requests. This full run includes delayed/out-of-order checkpoint promotion, worker failure/shutdown, consensus restart, DKG, signing, and payment recovery regressions.
- Final feature matrix: federation disabled, core federation, BDK only, Bark only, FFI federation, and workspace all-targets all pass. The three narrow daemon feature builds also pass strict clippy. The workspace check includes Swift, Kotlin, Dart, and Go binding crates. FFI exports are gated at the exported implementation boundary, because UniFFI expands method attributes before per-method feature elimination.
- Wallet-only WASM check passes with the existing Nix stable toolchain; host-only C hardening flags were disabled for the WASM target.
- Both bounded security/FROST fuzz targets pass **1,000 seeded runs each**. Nix formatting and typos pass. Final combined all-target clippy passes with warnings denied for CDK; the external Bark dependency retains three existing warnings. All **140 changed/new CDK Rust files** pass formatting. The full workspace formatting check also passes after the Git pin removed the sibling Bark checkout from Cargo's local workspace discovery.
- Fresh three-member fake-backend CLI smoke passes mint, ecash transfer, double-spend rejection, melt/change, restore, proof checks, and final checkpoint quorum convergence. Slow-member and mismatched-member final process runs also pass.
- Full Nix regtest shell is unavailable on this macOS host because its CLN dependency failed to build. Local Docker regtest uses task-owned Bitcoin, PostgreSQL, two CLN nodes, and Ark, with a dealer-free three-member BLS/FROST federation and fresh test funds.
- Bark revision [`8d1eda643dd11e9eb600ec44e36622c8f77b6322`](https://github.com/callebtc/bark/commit/8d1eda643dd11e9eb600ec44e36622c8f77b6322) was published, with user approval, to the new `codex/frost-wallet-cdk-20260907` branch in the existing fork. Existing branches remain intact. Cargo fetched that exact commit from GitHub. The manifest and lockfile now use the immutable Git source for all six Bark packages, including `bip321`, with no package version changes.
- Remote-dependency verification uses an isolated copy of all 1,602 source files with no sibling Bark checkout. Cargo metadata confirms that all local packages resolve within that copy and all six Bark packages resolve to the pinned Git revision. The locked offline workspace all-target check passed in **167.24 seconds**. The locked offline mintd/CLI build with BDK, Bark, and Iroh federation support passed in **114.57 seconds**. The existing compiled-dependency cache was reused; this does not claim a cache-empty build. The manifest/source change uses the same Bark commit as the completed payment tests; no payment implementation or package version changed during pinning.

### Regtest-driven corrections

1. On-chain melt change can contain fewer signatures than requested blank outputs. Wallet aggregation now accepts a valid prefix for change while retaining exact counts for ordinary signing/restore and all metadata, amount, keyset, cryptographic, and threshold checks. A regression also verifies recovery returns the same change signatures.
2. Scheduled checkpoint capture used quorum arrival as the next-boundary clock. Members receiving signatures at different times could select different checkpoints. Capture now extends the deterministic candidate lineage; ancestors remain available for signature exchange. A healthy member also promotes locally available quorums in sequence when no peer has newer operations. Tests cover delayed and out-of-order delivery, retained ancestors, and promotion without new network data.
3. An accepted Bark payment could be prevented from progressing by local recovery degradation. Already accepted actions now use the caught-up recovery gate; new plans still require readiness. Wallet/projection/participant failures remain excluded.
4. The outgoing actor retried initiation before checking terminal status, even when another selected member had already paid the invoice. All selected observers now poll the exact accepted payment first and initiate only on a typed missing result. The previously stuck transfer finalized, and a fresh 1,000-sat send completed automatically in 74.17 seconds, including wallet startup/recovery, under concurrent build load.
5. Private Bark evidence wakes, checkpoint recovery requests, and the AlephBFT replay-readiness watcher are owned by the federation task group. The readiness watcher is cancellable. Setup shutdown registers its idle notification before reading the active-task count to avoid missing the last completion.

### Local payment evidence

| Direction | Result |
| --- | --- |
| Incoming on-chain | 100,000 sat initially and 40,000 sat in the final run confirmed and issued as ecash. |
| Outgoing on-chain | Two 10,000-sat withdrawals confirmed at the recipients. The second CLI melt succeeds with correct change. Confirmation timing includes explicit manual block mining. |
| Incoming Lightning | 200,000 sat initially and 40,000 sat in the final run paid and issued. One separate invoice failed because a Lightning node lagged a newly mined block; it was cancelled and never issued. |
| Outgoing Lightning | Two 10,000-sat payments received and finalized. The second required process restart recovery; the wallet recovered its pending operation. Two further 1,000-sat payments received and finalized; the last completed without process restart. |
| Outgoing ecash | Wallet A sent 700 sat to B; B received it. |
| Incoming ecash | Wallet B sent 300 sat back to A; A received it. |

Final wallet balances are **334,566 sat** and **2,400 sat**, with zero pending proofs. Issuance was **380,000 sat**; external outgoing principal was **42,000 sat** and paid fees were **1,034 sat**; the remaining **336,966 sat** exactly equals the two wallet balances. All three members report the same 110-operation journal and application digests, healthy readiness, and paid quorum evidence for the completed payments. Process restart and replay preserved the current database and recovered pending wallet operations.

The final source was rebuilt into isolated binaries and all three members restarted from these same databases. Readiness, convergence, custody status, wallet balances, and proof checks all passed again: **65 + 30 unspent proofs, zero pending proofs**, no payment conflicts/unresolved observations, and no digest alerts. The latest scheduled quorum equals the trusted checkpoint on every member, with zero checkpoint lag. Spendable backing is **119,266 sat on-chain + 217,700 sat in Bark = 336,966 sat**, matching outstanding ecash. The local regtest environment remains running for follow-up inspection.

### Measured work reduction

At the same three-member, 64-denomination debug DKG-status workload, removing repeated validation of the same finalized configuration reduced elapsed time from **314.412 s to 151.445 s** (about **52%**). Each measurement was one run; both produced identical configuration/transcript digests. This is a measurement of that command and workload, not a general throughput claim.

Physical Rust line accounting across changed, new, and removed files (including comments/blanks, with test/benchmark files and inline test modules classified separately): source **122,501 → 122,173 (−328)**; tests/benchmarks **100,719 → 100,835 (+116)**. Mechanical module moves cancel across the totals. The cleanup removes obsolete APIs/branches and duplicated ownership; it does not treat moving a large test module as deleting tests.

Removed production surfaces include the compatibility status serializers, bare-envelope storage decoder, Bark v1 branch and uncertified receive branch, single-VTXO allocation column, legacy schedule converters, unused consensus `Default` item, duplicate mint-mode flag, unused router startup variants and pass-through functions, observation constructor wrappers, and the zero-state wallet validator facade. Six obsolete federation migration files and the mobile patch/copy overlay are removed. Safety-domain constants, independent signer validation, and deployment profiles remain intentional.
