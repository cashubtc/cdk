# Federation cleanup & refactor implementation plan (2026-07-23)

This plan sequences the cleanup of the federation branch (`bls-federation`, forked
from `bls`) so the diff becomes smaller, follows standalone CDK conventions, and is
easier to review. It was produced from a five-way code review of `git diff
bls...HEAD` (~233k lines in `crates/`, 334 files).

Relationship to `docs/federated-cashu-implementation-plan.md`: this plan implements
the "prefer simplification and reuse before adding parallel implementations" and
"avoid scattering federation conditionals through standalone mint, wallet, HTTP,
storage, or signatory code" principles from that contract. Every change here is a
refactor behind existing behavior; no protocol, wire-format, or consensus-safety
semantics change intentionally.

## Out of scope (deferred, tracked separately)

- Removal/split of the bark feature surface (`crates/cdk-bark`,
  `crates/cdk-frost-bark`, `crates/cdk-axum/src/federation/bark_*`,
  `federation-bark` feature, `FederationBark*` types in `cdk-common::federation`).
  Agreed as desirable but explicitly deferred. Where a step below touches bark code,
  prefer the minimal change and note that the code is slated for removal.
- Removal of `scripts/` tooling (fed-cdk-tui, mobile-bindings, nocean scripts) and
  pruning of docs artifacts (`nocean-invite-code.txt`, rendered HTML, dated
  TODO/prompt files).
- Splitting standalone-visible behavior changes (melt `Unpaid` ambiguity,
  `load_wallets` discovery removal, `retry_on_inactive_keyset`, metadata-cache
  replace-not-merge, proof-selection rewrite) into a precursor PR against `bls`.
  Recommended but requires maintainer sign-off on history surgery.

## What is already good (do not regress)

- Wallet layering: zero federation branches in sagas; `FederatedMintConnector`
  implements the existing `MintConnector` trait; apps inject it via
  `WalletBuilder::mint_connector`. The mint side should copy this pattern.
- Feature gating: `federation` / `federation-bark` are opt-in; nothing federation
  compiles by default.
- Conventions: zero `.unwrap()` outside tests, full-path `tracing::` macros, doc
  comments on pub items, paired sqlite/postgres migrations enforced by tests,
  wire types centralized in `cdk-common::federation`.

## Working rules

- Conventional commits; one commit per numbered step where feasible so phases can
  be reviewed and reverted independently.
- After every step: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`.
- After every phase: `cargo check --workspace --all-targets`, plus
  `cargo check -p cdk-axum --features federation` and
  `cargo check -p cdk --features federation` (federation-enabled and standalone
  configurations must both build).
- State-affecting steps (Phases 3-5) also require:
  `cargo test --lib --workspace --exclude cdk-postgres` and the federation
  integration tests (`--test-threads 1`).
- Never weaken assertions or increase timeouts to make a refactor pass.

---

## Phase 1 — Structural quick wins (mechanical, low risk)

Goal: remove convention violations and duplicated micro-helpers before touching
semantics, so later diffs are small and readable.

### 1.1 Replace `include!()` with real modules

- `crates/cdk/src/mint/federation/mod.rs:13-15` uses
  `include!("configuration_and_admission.rs")`, `include!("application.rs")`,
  `include!("helpers.rs")`; the axum federation test harness does the same in
  `crates/cdk-axum/src/federation/tests/mod.rs:110-115`.
- Convert to `mod x;` with explicit `pub(crate)` visibility per AGENTS.md. The
  `include!()` trick hid duplicate definitions from the compiler (see 1.2).

### 1.2 Consolidate triple-defined helpers

- `federation_operation_uuid` is defined three times:
  `crates/cdk/src/mint/federation/helpers.rs:1`,
  `crates/cdk/src/mint/federation/projection.rs:2813`, and
  `crates/cdk/src/mint/melt/melt_saga/mod.rs:484`. Keep exactly one (in
  `mint/federation` or `cdk-common::federation`) and import it.
- Share→`BlindSignature` conversion exists three times:
  `crates/cdk/src/mint/mod.rs:80-91`
  (`federation_signature_share_to_blind_signature`),
  `crates/cdk/src/mint/federation/helpers.rs:359`
  (`federation_share_signatures`), and inline in `prepare_federation_melt_change`.
  Keep one.

### 1.3 Move the inline test dump out of custom_handlers.rs

- `crates/cdk-axum/src/custom_handlers.rs` is ~10,160 lines, of which the inline
  `#[cfg(test)] mod tests` spans lines 1167-10160 (~9,000 lines, 90 tests) next to
  a dedicated 32k-line `federation/tests/` directory.
