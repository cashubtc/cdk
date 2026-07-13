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

The Axum runtime is now a direct `FederationConsensusRuntime` rather
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
  status, and public `GET /federation/v1/config` tests pass;
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
- `crates/cdk-common/src/federation/transport.rs` and obsolete external-finalization APIs.
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
delete statements. At this checkpoint, the then-current journal hashes, signed
catch-up certificates, redacted row fingerprints, mismatch metrics, and catch-up
digest comparison were unchanged; sections 20 and 21 later removed the
redundant journal hashes without changing the materialized commitment.

Verification:

- all 219 focused `cdk-common` federation tests pass;
- all 22 SQLite federation persistence, fresh-schema, and index tests pass;
- all 99 mint federation tests pass;
- all 420 Axum federation tests pass, including live AlephBFT finalization,
  restart, catch-up, mismatch detection, DKG, and public `GET /federation/v1/config`;
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
deserializing obsolete development shapes. Convergence-capable local status and
catch-up responses require the materialized-state digest, while fast order-only
peer status represents it as absent; journal entries require conflict keys and
the immutable order chain;
materialized rows require their deterministic field map; federation keysets
require their fee; wallet registry entries require the current schema version
and connector policy; and operator status/payment-observation reports require
their current counters, flags, and collections. The status request also carries
its materialized-row choice explicitly, including `false`.

The later digest cleanup in sections 20 and 21 removed the derivable journal
outcome/state hashes and their fresh-schema columns entirely. Missing-field
regressions cover convergence status, catch-up, journal order metadata, keyset
fees, and wallet registry policy.
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
tables, defines the journal order chain from the start, and creates the complete
current index set. The later digest audit removed the derivable journal
outcome/state columns from this same undeployed migration. Every standalone
migration predating
federation is unchanged.

Deleted the 368-line test that searched individual migration source strings for
column names. SQLite and PostgreSQL now open fresh real databases and inspect
the resulting catalog instead: both require exactly one federation migration,
all standalone and federation tables, all current DKG tables, the saga column,
the complete index set, required journal order/conflict fields, and nullable
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

**Status:** Complete

### Checkpoint 1: explicit runtime ownership

`MintState` no longer owns 13 unrelated federation handles directly. One
concrete `FederationNodeRuntime` now owns consensus, peer-sync policy, DKG
admission/config, sequence tracking, readiness, notifications, private REST
admission, invalid-operation alerts, inbound network state, live-session state,
and test finality controls. Standalone mints
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

Duplicate finalized-unit validation and post-application response
reconstruction now execute on `FederationNodeRuntime`. Durable
journal/finalized-item persistence and mint state application remain explicit
`MintState` boundary calls until that whole transactional group can move
together. The temporary pending source-sequence tracking from this checkpoint
was subsequently deleted by Item 32 because operation IDs provide durable
idempotency.

### Checkpoint 4: readiness state machine

Readiness status lookup and every lagging, catching-up, evidence-verification,
materialized-mismatch, healthy, and halted transition now execute on
`FederationNodeRuntime`. Live Aleph session/network availability is checked by
the runtime as part of authoritative-read admission. Readiness is now entirely
derived in memory; the former status writes and startup label restoration have
been removed. Digest-mismatch metrics and public-submit gating remain.

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
outcome writes, pending Aleph operations, finalized items plus pending removal,
and catch-up session/finalized/journal/evidence
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
latency recording, and waiter notification now run
as one ordered method on `FederationNodeRuntime`. `MintState` retains only the
thin adapter used by the Aleph session and tests. The existing operation order
and failure boundaries are unchanged. All five focused finalized-unit tests,
including duplicate replay with unmaterialized entries and recovery after lost
waiters, pass. Durable sender-sequence advancement from the original checkpoint
was subsequently removed by Item 32.

### Checkpoint 10: consensus submission ownership

The ordered AlephBFT submission path now runs on `FederationNodeRuntime`:
existing-journal and pending-operation duplicate handling, waiter registration,
mempool admission and bounds, durable pending-operation persistence, activity
wake-up, test-local finality, timeout reporting, and
submission metrics. `MintState` keeps public readiness/admission and final
Cashu response validation as thin HTTP-facing adapters. Seven focused AlephBFT
submission tests plus duplicate-pending, response-cache, operation-notification,
duplicate-race, and Prometheus acceptance/rejection tests pass. The metric test
also restored its intended observation point: an admitted item is counted after
durable queueing, while a deterministic finalized conflict is counted only when
its rejected journal response returns.

### Checkpoint 11: catch-up reconstruction ownership

Signed catch-up certificate quorum verification, local-suffix selection,
finalized-item evidence validation, AlephBFT runtime reconstruction, local
application-state overlay, overlap verification, atomic catch-up persistence,
mint replay, and post-replay rejection equality checks now execute on
`FederationNodeRuntime`. Private REST and test helpers retain only thin adapters.
All 29 focused catch-up tests pass, covering malformed, incomplete, and
reordered evidence; digest divergence; rejected-entry continuation; stale local
overlap retries; and mint, swap, melt, payment-observation, and quarantine
materialization.

### Checkpoint 12: derived readiness ownership

Every readiness transition is owned in memory by `FederationNodeRuntime`,
including lagging, catching up, quorum verification, materialized-state
mismatch, healthy, and halted states. Startup derives readiness from live Aleph
availability, journal position, and catch-up certificate quorum instead of
restoring an operational label. The catch-up certificate helper accepts only
`&Mint`, the dependency it actually reads, instead of all of Axum `MintState`.
Startup, quorum-prefix, lagging-readiness, and materialized-state-mismatch tests
cover the fail-closed gates.

### Checkpoint 13: DKG ceremony-state ownership

Authenticated sequence advancement, commitment/reveal persistence, secret-share
verification and persistence, transcript-signature validation, activation,
restartable setup-state loading, locally derived transcript hashing, and the
finalized-member-config cache now execute on `FederationNodeRuntime`. DKG routes
and the driver retain thin adapters and orchestration only. All 50 focused DKG
tests pass, including restart at every durable phase, response loss after
persistence, partition recovery, duplicate and conflict handling, cross-ceremony
rejection, transcript/activation checks, admission fairness, and bounded CPU
responsiveness. Live session task lifecycle remains and is coordinated with
Item 18 rather than introducing two competing owners.

### Checkpoint 14: owned session supervisor boundary

The runtime's former live-session handle is now the concrete
`AlephBftSessionSupervisor`. It owns one explicit task set and
Running/Stopping/Stopped state; startup installs the inbound queue only after
all fallible construction succeeds. Shutdown and rotation abort and join the
session, activity broadcaster, network pump, apply loop, completion watcher,
and every upstream Aleph child task. Focused supervisor, upstream spawn, start,
manual rotation, automatic rotation, backup restart, and idle-cadence tests
pass. The final complete gates pass all 420 federation-enabled Axum tests and
all 28 standalone Axum tests, so Item 17 is complete.

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

**Status:** Complete

### Checkpoint 1: deterministic task ownership and shutdown

