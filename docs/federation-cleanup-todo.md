# Federated CDK Cleanup TODO

This is a living list of cleanup work needed to make federated CDK lean,
understandable, and production-ready. Add one section per specific cleanup task,
covering unnecessary complexity, code smells, hard-coded development baggage,
obsolete compatibility, unclear ownership, or other avoidable machinery, with
references to the affected code and documentation.

Treat every entry as a starting point, not an exhaustive inventory. Before
implementing one, inspect adjacent paths, call sites, tests, configuration,
generated tooling, and documentation for related cases that this list may have
missed. Verify the premise against the current code and preserve consensus
safety, deterministic replay, and standalone CDK behavior.

## 1. Decouple melt payment election from development payment finality

**Status:** Rejected in the current architecture (2026-07-12)

### Verdict

Do not remove the development-policy branch from
`should_attempt_melt_payment` as a standalone cleanup. A direct A/B test showed
that this changes consensus application behavior, not just payment-side-effect
ownership:

- with the branch removed, the focused mint melt suite passed all 27 tests and
  the three-member route-level quorum-policy melt test passed;
- a fresh five-member, threshold-three AlephBFT fakewallet smoke completed mint
  and swap, then timed out after 300 seconds in `standard.melt-confirm`;
- signed status during the failure showed the journal stopped before accepting
  the melt and one member had a different materialized-state digest; the
  readiness/convergence guards correctly prevented mismatched state from being
  treated as authoritative;
- after restoring the branch and rebuilding the same isolated binary, the same
  live smoke completed mint, swap, melt, and restore successfully, with melt
  confirmation completing in about one second.

The current development policy relies on every member deterministically
applying the synchronous fakewallet result during the accepted melt operation.
Electing only one member makes that operation's local apply result asymmetric.
A safe future implementation must first move external payment execution out of
the deterministic melt-apply transition: every member should reserve the same
pending state, exactly one actor should perform the side effect, and a later
accepted payment observation should finalize every member identically. That is
a payment-state-machine redesign and overlaps item 28; adding it here would
increase rather than remove complexity. Production `MemberQuorum` policy
already elects one actor and remains unchanged.

### Problem

Federated melt actor selection currently depends on the payment-observation
policy. `should_attempt_melt_payment` elects one member for a quorum policy, but
returns `true` for every member when `DevelopmentSingleObservation` is enabled.
This makes every member call its configured payment backend in local
development, even though fakewallet can use the same deterministic one-member
election as any real backend.

This also mixes two independent concerns:

- payment actor selection decides which member may cause the external outgoing
  payment side effect;
- payment observation policy decides how much evidence is required before the
  federation finalizes the result.

### Cleanup

- Elect exactly one melt payment actor for every federation payment policy and
  every configured payment backend.
- Make `should_attempt_melt_payment` depend only on the deterministic member
  election, or replace it with a name/API that makes this invariant explicit.
- Keep `DevelopmentSingleObservation`, if still needed by isolated local test
  backends, solely as a finality policy. It must not change who calls the
  outgoing payment backend.
- Keep federation production code backend-neutral. Do not add checks for
  fakewallet, LND, CLN, BDK, or any other concrete backend.
- Do not change the deterministic election input or introduce failover as part
  of this cleanup. Consensus-controlled payment-member failover is separate
  production work with additional idempotency requirements.

### Known locations

- `crates/cdk/src/mint/federation.rs`: `elected_melt_payment_member` and
  `should_attempt_melt_payment`.
- `crates/cdk/src/mint/melt/mod.rs`: accepted federated melt execution and the
  `should_attempt_external_payment` decision.
- `crates/cdk/src/mint/federation/tests/melt.rs`: actor-selection, catch-up, and
  counting-payment-processor tests, including the assertion that development
  policy currently lets every member pay.
- `crates/cdk-axum/src/custom_handlers.rs` and its federation tests: public melt
  fanout, payment observation submission, and duplicate-payment coverage.
- `crates/cdk-common/src/federation.rs`: payment verification policy semantics.
- `crates/cdk-mintd/src/federation_setup.rs`, generated local federation
  tooling, and `docs/federated-cashu-payment-trust-model.md`: development policy
  configuration and explanatory wording that may imply all members pay.

### Completion checks

- Prove that exactly one member attempts an outgoing melt payment under both
  `DevelopmentSingleObservation` and `MemberQuorum` policies.
- Prove the elected member's single fakewallet observation can still finalize a
  development-policy melt on every member.
- Preserve quorum-policy observation requirements and prove actor election does
  not weaken payment finality.
- Preserve duplicate request, replay, restart, and catch-up behavior without a
  second payment attempt.
- Run the live fakewallet federation mint, swap, and melt smoke flow and verify
  journal and materialized-state convergence.
- Keep standalone mint and fakewallet behavior unchanged.
- Search the whole workspace for actor-selection assumptions, fakewallet
  federation branches, stale comments, and tests not listed above before
  marking this item complete.

## 2. Remove unenforced payment trust-model claims

**Status:** Completed (2026-07-12)

### Result

Removed `FederationPaymentTrustModel`, the `MemberQuorum.trust_model` field,
the DKG CLI argument and conversion layer, and every production/test fixture
that supplied the unused label. `MemberQuorum` now contains only the enforced
`required` observation count. Deserialization rejects unknown fields, so the
undeployed old `trust_model` shape fails closed instead of being silently
accepted by Serde.

Documentation now distinguishes the implemented authenticated matching-member
quorum from future backend evidence verifiers. Generated DKG public config is
covered by a regression asserting that no `trust_model` field is emitted, and
the removed CLI flag is covered by a parser rejection.

Verification:

- all 379 `cdk-common` tests with `federation` passed, covering canonical
  operation/config encoding, DKG transcripts, config digests, invites, and
  wallet/member metadata validation;
- all 99 mint federation tests passed, including mint/melt payment quorum,
  conflict, replay, catch-up, and signing paths;
- focused DKG assembly/bind, DKG CLI parsing, and Axum materialized-state digest
  tests passed;
- standalone `cdk` and `cdk-mintd` with SQLite compiled without federation;
- a fresh five-member, threshold-three AlephBFT fakewallet config contained no
  trust-model field and completed mint, swap, melt, and restore successfully.

### Problem

`FederationPaymentTrustModel` advertises `SharedBackendReadOnly`,
`OriginMemberProof`, `ThresholdGateway`, and `OnchainProof`, but the selected
variant does not affect validation, observation admission, quorum evaluation,
or quote state transitions. Runtime behavior only uses the member-quorum
`required` count. The CLI and DKG setup therefore let operators commit to
security models that CDK does not implement or enforce.

### Cleanup

- Remove `FederationPaymentTrustModel` and the unused `trust_model` field from
  `FederationPaymentVerificationPolicy::MemberQuorum`.
- Keep the implemented policy honest and small: a required count of matching,
  authenticated member observations.
- Remove the DKG CLI `--payment-trust-model` argument and its conversion layer.
- Remove documentation that presents the four labels as implemented behavior.
- Add a trust model back only together with its concrete verifier, admission
  rules, evidence format, failure behavior, and end-to-end tests.
- Since federation mode is not deployed, update canonical config fixtures and
  protocol/config digests directly instead of preserving obsolete config
  compatibility.

### Known locations

- `crates/cdk-common/src/federation.rs`: `FederationPaymentTrustModel`,
  `FederationPaymentVerificationPolicy`, policy constructors, validation, and
  tests.
- `crates/cdk-mintd/src/cli.rs`: `FederationPaymentTrustModelArg` and the DKG
  roster argument.
- `crates/cdk-mintd/src/federation_dkg_setup.rs`: trust-model conversion and
  production policy construction.
- Federation setup, DKG, mint, Axum, and database tests that construct
  `MemberQuorum` with `SharedBackendReadOnly`.
- `docs/federated-cashu-payment-trust-model.md`, the implementation plan, and
  progress/current-state documentation.

### Completion checks

- Prove quorum evaluation and payment-observation state transitions are
  unchanged apart from the removed descriptive field.
- Regenerate and verify public-config, DKG transcript, canonical-byte, config
  digest, invite, and wallet validation fixtures affected by the schema change.
- Ensure no CLI, generated config, status output, or documentation claims CDK
  verifies a payment model that has no implementation.
- Search for every removed variant and for equivalent renamed placeholders
  before marking this item complete.

## 3. Remove or relocate inactive payment-observation limits

**Status:** Completed (2026-07-12)

### Result

Removed `max_pending_quotes`, `pending_quote_ttl_secs`, and
`stale_observation_secs` from `FederationPaymentObservationPolicy`, together
with their defaults, validation error, constructors, and fixtures. The policy
now contains only mint and melt finality rules that affect deterministic state
transitions. Strict deserialization rejects the undeployed old operator-limit
fields instead of ignoring them.

Axum payment-observation status retains stale classification with one local,
documented 15-minute default. This remains presentation-only: quote age and
recommended action are not committed to federation config or consensus digest.
Tests that previously mutated a dead limit merely to create a different config
digest now mutate an actual payment verification policy.

Verification:

- all 379 `cdk-common` tests with `federation` passed, including canonical
  config, DKG transcript, config digest, invite, quorum, and report-summary
  coverage;
- Axum stale-report status and conflicting startup config-digest tests passed;
- SQLite federation config persistence passed with the simplified schema;
- standalone `cdk` and `cdk-mintd` with SQLite compiled without federation;
- freshly generated five-member configs contained none of the removed fields,
  and the threshold-three AlephBFT smoke completed mint, swap, melt, and restore.

### Problem

`FederationPaymentObservationPolicy` contains three operational limits alongside
consensus finality rules:

- `max_pending_quotes` is validated and committed to config but does not apply
  backpressure or produce an alert.
- `pending_quote_ttl_secs` is validated and committed to config but is not read
  by runtime code.
- `stale_observation_secs` is read only to classify operator-facing status.

These fields enlarge the consensus-visible public configuration without
currently enforcing the behavior their names promise. The status-only stale
threshold is an operational preference, not payment consensus policy.

### Cleanup

- Remove `max_pending_quotes` and `pending_quote_ttl_secs` until corresponding
  behavior is deliberately implemented and tested.
- Move `stale_observation_secs` to local mint/operator status configuration, or
  use one documented status default until configurability is justified.