- Move the tests into `crates/cdk-axum/src/federation/tests/` (merge with the
  existing suite); leave only production handler code in `custom_handlers.rs`.

### 1.4 Eliminate `#[allow(dead_code)]` on shipped code

- `crates/cdk-axum/src/federation/frost_signing.rs:89` (`CoordinatedFrostSignature`)
  and `:253` (`sign_accepted_transaction`): wire them into the signing flow or gate
  them; shipping dead coordinators is not acceptable.
- `crates/cdk-axum/src/federation/bark_secret.rs:24,29`: bark-slated-for-removal;
  leave or gate behind `federation-bark`, do not invest further.

### 1.5 Shared utility module

Create `crates/cdk-axum/src/federation/util.rs` (or push into `cdk-common` where
cross-crate) and replace all local copies:

- Saturating conversions: `usize_to_u64_saturating` defined identically in
  `frost_signing.rs:54` and `status_routes.rs:536`; ~36 scattered
  `u64::try_from(...).unwrap_or(u64::MAX)` sites.
- Unix-time helpers: `current_unix_time`/`current_unix_time_ms`
  (`payment_observation.rs:1074,1081`), `unix_time` (`bark_incoming.rs:1235`),
  `current_unix_time_seconds` (`bark_maintenance.rs:628`).
- One peer fan-out helper for the hand-rolled
  `join_all(members.iter().map(...))` signature/share collections at
  `frost_signing.rs:153,195,237`, `dkg_driver.rs:516`, `bark_secret.rs:59`,
  `aleph_bft_session.rs:1211`, and the `FuturesUnordered` peer-attempt loops at
  `sync_loop.rs:231-236` and `:403-418`.
- One bounded retry/sleep helper for the local retry loops at `sync_loop.rs:83`,
  `dkg_driver.rs:670/1443/2286`, `aleph_bft_session.rs:1944`,
  `aleph_bft_runtime.rs:1395`, `payment_observation.rs:856`.

### 1.6 DKG route prologue dedup (~-300 lines)

- `crates/cdk-axum/src/federation/dkg_routes.rs` hand-rolls the same 6-step
  admission sequence 10 times (e.g. `post_dkg_readiness` at :74-111):
  `acquire_private_rest_admission` → `verify_federation_request` →
  `acquire_private_dkg_member_admission` → `active_dkg_config` →
  `federation_runtime_config` → `record_federation_sequence`.
- Nine other route files already use the consolidated `admit_private_request`
  helper (`private_request.rs:42`). Add a generic `admit_dkg_request` wrapper on
  top of it and delete the copies.

### 1.7 Hygiene

- Rewrite stale docs on `MintBuilder::with_federation_mode` /
  `with_federation_config` (`crates/cdk/src/mint/builder.rs`) — they claim to be
  "intentionally inert until later federation milestones" but are fully wired.
- Remove the test-only `federation_effective_config_rebuild_count` atomic from the
  production `Mint` struct (`crates/cdk/src/mint/mod.rs:129-130`); track it inside
  the effective-config cache under `#[cfg(test)]`.
- Fix double invoice parse: `prepare_bolt11_mint_quote_with_invoice`
  (`crates/cdk/src/mint/issue/mod.rs:265-310`) parses and validates the invoice,
  then `create_mint_quote_with_incoming_response` (:473-503) re-parses it to
  re-derive the payment hash. Parse once, pass the hash structurally.