The existing live-session owner was evolved into
`AlephBftSessionSupervisor`; no parallel lifecycle service or general task
framework was added. It has explicit Running/Stopping/Stopped state, one
shutdown entry point, a single owned CDK task set, and an upstream Aleph task
tracker that waits for active children to exit after abort. Manual and automatic
rotation await shutdown without self-joining the completion watcher. Startup
publishes neither the inbound queue nor the supervisor until construction is
complete. New tests prove all five CDK tasks and pending upstream tasks are
joined, and existing start/rotation/restart/idle tests pass.

### Checkpoint 2: partial-start and root-failure cleanup

All fallible Aleph configuration and peer-client construction now completes
before the first task spawn, and live supervisor/inbound state is published only
after the complete task set exists. The always-present completion watcher
distinguishes normal completion from a caught root-session panic. Panic,
completion without enabled rotation, and failed automatic rotation persist a
halted readiness state; panic also takes the supervisor, joins the remaining
tasks, and clears inbound admission. Test-only fault injection proves a partial
startup publishes no task or inbound state and a root panic reaches Stopped plus
durable Halted. Automatic rotation success remains green.

### Completion

The full 420-test federation Axum suite passes across live multi-member
finality, one-offline windows, partitions, equivocation, restart before and
after backup, manual and automatic rotation, DKG, catch-up, and all value
operations. All 28 standalone Axum tests pass unchanged. The federation library
also compiles warning-free after removing the final test-only dead-code noise.

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

**Status:** Complete

### Review and result

The wrapper problem was real but narrower than the original wording implied.
`FederationConsensusRuntime` forwarded every operation to its sole Aleph
adapter instance, so the wrapper state and adapter type were collapsed into one
concrete runtime. Axum, database tests, and common
consensus tests now use that single type directly. The bounded proposal queue,
unit data provider, and finalized-unit verifier remain as private concrete
components because they enforce distinct size, deduplication, roster,
session-order, and replay invariants and provide useful direct unit-test
boundaries. Axum's similarly named types implement upstream Aleph I/O traits and
only bridge the runtime to `run_session`; they are not a second consensus
engine. No generic trait, backend enum, alias, or one-implementation dispatch
remains at the production consensus boundary.

The public federation facade no longer exports the internal queue, data
provider, or finalization verifier. All 329 `cdk-common` tests pass, including
the direct consensus and database restart/recovery cases. Axum federation test
compilation plus 5 finalized-unit, 7 submission, and 29 catch-up regressions
pass.

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

**Status:** Complete

### Review and result

The accepted-order commitment is now only `FederationJournalDigest`. Each link
hashes its domain, previous digest, zero-based index, and operation ID; the
operation ID already commits the versioned canonical operation envelope. It
detects omission, insertion, reordering, and envelope substitution, and is
immutable after finalization. It deliberately does not claim that local apply
has completed.

The database commitment is now the distinct
`FederationMaterializedStateDigest`. It hashes sorted deterministic rows for
consensus-relevant quote, proof, signature, completed-operation, payment,
keyset, and rejection state. It excludes private shares, local transport state,
logs, caches, and transient apply progress. Signed catch-up certificates and
catch-up responses bind both commitments. Fast peer status carries no
materialized digest; operator/convergence status carries the computed digest,
and mintd rejects its absence.

The second journal chain, which hashed the same `(index, operation_id)` pairs
under another domain, was removed from the entry type, runtime, SQL codecs, and
fresh SQLite/PostgreSQL schemas. Golden vectors preserve the retained order and
materialized algorithms. Reorder, omission, altered rejection, altered
materialized rows, restart replay, and digest mismatch regressions pass.

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

**Status:** Complete

### Review and result

The audit found no independent consumer. `operation_outcome_hash` was a pure
function of entry index, operation ID, previous/order digests, and the removed
duplicate journal state chain. It did not include status or rejection, so it
did not commit the post-consensus outcome its name implied. Ordering tamper is
already rejected by the order chain; application and rejection substitution is
detected by the materialized-state commitment and signed catch-up evidence.

The type, error variants, journal field, validation, retry comparison, SQL
column/codecs, and fixtures were deleted. Fresh-schema tests explicitly assert
that neither removed hash column exists.

Verification passes all 327 `cdk-common` tests, all 420 federation-enabled
Axum tests, 4 focused SQLite journal tests plus the fresh-schema migration test,
42 mintd status tests, 11 mintd digest-comparison tests, 28 standalone Axum
tests, and 134 standalone SQLite tests. Federation-enabled PostgreSQL, CDK,
Axum, and mintd compile gates are clean.

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

**Status:** Complete

### Review and result

Finalized AlephBFT items are now the sole authoritative replayable catch-up
history. The response no longer duplicates full journal envelopes, conflict
keys, and journal consensus evidence. Instead, `FederationCatchUpOperation`
carries only the operation index and ID, order-chain links, local application
status, and retained standard Cashu rejection needed to bind range evidence and
verify post-replay application results.

The compact projection validates contiguous indexes, every order-digest link,
and rejection/status consistency before certificate or quorum verification.
The receiver reconstructs full journal entries exclusively from finalized
AlephBFT items, compares the compact projection with that reconstruction, then
applies mint state and verifies rejected `ErrorResponse` equality. Independent
operation and finalized-item cursors remain because non-operation and duplicate
Aleph items do not have a one-to-one journal row.

Catch-up response construction now requires an explicit materialized-state
digest; the production route computes the real digest before signing rather
than constructing a placeholder genesis value and overwriting it later. The
test-only adapter that fabricated Aleph units from duplicated journal envelopes
was deleted, so catch-up tests exercise the same finalized-item reconstruction
boundary as production.

Verification passes all 327 `cdk-common` tests, all 420 federation-enabled Axum
tests, all 155 SQLite tests, and all 28 standalone Axum tests. PostgreSQL and
mintd federation compile gates are clean. Coverage includes compact-wire shape
and obsolete full-entry rejection, commitment tampering, missing, incomplete,
and reordered finalized evidence, stale overlap, deterministic rejection
equality, mint/swap/melt materialization, restart, live partitions, and digest
divergence.

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

## 23. Use catch-up certificate terminology consistently

**Status:** Complete

### Review and result

The range proof is now `FederationCatchUpCertificate` everywhere. The same
vocabulary is used by public protocol-version metadata, private REST responses,
signature-domain separation, error variants, consensus items, database traits,
fresh SQLite/PostgreSQL schemas, peer sync, readiness states, status JSON,
metrics, mintd config and CLI guards, generated smoke scripts, tests, and
documentation. Logs now say "catch-up certificate" while Aleph session logs
continue to describe actual session lifecycle and finalization.

This is a clean undeployed format change. There are no compatibility aliases,
dual JSON fields, legacy status labels, old federation tables, or schema-copy
migrations. Obsolete certificate JSON is rejected through strict unknown-field
handling, and the signed domain is explicitly
`cdk-federation-catch-up-certificate-signature-v1`.

The field audit removed three derivable certificate fields:
`operation_count`, `latest_index`, and `range_order_digest`. They are computed
from `from_index`, `previous_order_digest`, and the ordered operation IDs, while
the signed transcript continues to bind the same derived values. Their unused
SQL columns were also removed; only indexed range identity and canonical
certificate JSON remain. Tests prove the derived values match the returned
operation range and reject obsolete redundant JSON fields.

