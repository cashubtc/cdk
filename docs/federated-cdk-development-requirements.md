# Federated CDK Development Requirements

> **Current contract clarification — 2026-07-29:** This remains a normative
> engineering contract. References below to an `undeployed` federation mean
> that no supported production federation format/release has been declared;
> they do not mean no test deployment exists. The existing Nocean testing
> federation was upgraded in place to cleanup commit `a7df24ebb` on 2026-07-29.
> That operational smoke does not change the cut-through development policy or
> close the production gates in
> [`federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md).

This is the compact engineering contract for federated CDK. Apply it to code,
configuration, protocol shapes, migrations, tests, tooling, and documentation.
Federation is an explicit opt-in feature; existing standalone CDK behavior is a
compatibility boundary.

## 1. Product and protocol boundaries

- Do not change standalone mint, wallet, HTTP, storage, payment, or signatory
  behavior while implementing federation. Shared code may change only when
  federation requires it, with standalone behavior covered by tests.
- Preserve the public Cashu wallet-to-mint protocol and ordinary Cashu proofs
  wherever practical. Federation-specific behavior belongs behind explicit,
  feature-gated APIs and adapters.
- Federation is v3/BLS-only: use only `KeySetVersion::Version02` (`02` keyset
  IDs). Reject non-v3 aggregate/member keysets, secret shares, signing
  targets, restore placeholders, and wallet metadata at validation boundaries.
- Federation is undeployed until a supported release is defined. Do not add
  migration shims, legacy aliases, permissive defaults, or compatibility code
  for experimental federation formats. Cut through obsolete configs, invites,
  routes, registries, fixtures, and migrations; regenerate them and reject
  unknown or missing required fields where the shape is authoritative.
- A public status, config, or readiness surface must never claim a capability
  the runtime does not enforce. Name policies after their actual verification
  rules; do not expose placeholder trust models or evidence types.

## 2. Safety and correctness invariants

- Every safety-critical mint, swap, melt, quote, payment-observation, and
  configuration transition must be accepted by consensus before any member
  returns a usable signature share or treats the state as authoritative.
- Apply operations as deterministic state-machine transitions. All honest
  members must derive the same durable journal, materialized state, status,
  digest, and outcome from the same accepted operation and journal time.
- Persist accepted evidence before relying on it. Authorization is temporal:
  payment evidence, quote creation, reservations, and other prerequisites must
  exist at or before the operation index they authorize; later evidence cannot
  authorize an older operation.
- Make retries, exact duplicates, replay, catch-up, and restart recovery
  idempotent. They must converge on the original durable result without
  duplicate signatures, proof insertion, spending, payment attempts, or other
  external side effects. Route normal and duplicate terminal outcomes through
  the same publication/cache/waiter path.
- Keep transaction and crash boundaries explicit. Commit domain mutations and
  accepted status together, explicitly roll back on failure, and never hide a
  multi-step external payment saga behind a fictitious atomic transaction.
  External side effects require deterministic actor election, durable
  reservation/claim state, and a later consensus-controlled finalization path.
- Fail closed on conflicting keysets, config digests, journal order, evidence,
  member identity, signatures, state digests, readiness, and recovery checks.
  A restart must compare recomputed local state with certified quorum evidence
  before becoming healthy or authoritative.
- Session generations, supervisors, queues, pending operations, waiters, and
  cleanup must be generation-scoped. A failed or completed session cannot be
  published as live, and an old watcher cannot tear down a replacement.

## 3. Architecture and implementation

- Keep the interface to existing CDK code narrow. Organize federation into
  cohesive modules with clear ownership of admission, consensus, application,
  persistence, signing, wallet aggregation, networking, status, and recovery.
  Do not scatter federation conditionals through unrelated standalone paths.
- Reuse the authoritative lifecycle, journal, persistence, transaction,
  error, retry, status, and test abstractions. Consolidate equivalent behavior
  into one helper/state machine/representation; introduce a new abstraction
  only when the existing boundary cannot safely express the requirement.
- Prefer the simplest CDK-native design that preserves the invariant. Use
  Fedimint as a reference for consensus, DKG, authenticated peers, catch-up,
  state-machine boundaries, and threshold-signing safety; copy its lessons,
  not unnecessary complexity.
- Keep federation transport private and authenticated (currently signed REST)
  and bind requests to the configured roster, member identity, session, and
  operation where applicable. NAT traversal is an operational concern, not a
  relaxation of protocol authentication or correctness.
- Bound projection and recovery work by federation-owned IDs. Use keyed/bulk
  reads, bounded batches, backpressure, retry fairness, and explicit
  observability; do not turn a small federation journal into full-table scans.
- Use CDK fakewallet as the standard payment/Lightning backend for federation
  development, automated testing, local environments, and test deployments.
  Its automatic affirmative payment result simulates the still-unimplemented
  threshold-capable payment backend; it is test infrastructure, not proof of a
  production payment trust model.
- Keep all federation payment interfaces and state machines backend-neutral so
  other CDK payment backends can be selected without redesigning consensus,
  quote, mint, melt, observation, or recovery paths. Do not branch production
  protocol behavior on fakewallet or any concrete Lightning/on-chain backend.
- Treat payment-observation trust policy as provisional until real payment
  backends with threshold-signing/evidence support are implemented. Implement
  only the currently enforced observation rules, document their limitations,
  and do not claim cryptographic threshold payment verification or invent
  compatibility guarantees for a future policy.

## 4. Security, error handling, and code quality

- Treat every input, peer message, share, config, and persisted record as
  untrusted. Verify identity, signatures, provenance, threshold membership,
  canonical encoding, keyset binding, and replay/sequence constraints before
  admission or signing.
- Use typed `thiserror` errors and `?` propagation with context. Never weaken
  validation, use fake shortcuts, ignore errors, or turn an essential apply,
  digest, persistence, or network failure into a successful/empty result.
- Follow workspace Rust rules: no unsafe code, no `.unwrap()` in production
  code, public documentation, `Self` in impls, module-based organization,
  project import/logging/style conventions, and `cargo fmt --all` formatting.
- Keep temporary probes, debug logs, instrumentation, generated artifacts,
  one-off tools, and workaround names out of production paths. Retain
  instrumentation only when it is opt-in, low-overhead, useful, tested, and
  documented.
- Never commit secrets, private member configs/key shares, machine-specific
  paths, user PII, or environment details. Deterministic fake credentials are
  acceptable only when clearly non-production.

## 5. Verification and delivery

- Before changing a cleanup/review item, inspect adjacent code, call sites,
  tests, config, generated tooling, migrations, and documentation; verify the
  premise against current behavior rather than assuming the TODO is complete.
- Add a focused regression for the exact failure and, where relevant, its
  retry, duplicate, replay, restart, partition, catch-up, and neighboring
  success paths. Prefer deterministic barriers and fault injection over sleeps
  or timing-sensitive tests.
- Run formatting and the smallest relevant checks while developing, then the
  complete affected federation suites plus standalone suites proving no
  regression. Include live/private-network tests with the required permission
  from the first attempt when they bind localhost or exercise real networking.
- A blocker is resolved only after the fix is tested, cleanly integrated, and
  free of debug scaffolding or relaxed validation. Update the implementation
  plan, cleanup TODO, security review, and related status documentation with
  evidence and remaining risk.
- Commit coherent, reconstructable milestones separately using conventional
  commits (`feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `chore:`). Each
  commit must include the code, schema/config changes, tests, and documentation
  needed to understand and reproduce its state.
