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

**Status:** Todo

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

**Status:** Todo

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

**Status:** Todo

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

**Status:** Todo

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

**Status:** Todo

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
  task; those fields have current state-transition or reconciliation uses.

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

**Status:** Todo

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