Verification passes all 327 `cdk-common` tests, all 420 federation-enabled Axum
tests, all 155 SQLite tests, 42 mintd federation-status tests, 15 mintd setup
tests, 8 Prometheus tests, all 546 Cashu tests, and all 28 standalone Axum
tests. PostgreSQL and mintd federation compile gates are clean.

### Problem

The range proof signs arbitrary contiguous journal ranges rather than
describing the output of one Aleph session. Reusing lifecycle terminology for
it made catch-up diagnostics hard to interpret.

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

**Status:** Complete

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

### Completion record

The canonical projector now belongs to the mint domain in
`crates/cdk/src/mint/federation/projection.rs`; Axum no longer matches on mint
operations or constructs mint database rows. It scans accepted journal entries
once and issues bounded bulk reads for the referenced state. Mint quote
projection uses a minimal durable-state view in 500-ID chunks, avoiding payment
and issuance histories that are not part of the convergence commitment. SQL
tests prove input ordering, missing rows, aggregate counters, derived quote
state, and all-observation ordering.

The quote-only 1,000-operation benchmark improved from 44,419us to 27,366us in
debug and from 5,917us to 4,231us in release; a repeat release sample was
4,091us. The important scaling change is a reduction from roughly 3,000 quote
and child-history reads to five bounded reads. Reusing the full multi-quote API
and batching full history vectors were rejected after release regressions.
Durable incremental fingerprints were also rejected for now: measured full
recomputation is linear and about 4ms at 1,000 rows, so another durable state
machine is not justified. Full measurements and rejected-candidate evidence are
recorded in `docs/federated-cashu-materialized-state-benchmark.md`.

Verification passes 327 common tests, 156 SQLite tests, all 420 non-ignored
federation Axum tests (the two loopback cases rerun with network permission),
28 standalone Axum tests, and PostgreSQL plus mintd federation checks.

## 25. Centralize consensus-application transaction ownership

**Status:** Complete

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

### Completion record

Federation mint application now has two narrow transaction finalizers rather
than a wrapper type or callback framework. Every transaction opened by the
mint federation appliers passes its body result to an explicit finalizer. A
successful terminal apply writes `Applied` in the same transaction as its mint
rows before commit; any handled body or status-write error explicitly rolls
back before returning. Operation ID, operation kind, and phase are attached to
commit/rollback failure logs. Mint quote, mint payment observation, issuance,
swap, melt quote, melt observation evidence/state, quarantine, key rotation,
and terminal melt status all use this lifecycle. The redundant second
`Applied` transaction for a `PaymentFailed` melt was removed.

The runtime's post-rollback failure path is consolidated separately because it
must preserve the existing Cashu error classification. One function now
persists either retryable `Failed` or canonical `Rejected` plus
`ErrorResponse`, explicitly rolls back a failed write, and only then updates
the in-memory consensus view. No federation-specific error classifier was
introduced.

A database transaction intentionally does not span payment observation
evidence, an outgoing payment, or the existing melt saga phases. Those are
external-side-effect and crash-recovery boundaries, so claiming one atomic
transaction would be incorrect. Each local transaction now has explicit
finalization, while operation IDs, durable saga state, idempotent quote checks,
and replay preserve exactly-once payment behavior across the staged lifecycle.

Verification passes all 100 mint federation tests, including late mint/swap
write failure, failed/paid melt replay, concurrent melt-quote apply, duplicate
apply, and the new after-write observation rollback regression. The complete
federation Axum matrix passes all 420 non-ignored tests (418 in the restricted
sandbox plus two permissioned loopback tests), standalone Axum passes 28/28,
and mintd federation compilation is clean.

## 26. Model journal application state as one explicit enum

**Status:** Complete

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

### Completion record

`FederationJournalEntry` now has one `FederationApplicationState` field rather
than independently mutable status and rejection fields. The legal states are
accepted, applying, applied, retryable/ambiguous failed, consensus-conflict
rejected, and deterministic mint rejected with the existing Cashu
`ErrorResponse`. The two rejection variants are intentionally distinct:
consensus-conflict rejection has no wallet error, while mint rejection always
carries the response reconstructed by public routes and wallet fanout.

The accepted-order fields and digest remain unchanged. Application metadata is
still post-consensus and stored in the existing journal row; no table,
consensus item, AlephBFT round, or compatibility layer was added. One typed
database setter now derives both the existing `status` and `rejection_json`
columns atomically. SQLite and PostgreSQL fresh federation schemas constrain
status labels and reject rejection JSON on non-rejected rows. SQL decoding also
rejects impossible column combinations.

Catch-up now transports and compares the explicit application state. Journal
reconstruction also preserves the conflict-key reservation of an operation
that consensus accepted before mint logic rejected it, while a
consensus-conflict rejection remains non-reserving. This fixes a distinction
that the former flattened rejected status could not represent after restart.
Rejected operations remain absent from `completed_operations`, and persisted
business rejections remain part of the materialized-state digest without
entering the accepted-order hash.

Serialization rejects extra outcome fields and a business rejection without
an `ErrorResponse`. Shared database tests round-trip and replay all six legal
states in a valid journal sequence. Verification passes 329 common tests, 157
SQLite tests, 100 mint federation tests, the complete permissioned Axum matrix
of 420 passing tests plus one ignored manual benchmark, and all 28 standalone
Axum tests. The PostgreSQL federation test target compiles; its runtime suite
still requires the repository's external PostgreSQL test service.

## 27. Split federation configuration into setup and active phases

**Status:** Rejected

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

### Review record

The performance premise was valid earlier in development but is obsolete in
the current runtime. Full config validation, including threshold BLS
interpolation and transcript checks, is now confined to trust boundaries such
as external import, mint startup, DKG record assembly, and checked public
constructors. `FederationRuntimeConfig` retains the fully checked public and
private configs plus a precomputed digest. `FederatedMintConnector` retains its
digest and member maps after construction. Aggregate keys are already stored in
each `FederatedKeyset`; request paths do not reconstruct them.

Milestone 20A measured why those boundaries matter. At 20-of-14 with eight
denominations, complete activation validation took about 1.300 seconds while
the retained validated-config activation path took about 1.527 milliseconds,
an 851x improvement. That optimized path is already in use after the fallible
finalized-config cache has performed full validation. Mint-info likewise uses
the startup-validated cached digest rather than rerunning interpolation.

A setup config is intentionally installed in the mint before keysets and a
signed transcript exist so the authenticated DKG runtime has the roster,
identity keys, thresholds, and payment policy it needs. Durable transcript
signatures and all-member activation records govern replacement with the final
config. Separate raw, validated-setup, and validated-active wrappers would not
remove this lifecycle or any current branch; they would add conversions and
ownership plumbing across common DKG, transport, mint builder/runtime, Axum,
mintd, CLI registry persistence, wallet connector construction, and a large
fixture surface.

This is therefore not a simplifying cleanup in the current codebase. Keep the
existing checked trust boundaries, cached runtime values, DKG finalized-config
cache, exact activation checks, and v3/BLS admission tests. Reconsider a common
active-config wrapper only if a concrete bypass bug appears or CDK gains a
long-lived wallet-config cache that can consume it without adding another
parallel configuration model.