- Remove validation errors, defaults, constructors, fixtures, and documentation
  that exist only for the removed fields.
- Keep quote finality thresholds in the public federation config because those
  do affect deterministic state transitions.

### Known locations

- `crates/cdk-common/src/federation.rs`: policy fields, hard-coded defaults,
  validation, and `InvalidPaymentObservationLimit`.
- `crates/cdk-axum/src/federation/payment_observation.rs`: status-only stale
  classification.
- `crates/cdk-common/src/federation/operation.rs`: report summary age/stale
  calculation.
- DKG transcript, config digest, Axum status, database, setup, and serialization
  tests that mutate or serialize the three fields.
- `docs/federated-cashu-payment-trust-model.md` and generated federation config
  examples.

### Completion checks

- Confirm no admission, consensus, payment, database, or operator-alert path
  reads the fields before removing or relocating them.
- Prove payment observation quorum and quote transitions remain unchanged.
- Verify public-config and DKG canonical bytes converge after fixtures are
  regenerated.
- Verify stale status remains useful and consistent after becoming local
  operational configuration.
- Search for stale field names, defaults, error variants, JSON fixtures, and
  documentation before marking this item complete.

## 4. Require explicit payment policy construction

**Status:** Completed with a safe production constructor (2026-07-12)

### Result

Removed `Default` from `FederationPaymentObservationPolicy`, so generic code can
no longer request an implicit development policy. `FederationConfig::new` now
derives the production-safe `MemberQuorum` policy from the already validated
consensus threshold. Requiring a fourth constructor argument at roughly sixty
mostly-test call sites was rejected as noisy API churn: there is one
unambiguous safe policy derivable from the existing threshold parameters, while
the exceptional development policy should be conspicuous.

Trusted-dealer/local-test generation now opts into
`development_single_observation_for_testing` by name. DKG roster construction
uses the safe quorum unless an operator explicitly supplies a different
observation threshold, and setup-to-finalized DKG assembly preserves the exact
configured policy instead of reconstructing it. The test-only public helper
`ensure_single_payment_observation_can_finalize` was removed; tests inspect the
actual policy evaluation API. Mintd retains its narrow raw-JSON check so a
missing field has a clear startup error, and still rejects the development
policy unless the local-development override is explicit.

Verification:

- all 379 `cdk-common` tests with `federation` passed, including config
  canonicalization, DKG transcript equality, participant binding, config
  digests, invites, and trusted-dealer output;
- all 99 mint federation tests passed, and 24 mintd DKG/status tests plus 20
  Axum DKG route tests passed;
- five mintd startup tests proved missing policy and unapproved development
  policy fail closed alongside existing key/config checks;
- standalone `cdk` and `cdk-mintd` with SQLite compiled without federation;
- a fresh five-member, threshold-three generated config contained an explicit
  development policy and local override, then completed the AlephBFT mint,
  swap, melt, and restore smoke with converged operation results.

### Problem

`FederationConfig::new` silently installs
`FederationPaymentObservationPolicy::default()`, and that default enables
development single-observation finality. Mintd then carries separate startup
checks and an `allow_development_payment_observation_policy` escape hatch to
prevent accidental production use. A security-sensitive public config should
not begin life with a permissive test policy that callers must remember to
replace.

The public helper `ensure_single_payment_observation_can_finalize` is also used
only by tests and exposes another API for reasoning about the same policy.

### Cleanup

- Make federation config construction require an explicit payment-observation
  policy, or provide clearly separate production and test constructors.
- Remove `Default` for `FederationPaymentObservationPolicy` if no non-test caller
  has a safe reason to request an implicit policy.
- Keep development single-observation finality behind explicit local setup or
  test construction for as long as it remains necessary.
- Remove `ensure_single_payment_observation_can_finalize` if the final code has
  no production caller; tests should assert the policy/evaluation behavior
  directly.
- Simplify mintd's raw-config/startup checks after explicit construction makes
  accidental fallback impossible, while retaining a clear rejection for
  development policy in production mode.

### Known locations

- `crates/cdk-common/src/federation.rs`: policy `Default`,
  `FederationConfig::new`, `ensure_single_payment_observation_can_finalize`, and
  related tests.
- `crates/cdk-mintd/src/lib.rs`: explicit-field and development-policy startup
  checks.
- `crates/cdk-mintd/src/config.rs`: development-policy opt-in.
- Federation DKG/setup constructors and tests throughout `cdk-common`, `cdk`,
  `cdk-axum`, and `cdk-mintd`.

### Completion checks

- Prove every production config construction site selects its policy
  explicitly.
- Prove missing policy and accidental development policy fail at startup with a
  clear error.
- Keep generated local fakewallet federation configuration explicit and
  functional.
- Verify standalone CDK remains unaffected and audit all config constructors,
  deserialization paths, fixtures, and setup commands before completion.

## 5. Remove unpopulated free-form melt failure evidence

**Status:** Completed (2026-07-12)

### Result

Removed `failure_reason` from `FederationMeltQuotePaymentObservation`, quorum
matching, runtime-config builders, Axum submission, smoke tooling, fuzz input,
and all fixtures. The full audit found no payment backend, mint application,
database, recovery path, status route, catch-up path, or operator workflow that
used the value. Production Axum always supplied `None`; only tests and the
federation smoke helper supplied static text, without reading it later.

This is a direct schema deletion because federation mode is undeployed. It does
not change canonical bytes or operation IDs for real observations: the removed
field was optional, `None` was omitted during serialization, and every
production producer used `None`. Payment IDs, proofs, and all
paid/failed/unpaid/pending/unknown transitions remain intact. Observer-local
times were intentionally left for the separate audit in Item 12 and were later
removed from consensus there. The adjacent federation fuzz target was repaired
to enable its required `cdk-common/mint` feature and initialize the current
journal/observation fields.

Verification:

- 30 canonical federation-operation tests and 17 journal/digest/replay tests
  passed;
- all 99 mint federation tests passed, including failed-payment quorum rollback,
  paid settlement, unknown status, replay, restart, and catch-up paths;
- 11 focused Axum observer, quote-check, terminal observation, and catch-up
  tests passed;
- the federation request-conversion fuzz target compiled successfully;
- standalone `cdk` and `cdk-mintd` with SQLite compiled without federation;
- a fresh five-member, threshold-three AlephBFT smoke completed mint, swap,
  melt, and restore with the simplified observation schema;
- workspace search found no remaining federation-code producer or consumer of
  `failure_reason`; unrelated LND protobuf/backend fields were left untouched.

### Problem

`FederationMeltQuotePaymentObservation` includes
`failure_reason: Option<String>`, and this field participates in observation
matching, but the production Axum observation path always sets it to `None`.
The only non-empty value found by the initial audit is in operation-level test
data. Carrying an arbitrary string in consensus evidence without a producer or
defined interpretation adds schema and conflict surface without functionality.

### Cleanup

- Reconfirm that no payment backend, recovery path, private route, operator
  command, or catch-up path produces or consumes `failure_reason`.
- Remove the field from the operation, builders, canonical serialization,
  matching logic, fixtures, and documentation if it remains unused.
- If deterministic failure classification is required, design a typed,
  backend-neutral value with explicit semantics instead of preserving an
  unused free-form string.
- Do not remove payment IDs, observed times, or payment proofs as part of this
  task. Item 12 subsequently audited and removed only the observer-local time;
  payment IDs and proofs remain consensus evidence.

### Known locations

- `crates/cdk-common/src/federation/operation.rs`: observation field and
  evidence matching.
- `crates/cdk/src/mint/federation.rs`: melt observation builders and apply
  helpers.
- `crates/cdk-axum/src/custom_handlers.rs`: production observation construction
  currently setting `failure_reason` to `None`.
- Federation journal, canonical operation, and payment-observation tests.

### Completion checks

- Search the full workspace, including feature-gated code and generated smoke
  tooling, for producers and consumers before removing the field.
- Verify canonical operation IDs, journal replay, catch-up, state digests, and
  matching/conflict tests using regenerated fixtures.
- Prove paid, failed, unpaid, pending, and unknown backend observations retain
  their existing behavior without the free-form field.

## 6. Remove the production in-memory consensus backend

**Status:** Completed (2026-07-12)

### Progress

Removed the unused public `InMemoryFederationConsensusNetwork`,
`InMemoryFederationConsensusAdapter`, `FederationConsensusBackendAdapter`, and
`FederationConsensusBackendAdapterRuntime` architecture. No production caller
constructed or referenced this second adapter stack; it existed only to test
its own deterministic simulator. The useful Aleph recovery assertion was moved
to `FederationConsensusRuntime`, the runtime production actually uses, before
deleting the dead tests.

This checkpoint removes roughly 1,950 lines and 22 simulator/adapter-only tests
while retaining 357 passing `cdk-common` tests covering the real Aleph adapter,
unit data provider, finalization handler, runtime recovery, duplicate/conflict
handling, direct journal behavior, and catch-up validation. Federation-enabled
`cdk`, Axum, and mintd compile successfully.

`InMemoryFederationConsensus`, the backend enum, runtime dispatch, mintd
selector/environment/config fields, and local-finality simulator tests are now
removed. `InMemoryFederationJournal` remains intentionally: despite its old
name, it is the deterministic finalized-journal reconstruction and validation
type, not a selectable consensus backend. Renaming that journal view is a
separate naming decision and does not leave a local-finality path.

The production selection surface is now removed: mintd has no consensus
backend config field or environment variable, old `consensus_backend` config
keys fail closed, generated smoke/DKG scripts and just recipes contain no
backend switch, and `FederationRuntimeConfig::new` selects AlephBFT. A fresh
five-member, threshold-three dealer-free DKG smoke completed interrupted-member
setup recovery, finalized-config export, active AlephBFT startup, mint, swap,
melt, and restore without a local-backend setup phase. The remaining work is
the internal enum/local engine and its Axum test callers, plus status fields and
dispatch branches that become constant once that engine is gone.

Mint-library conflict tests no longer construct the local consensus engine.
They now append directly to `InMemoryFederationJournal`, which is the actual
boundary responsible for idempotent duplicates, conflict-key reservation, and
sliding-window rejection. All 99 focused federation mint tests pass after this
test-only dependency removal.