**Gate:** full workspace check/clippy/fmt; `just test`.

---

## Phase 2 — Wallet connector cleanup (self-contained)

Goal: make `crates/cdk/src/wallet/federation.rs` (2,779 lines) match the quality
of the saga layer it serves. No behavior change except 2.1, which is a bug fix.

### 2.1 Bug fix: `get_mint_keyset` aborts the federation on one bad member

- `crates/cdk/src/wallet/federation.rs:2130-2136` uses `?` on member-validation
  failures inside the success arm: the first member with mismatched mint
  info/keyset fails the whole call even if honest members would reach threshold.
  Everywhere else (:851, :854) invalid members are recorded as failures and
  skipped, per the connector's own invariant (comment at :62-64).
- Count as member failure and continue. Add a test: one Byzantine member +
  threshold honest members must still succeed.

### 2.2 One fan-out scheduler

- `collect_member_results_from` (:618-734) is the generic scheduler, but the same
  loop is re-implemented inline in `validate_member_active_keysets_with_mint_info`
  (:787-916 — including a verbatim-duplicated validation body for the early-return
  drain, cf. :832-855 vs :858-891) and in `get_mint_keyset` (:2112-2144 — a third
  unbounded loop that ignores `fanout_concurrency`, `member_request_timeout`, and
  `member_request_max_retries`).
- Express both as `collect_member_results_from` with per-member closures and a
  `stop_after` predicate; the drain-on-stop logic already exists at :713-727.

### 2.3 One generic quote create/verify path

- `create_mint_quote` (:1225-1268) vs `create_melt_quote` (:1270-1313), and
  `verify_mint_quote_visible_to_threshold` (:1199-1223) vs
  `verify_melt_quote_visible_to_threshold` (:1340-1364) are near-verbatim pairs;
  all four overlap `call_first_healthy_member_from` (:537-576).
- Introduce one generic `create_quote_with_visibility_check<Req, Resp>`
  parameterized over the per-member call and response validator.
- Rename the `FederatedMintConnectorConfig` fields `mint_quote_visibility_attempts`
  / `mint_quote_visibility_retry_delay` (:89-92) to `quote_visibility_*` (the melt
  path already reuses them). Cut-through compatibility policy allows this.

### 2.4 Split the module

Split `federation.rs` into `federation/mod.rs` (struct, constructors, config),
`federation/fanout.rs` (2.2, 2.3), `federation/aggregate.rs` (share
verification/aggregation, :1395-1933), `federation/connector.rs` (the
`MintConnector` impl, :2057-2479); keep `validation.rs`. Also:

- Demote the test-only facade methods `FederationWalletConnector::
  validate_member_mint_info` (`validation.rs:14`, zero non-test callers) and
  `validate_member_keysets` (:45, test-only) to `#[cfg(test)]` or delete; stop
  re-exporting `FederationWalletConnector` from `wallet/mod.rs:93-96`.
- Add doc comments to `resolve_dns_txt` / `fetch_lnurl_*` (:2059-2090) noting they
  proxy through an arbitrary member's HTTP client and are not mint operations.

### 2.5 Decouple perf recording from the federation module

- Standalone files import federation internals under `#[cfg(feature =
  "federation")]` purely for metrics: `issue/saga/mod.rs:901-906`,
  `receive/saga/mod.rs:271-277`, `wallet/mod.rs:963-970`,
  `blind_signature.rs:103-109` (with a triplicated cfg-gated `use
  std::time::Instant;`).
- Move `record_wallet_performance_stage` / `FederationWalletPerformanceStage` into
  an always-compiled `wallet::metrics` (or `wallet::perf`) module; federation only
  adds stages. Removes the inverted dependency and the cfg islands in standalone
  BLS verification paths.

### 2.6 Deferred design note (not this plan)

`melt_change_outputs` (:65-67) keeps an in-memory copy of data the melt saga
persists durably, so post-restart recovery falls back to the weaker "interpolate
tolerantly" path (:1850-1908). Recorded here as a known limitation; a proper fix
(passing original outputs through the connector/status call) is a behavior change
and out of scope.