## 28. Centralize federation payment observation in one service

**Status:** Completed (2026-07-12)

### Result

Confirmed the split ownership and replaced it with one backend-neutral
`FederationPaymentObserver`, owned by `FederationNodeRuntime`. Mint and melt
notifications, repair scans, quote-status requests, immediate post-quote probes,
and pre-signing mint checks now all use the same probe, duplicate suppression,
quorum check, envelope construction, and consensus-submission path. Production
HTTP handlers no longer contain their own payment-observation builders or
submission rules.

Normal CDK payment event streams remain active in federation mode. Instead of
running standalone quote or saga mutations, the mint forwards events through a
bounded 256-entry broadcast channel. The observer targets the affected quote,
re-queries the backend rather than trusting the notification payload as durable
evidence, and submits the result through consensus. A lagged channel, startup
race, backend without notifications, or temporarily offline observer is repaired
by the five-second scan; backend probes are grouped in batches of at most 100.
Federation pubsub reads likewise avoid the standalone backend-query side effect.
Standalone event handling and saga recovery are unchanged.

Accepted external melts now reserve the quote, proofs, outputs, and saga as
`Pending` without performing backend I/O during deterministic consensus apply.
The deterministically elected member initiates payment through the observer;
other members observe according to the configured policy; an accepted
`MeltQuotePayment` operation alone finalizes or rolls back the saga. Fake wallet
uses this same backend-neutral path. Catch-up and restart replay observations
without repeating payment, while `Pending` and backend `Unknown` remain
recoverable rather than durable terminal quote states.

The wallet now keeps the elected payer in its bounded melt connector cohort and
does not return a pending threshold view before that member responds. This fixes
the liveness hole where keyset-validation threshold selection or two fast
pending members could cancel the only member able to initiate payment. Terminal
threshold responses still win immediately, and ambiguous mixed views remain
recoverable.

Verification:

- all 100 mint federation tests and all 84 wallet federation tests pass;
- all 91 federation handler tests pass, including notification-before-consensus,
  success, failure, unknown, delayed settlement, duplicate submission, restart,
  catch-up, and real HTTP connector coverage;
- all 41 live AlephBFT tests pass, including below-finality recovery and the
  four- and five-member one-offline payment windows with exactly one external
  payment;
- all 417 non-ignored federation-enabled Axum tests pass (the one ignored test
  is the manual materialized-state benchmark);
- standalone payment-stream retry and all 12 standalone custom-handler tests
  pass, and federation-enabled core/Axum checks are warning-free apart from an
  unrelated deprecated test helper warning.

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

**Status:** Completed

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

### Resolution

- Kept the existing `collect_member_results` scheduler as the single bounded
  fanout implementation for mint, swap, melt, restore, quote status, batch
  quote status, and check-state. It already centralizes member identity,
  concurrency, retries, per-member timeouts, typed `Error` retention, failure
  summaries, and cancellation by dropping outstanding futures.
- Extended its endpoint policy callback to observe typed failures as well as
  successful responses. Mint/swap and melt now terminate as soon as either a
  cryptographically valid success threshold or an exact standard
  `ErrorResponse` rejection threshold is available; one member and mixed
  rejections remain ambiguous.
- Left BLS aggregation, matching quote/status views, restore reconstruction,
  check-state views, and permitted primary fallback explicit in their endpoint
  collectors. A `QuorumPolicy` trait, parallel outcome hierarchy, and generic
  impossibility solver were rejected as complexity without a demonstrated
  behavioral gain.
- Added a 3-of-5 liveness regression with two 30-second members proving that a
  matching Cashu rejection returns immediately and cancels outstanding work.
  The complete 84-test federation wallet module passes, including threshold
  success, exact/mixed rejection, retry, timeout, invalid-share, fallback,
  cancellation, restore, check-state, mint, swap, and melt coverage.

## 30. Separate payment quote coordination from generic wallet primary behavior

**Status:** Completed

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

### Resolution

- Renamed the wallet and CLI preference from `primary_member_id` /
  `--primary-member-id` to `quote_coordinator_member_id` /
  `--quote-coordinator-member-id`. This undeployed federation code
  intentionally carries no compatibility alias. Item 43 later supersedes only
  the development-history version number: the complete current registry shape
  becomes initial v1 instead of v2, without accepting the old field or flag.
- Restricted the preference to ordering quote-creation attempts. A failed or
  unavailable coordinator falls back through the validated roster using the
  same assigned quote ID and existing threshold-visibility recovery.
- Removed coordinator ordering from mint-info, LNURL, and auth-wallet lookup.
  Those availability-only calls now use deterministic roster order and confer
  no coordinator authority.
- Removed single-member mint, batch-mint, and melt quote-status fallbacks and
  the matching one-response aggregator branches. Public status and saga
  recovery now require a matching threshold view; unavailable quorum stays an
  ordinary ambiguous wallet error so existing saga recovery remains in charge.
- Updated tracked CLI smoke commands and added explicit one-member rejection
  tests for all three quote-status families. All 84 federation wallet tests and
  all 27 serial CLI federation tests pass, including coordinator outage,
  delayed/ambiguous quote creation, threshold status, and recovery coverage.

## 31. Derive transient federation readiness instead of persisting it

**Status:** Completed

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

### Resolution

- Removed `FederationMemberStatusKind`, `FederationMemberStatusSnapshot`, the
  mint database trait methods, SQL row conversion and queries, SQLite/Postgres
  backend tests, and the undeployed `federation_member_status` schema table.
  Readiness transitions no longer open database transactions.
- Kept healthy, lagging, catching-up, catch-up-certificate verification,
  materialized-state mismatch, and runtime halt as one in-memory
  `FederationSyncStatus` state machine owned by `FederationNodeRuntime`.
- Removed the startup status-restore pass. After the live Aleph session is
  available, startup derives readiness from journal length and verified catch-up
  certificate quorum: empty or fully certified history opens healthy; replayed
  uncertified history remains fail-closed in verification.
- Durable safety evidence remains in its real sources: finalized journal and
  Aleph items, catch-up certificates, materialized-state digest comparison,
  payment-observation quarantine operations, and Aleph backup/session records.
  A stale operational label can no longer reopen or indefinitely halt a member.
- Seventeen focused startup/restart tests pass, including transient lag discard,
  no-live-session halt, empty-history activation, replay without quorum,
  restore/check-state gating, and replay with quorum. The SQLite empty-schema
  migration test and federation-enabled PostgreSQL build also pass. Final gates
  pass all 329 shared-core tests, all 156 SQLite tests, all 416 non-ignored Axum
  federation tests (two loopback tests run with network permission), and all 28
  standalone Axum tests; one manual benchmark remains ignored.

### Security follow-up (2026-07-13)