`FederationRuntimeConfig` no longer stores a consensus-backend field or exposes
an alternate backend constructor. Mintd startup and durable DKG activation now
build the same validated runtime config. The focused mint runtime tests, Axum
federation test build, and mintd AlephBFT startup test pass.

Peer status and operator digest comparison no longer carry or compare a
consensus-backend label. With AlephBFT mandatory, that field was constant wire
surface rather than a convergence invariant. Status now always requires the
Aleph finalized-item cursor; catch-up always serves finalized-item evidence;
order digest, materialized-state digest, operation height, and quorum evidence
remain the safety checks. Verification passes 356 `cdk-common` tests, 22 Axum
peer-client tests, 10 digest-comparison tests, 39 mintd status tests, three
private journal-route tests, and seven Aleph catch-up tests.

The Axum runtime is now a direct `FederationAlephBftConsensusAdapter` rather
than an enum-dispatched backend. Startup recovery, submission, catch-up,
readiness, private Aleph routes, and live-session startup no longer branch on a
backend label. The old synchronous append/finalize branch and its idle
finalization task are deleted. Tests that need immediate deterministic
application use one narrow `#[cfg(test)]` hook that drains and applies the real
Aleph adapter unit; explicit finality, timeout, live-session, restart, and
catch-up tests continue to control finalization externally.

This checkpoint also fixes a notification-state shortcut exposed by the
runtime migration. A pre-registered response observer is no longer mistaken
for a queued operation; duplicate proposal detection remains owned by the
Aleph data provider under the consensus lock, and duplicate callers join the
same waiter without requeueing. All 352 federation-enabled `cdk-common` tests,
all 99 mint federation tests, and the complete 456-test Axum federation suite
pass, including duplicate/fanout, Aleph catch-up, startup-decision, live
private-message finality, and mint/swap/melt/notification coverage.

The now-single-implementation `FederationConsensusEngine` trait and
`FederationConsensusBackendKind` enum are deleted. `FederationConsensusRuntime`
exposes its real Aleph queue/finality and finalized-journal operations directly;
the impossible synchronous local-submit API and its backend-specific errors are
gone. The test-only `InMemoryFederationConsensus` is also deleted. Peer,
catch-up, handler, restart, and materialization fixtures now finalize actual
Aleph adapter units and carry finalized-item evidence; direct journal tests
continue to test journal behavior at the narrower boundary.

Final verification passes all 333 remaining federation-enabled `cdk-common`
tests, 94 custom-handler tests, 30 catch-up tests, 22 private REST peer-client
tests, and the complete 456-test Axum federation suite, including partitions,
session rotation, restart, catch-up, DKG, and mint/swap/melt flows. A workspace
code search finds no in-memory consensus engine, backend enum, consensus-engine
trait, backend selector, or local append-finality path.

### Problem

Federation mode still exposes two selectable consensus backends even though
AlephBFT is the production consensus system. `in_memory` is currently the
default and provides a local append-log finalization path rather than
multi-member Byzantine consensus. Keeping it constructible in production adds
backend dispatch throughout the runtime and creates a dangerous configuration
in which an operator can start something called a federation without real
federation consensus.

This task concerns the in-memory **consensus backend**. It must not blindly
delete `InMemoryFederationJournal`, which is also used as a deterministic
journal reconstruction/view type in tests and runtime validation. That type
should be retained or renamed according to its actual remaining responsibility.

### Cleanup

- Make AlephBFT the only production federation consensus implementation.
- Remove `FederationConsensusBackendKind::InMemory`, backend selection, aliases,
  defaults, environment variables, generated-script switches, and config
  documentation for `in_memory`.
- Collapse runtime enums, adapters, and match branches that exist only to
  dispatch between AlephBFT and the in-memory backend.
- Remove `InMemoryFederationConsensus`,
  `InMemoryFederationConsensusNetwork`, and
  `InMemoryFederationConsensusAdapter` from production exports and code.
- Remove local-finalization shortcuts and backend-specific errors or status
  diagnostics that become impossible with one consensus implementation.
- Replace tests with the real AlephBFT runtime, direct journal/application
  tests, narrow `#[cfg(test)]` finalization fakes, or transport mocks according
  to what each test actually verifies. Do not preserve a second fake consensus
  engine under another name.
- Audit `InMemoryFederationJournal` separately. Rename it to
  `FederationJournal` or `FederationJournalView` if it survives solely as an
  in-process ordered journal representation.

### Known locations

- `crates/cdk-common/src/federation/consensus.rs`: backend kind, runtime backend
  enum, in-memory consensus engine, network, adapter, and backend-dispatch
  tests.
- `crates/cdk/src/mint/federation.rs`: `FederationRuntimeConfig` backend field
  and its in-memory default constructor.
- `crates/cdk-axum/src/lib.rs`: startup recovery, submission, readiness, and
  finalization branches selected by backend kind.
- `crates/cdk-axum/src/federation.rs`: runtime construction and journal recovery
  helpers.
- `crates/cdk-mintd/src/config.rs`, `env_vars/federation.rs`, `lib.rs`, and
  `federation_setup.rs`: user-selectable backend configuration and generated
  smoke scripts.
- Mintd status and digest comparison code that reports or rejects mixed
  consensus backend views.
- `crates/cdk-common/src/federation/journal.rs` and its many callers: the
  distinct journal-view type that requires an explicit keep/rename decision.
- `docs/federated-cashu-refactoring-plan.md`, section R4, which already defines
  the intended one-backend architecture and test replacement strategy.

### Completion checks

- No production config, CLI, environment variable, alias, or generated script
  accepts `in_memory`, `memory`, or an equivalent value.
- No production crate exports or constructs an in-memory federation consensus
  engine or local-finality shortcut.
- AlephBFT is the only federation runtime path from mintd startup through
  proposal, finalization, journal application, restart, and catch-up.
- Proposal admission, deterministic ordering, duplicate handling, below-quorum
  non-finality, partition/heal, restart, and catch-up tests run against AlephBFT
  or an appropriately narrower test boundary.
- Direct journal tests continue covering canonical append, replay, rejection,
  digest, and tamper detection without pretending to test consensus.
- Search all production code, tests, generated tooling, examples, and docs for
  backend-selection assumptions and stale in-memory terminology before marking
  this item complete.

## 7. Remove the legacy external-finalization Aleph driver

**Status:** Completed (2026-07-12)

### Result

Deleted the periodic external-finalization driver, its router startup task,
quorum-broadcast policy, private `/aleph-bft/finalized-units` route, signed
request/response DTOs, REST message kind, peer-client method, mock transport
surface, finality-mode enum/status field, preview/exact-drain APIs and errors,
and their driver-only tests. Removed the second durable finalized-unit apply
implementation as well. Test fixtures now pull the next unit exactly once from
the same data-provider API used by upstream AlephBFT and feed finalized units
through the normal durable apply path.

Public and authoritative-read readiness now requires both the real live-session
handle and inbound network queue. A healthy sync flag alone cannot enable
value operations. Operator status reports the observable live-session,
inbound-network, queue, pending-finalization, finalized, and accepted counters;
it no longer advertises a synthetic finality source.

Verification:

- all 328 federation-enabled `cdk-common` tests pass after deleting the
  preview/exact-drain contract and its tests;
- all 94 Axum custom-handler tests pass, including pending mint, swap, and melt
  timeouts, duplicate waiter joins, manual finality, and persisted responses;
- focused live-session startup, readiness, rotation, pending-mempool, restart,
  status, and public `GET /v1/config` tests pass;
- all 39 mintd federation-status tests pass and `cdk-mintd` checks without
  warnings under the federation feature;
- all 424 Axum federation tests pass after correcting one stale test to
  distinguish the drained proposal queue from the pending-finalization set.

A workspace production-code search finds no external-finalization mode,
finalized-unit REST route, preview/exact-drain API, proposal-mismatch error, or
second finalization driver. Finalization, partition/heal, equivocation,
restart, rotation, and catch-up coverage now exercises upstream AlephBFT only.

### Problem

The upstream AlephBFT session already finalizes units locally, but CDK still
starts a second driver that previews adapter units, broadcasts them through a
private finalized-unit REST route, waits for remote delivery, and drains them
into the journal. The driver describes itself as not being a production BFT
ordering engine and becomes idle when network finality is available.

### Cleanup

- Remove the external-finalization driver loop and finality-mode dispatch.
- Remove the finalized-unit private route, transport messages, peer-client
  methods, preview/drain APIs, errors, configuration, metrics, and tests that
  exist solely for that path.
- Preserve finalization through the real Aleph session and prove that no
  startup, recovery, or test path still depends on externally delivered units.

### Known locations and completion checks

- `crates/cdk-axum/src/federation/aleph_bft_driver.rs`.
- `crates/cdk-axum/src/federation/aleph_bft_routes.rs`,
  `peer_client.rs`, `aleph_bft_session.rs`, and Axum startup wiring.
- `crates/cdk-common/src/federation/transport.rs` and consensus adapter APIs.
- Verify proposal, finalization, partition/heal, restart, and catch-up using only
  upstream AlephBFT finality, then search for finalized-unit terminology.

## 8. Remove legacy mint-to-mint primary forwarding

**Status:** Completed (2026-07-12)

### Result

Removed `submit_operation_to_primary_if_needed`, its generic transport path,
the first-roster-member selection, forwarding-specific readiness gate, local
catch-up/evidence collection after forwarding, and all forwarding-only tests.
Every federation member now admits and queues operations directly through its
AlephBFT runtime; no member is a privileged consensus ingress.

Wallet quote coordination is unchanged. The wallet may still prefer or fall
back between members for payment-facing quote creation, which is distinct from
the deleted mint-to-mint consensus forwarding path.

Verification proves direct Aleph submission and notification for mint, swap,
melt, payment observations, and private member operation requests; duplicate
fanout queues one operation and joins existing waiters; live private-REST
Aleph sessions finalize successfully; and catch-up/restart paths retain their
ordering, finalized-item, quorum-evidence, and materialized-state checks. A
workspace search finds no forwarding helper, primary-forwarding readiness
logic, or forwarding-specific test path.

### Problem

Non-primary members can forward operations to the first member, synchronize
from it, and collect range evidence. AlephBFT explicitly bypasses this path, so
it survives only for the in-memory consensus backend and adds an unnecessary
private submission path and single-ingress assumption.