**Gate:** `cargo test -p cdk --features federation`; wallet federation unit tests.

---

## Phase 3 — Mint domain dedup (the big equalization)

Goal: eliminate the forks where federation re-implements standalone Cashu business
logic, per the design contract's "reuse existing CDK validation, quote handling,
proof checks, state transitions". Expected removal: several hundred lines.

### 3.1 One implementation of input/output/balance/fee verification

Duplicated pairs (standalone ↔ federation):

- `Mint::verify_inputs` (`crates/cdk/src/mint/verification.rs:197-256`) ↔
  `verify_federation_inputs_with_config_inner`
  (`crates/cdk/src/mint/federation/application.rs:2506-2567`) — near line-for-line
  copy (max-inputs, content bounds, uniqueness, per-keyset expiry, single-unit,
  auth-unit rejection, total).
- `verify_outputs` (:163-190) ↔ `verify_federation_outputs_with_config`
  (application.rs:920-970).
- `verify_transaction_balanced` (:260-293) ↔
  `verify_federation_transaction_balanced_with_config` (application.rs:2569-2601).
- `get_proofs_fee` (`crates/cdk/src/mint/proofs.rs`) ↔
  `get_federation_proofs_fee_with_config` (application.rs:1026-1054).
- Inline secret/witness bounds check (verification.rs:212-242) ↔
  `validate_proof_content_bounds`
  (`configuration_and_admission.rs:789-822`, same `MAX_PROOF_CONTENT_LEN`).

Refactor: make the standalone functions generic over a keyset lookup (e.g. a
`KeysetSource` trait with impls for the signatory cache and `FederationConfig` —
the only real difference is the keyset source: `self.keysets` ArcSwap vs
`config.public_config().keysets`). Extract the content-bounds loop into one
`pub(crate) fn` in `verification.rs`. Both modes call the shared implementation.

Also merge the near-copy pair *within* federation:
`verify_federation_outputs_with_config` vs
`verify_federation_change_outputs_with_config` (application.rs:972-1024) differ
only in `validate_blinded_output` vs `validate_blinded_change_output` — one
function taking the per-output validator as a closure.

### 3.2 Unify melt change and internal settlement

Three forks in `crates/cdk/src/mint/melt/shared.rs`:

- `prepare_federation_melt_change` (~:462-560) re-implements
  `process_melt_change` (:307-388): identical change-target arithmetic,
  fee-too-high bail, `split`, sort-descending fallback, amount assignment. Only
  the signer differs (`mint.blind_sign` vs
  `config.private_config().sign_blinded_messages`) plus share-record persistence.
- `get_federation_keyset_fee_and_amounts` (:61-87) copies
  `get_keyset_fee_and_amounts` (:42-58) including the silent
  `(0, 2^0..2^31)` fallback.
- `finalize_federation_internal_melt_quote` (~190 lines) re-implements standalone
  internal settlement: mint-quote lookup, `RequestAlreadyPaid`,
  `InsufficientFunds` (copied from `MeltSaga::attempt_internal_settlement`,
  `melt_saga/mod.rs:647-732`), proof spend, quote→Paid, completed-operation
  record, saga delete (copied from `finalize_melt_quote_inner`).

Refactor: extract `compute_change_split(...)` and an internal-settlement check
helper used by both modes; parameterize signing as a closure returning signatures
plus optional share records. Note the federation copy already diverges
stylistically (early `tx.rollback()` before each error return) — the shared
version must pick one transaction discipline.

### 3.3 One finalize entry point

- The `finalize_melt_quote` vs `finalize_federation_melt_quote` if/else fork is
  written identically in three places:
  `crates/cdk/src/mint/melt/melt_saga/mod.rs` (~:1132-1170),
  `crates/cdk/src/mint/saga_recovery.rs:82-118`,
  `crates/cdk/src/mint/start_up_check.rs:106-145`.