The security review found that the count-only startup reconstruction discarded
the materialized-state digest authenticated by catch-up certificate quorum.
Startup could therefore reopen Healthy after restart without proving the local
projection still matched certified evidence. The follow-up retains the latest
digest across a contiguous certified prefix, recomputes the local projection
while readiness remains fail-closed, and publishes one final status only if the
startup state has not been superseded. Matching evidence opens Healthy;
incomplete coverage remains verifying; mismatched evidence remains
`MaterializedStateMismatch`; and storage/projection failures remain Halted.
Focused shared-core certificate, Axum quorum-prefix, matching startup, and
mismatched startup/read-gating tests pass. No general persisted readiness
snapshot was restored.

## 32. Split private REST replay protection by message semantics

**Status:** Completed

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

### Resolution

- Classified effectful DKG commitment, reveal, secret-share,
  transcript-signature, and activation requests as one durable sender stream.
  Renamed the undeployed SQL table and database API to
  `federation_dkg_member_sequences` so this storage cannot be mistaken for a
  generic private-REST ordering mechanism.
- Split status, catch-up, DKG readiness, AlephBFT activity, and AlephBFT network
  messages into independent per-kind in-memory replay windows. Each window
  retains at most 4,096 recent sender sequences and rejects traffic older than
  its bounded reorder history; high-rate gossip cannot advance or grow the
  replay state of another message kind.
- Removed the operation-to-source-sequence map, sequence writes during AlephBFT
  finalization, finalized-unit sequence plumbing, and the generic
  submit-with-sequence methods. Authenticated operation submissions use their
  canonical operation ID plus durable pending/finalized records for idempotency,
  including after restart; REST sequence numbers are not consensus order. The
  signed `SubmitOperation.sequence` is therefore transport provenance rather
  than a replay watermark, and distinct valid operation IDs may reuse it. The
  shared sequence tracker encodes that exception and cannot allocate a
  `SubmitOperation` replay window.
- Kept signed sender, receiver, federation, kind, body-hash, request-size, and
  operation admission validation unchanged. Exact signed operation replays
  return the durable duplicate outcome, while two separately signed operations
  cannot alias even when a sender reuses a transport sequence.
- R13's proposed typed extractor remains deferred: the semantic split was
  worthwhile, but adding a trait/extractor hierarchy now would add code without
  removing the route-specific acceptance points that DKG and liveness traffic
  require.
- Verification passes all 332 `cdk-common` federation tests, all 156 SQLite
  federation tests, all 417 non-ignored Axum federation tests in the serial
  loopback-enabled gate, and all 28 standalone Axum tests; one manual benchmark
  remains ignored. A parallel Axum run saturated the test host and missed three
  live-session wall-clock bounds, but each passed in isolation and all three
  passed again in the full serial gate.

## 33. Separate wallet-facing and member-facing protocol versions

**Status:** Completed

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

### Resolution

- Replaced the public four-component `FederationProtocolVersions` bundle with
  one required `wallet_protocol_version` in `FederationConfig` and
  `GET /v1/info`. Wallet import, connector construction, mint startup, config
  digest validation, and member-advertisement checks now gate only the contract
  wallets actually interpret.
- Kept member compatibility at each governing artifact instead of adding a
  second aggregate version struct: operation envelopes, consensus items,
  private signed REST requests, catch-up certificates, and AlephBFT unit
  payloads each retain exact local version validation.
- Removed `FederationProtocolInfo`, the aggregate public config field, its
  cross-layer conversion and comparison code, and the duplicate
  `protocol_versions` and `consensus_item_version` fields from AlephBFT unit
  payloads. Embedded consensus items remain independently version checked.
- The public config and mint-info JSON now contain no private REST,
  consensus-item, operation, or catch-up version advertisement. A future
  member-only protocol change therefore cannot invalidate an otherwise
  compatible wallet config or change its wallet compatibility decision.
- Verification passes all 548 Cashu tests, all 332 shared federation tests, all
  84 wallet federation tests, all 100 mint federation tests, all 17 CLI
  federation registry/discovery tests, focused Axum public-config,
  finalization, catch-up, and live private-REST finality tests, and all 28
  standalone Axum tests. The federation-enabled PostgreSQL test target compiles.

## 34. Split the cdk-common federation module by domain

**Status:** Completed (2026-07-13)

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

### Review decision

- The problem still exists: `federation.rs` is 7,286 lines and owns errors,
  identity/configuration, payment policy, keysets, most DKG records, invites,
  snapshots, and wallet-facing validation, while operation, journal, transport,
  consensus, signing, and trusted-dealer code are already separate modules.
- Items 37-42 are complete. Item 43 was later reopened for a narrow v1 constant,
  domain, and fixture reset; that work uses the completed module boundaries and
  does not reopen this structural split.
- Use a few coarse modules, preserve every serialized/hash form, and avoid the
  fine-grained file hierarchy sketched in R11 unless a compile-time dependency
  boundary requires it. This is a mechanical ownership cleanup, not an excuse
  to alter federation behavior or wire formats.

### Resolution

- Extracted federation identity, threshold, payment-policy, keyset,
  public/private config, invite, and snapshot ownership into a private
  `federation/config.rs` module.
- Extracted dealer-free ceremony records, participant state, commitment rounds,
  transcript verification, and finalized DKG config assembly into a private
  `federation/dkg.rs` module.
- Kept the established `cdk_common::federation::*` public paths through the
  existing facade. The new modules use explicit imports across config, DKG,
  operation, and transport boundaries; no Axum type or concern entered the
  common domain.
- Left operation, journal/application state, consensus, signing, private
  transport/certificates, DKG orchestration, and trusted-dealer ownership in
  their existing focused modules. A finer file-per-type split and public API
  relocation were rejected as churn without a stronger dependency boundary.
- Preserved every struct field, serde representation, canonical encoding, hash
  domain, protocol version, and validation path. The only internal visibility
  adjustments are `pub(super)` accesses that replace same-file private access
  between the new sibling modules and existing tests.

### Verification

- All 332 `cdk-common` federation-enabled tests pass, including operation-ID,
  journal-digest, config, DKG transcript/signature, restart, and trusted-dealer
  coverage.
- All 187 focused `cdk` mint and wallet federation tests pass.
- The complete federation-enabled Axum suite passes 419 tests with one manual
  benchmark ignored; all 28 standalone Axum tests pass unchanged.
- Federation-enabled PostgreSQL tests compile, the SQLite/fake-wallet mintd
  federation target compiles, and `cdk-common` compiles without federation or
  default features.
- A warning-denying Clippy run remains blocked by pre-existing federation and
  standalone lints outside this mechanical move; normal warning-free checks for
  the changed shared crate pass.

## 35. Split federation database traits and centralize row codecs

**Status:** Completed

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

### Review decision

- The ownership problem still existed, but splitting every query into a narrow
  trait would have increased public API surface without narrowing production
  dependencies. All mint callers use the composite mint database, and SQL is
  the only federation persistence implementation. The retained split therefore
  has exactly three coherent families: ordered journal/runtime state,
  post-consensus mint application data, and DKG ceremony state.
- `FederationJournalTransaction` and `FederationJournalDatabase` now contain
  only config, ordered journal, catch-up certificate, AlephBFT, pending proposal,
  and durable effectful-DKG replay state. New application and DKG transaction
  and read traits own their respective methods, while the existing composite
  mint `Transaction` and `Database` traits preserve the caller-facing API.