### Cleanup

- Complete this together with, or immediately after, section 6.
- Remove `submit_operation_to_primary_if_needed`, its transport abstraction,
  forwarding-specific readiness logic, and tests.
- Keep wallet quote coordination separate; wallet primary-member behavior has
  payment semantics and must not be removed as part of this task.

### Known locations and completion checks

- `crates/cdk-axum/src/federation/sync_loop.rs` and
  `crates/cdk-axum/src/federation.rs`.
- `crates/cdk-axum/src/lib.rs` public and payment-observation submission paths.
- Prove every Aleph member can submit an identical operation, duplicate
  operation IDs remain idempotent, and no forwarding route remains.

## 9. Remove restore from the consensus operation model

**Status:** Completed (2026-07-12)

### Result

Removed `FederationRestoreOperation`, the restore operation kind and enum
variant, its no-op admission/apply branches, transcript and balance handling,
materialized-state projection, public export, and all restore-envelope
fixtures. Generic private-route and catch-up fixtures now use an actual
payment-observation quarantine operation instead of relying on a state-free
restore journal entry. The artificial restore state-digest test was deleted;
real mint, swap, melt, quarantine, and completed-operation tests cover rows
that consensus can actually create.

Restore remains a direct, readiness-gated read of signature-share rows that
were persisted by accepted mint, swap, or melt-change operations. Mint members
still reject non-v3 restore placeholders, the wallet still requires matching
threshold member views and validates/aggregates BLS shares, and lagging members
still refuse both restore and check-state responses.

Verification:

- a production-code search finds no restore consensus type, kind, enum
  variant, envelope constructor, apply branch, or projection branch;
- all 217 focused `cdk-common` federation tests and all 99 mint federation
  tests pass;
- all 73 federated connector tests pass, including threshold restore
  aggregation, matching-view, no-threshold, and non-v3 pre-fanout coverage;
- the interrupted-receive recovery regression passes, proving the existing
  check-state-then-restore saga path remains the sole recovery mechanism;
- all seven normal standalone restore tests pass under CDK's default
  non-federation features;
- all 423 Axum federation tests pass, including direct restore admission,
  authoritative-read readiness, startup replay, catch-up, private REST, DKG,
  and live four- and five-member one-offline mint/swap/restore/melt windows.

### Problem

Federated restore is a threshold read of already persisted signature shares.
It does not sign new outputs or mutate mint state, and the consensus apply path
returns without applying a state transition. Keeping a restore operation type
expands canonical serialization, conflict analysis, replay, projection, and
catch-up without providing ordering safety.

### Cleanup

- Remove `FederationRestoreOperation`, its operation kind and enum variant,
  transcript handling, conflict/balance analysis, projection, and fixtures.
- Keep authoritative-read readiness checks and wallet threshold share
  validation and aggregation.

### Known locations and completion checks

- `crates/cdk-common/src/federation/operation.rs`.
- `crates/cdk/src/mint/federation.rs`, public restore handlers, and
  `crates/cdk/src/wallet/federation.rs`.
- Prove restore is read-only, interrupted receive still recovers through
  restore/check-state, and no production caller constructs a restore envelope.

## 10. Remove the unused request-transcript abstraction

**Status:** Completed (2026-07-12)

### Result

Confirmed that `FederationRequestTranscript`, its hash, constructor, envelope
method, operation predicate, and size error had no production consumer in
admission, signing, ordering, persistence, catch-up, wallet code, HTTP, CLI, or
operator tooling. Deleted the complete abstraction and its public exports.

The useful mutation coverage now asserts the actual consensus commitment
directly: canonical envelope bytes and `FederationOperationId`. NUT-20 mint
outputs, input witnesses, P2PK/HTLC witnesses, and SIG_ALL output sets still
produce different operation IDs, while sorted custom JSON produces the same
ID and the canonical operation golden vector remains unchanged.

Verification:

- a full Rust-source search finds no request-transcript type, method, hash,
  predicate, error, or domain separator;
- all 29 operation tests and all 216 focused `cdk-common` federation tests
  pass;
- federation-enabled `cdk`, `cdk-axum`, and `cdk-mintd` checks pass;
- standalone `cdk-common` still checks without the federation feature;
- the removal changes no serialized operation envelope or operation-ID bytes.

### Problem

`FederationRequestTranscript` and its hash duplicate the canonical operation
envelope and operation ID. The initial audit found meaningful use in tests but
not in production admission, ordering, application, or catch-up.

### Cleanup

- Reconfirm there is no external audit or protocol consumer.
- Remove transcript types, builders, hashes, exports, test fixtures, and
  documentation while retaining the canonical operation-ID commitment.

### Known locations and completion checks

- `crates/cdk-common/src/federation/operation.rs` and its tests.
- Search for `request_transcript`, `has_request_transcript`, and transcript hash
  consumers across feature-gated builds.
- Prove operation IDs still bind every consensus-relevant request field and
  preserve mix-and-match protection.

## 11. Remove the singular journal conflict key

**Status:** Completed (2026-07-12)

### Result

Removed the singular `FederationJournalEntry::conflict_key`, the
`FederationOperation::conflict_key()` helper, `conflict_key_json` SQL column,
row codec and bindings, migration definitions, retry comparison, fixtures, and
assertions. Journal entries now have one authoritative representation: the
complete typed `conflict_keys` vector.

The in-memory journal, pending item queue, and Aleph pending-finalization set
now index `FederationOperationConflictKey` directly in hash maps/sets. They no
longer serialize every key into canonical JSON during ordinary admission,
replay, insertion, and release. JSON conversion happens only when constructing
the existing diagnostic conflict error string. The undeployed initial SQLite
and Postgres federation journal migrations were edited in place; no compatibility
migration was added.

Verification:

- source and migration searches find no singular journal field, operation
  helper, or `conflict_key_json` column;
- all 216 `cdk-common` federation tests pass, covering zero-key swaps,
  one-key quote/rotation operations, multi-key batch mint conflicts, pending
  queue conflicts, finalized losers, and deterministic replay;
- both SQL migration tests and all four SQLite journal persistence,
  concurrency, metadata-update, and index-plan tests pass;
- all 99 mint federation tests and all 423 Axum federation tests pass,
  including SQLite restart, catch-up, conflicting finalized operations,
  private REST, live AlephBFT, and DKG cases;
- federation-enabled SQLite, Postgres, and mintd checks pass.
- standalone `cdk-common`, `cdk-sql-common`, and `cdk` checks remain clean.

### Problem

Journal entries persist both `conflict_key` and `conflict_keys`, with the former
representing only the first element of the complete vector. The duplicate field
and SQL column allow inconsistent representations and enlarge codecs and tests.

### Cleanup

- Keep only the typed `conflict_keys` collection.
- Remove the singular field, operation helper, SQL column, row codec, migration
  history, fixtures, and assertions.
- Prefer ordering typed conflict keys directly instead of repeatedly converting
  them to canonical JSON strings for in-memory maps.

### Known locations and completion checks

- `crates/cdk-common/src/federation/journal.rs` and `operation.rs`.
- `crates/cdk-sql-common/src/mint/federation.rs` and federation migrations.
- Prove conflict detection and deterministic replay produce identical results
  for zero, one, and multiple conflict keys.

## 12. Remove observation wall-clock time from consensus operations

**Status:** Completed (2026-07-12)

### Result

Removed `observed_time` from mint and melt payment-observation structs,
runtime-config builders, Axum backend-observer submission, wallet/local test
connectors, smoke tooling, journal/database fixtures, and all canonical test
data. Deleted the special operation-ID normalization that cloned observations
and stripped the timestamp: operation IDs now hash exactly the same canonical
bytes that consensus orders and the journal persists.

Mint payment application now records the deterministic accepted logical time
`operation_index + 1` when an observation quorum adds payment to a quote. It no
longer chooses one member's backend timestamp. Existing standalone payment
backend timing remains untouched and local; no federation consumer needed a
separate diagnostic observation timestamp after submission.

Verification:

- a full federation Rust-source search finds no `observed_time` producer,
  field, normalization branch, or consumer;
- all 29 operation tests and all 216 `cdk-common` federation tests pass; exact
  duplicate evidence has identical canonical bytes and IDs, while changed
  payment proof material has different bytes/IDs and conflicts normally;
- all 99 mint federation tests pass, including a focused two-observer quorum
  regression proving the durable payment time is consensus logical time;
- five focused Axum observer tests and all 423 Axum federation tests pass,
  including stale-status reporting, materialized digests, paid/failed/unknown
  observations, catch-up, SQLite restart, live AlephBFT, and DKG;
- federation-enabled common/core/Axum/mintd checks and standalone
  core/Axum/mintd checks pass.

### Problem

Mint and melt payment observations serialize `observed_time`, but operation-ID
derivation intentionally strips it. Two byte-distinct operations can therefore
share an identity. Application also has a deterministic consensus acceptance
time available.

### Cleanup

- Remove `observed_time` from canonical consensus observation payloads.
- Derive durable payment transition time from accepted logical time.
- Keep local backend observation timing only as non-consensus operational
  metadata if it remains useful for diagnostics.

### Known locations and completion checks

- `crates/cdk-common/src/federation/operation.rs` operation-ID normalization.
- `crates/cdk/src/mint/federation.rs` payment-observation application.
- Regenerate canonical fixtures and prove operation identity, replay, matching,
  stale-status reporting, and materialized digests remain deterministic.

## 13. Remove unused materialized-state digest checkpoints

**Status:** Completed (2026-07-12)

### Result

Confirmed that every production use was a writer or pruner; status, catch-up,
monitoring, digest comparison, and operator tooling had no checkpoint-history
reader. Deleted the checkpoint type, transaction/database methods, SQL codec,
runtime recorder, retention configuration, SQLite/PostgreSQL table migrations,
and checkpoint-only tests. Catch-up tests continue to assert the actual
materialized rows, terminal rejection state, signed status output, and live
materialized-state digest verification.

Each finalized or replayed batch now avoids one extra full materialized-state
snapshot plus a separate transaction containing checkpoint insert and retention
delete statements. Journal outcome/state digests, signed session outcomes,
redacted row fingerprints, mismatch metrics, and catch-up digest comparison are
unchanged.