- Meanwhile `finalize_melt_quote_inner` itself carries ~10 `#[cfg(feature =
  "federation")]` / `#[cfg(not(...))]` islands (fee breakdown, provenance lookup,
  change processing, `federation_completed_at`, fault-injection hook).

Refactor: single entry `finalize(mint, ..., federation_operation_id:
Option<FederationOperationId>)`; all three call sites pass the saga's parsed id.
Inside, hoist the mode decision once (compute fee breakdown, change signatures,
completed-at in a small mode-specific prelude) instead of interleaving cfg blocks.

### 3.4 Keyset view consistency (correctness fix)

- `keyset_pubkeys`/`pubkeys`/`keysets`/`keyset` (`crates/cdk/src/mint/keysets/
  mod.rs:82-180`) and `get_active_keysets`/`get_keyset_info` (`mod.rs:1240-1281`)
  read the static startup `federation_config()`, while
  `effective_federation_keyset_pubkeys`/`*_pubkeys`/`*_keysets` (:16-77) read the
  replay-derived `effective_federation_config()`. After a `KeysetRotation`
  operation the NUT-02 endpoints and the federation effective view disagree;
  restore (:1433-1442) uses effective while `get_keyset_info` uses static.
- `federated_keyset_to_mint_keyset_info` (keysets/mod.rs:258-277) fabricates
  `valid_from: 0` and `DerivationPath::master()` — placeholder metadata flowing
  into `get_keyset_info`/`restore` responses.

Refactor: route every NUT-02 read through `effective_federation_config()`; give
`FederatedKeyset` real `valid_from`/derivation metadata or change the adapter to
omit fields it cannot source honestly. Add a rotation test asserting endpoint
consistency.

### 3.5 Move DKG ceremony reconciliation out of MintBuilder

- `activate_federation_dkg_records_if_complete` (`crates/cdk/src/mint/builder.rs:
  160-296`, ~140 lines) does durable DKG reconciliation (lifecycle-record
  filtering, share retention by ceremony id, transcript-signature quorum waiting,
  activation validation with string-matching on error reasons) inside mint
  construction code.
- Move to the federation module as e.g.
  `FederationRuntimeConfig::activate_from_durable_dkg(...)`; builder calls one
  line. Replace error-string matching with structured error variants.

**Gate:** mint unit tests, `just test`, federation integration tests (mint, swap,
melt, restart/replay) — this phase touches state transitions.

---

## Phase 4 — MintMode isolation

Goal: one explicit mode abstraction on `Mint` instead of federation conditionals
scattered through standalone paths. Modeled on the wallet side, which already
achieved this via the connector trait.

Federation conditionals currently live in at least these standalone sites:

- `crates/cdk/src/mint/mod.rs:375-385` (skip melt-saga recovery in member mode),
  :871-875 (payment-event stream divert), :666-683 (`mint_info` federation field),
  :1240-1281 (keyset early returns), `restore()` (:1350-1455 — including a
  textually duplicated `get_blind_signatures` block under both cfg arms).
- `issue/mod.rs:778-790` (`query_backend` bool), `melt/mod.rs:966-968`
  (`reconcile_backend` bool), `subscription.rs:30` (`query_backend` on
  `MintPubSubSpec` + boolean constructor param at `mod.rs:306-320`).
- `builder.rs:804-812` (skip standalone keyset rotation for federated units).
- `Mint` gains 6 federation fields (`mod.rs:120-137`) and ~35 methods spread over
  `impl Mint` blocks in `application.rs`, `projection.rs`,
  `transaction_state.rs`, `address_allocation.rs`, `chain_observation.rs`.

Steps:

1. Introduce `enum MintMode { Standalone, Federation(Arc<FederationContext>) }`
   on `Mint`; `FederationContext` owns the config, effective-config cache,
   payment-events sender, and active-melt-payments set. Collapses 6 fields to 1.
2. Give query-style methods (`get_keyset_info`, `get_active_keysets`, `pubkeys`,
   `keysets`, `keyset`) a single `keyset_source()` dispatch (builds on 3.1/3.4)
   instead of five inline early returns.