- The generic SQL implementation is divided into `federation/journal.rs`,
  `federation/application.rs`, and `federation/dkg.rs`. Shared validation and
  row decoders remain in the parent module, so SQLite and PostgreSQL cannot
  acquire separate conversion behavior.
- The fragile nine-column journal row and ten-column finalized-Aleph-item row
  each have one compile-time static SELECT shape paired with one typed decoder.
  All retrieval variants reuse those shapes. Simpler one-column and JSON-only
  queries remain explicit; no dynamic SQL builder or duplicate
  `FederationJournalRow` mirror was added because either would increase code
  without strengthening the current typed `FederationJournalEntry` decoder.

### Verification

- `cargo test -p cdk-sqlite --features federation -- --test-threads 1`: all
  156 tests passed, including the 22 config, journal, concurrent-write,
  application-state, payment, signature-share, DKG, rollback/recovery,
  fresh-schema, and query-plan federation tests plus the established mint and
  wallet storage regressions.
- `cargo test -p cdk-axum --features federation catch_up -- --test-threads 1`:
  all 41 catch-up, restart-readiness, rejection-metadata, and materialized-state
  verification tests passed with loopback permission.
- `cargo test -p cdk-axum --features federation dkg_driver -- --test-threads 1`:
  all 21 authenticated DKG partition, retry, durable-phase restart, and
  activation tests passed with loopback permission.
- Federation-enabled SQLite and PostgreSQL checks pass, the full PostgreSQL
  test target compiles, and standalone SQLite compiles without the
  federation feature. A live PostgreSQL persistence test was attempted but the
  local test server was not running, so no live PostgreSQL result is claimed.

## 36. Consolidate duplicate federation status DTOs

**Status:** Completed

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

### Review decision

- The duplication still existed exactly: Axum and mintd each declared the same
  detailed local-status response and invalid-operation alert summary. The
  canonical private REST wire records now live beside `FederationPeerStatus` in
  `cdk-common` transport, where both producer and consumer already share their
  other signed status payloads.
- Axum retains ownership of deriving synchronization state, runtime counters,
  payment reports, and alert summaries. Mintd retains only query, validation,
  guard, and display behavior. Neither crate defines a second serialization
  shape or conversion layer.
- Every current field remains required on deserialization unless it is an
  explicitly optional value. No undeployed compatibility defaults or fallback
  decoding were added.

### Verification

- Workspace search finds one `FederationLocalStatusResponse` and one
  `FederationInvalidOperationAlertSummary` definition, both in
  `cdk-common/src/federation/transport.rs`.
- `cargo test -p cdk-common --features federation federation::transport -- --test-threads 1`:
  all 44 private transport tests passed, including an exact 27-field local
  status JSON fixture with optional values, Aleph counters, row fingerprints,
  and invalid-operation alerts.
- `cargo test -p cdk-axum --features federation private_local_status_route_reports_ -- --test-threads 1`:
  all eight focused reporting tests passed for lagging, catching-up, catch-up
  verification, payment conflicts, stale payment observations,
  invalid-operation alerts, and redacted halted state. Separate focused healthy
  and quarantined-payment local-status route tests also passed.
- `cargo test -p cdk-mintd --features federation,sqlite federation_status -- --test-threads 1`:
  all 42 status query, display, convergence, readiness, catch-up evidence,
  payment alert, invalid-operation alert, and redaction tests passed.
- `cargo check -p cdk-mintd --features federation,sqlite` passes without
  warnings; standalone builds do not compile or expose these federation-only
  records.

## 37. Move generated federation smoke programs out of Rust source

**Status:** Completed

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

### Review decision

- The problem still existed in `federation_setup.rs`: two Bash programs totaling
  roughly 1,500 lines and three stable TOML templates were embedded in Rust
  format strings. They now live under `crates/cdk-mintd/dev/federation` and are
  included only by the explicit fakewallet federation development build.
- Rust still owns every generated mnemonic, path, URL, member ID, unit, port,
  schema name, and member-case block. Existing TOML encoding and shell
  single-quote helpers run before rendering, so extraction did not move secret
  creation or trust input strings to shell.
- One small fail-closed renderer requires each declared marker exactly once,
  rejects duplicate substitutions and unknown markers, and performs no
  evaluation. Multiline insertion markers are valid Bash comments in the raw
  templates, allowing direct syntax inspection as well as generated-script
  checks.
- `federation_smoke.rs` remains Rust. It contains typed wallet/mint orchestration
  and state assertions rather than embedded script text; moving it would hide
  behavior and add indirection without reducing generated baggage.

### Verification

- Both committed `.sh.in` templates pass `bash -n`; generated scripts also pass
  the existing `bash -n` checks after every marker is rendered.
- `cargo test -p cdk-mintd --features federation,fakewallet,sqlite federation_setup -- --test-threads 1`:
  all 16 core/setup tests passed, including fail-closed renderer cases,
  deterministic core artifacts, restrictive private-file permissions, SQLite
  and PostgreSQL config generation, four/five-member matrices, and generated
  scripts rooted in a path containing spaces.
- `cargo check -p cdk-mintd --no-default-features --features federation,sqlite`
  passes, proving production-minimal federation compilation does not require or
  expose fakewallet setup rendering.
- `cargo package -p cdk-mintd --list --allow-dirty` includes all five templates.
  Workspace search finds no private key, credential, or machine-local path in
  the committed templates; generated secret-bearing files remain runtime
  artifacts protected by the existing restrictive writers.

## 38. Add a federation production build profile

**Status:** Skipped

**Decision:** Do not add a dedicated federation production build profile in
this cleanup pass. The existing explicit `--no-default-features` federation
build remains the supported way to inspect a minimal binary, while deployment
feature selection stays with the operator. No Cargo feature aliases, compile
guards, command-surface changes, or CI jobs are required for this item.

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

**Status:** Completed

**Decision:** Apply this selectively at classification boundaries. Do not add
parallel `FederationRuntimeError`, `FederationConsensusError`, and
`FederationTransportError` taxonomies merely to wrap diagnostics. Existing
Cashu business errors, wallet ambiguity messages, storage errors, and leaf
startup diagnostics remain in their established types when no caller branches
on their display text.

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

### Implementation result

- Added structured variants to the existing `FederationError` for inactive
  runtime components, each non-ready synchronization state, missing finalized
  cursors, invalid AlephBFT quorum configuration, and session rotation blocked
  by pending operations. The variants retain member, operation-count, session,
  threshold, and component fields where those values govern control flow.
- `FederationNodeRuntime` no longer constructs `Error::Custom` for readiness,
  missing runtime handles, activity admission, consensus submission, catch-up,
  DKG runtime access, or duplicate finalized-unit recovery. Replay corruption
  uses the existing typed finalized-item recovery error.
- Peer status and catch-up validation now reuse typed REST, member, federation,
  cursor, digest, no-progress, and materialized-state errors. The existing
  private REST timeout/backpressure error remains the transport authority;
  tests downcast it instead of parsing prose.