Verification:

- all 219 focused `cdk-common` federation tests pass;
- all 22 SQLite federation persistence, fresh-schema, and index tests pass;
- all 99 mint federation tests pass;
- all 420 Axum federation tests pass, including live AlephBFT finalization,
  restart, catch-up, mismatch detection, DKG, and public `GET /v1/config`;
- federation-enabled PostgreSQL and mintd checks pass;
- all 28 standalone Axum and 134 standalone SQLite tests pass, and the
  federation-disabled mintd check passes.

### Problem

The runtime writes and prunes state-digest checkpoint rows after application,
but the initial audit found no production reader. This imposes extra writes and
schema/configuration surface without contributing to current verification.

### Cleanup

- Reconfirm that status, catch-up, monitoring, and operator tooling do not read
  checkpoint history.
- Remove the checkpoint table, retention setting, database methods, writes,
  pruning, migrations, and tests if it remains write-only.
- Keep the current materialized-state digest needed by catch-up verification.

### Known locations and completion checks

- `crates/cdk-axum/src/lib.rs` finalization and checkpoint recording.
- Federation database traits, SQL codecs, SQLite/PostgreSQL migrations, and
  state-digest tests.
- Compare pre/post cleanup status and catch-up digests and measure reduced write
  work during sustained operation batches.

## 14. Replace durable Aleph gossip retries with bounded runtime queues

**Status:** Completed (2026-07-12)

### Result

Confirmed that AlephBFT 0.45.4 persists its DAG backup and independently
rebroadcasts units plus repeated coordinate, parent, and newest-unit requests.
Fedimint likewise feeds Aleph traffic through bounded reconnecting in-memory
peer channels while persisting Aleph's backup, not individual failed sends.
CDK already had a bounded 4,096-entry runtime queue that retains only failed
peer subsets, prioritizes fresh outbound traffic, drains bounded work while
idle, and caps each retry attempt.

Deleted the durable retry record type, database APIs, SQL codec/table/index,
SQLite/PostgreSQL migrations, startup reload/orphan-cleanup branches, retry-ID
hashing, and persistence-only tests. Queue overflow now reports an explicit
structured dropped-delivery count. Aleph backup, pending-operation persistence,
finalized-item evidence, and catch-up are unchanged.

Adding a second per-peer backoff/expiry subsystem was rejected as unnecessary
complexity: Aleph already owns rebroadcast and missing-unit recovery, while the
existing queue is hard-bounded, periodically retried, and evicts oldest work.
There is no durable stale gossip to expire after restart.

Verification:

- focused runtime tests pass for failed-peer recovery, one retry per idle tick,
  non-retryable errors, slow-peer quorum fast path, and bounded eviction;
- restart before Aleph backup flush recovers the durable pending operation and
  reaches finality without persisted gossip;
- restart from Aleph backup and five-member post-backup multi-operation
  continuation pass;
- all 219 focused `cdk-common` federation tests pass;
- all 21 SQLite federation persistence/fresh-schema tests pass and prove the
  retry table is absent;
- all 416 Axum federation tests pass, including partition/heal and one-offline
  value windows;
- federation-enabled PostgreSQL and mintd checks pass, as do all 28 standalone
  Axum and 134 standalone SQLite tests.

### Problem

Failed Aleph network-message deliveries are stored as database retry rows and
restored at startup. Gossip is transient liveness traffic, while Aleph backup
state should contain the durable information required to resume the protocol.
Persisting each failed delivery increases write contention and recovery code.

### Cleanup

- First prove from the Aleph protocol and CDK backup implementation that a node
  can regenerate or request all required traffic after restart.
- If proven, replace database retry rows with bounded per-peer in-memory queues,
  backoff, expiry, and metrics.
- Do not remove Aleph session backup persistence.

### Known locations and completion checks

- Aleph network pump/session runtime and federation retry database methods.
- `crates/cdk-sql-common/src/mint/federation.rs` and federation migrations.
- Test peer outage, queue saturation, restart during failed delivery, heal, and
  final convergence without message loss or unbounded retry amplification.

## 15. Remove undeployed federation wire-compatibility defaults

**Status:** Completed (2026-07-12)

### Result

Current consensus-visible federation fields now fail closed instead of silently
deserializing obsolete development shapes. Peer status and catch-up responses
require the materialized-state digest; journal entries require conflict keys,
the accepted-operation outcome hash, and the post-application state digest;
materialized rows require their deterministic field map; federation keysets
require their fee; wallet registry entries require the current schema version
and connector policy; and operator status/payment-observation reports require
their current counters, flags, and collections. The status request also carries
its materialized-row choice explicitly, including `false`.

The journal and SQL codecs now model accepted outcome/state hashes as concrete
values rather than optional compatibility fields, and fresh SQLite/PostgreSQL
federation schemas require both columns. Missing-field regressions cover peer
status, catch-up, journal metadata, keyset fees, and wallet registry policy.
There were no legacy serde aliases to retain or remove.

Optionality with a current meaning remains intact: setup/DKG phase artifacts,
pagination and known-digest hints, optional consensus/session evidence,
application rejection metadata, payment identifiers/proofs, empty-history
indexes, and optional operator diagnostics. Explicit protocol-version checks
remain the boundary for genuinely different wire versions.

Verification:

- all 221 focused `cdk-common` federation tests pass;
- all 21 SQLite federation persistence and fresh-schema tests pass;
- all 11 focused CLI registry tests and all 99 mint federation tests pass;
- all 41 focused mintd federation-status tests pass;
- all 332 Axum federation tests pass, including live AlephBFT, private REST,
  DKG, restart, and catch-up paths;
- all 28 standalone Axum and 134 standalone SQLite tests pass;
- federation-enabled PostgreSQL and mintd compile checks pass.

### Problem

Private REST, status, journal, and catch-up structures contain defaults and
optional fields that accept obsolete experimental shapes, including missing
materialized-state information. Federation has not been deployed, so these
branches hide malformed current messages rather than protecting users.

### Cleanup

- Make every current protocol field required unless absence has a real current
  semantic meaning, such as optional pagination or optional evidence.
- Remove legacy aliases, genesis fallbacks, compatibility fixtures, and negative
  tests for obsolete shapes.
- Retain explicit protocol-version rejection for genuinely different versions.

### Known locations and completion checks

- `crates/cdk-common/src/federation/transport.rs`, `journal.rs`, and
  `operation.rs`.
- Axum private routes, wallet federation metadata, and setup/config parsing.
- Regenerate all current fixtures and prove malformed or incomplete current
  messages fail closed.

## 16. Squash undeployed federation database migrations

**Status:** Completed (2026-07-12)

### Result

Replaced the 20-file experimental federation migration chain in each backend
with one direct final-schema migration:
`20260712000000_create_federation_schema.sql`. The migration adds the one
federation field on standalone saga rows, creates all 18 current federation
tables, defines journal outcome/state hashes as required from the start, and
creates the complete current index set. Every standalone migration predating
federation is unchanged.

Deleted the 368-line test that searched individual migration source strings for
column names. SQLite and PostgreSQL now open fresh real databases and inspect
the resulting catalog instead: both require exactly one federation migration,
all standalone and federation tables, all current DKG tables, the saga column,
the complete index set, required journal hashes/conflict keys, and nullable
consensus-evidence/rejection metadata. Existing CRUD, concurrency, restart, and
catch-up tests exercise the schema through the database APIs.

Verification:

- all 21 SQLite federation schema, persistence, concurrency, and index tests
  pass;
- all 19 PostgreSQL federation schema, persistence, and concurrency tests pass
  against the repository's local PostgreSQL test instance;
- all 29 focused Axum catch-up tests and all 8 SQLite restart tests pass;
- all 134 standalone SQLite and 125 standalone PostgreSQL tests pass, including
  the existing SQLite legacy-upgrade regression;
- the backend-Unknown melt status remains transient and its schema regression
  still passes.

### Problem

SQLite and PostgreSQL carry a long sequence of migrations for experimental
federation tables, columns, and corrections that have never been deployed.
Maintaining this history makes the intended schema difficult to understand and
keeps obsolete design decisions alive.

### Cleanup

- Perform this after the other federation schema removals stabilize.
- Replace experimental federation migration chains with one clean initial
  federation schema per database backend.
- Preserve all standalone CDK migration history and behavior.
- Replace migration-source string tests with schema parity and behavior tests.

### Known locations and completion checks

- SQLite and PostgreSQL migration directories and migration-registration code.
- SQL federation codecs and database tests.
- Create fresh databases for both backends, compare schemas and constraints,
  run federation restart/catch-up tests, and verify standalone upgrades remain
  unchanged.

## 17. Extract a FederationNodeRuntime from Axum MintState

**Status:** In progress

### Checkpoint 1: explicit runtime ownership

`MintState` no longer owns 13 unrelated federation handles directly. One
concrete `FederationNodeRuntime` now owns consensus, peer-sync policy, DKG
admission/config, sequence tracking, readiness, notifications, private REST
admission, invalid-operation alerts, pending Aleph sequence metadata, inbound
network state, live-session state, and test finality controls. Standalone mints
inside a federation-enabled binary use one explicit inert runtime constructor.

All production and test call sites now name the ownership boundary as
`state.federation.<component>`; no deref trick, dynamic trait, event bus, or
duplicated handle set was introduced. This checkpoint is behavior-neutral and
prepares moving lifecycle methods without a single mega-refactor. Both
federation-enabled and federation-disabled test builds compile, all 332 Axum
federation tests pass, and all 28 standalone Axum tests pass.

Remaining: move submission/notification, finalization/catch-up, readiness and
startup recovery, then task startup/shutdown methods onto the concrete runtime
in reviewable groups. Coordinate live-session task ownership with Item 18.

### Checkpoint 2: notification ownership

Operation waiter registration, response-cache limits and counts, notification
statistics, and finalized-operation delivery now execute on
`FederationNodeRuntime`. `MintState` retains thin temporary delegators so HTTP
handler behavior and test call sites stay unchanged while larger submission and
finalization groups move. Focused waiter saturation/timeout, bounded cache,
cache eviction, and operation-notification tests pass.

### Checkpoint 3: finalization coordination