3. Collapse the boolean flags (`query_backend`, `reconcile_backend`, pubsub flag)
   into mode checks at one call site each; remove the new constructor params.
4. Move the ~35 federation `impl Mint` methods onto `FederationContext` (or
   free functions in `mint/federation/`) so `Mint` itself stops growing.
5. Keep the existing `reject_standalone_federation_path` guards
   (`issue/mod.rs:832`, `melt/mod.rs:1057`, `swap/mod.rs:24`, `mod.rs:1304`) —
   they are the cleanest pattern in the branch and should remain the model.

**Gate:** standalone mint test suite must pass unchanged (behavior-neutral for
standalone); federation integration tests.

---

## Phase 5 — cdk-axum layering

Goal: HTTP crate returns to thin adapters; consensus/DKG/runtime logic lives in
`federation/`; business validation lives in `cdk::mint`.

### 5.1 Empty lib.rs of federation implementation

`crates/cdk-axum/src/lib.rs` grew ~3,929 lines (~150 → 4,080). Move into
`federation/`:

- State structs: `FederationDkgAdmissionState` (:181),
  `FederationInvalidOperationAlertTracker` (:378),
  `FederationOperationNotifications` + state/stats (:561-618),
  `FederationSyncState` (:1070), 13 type aliases (:68-93), retry constants
  (:96-104).
- Consensus/journal verification: `catch_up_response_suffix_for_local_journal`
  (:828), `aleph_bft_finalized_item_suffix_for_local` (:876),
  `ensure_catch_up_entries_match_recovered_journal` (:919),
  `historical_entries_referenced_by_finalized_items` (:2807) — merge with
  `federation/checkpoint_catch_up.rs`'s mandate.
- ~60 federation methods on `MintState` (:1488-2160).
- Runtime wiring: `create_federation_runtime_state_with_wallet` (:2958, 347
  lines), `reconcile_federation_bark_runtime` (:3308, 286 lines, bark-deferred —
  move as-is, do not invest), `create_mint_router_with_custom_cache_inner`
  (:2519, 269 lines).

Target: lib.rs returns to router construction only (~500 lines).

### 5.2 Collapse combinatorial router constructors and wrapper chains

- Five nested public router constructors (:2359, :2392, :2413, :2436, :2466):
  `create_mint_router_with_custom_cache[_and_federation_peer_sync_config]
  [_and_federation_dkg][_and_wallet][_and_wallets]` → one `MintRouterConfig`
  builder (follows AGENTS.md `with_*` convention).
- Five-layer `start_federation_aleph_bft_live_session[_with_transport]
  [_with_config][_with_options_and_transport][_inner]` chain
  (`aleph_bft_session.rs`, re-exported at `federation.rs:77-90`) and the
  `run_federation_dkg_driver[_loop][_loop_with_client][_tick][_tick_with_cache]`
  chain (`dkg_driver.rs`; four re-exported publicly at `lib.rs:60-61`) → one
  entry point + one options struct each.

### 5.3 Push business logic out of the handler layer

- `ensure_federation_mint_quotes_paid_for_signing` and
  `wait_for_federation_mint_quotes_paid_locally` (`custom_handlers.rs:305-366`)
  implement a poll-until-paid loop with a local
  `FEDERATION_MINT_PAYMENT_WAIT_POLL_INTERVAL` (:33) — payment-finality waiting
  belongs behind one `cdk::mint` / federation-runtime call.
- `submit_federation_chain_observation` (lib.rs:~1492-1560) does script
  derivation validation and wallet-readiness checks in cdk-axum — move to
  `cdk::mint::federation::chain_observation`.
- Replace `federation_invalid_operation_alert_middleware` (lib.rs:2312-2351),
  which string-matches request paths and 4xx status codes, with typed error
  reporting at the operation-submission boundary.