- Private member REST maps typed conflicts to `409`, bounded admission to
  `429`, unavailable/readiness failures to `503`, malformed requests to `400`,
  and local recovery invariants to `500`. Internal response bodies are generic;
  operator diagnostics pass through the existing secret/path redactor.
- Public Cashu routes still use the normal `Error -> ErrorResponse` conversion.
  Runtime failures remain ambiguous under the unchanged wallet classifier,
  canonical business rejections remain authoritative, and halted-route details
  preserve the already-redacted operator reason.
- SQL adapters keep the established storage error type. Wallet collectors keep
  final `InvalidMintResponse` diagnostics only after typed quorum decisions;
  neither layer classifies errors by display text, so wrapping them again would
  increase code without improving behavior.

### Verification

- All 334 `cdk-common` federation-enabled library tests pass, including the new
  runtime-unavailability ambiguity regression.
- The complete federation-enabled Axum run passed 415 unaffected tests and
  identified six stale public-readiness wording expectations; after preserving
  the redacted halt detail and matching direct errors by variant, all six pass
  in focused reruns. The one manual benchmark remains ignored.
- Focused peer timeout/backpressure, cursor, catch-up, readiness, replay,
  mempool, waiter, session-rotation, HTTP classification/redaction, and
  AlephBFT-threshold tests pass.
- All 28 standalone Axum library tests pass with federation disabled.

## 40. Split oversized federation test modules by subsystem

**Status:** Skipped (2026-07-12)

This organizational refactor is intentionally skipped. Splitting the existing
test files would create substantial test-only churn without simplifying the
production federation or improving its behavior. The current subsystem tests
remain in place; Item 42 targets the higher-value test reliability work by
replacing timing-dependent synchronization without first relocating the suites.

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

**Status:** Completed (2026-07-12)

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

### Resolution

- Removed Rust-source scanners that counted signing calls, sliced handler
  bodies, inferred call ordering, checked nearby `cfg` attributes, searched
  manifests, or searched production modules for backend names. Those checks
  were sensitive to formatting and comments and did not enforce the claimed
  runtime boundary.
- Consensus-gated mint quote, mint, swap, melt quote, and melt behavior remains
  covered by route tests that assert no work is queued or persisted before
  AlephBFT admission. Restore, replay, duplicate handling, transactional saga
  effects, signature-share provenance, and standalone paths retain their
  existing behavioral suites.
- The production-style mintd feature matrix now compiles with
  `federation,sqlite` and without `fakewallet`. A parser regression proves the
  development smoke command is absent from that built command surface.
- Replaced migration-source matching with a migrated SQLite schema test that
  successfully inserts `UNPAID`, `PENDING`, `PAID`, and `FAILED` melt quotes
  and proves the database rejects transient backend `UNKNOWN`.
- Kept `include_str!` checks for generated scripts, configuration templates,
  documentation, and legacy database fixtures where the embedded text is the
  artifact under test.

### Verification

- All 332 federation-enabled `cdk-common` library tests pass.
- The five focused federation public-route fail-closed tests pass.
- Federation-enabled `cdk` library tests compile, including the restored
  application-database trait scope for split mint tests.
- The migrated SQLite schema regression and the production-feature mintd CLI
  regression pass.
- All 28 standalone Axum tests pass, and standalone mint library tests compile.

## 42. Replace sleep-driven federation tests with deterministic synchronization

**Status:** Completed (2026-07-12)

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

### Resolution

- Added a test-only notification for federation response-waiter count changes.
  Mint, swap, melt, timeout, and duplicate-join tests now wait for the exact
  registry state instead of sleeping in 5 ms loops or assuming a fixed number
  of scheduler yields is sufficient.
- Reused the existing finalized-operation notifier for live-session applied and
  rejected operation waits. Tests still inspect the database after the signal,
  but no longer poll SQL every 20 ms to discover finalization.
- Added explicit test notifications to the AlephBFT latency recorder and both
  in-process REST transports. Stage and network-message tests now wake on the
  recorded event while retaining a diagnostic outer timeout.
- Kept bounded wall-clock waits where time is the behavior under test: retry
  and backoff cadence, AlephBFT idle/activity grace, injected slow or reordered
  transports, real HTTP member readiness, and asynchronous backup/database
  persistence. Production timers and consensus behavior were not changed for
  test convenience.
- A broader cross-crate database event bus and paused-time conversion for live
  upstream AlephBFT sessions were rejected. They would add production-shaped
  test infrastructure or distort real runtime scheduling for little benefit.

### Verification

- The four duplicate pending-request tests measured 2.54 seconds before and
  2.50 seconds after, confirming runtime is neutral within noise rather than
  claiming an artificial speedup. Five consecutive post-change runs passed in
  2.46-2.49 seconds.
- Focused pending-finalization, waiter-limit, duplicate-submit, session
  rotation, recovered-session, latency-stage, below-finality conflict/melt,
  and partition/catch-up tests pass.
- The complete federation-enabled Axum suite passes 419 tests with one manual
  benchmark ignored in 97.24 seconds under normal parallel execution.
- All 28 standalone Axum tests pass.

## 43. Cut through every federation-owned format to initial version 1

**Status:** Completed (2026-07-13)

### Decision

Apply the project's cut-through policy completely before the first supported
federation release. Pre-cut-through nodes, payloads, registries, journals,
Aleph backups, DKG state, configs, and databases are unsupported and will be
recreated. The complete current implementation—not any discarded development
iteration—is the initial v1.

Reset every remaining federation-owned v2 marker: the operation
envelope/version and operation-ID domain, the DKG commitment domain, persisted
DKG participant-state version and transcript-hash domain, and the local CLI
federation registry version. Other federation-owned boundaries are already v1
and retain their current complete shapes. The consolidated migration timestamp
and Cashu v3/BLS keyset version are different version spaces and are not reset.

No compatibility decoder, alias, legacy DTO, deprecated flag, version range,
migration, or alternate format will be added. Remove any such federation path
found during the workspace audit. Regenerate every affected deterministic
artifact after the current operation shape stabilizes, then freeze the
resulting v1 formats. Future incompatible changes advance only the governing
local version/domain after the first supported release.

### Problem

Federated CDK has no supported deployment history, but some current federation
formats carry version numbers and hash-domain labels inherited from incompatible
development iterations. The only supported operation envelope is advertised as
version 2; DKG commitment, participant-state, and transcript formats retain v2
labels; and the current CLI registry shape is numbered v2 after a pre-release
field rename. These numbers describe discarded development history rather than
real compatibility boundaries.

Keeping that history makes the initial federation protocol appear to have an
upgrade and migration burden that does not exist. It also leaves documentation
inconsistent about whether the first operation encoding is v1 or v2.

### Cleanup

- Treat the complete current federation implementation as the initial version
  1 protocol. Keep `wallet_protocol_version` at 1 and rebase the current
  operation envelope and operation-ID hash domain to v1 at the operation
  payload boundary; do not reintroduce internal versions into wallet metadata.
- Rebase the complete current DKG commitment hash, persisted participant-state
  format, and signed transcript hash domain to v1. Keep the present ceremony
  binding, transcript contents, cryptographic verification, and restart-safety
  requirements unchanged.