Pending source-sequence tracking, duplicate finalized-unit validation, and
post-application response reconstruction now execute on
`FederationNodeRuntime`. Durable journal/finalized-item persistence and mint
state application remain explicit `MintState` boundary calls until that whole
transactional group can move together. Five finalized-unit regressions and the
source-sequence persistence-after-finalization regression pass.

### Checkpoint 4: readiness state machine

Readiness status lookup and every lagging, catching-up, evidence-verification,
materialized-mismatch, healthy, and halted transition now execute on
`FederationNodeRuntime`. Live Aleph session/network availability is checked by
the runtime as part of authoritative-read admission. `MintState` persists only
the changed status returned by the runtime and retains digest-mismatch metrics.
Status persistence, startup restoration, readiness, and five public-submit
gating regressions pass.

### Checkpoint 5: live-session control

AlephBFT status and pending-mempool reads plus local and authenticated peer
activity signaling now execute on `FederationNodeRuntime`. Session mismatch and
missing-receiver failures remain unchanged. All 11 focused activity-wake tests
and the local status adapter-counter regression pass. Startup recovery remains
at the mint/database boundary by design; live task ownership moves with the
session supervisor in Item 18.

### Checkpoint 6: mint application dependency

An active runtime now holds an `Arc` to the same `cdk::Mint` application used
by Axum; the inert standalone runtime holds no mint. This avoids a new one-use
trait and avoids threading a mint argument through every submission/replay
method. Effective-keyset admission validation is the first mint-dependent
method moved onto the runtime. Rotated-keyset admission and all 28 standalone
Axum regressions pass in both feature modes.

### Checkpoint 7: durable operation transactions

The runtime now owns the atomic database transactions for journal/session
outcome writes, source-sequence persistence, pending Aleph operations, finalized
items plus pending removal, and catch-up session/finalized/journal/evidence
writes. `MintState` retains thin delegators and mint-state application ordering.
All five finalized-unit and all 29 focused catch-up regressions pass after the
transaction move.

### Checkpoint 8: replay and rejection application

Journal replay, transient storage retry/backoff, durable Applied/Failed/Rejected
status updates, standard Cashu `ErrorResponse` rejection persistence, consensus
application-state overlay, and catch-up rejection equality checks now execute on
`FederationNodeRuntime`. The existing error classifier and transaction order are
unchanged. Failed mint/swap replay, rejected-swap response, catch-up rejection
mismatch, and accepted-on-startup replay regressions pass.

### Checkpoint 9: authoritative finalized-unit application

Duplicate-finalization recovery, AlephBFT unit finalization, atomic finalized
item and journal persistence, consensus-state replacement, mint application,
source-sequence advancement, latency recording, and waiter notification now run
as one ordered method on `FederationNodeRuntime`. `MintState` retains only the
thin adapter used by the Aleph session and tests. The existing operation order
and failure boundaries are unchanged. All five focused finalized-unit tests,
including duplicate replay with unmaterialized entries, durable sender sequence
advancement, and recovery after lost waiters, pass.

### Checkpoint 10: consensus submission ownership

The ordered AlephBFT submission path now runs on `FederationNodeRuntime`:
existing-journal and pending-operation duplicate handling, waiter registration,
mempool admission and bounds, durable pending-operation persistence, activity
wake-up, source-sequence retention, test-local finality, timeout reporting, and
submission metrics. `MintState` keeps public readiness/admission and final
Cashu response validation as thin HTTP-facing adapters. Seven focused AlephBFT
submission tests plus duplicate-pending, response-cache, operation-notification,
duplicate-race, and Prometheus acceptance/rejection tests pass. The metric test
also restored its intended observation point: an admitted item is counted after
durable queueing, while a deterministic finalized conflict is counted only when
its rejected journal response returns.

### Checkpoint 11: catch-up reconstruction ownership

Signed session-outcome quorum verification, local-suffix selection,
finalized-item evidence validation, AlephBFT runtime reconstruction, local
application-state overlay, overlap verification, atomic catch-up persistence,
mint replay, and post-replay rejection equality checks now execute on
`FederationNodeRuntime`. Private REST and test helpers retain only thin adapters.
All 29 focused catch-up tests pass, covering malformed, incomplete, and
reordered evidence; digest divergence; rejected-entry continuation; stale local
overlap retries; and mint, swap, melt, payment-observation, and quarantine
materialization.

### Problem

`MintState` directly owns consensus, synchronization, DKG, sequence tracking,
notifications, finalization, admission, pending operations, network inbound,
and live-session state. Its implementation also contains much of the federation
lifecycle, making ownership and shutdown dependencies difficult to follow.

### Cleanup

- Introduce one `FederationNodeRuntime` with explicit construction, startup,
  readiness, submission, synchronization, and shutdown responsibilities.
- Keep HTTP handlers as adapters and mint business logic in `cdk`.
- Avoid a mechanical mega-struct move; split services behind narrow methods and
  preserve standalone `MintState` construction.

### Known locations and completion checks

- `crates/cdk-axum/src/lib.rs`, `federation.rs`, and the federation submodules.
- `docs/federated-cashu-refactoring-plan.md` section R3.
- Prove runtime task ownership, startup failure cleanup, graceful shutdown,
  restart, and non-federated router behavior.

## 18. Introduce an explicit AlephBFT session supervisor

**Status:** Todo

### Problem

A live Aleph session owns several task handles for the session, activity,
network pump, application, and completion watcher and aborts them manually.
Rotation and failure handling are spread across startup and runtime paths.

### Cleanup

- Create a supervisor with explicit states and one cancellation mechanism.
- Centralize session start, completion, automatic rotation, error propagation,
  task joining, and shutdown.
- Ensure an idle session sleeps without reintroducing periodic empty units.

### Known locations and completion checks

- `crates/cdk-axum/src/federation/aleph_bft_session.rs`,
  `aleph_bft_runtime.rs`, and Axum startup wiring.
- `docs/federated-cashu-refactoring-plan.md` section R10.
- Test partial startup failure, task panic/error, idle wake, rotation, restart,
  and shutdown with no detached tasks.

## 19. Collapse duplicate consensus abstraction layers

**Status:** Todo

### Problem

CDK models data providers, finalization handlers, adapters, engines, runtimes,
backend runtimes, and in-memory variants in common code, while the upstream
Aleph integration has another provider/finalizer/runtime stack. Much of this
exists to support multiple backends or the retired external driver.

### Cleanup

- Complete after sections 6 and 7.
- Reduce the production boundary to proposal input, finalized ordered output,
  durable Aleph backup, and journal application.
- Remove one-implementation traits, wrapper enums, aliases, conversions, and
  errors that no longer provide a test or ownership boundary.

### Known locations and completion checks

- `crates/cdk-common/src/federation/consensus.rs` and its tests.
- `crates/cdk-axum/src/federation/aleph_bft_runtime.rs` and session code.
- Prove direct unit-test boundaries remain available without constructing a
  second fake consensus engine.

## 20. Give order and materialized-state digests distinct semantics

**Status:** Todo

### Problem

The journal order digest and a journal state digest both chain operation IDs by
index under different domains, while the same state-digest type is also used
for a full materialized database snapshot. This obscures which invariant each
hash proves and may duplicate work without adding a commitment.

### Cleanup

- Specify the exact threat model and input set of every retained digest.
- Keep one immutable ordering commitment and a distinctly typed
  materialized-state commitment.
- Remove a redundant journal hash only after proving no certificate, catch-up,
  tamper check, or operator comparison relies on an independent commitment.

### Known locations and completion checks

- `crates/cdk-common/src/federation/journal.rs`.
- Axum materialized digest computation, catch-up transport, range evidence, and
  status comparison.
- Add golden vectors and tamper tests for reorder, omission, altered outcome,
  altered rejection, and altered materialized rows before changing hashes.

## 21. Evaluate and remove a derivable operation outcome hash

**Status:** Todo

### Problem

`operation_outcome_hash` is constructed, stored, validated, and compared, but
the initial audit did not find an independent production consumer beyond data
already represented by journal application status and rejection metadata.

### Cleanup

- Document its current security property and every reader before removal.
- If it is fully derivable from retained canonical fields and commitments,
  remove the field, SQL column, codecs, catch-up checks, and fixtures.
- Keep it if the proof shows it independently binds application outcome in a
  way no retained digest provides.

### Known locations and completion checks

- Federation journal types, application finalization, SQL federation storage,
  catch-up verification, and materialized digest construction.
- Use golden and mutation tests to prove any removal does not permit outcome or
  rejection substitution.

## 22. Use one authoritative representation for catch-up history

**Status:** Todo

### Problem

Catch-up responses carry full journal entries and full finalized Aleph items,
plus digests, completion flags, and signed range evidence. The receiver rebuilds
one representation and compares it with the other, creating substantial code
and multiple mismatch modes.

### Cleanup

- Design one authoritative ordered history representation.
- Carry only the additional post-consensus application metadata, rejection,
  materialized commitment, and quorum evidence needed to verify and apply it.
- Do not weaken ordering, range continuity, quorum certification, or state
  verification in pursuit of a smaller payload.

### Known locations and completion checks

- Federation catch-up transport structures in `cdk-common`.
- Catch-up creation, verification, replay, and synchronization in `cdk-axum`.
- Test truncation, duplication, reordered items, altered application metadata,
  stale evidence, partial ranges, restart, and partition recovery.

## 23. Rename session outcome evidence to catch-up range certificates

**Status:** Todo

### Problem

`FederationSessionOutcome` signs arbitrary contiguous journal ranges rather
than describing the output of one Aleph session. The current name collides with
actual Aleph session lifecycle terminology and makes catch-up diagnostics hard
to interpret.

### Cleanup

- Rename the concept, routes, tables, status fields, errors, and documentation
  to `FederationCatchUpCertificate` or another precise range-proof name.
- After renaming, remove fields that are provably derivable without weakening
  signed-domain separation or validation.

### Known locations and completion checks

- `crates/cdk-common/src/federation/transport.rs`.
- Axum evidence collection, sync status/readiness, SQL storage, metrics, and
  progress documentation.
- Confirm logs clearly distinguish Aleph session completion from certified
  journal-range evidence.

## 24. Make materialized-state digest computation scalable

**Status:** Todo

### Problem