- Add a design note for `post_operation` (`federation.rs:579-593`): private REST
  is an alternate consensus *submission* ingress (verified safe — admission
  happens in `cdk-common/consensus.rs:756`), but the asymmetry with the public
  publisher path (`wallet_operation_publisher.rs`) should be documented.

### 5.4 Break up god functions

- `start_federation_aleph_bft_live_session_with_options_and_transport_inner`
  (aleph_bft_session.rs:643, ~402 lines).
- `run_frost_dkg_tick` (dkg_driver.rs:765, ~254) and
  `run_federation_dkg_driver_tick_with_cache` (:1059, ~244).
- `record_federation_local_status_metrics` (status_routes.rs:330, ~204 lines of
  field-by-field recording) — a macro or struct walk collapses this.
- Bark `*_inner` functions (bark_incoming.rs:285 ~363, bark_maintenance.rs:83
  ~345): skip — slated for removal.
- `validate_federation_bark_operation_submission`
  (`crates/cdk/src/mint/federation/projection.rs:68-1547`, ~1,480 lines): the
  per-variant extraction was started (:1548, :1672, free fns 2180-2770) — complete
  it only for non-bark variants; bark variants leave with the bark removal.

**Gate:** `cargo check -p cdk-axum` with and without `federation`; axum federation
test suite.

---

## Phase 6 — Error handling: anyhow → thiserror

- cdk-axum federation code expanded anyhow from ~2 uses on `bls` to 116 sites
  across 12 files: `bark_custody.rs`, `bark_secret.rs`, `catch_up_routes.rs:343`,
  `dkg_driver.rs`, `sync_loop.rs`, `operation_pipeline.rs`, `status_routes.rs`,
  `wallet_liveness.rs`, `frost_signing.rs`, `runtime_components.rs`,
  `aleph_bft_network.rs`. AGENTS.md mandates thiserror with structured named
  fields; `cdk-common/src/federation.rs:477` (`FederationError`) and
  `peer_client.rs:48` / `sync_config.rs:21` already set the pattern.
- Convert per module, one commit each, skip bark files. Keep anyhow (if anywhere)
  only at the outermost route-to-response boundary.

**Gate:** clippy; axum federation tests.

---

## Phase 7 — Documentation & final polish

1. AGENTS.md: add the new workspace crates (`cdk-bdk`, `cdk-frost`,
   `cdk-frost-bip32`) to the structure section (still says "24 crates").
2. Migration notes: the two non-gated `MintDatabase` trait additions
   (`SignaturesDatabase::get_blinded_secrets_for_quote`,
   `SagaTransaction::compare_and_set_saga_state`) are breaking for downstream
   implementors; the unconditional federation schema migrations (~31 tables +
   `ALTER TABLE saga_state ADD COLUMN federation_operation_id`) land on standalone
   DBs — document the forward-only-schema policy explicitly.
3. Amend `docs/federated-cashu-implementation-plan.md` to state the two-scheme
   design explicitly: BLS threshold blind signatures for ecash keysets (v3 only),
   FROST/musig2 for on-chain wallet custody, cdk-bdk as the production on-chain
   backend. "BLS-only" currently invites misreading.
4. Final gate: `cargo fmt --all -- --check`, `just clippy`, `typos`, `just test`,
   `cargo test --doc`, `cargo check -p cdk --target wasm32-unknown-unknown`,
   federation integration tests (`--test-threads 1`), and a final `git diff
   bls...HEAD --stat` comparison against the baseline in this document.

## Success metrics (vs. branch point of this plan)

- `custom_handlers.rs` back under ~1,200 lines; `cdk-axum/src/lib.rs` under ~600.
- No `include!()`, no `#[allow(dead_code)]` outside bark, no anyhow in
  non-bark federation modules.
- Zero duplicated verification/settlement implementations between standalone and
  federation mint paths (verifiable by diffing the function pairs listed in 3.1).
- `Mint` struct carries a single `MintMode` field for federation state; no
  federation `bool` parameters on standalone constructors or pubsub.
- Wallet `federation.rs` split into ≤5 focused modules with one fan-out scheduler.