- Rebase the complete current CLI federation registry shape to v1. Preserve
  `quote_coordinator_member_id` and `--quote-coordinator-member-id`; do not load,
  translate, alias, or default the removed `primary_member_id` shape or flag.
- Audit every federation-only wire version, persisted schema version,
  cryptographic domain separator, fixture, generated artifact, and compatibility
  branch. Include Serde aliases/defaults, untagged or alternate decoders,
  version dispatch/ranges, compatibility DTOs, deprecated flags, and migration
  adapters. Remove anything whose sole purpose is accepting a superseded
  development format.
- Retain optional/defaulted fields only when they express a documented current
  v1 lifecycle or API semantic. Do not mistake legitimate optional DKG setup,
  payment evidence, consensus evidence, or catch-up request fields for legacy
  compatibility, but require missing mandatory v1 fields to fail closed.
- Regenerate operation golden vectors, DKG vectors, public/private federation
  configs, config digests, transcripts, invites, current-v1 registry fixtures,
  and other artifacts affected by the rebased versions.
- Require operators and developers to recreate experimental federation
  databases, DKG state, configs, and registries. Do not provide migration paths
  from any pre-cut-through federation format.
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
- `crates/cdk-cli/src/sub_commands/federation.rs`: registry version, strict
  current-shape validation, and stale-version tests. Reset the current shape to
  v1 and retain the absence of a `primary_member_id` compatibility alias.
- `crates/cashu/src/nuts/nut06.rs`, mint-info construction, wallet federation
  validation, mint admission, CLI federation registry/import, setup tooling,
  generated configs, and federation documentation.
- SQLite and PostgreSQL federation schemas and migration registration: retain
  the single consolidated federation migration per backend and every earlier
  standalone migration; do not add an experimental-federation upgrade chain.
- Prove `/v1/info` advertises wallet protocol version 1, while operation,
  consensus-item, private-REST, Aleph-unit, and catch-up-certificate versions
  remain in the payloads they govern. Altered version 0, 2, and future values
  must fail closed only at the relevant wallet or member boundary.
- Prove the canonical v1 operation vector, operation IDs, DKG commitments,
  participant-state restart, transcript equality/signatures, config digests,
  invites, registry round trips, and materialized-state verification are
  deterministic across all members.
- Start fresh SQLite and PostgreSQL federations and run DKG, mint, swap, melt,
  restore, restart, offline-member catch-up, and convergence tests. Confirm old
  experimental federation artifacts and registry shapes are rejected rather
  than migrated.
- Run standalone migration, mint, wallet, and non-federated keyset regressions
  unchanged, and search the whole workspace for stale v2 labels, obsolete
  aliases/defaults/DTOs/version branches, compatibility wording, and unlisted
  federation version constants before marking this item complete.

### Result

- Reset the operation envelope and operation-ID domain, DKG commitment domain,
  persisted participant-state version, DKG transcript domain, and complete
  current CLI registry shape to v1. Wallet metadata and every already-v1
  boundary remained independent and unchanged.
- Made federation-owned current-v1 DTOs reject unknown fields. Exact current
  shapes remain usable, while version 0, version 2, future versions, removed
  `failure_reason` / `observed_time` / `primary_member_id` /
  `protocol_versions` / `consensus_item_version` / `session_outcome` fields,
  and the removed `restore` operation fail closed.
- Regenerated and froze operation, journal-order, DKG commitment, and DKG
  transcript vectors. Current registry v1 round trips without an alias or
  translator for the discarded registry shape.
- A workspace search found no production federation-owned v2 domain/version,
  compatibility decoder, Serde alias, untagged alternate decoder, deprecated
  federation flag, or upgrade migration. The SQL migration timestamp and Cashu
  v3/BLS keyset version remain in their unrelated version spaces.
- Focused verification passed in `cdk-common`, `cdk-cli`, `cdk-sqlite`, the
  PostgreSQL federation build, and wallet federation aggregation. The complete
  Axum federation run was executed as an additional audit: its strict-v1,
  consensus, DKG, and protocol-boundary coverage passed. Remaining failures
  are stale single-member melt fixtures that bypass ordered quote creation and
  assume the local member is always the deterministically elected payer, plus
  one timing-sensitive live-session test; weakening production election is not
  an acceptable fix for that separate harness debt.

## 44. Reserve response waiters before durable pending admission

**Status:** Completed (2026-07-13)

New federation submissions now reserve their bounded response waiter after
consensus and mempool validation but before inserting the durable pending row.
The existing RAII registration releases on persistence failure or cancellation;
the prepared consensus clone is published only after the insert commits. This
removes the state where waiter exhaustion leaves a durable operation absent
from the live queue without adding a compensating delete or durable waiter
model.

Focused tests prove waiter exhaustion leaves the rejected operation in neither
the database nor consensus, deterministic pending-write failure leaks no waiter
or row and permits one clean retry, and pre-backup restart recovers durable
pending work without wallet resubmission. All 28 standalone Axum tests pass.

## 45. Remove federation-wide melt-observation serialization

**Status:** Completed (2026-07-13)

The observer no longer holds one mutex for all melt quotes across backend and
consensus latency. A quote-scoped RAII guard suppresses overlapping local probes
for only the same quote and removes itself on every exit, while an atomic saga
compare-and-set durably claims `SetupComplete -> PaymentAttempted` before the
external side effect. Only the durable transition winner can call the payment
backend; unrelated quote IDs never share a lock.

Focused regressions prove the conditional saga claim is one-shot, concurrent
same-quote probes make exactly one backend call, and a second quote reaches its
backend while the first quote's backend call is deterministically blocked.
Standalone melt retains its existing transition and payment behavior.

## 46. Atomically rebind pending operations during session advance

**Status:** Completed (2026-07-13)

Every durable AlephBFT active-session write now validates all pending records,
rejects future-session inconsistencies, and rebinds older rows to the effective
monotonic active session in the same transaction. Startup performs this
transaction before it loads pending rows, so crash residue with an advanced
marker cannot hide older pending work from the recovered consensus queue.
Pending inserts lock the same marker row, verify their operation ID, and reject
stale sessions, closing the concurrent late-insert edge.

Database regressions cover commit, rollback, stale/future session handling, and
multi-row rebind. Consensus coverage rotates both queued and dispatched but
unfinalized items. An Axum restart regression recovers two old-session rows,
finalizes them in the active session, and removes them without wallet
resubmission.

## 47. Supervise essential AlephBFT tasks and unify terminal teardown

**Status:** Completed (2026-07-13)

Root, finalized-payload apply, and network-pump outcomes now feed one
generation-scoped supervisor watcher. Non-retryable apply no longer logs and
skips a finalized payload: it halts the exact installed generation, removes the
inbound queue, rejects new public submissions, and joins sibling tasks. A halt
epoch prevents setup racing a terminal transition from publishing afterward,
and a delayed old-generation failure cannot tear down a rotated supervisor.

Automatic-rotation failure, root/network panic, peer digest mismatch, and
no-progress catch-up use the same terminal helper. Focused regressions cover
non-retryable apply, network/root panic, stale generation, failed automatic
rotation, peer-sync teardown, normal shutdown, natural rotation, and durable
pending/restart recovery.

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