Materialized digest construction scans journal history and performs repeated
database lookups for quotes, proofs, blinded messages, completed operations,
shares, and observations. Recomputing this after operations and during status
or catch-up risks O(n-squared) lifetime work and N+1 database access.

### Cleanup

- Benchmark first with growing journal and materialized-state sizes.
- Move projection ownership to the mint domain and batch database reads.
- If still needed, maintain deterministic durable row fingerprints or another
  incremental projection, with periodic full recomputation as an audit.

### Known locations and completion checks

- `crates/cdk-axum/src/federation.rs` materialized digest builder.
- Mint federation application and database projection APIs.
- `docs/federated-cashu-refactoring-plan.md` section R8.
- Prove incremental and full digests agree under apply, rejection, restart,
  catch-up, and deliberate row tampering; record performance measurements.

## 25. Centralize consensus-application transaction ownership

**Status:** Todo

### Problem

Federation mint application opens and commits transactions in many operation
branches, while Axum separately coordinates journal application status,
rejections, digests, and notifications. It is difficult to see which state is
atomic and which failures leave an operation retryable.

### Cleanup

- Introduce one application transaction lifecycle with explicit operation
  context, commit, deterministic rejection, retryable failure, and rollback.
- Keep all operation-specific business decisions in existing mint logic.
- Ensure failed transactions roll back synchronously before their connection is
  released or reused.

### Known locations and completion checks

- `crates/cdk/src/mint/federation.rs` and mint operation modules.
- `crates/cdk-axum/src/lib.rs` journal finalization/application.
- `docs/federated-cashu-refactoring-plan.md` section R6.
- Test injected failure at each write boundary, duplicate apply, lock recovery,
  restart, and catch-up convergence.

## 26. Model journal application state as one explicit enum

**Status:** Todo

### Problem

Status, persisted rejection, and related application metadata are loose fields
that can theoretically represent invalid combinations. Callers repeatedly
interpret these fields to determine replay and response behavior.

### Cleanup

- Introduce an explicit application-state type such as pending, applied,
  rejected with `ErrorResponse`, or retryable failure.
- Encode only states the runtime can actually produce and preserve atomic
  rejected-response persistence from Milestone 26.
- Keep rejected operations out of completed operations.

### Known locations and completion checks

- Federation journal types, SQL codecs, catch-up transport, state digest, and
  public response reconstruction.
- `docs/federated-cashu-refactoring-plan.md` section R7.
- Add serialization and database constraints that reject impossible states and
  verify replay of every valid state.

## 27. Split federation configuration into setup and active phases

**Status:** Todo

### Problem

One `FederationConfig` represents both an incomplete setup/DKG ceremony and a
fully active federation. Validation is repeated across DKG, mintd startup,
mint construction, wallet activation, keyset checks, and digest calculation.

### Cleanup

- Deserialize into a raw type, then validate into separate setup and active
  configuration types.
- Cache the canonical config digest, aggregate BLS keys, threshold parameters,
  and validated member indexes in the active type.
- Make APIs accept the validated phase they require instead of rerunning partial
  checks or relying on empty keysets/transcripts.

### Known locations and completion checks

- `crates/cdk-common/src/federation.rs`, common DKG modules, mintd DKG setup,
  mint builder/startup, and wallet federation connector.
- `docs/federated-cashu-refactoring-plan.md` section R9.
- Prove invalid and non-v3 configurations cannot cross the active boundary and
  benchmark removal of repeated BLS aggregation and digest work.

## 28. Centralize federation payment observation in one service

**Status:** Todo

### Problem

Mint payment observations can be triggered by quote creation, periodic scans,
mint submissions, and status requests, while melt observation follows a
different request-driven path. HTTP handlers currently participate in backend
polling and consensus-observation side effects.

### Cleanup

- Create a backend-neutral `FederationPaymentObserver` service for both mint and
  melt quotes.
- Consume normal CDK payment notifications where available and use bounded
  polling only as repair or for backends without notifications.
- Let handlers notify or query the service without duplicating observation
  submission rules.

### Known locations and completion checks

- `crates/cdk-axum/src/custom_handlers.rs` and federation payment-observation
  modules.
- Mint payment processor abstractions and mint federation observation builders.
- Test success, failure, unknown, delayed backend, duplicate notification,
  restart, offline observer, and eventual catch-up with fake and real-shaped
  backend mocks.

## 29. Build one typed wallet quorum collector

**Status:** Todo

### Problem

Wallet federation code repeats member scheduling, retries, timeouts, threshold
matching, primary fallback, error agreement, response validation, and failure
summaries for mint, swap, melt, restore, and state queries.

### Cleanup

- Introduce a shared scheduler returning typed member IDs, successes, standard
  Cashu errors, transport failures, timeouts, and invalid responses.
- Keep endpoint-specific quorum and validation policies explicit.
- Preserve the standard `ErrorResponse -> Error -> saga -> CLI` pipeline and do
  not add a federation-specific business-error classifier.

### Known locations and completion checks

- `crates/cdk/src/wallet/federation.rs` and its test module.
- `docs/federated-cashu-refactoring-plan.md` section R5.
- Cover threshold success, matching rejection, mixed responses, timeout,
  invalid share, fallback, cancellation, and below-threshold ambiguity for each
  policy family.

## 30. Separate payment quote coordination from generic wallet primary behavior

**Status:** Todo

### Problem

The wallet's `primary_member_id` controls quote creation ordering, single-member
fallbacks, some quote status behavior, and connector ordering. This conflates a
payment coordinator with a general federation authority and can allow one view
to substitute for threshold evidence in paths that deserve separate policy.

### Cleanup

- Inventory every primary-member shortcut and classify it as quote creation,
  payment ownership, availability fallback, or unsafe consensus-view fallback.
- Introduce narrowly named quote/payment coordinator policy where one member is
  genuinely required.
- Remove single-member fallbacks from consensus-dependent reads unless the
  wallet saga explicitly treats the result as provisional or ambiguous.

### Known locations and completion checks

- `crates/cdk/src/wallet/federation.rs`, CLI federation registry/config, and
  wallet federation tests.
- Coordinate this with real payment-member election and failover design.
- Test coordinator outage, delayed responses, quote recovery, status polling,
  and prevention of one-member state or rejection decisions.

## 31. Derive transient federation readiness instead of persisting it

**Status:** Todo

### Problem

Healthy, lagging, catching-up, and evidence-verification states are persisted
and restored even though startup independently verifies journal, Aleph backup,
range evidence, and materialized state. Frequent status writes add database
work and can restore stale operational labels.

### Cleanup

- Identify which status is durable safety state and which is a derived runtime
  projection.
- Derive transient readiness from journal position, verified certificates,
  session state, peer observations, and current faults.
- Persist only durable operator halt/quarantine reasons if required.

### Known locations and completion checks

- Federation member-status database methods, Axum startup recovery, sync loop,
  status routes, and readiness gates.
- Test restart from each readiness condition, stale persisted state removal,
  partition/heal, mismatch quarantine, and reduced database writes.

## 32. Split private REST replay protection by message semantics

**Status:** Todo

### Problem

One sequence tracker and database table cover status, activity, Aleph gossip,
finalization, catch-up, DKG, and operation submission. High-frequency liveness
traffic does not have the same durability needs as effectful ceremony or
administrative requests.

### Cleanup

- Classify every private message as effectful, durable control, idempotent data
  fetch, or transient liveness/gossip.
- Keep durable replay protection only where restart must remember it.
- Use bounded in-memory replay windows for authenticated gossip/activity/status
  and operation IDs for operation idempotency.

### Known locations and completion checks

- Federation REST sequence tracker, transport message kinds, private route
  admission, SQL sequence storage, and peer client.
- `docs/federated-cashu-refactoring-plan.md` section R13.
- Test duplicate, reordered, replayed, restarted, and high-rate traffic for each
  class without weakening authenticated sender admission.

## 33. Separate wallet-facing and member-facing protocol versions

**Status:** Todo

### Problem

Public federation metadata advertises operation, consensus-item, private REST,
and range-evidence versions together. A wallet may reject an otherwise
compatible Cashu federation because members changed an internal transport or
catch-up protocol the wallet never uses.

### Cleanup

- Define the wallet-visible federation/Cashu contract separately from private
  member protocol versions.
- Keep internal versions in member configuration or the payloads they govern.
- Remove duplicated or meaningless cross-layer compatibility checks.

### Known locations and completion checks

- `crates/cdk-common/src/federation.rs`, transport payloads, wallet config
  validation, mint info, setup/DKG transcript, and startup checks.
- Test independent wallet-protocol and member-protocol incompatibility and
  confirm failures occur only at the relevant boundary.

## 34. Split the cdk-common federation module by domain

**Status:** Todo

### Problem

Federation configuration, member identity, thresholds, payment policy, DKG,
operations, journal, transport, consensus, and validation are exposed through a
broad common facade. Large modules and re-export surfaces obscure dependency
direction and make unsafe cross-layer coupling easy.

### Cleanup

- Establish modules for identity/configuration, DKG, ordered operations,
  journal/application state, private transport, certificates, and wallet-facing
  metadata.
- Minimize re-exports and keep transport/Axum concerns out of mint business
  types.
- Perform mechanical moves separately from behavior changes.

### Known locations and completion checks

- `crates/cdk-common/src/federation.rs` and the `federation/` submodules.
- `docs/federated-cashu-refactoring-plan.md` section R11.
- Use compile-time visibility and dependency boundaries to prevent modules from
  regaining inappropriate ownership.

## 35. Split federation database traits and centralize row codecs

**Status:** Todo

### Problem

Federation persistence spans a large method surface and a large SQL
implementation with repeated select lists and conversions. Runtime journal,
Aleph state, payment/signing application, and DKG ceremony storage have
different ownership and lifecycle requirements.

### Cleanup

- Split storage interfaces by coherent owner without creating tiny traits for
  every query.
- Define shared typed row codecs and canonical column lists once.
- Keep transaction-scoped mint application APIs distinct from runtime
  consensus and DKG persistence.

### Known locations and completion checks

- Federation database traits in `cdk-common`.
- `crates/cdk-sql-common/src/mint/federation.rs`, SQLite, and PostgreSQL.
- `docs/federated-cashu-refactoring-plan.md` section R12.
- Run backend parity, transaction rollback, restart, and catch-up tests against
  both SQL implementations.

## 36. Consolidate duplicate federation status DTOs

**Status:** Todo

### Problem

Local status responses and invalid-alert summaries are duplicated between Axum
and mintd. Duplicated serialization types can drift and require repeated
defaults and conversion code.

### Cleanup

- Give the private status wire format one owner and import it in status CLI
  code.
- Separate wire DTOs from display-only mintd structures.
- Remove undeployed compatibility defaults while consolidating the types.

### Known locations and completion checks

- `crates/cdk-axum/src/federation/status_routes.rs` and
  `crates/cdk-axum/src/lib.rs`.
- `crates/cdk-mintd/src/federation_status.rs`.
- Add one serialization fixture and prove status output remains complete for
  healthy, lagging, catching-up, quarantined, and halted members.

## 37. Move generated federation smoke programs out of Rust source

**Status:** Todo

### Problem

Mintd federation setup contains large embedded shell programs, configuration
templates, and smoke-flow text alongside typed setup logic. This inflates Rust
modules and makes shell behavior hard to lint, execute independently, and
review.

### Cleanup

- Move stable scripts/templates into an explicit development tooling directory
  or use `include_str!` with small typed substitutions.
- Keep secret-safe file creation and validated parameter rendering in Rust.
- Ensure generated artifacts remain deterministic and test-only helpers stay
  behind explicit development features.

### Known locations and completion checks

- `crates/cdk-mintd/src/federation_setup.rs`, `federation_smoke.rs`, justfile,
  and generated local federation scripts.
- Shell-check or execute generated scripts, test paths containing spaces, and
  verify no private key or local identifying path reaches committed fixtures.

## 38. Add a federation production build profile

**Status:** Todo

### Problem

Fakewallet, trusted-dealer commands, smoke generators, latency recorders, and
local setup helpers are feature-gated inconsistently or included by convenient
defaults. It is difficult to prove which development machinery ships in a
production federation binary.

### Cleanup

- Define and continuously build a production federation feature set that
  excludes fakewallet, trusted dealer, smoke commands, test recorders, and
  generated local ceremony helpers.
- Keep fakewallet available to standalone development and explicit federation
  test builds.
- Add CI checks for both production-minimal and development-complete builds.

### Known locations and completion checks

- Workspace and mintd Cargo features, CLI subcommands, fakewallet backend setup,
  DKG test utilities, and Aleph latency instrumentation.
- Inspect the production command surface and dependency tree and run a real
  backend-shaped startup test without development features.

## 39. Replace free-form federation infrastructure errors with typed errors

**Status:** Todo

### Problem

Consensus, synchronization, storage, catch-up, and private transport paths use
`Custom(String)`, generic context, and string-based assertions. This weakens
retry classification, observability, tests, and the boundary between Cashu
business errors and federation infrastructure failures.

### Cleanup

- Define structured internal errors with operation/member/session identifiers
  and deterministic sources.
- Preserve standard Cashu `Error` and `ErrorResponse` conversion and wallet saga
  authority for business failures.
- Map typed infrastructure errors to stable HTTP status and operator diagnostics
  without exposing sensitive internals.

### Known locations and completion checks

- Axum federation submission/sync/runtime code, mint federation application,
  wallet federation connector, and SQL adapters.
- `docs/federated-cashu-refactoring-plan.md` section R14.
- Remove string matching from tests and verify retryable, ambiguous, rejected,
  unavailable, and internal failures remain correctly separated.

## 40. Split oversized federation test modules by subsystem

**Status:** Todo

### Problem

The Axum federation test module exceeds twenty thousand lines, while handler,
wallet, common consensus, and setup modules also contain very large inline test
suites. Shared fixtures and unrelated scenarios are difficult to locate and
parallel test behavior is hard to understand.

### Cleanup

- Split tests into admission, Aleph runtime, journal application, catch-up,
  payment observation, wallet quorum, DKG, storage, and end-to-end suites.
- Create focused harness modules with explicit capabilities rather than one
  global fixture collection.
- Keep real-network and multi-process tests out of fast deterministic unit
  suites.

### Known locations and completion checks

- `crates/cdk-axum/src/federation/tests.rs`, `custom_handlers.rs`, wallet
  federation tests, common consensus tests, and mintd federation setup tests.
- `docs/federated-cashu-refactoring-plan.md` section R1.
- Preserve test count/coverage, document suite commands, and prove tests pass in
  isolation and under normal parallel execution.

## 41. Replace source-text architecture tests with enforceable boundaries

**Status:** Todo

### Problem

Some tests inspect Rust or migration source using `include_str!`, source slices,
and substring assertions. These tests are formatting-sensitive and can pass or
fail without proving runtime behavior or type-level architecture.

### Cleanup

- Replace source inspection with module visibility, feature-compilation tests,
  schema introspection, typed API tests, and behavioral integration tests.
- Keep text checks only for generated user-facing artifacts where exact text is
  itself the contract.

### Known locations and completion checks

- Search federation tests and migration tests for `include_str!`,
  `source_slice`, and `.contains` assertions over implementation source.
- Verify forbidden production features fail to compile or are absent from the
  built command surface rather than merely absent as text.

## 42. Replace sleep-driven federation tests with deterministic synchronization

**Status:** Todo

### Problem

Many consensus, session, catch-up, observer, and networking tests depend on
wall-clock sleeps and generous timeouts. They are slow, amplify sandbox/network
variability, and can conceal missed wakeups or ordering races.

### Cleanup

- Use paused Tokio time for timers and explicit notifications, barriers,
  finalized-index waits, or instrumented test transports for state changes.
- Retain a smaller integration layer with real sockets and wall-clock deadlines
  for behavior that cannot be simulated faithfully.
- Do not change production timing solely to make tests pass.

### Known locations and completion checks

- Aleph session/runtime tests, Axum federation tests, payment observer tests,
  wallet HTTP tests, DKG harnesses, and multi-member smoke tests.
- Compare runtime and flake rate before and after, run each suite repeatedly,
  and retain at least one real partition/heal and restart convergence test.

## 43. Rebase the undeployed federation protocol as version 1

**Status:** Todo

### Problem

Federated CDK has not been deployed, but some current federation formats carry
version numbers and hash-domain labels inherited from incompatible development
iterations. In particular, the only supported operation envelope is advertised
as version 2, and DKG commitment, participant-state, and transcript formats also
retain version-2 labels. The implementation does not negotiate or decode the
earlier formats, so these numbers describe discarded development history rather
than a real deployed compatibility boundary.

Keeping that history makes the initial federation protocol appear to have an
upgrade and migration burden that does not exist. It also leaves documentation
inconsistent about whether the first operation encoding is v1 or v2.

### Cleanup

- Treat the complete current federation implementation as the initial version
  1 protocol. In particular, advertise `protocol_versions.operation` as 1 and
  rebase the current operation envelope and operation-ID hash domain to v1.
- Rebase the complete current DKG commitment hash, persisted participant-state
  format, and signed transcript hash domain to v1. Keep the present ceremony
  binding, transcript contents, cryptographic verification, and restart-safety
  requirements unchanged.
- Audit every federation-only wire version, persisted schema version,
  cryptographic domain separator, fixture, generated artifact, and compatibility
  branch. Remove support for superseded development formats instead of adding
  decoders, adapters, translations, or migrations.
- Regenerate operation golden vectors, DKG vectors, public/private federation
  configs, config digests, transcripts, invites, wallet registry fixtures, and
  other artifacts affected by the rebased versions.
- Require operators and developers to recreate experimental federation
  databases, DKG state, configs, and wallet registries. Do not provide migration
  paths from any pre-release federation format.
- Retain explicit version fields, domain separation, and exact-version
  validation. Unsupported or malformed formats must continue to fail closed so
  a future deployed upgrade has a clear compatibility boundary.
- Preserve all pre-federation CDK features, standalone database migration
  history, standalone mint and wallet behavior, and standalone Cashu keyset
  support. Do not confuse federation protocol versions with Cashu keyset
  versions; federation mode remains v3/BLS-only.

### Known locations and completion checks

- `crates/cdk-common/src/federation/operation.rs`: operation envelope version,
  operation-ID hash domain, exact-version admission, and canonical golden
  vector.
- `crates/cdk-common/src/federation.rs`: public protocol advertisement and
  validation, DKG commitment/transcript hash domains, participant-state and
  public-ceremony versions, config digests, and DKG tests.
- `crates/cashu/src/nuts/nut06.rs`, mint-info construction, wallet federation
  validation, mint admission, CLI federation registry/import, setup tooling,
  generated configs, and federation documentation.
- SQLite and PostgreSQL federation schemas and migration registration: retain
  the single consolidated federation migration per backend and every earlier
  standalone migration; do not add an experimental-federation upgrade chain.
- Prove `/v1/info` advertises operation, consensus-item, private-REST, and
  session-outcome version 1, and that current members and wallets accept that
  configuration while altered version 0, 2, and future values fail closed at
  the relevant boundary.
- Prove the canonical v1 operation vector, operation IDs, DKG commitments,
  participant-state restart, transcript equality/signatures, config digests,
  invites, and materialized-state verification are deterministic across all
  members.
- Start fresh SQLite and PostgreSQL federations and run DKG, mint, swap, melt,
  restore, restart, offline-member catch-up, and convergence tests. Confirm old
  experimental federation artifacts are rejected rather than migrated.
- Run standalone migration, mint, wallet, and non-federated keyset regressions
  unchanged, and search the whole workspace for stale v2 labels, obsolete
  compatibility wording, and unlisted federation version constants before
  marking this item complete.

## Cleanup safety guardrails

The following mechanisms may look similar to cleanup candidates but carry
active safety or recovery responsibilities. Do not remove them without a new,
reviewed design that proves the same invariants:

- AlephBFT backup persistence, which provides consensus restart safety.
- Quarantine, quorum range evidence, completed-operation tracking, signature
  share provenance, pending-operation persistence, and durable payment
  observations.
- Separate signature and consensus thresholds.
- `InMemoryFederationJournal` while it remains a deterministic journal view;
  distinguish it from the removable in-memory consensus backend.
- Confidential authenticated DKG transport requirements for secret shares.

When implementing any section above, search beyond the listed locations,
revalidate the cleanup premise against the current code, and update or remove
the entry if later implementation has already changed its assumptions.
