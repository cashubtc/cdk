# Federated Cashu with CDK: implementation plan

> **Current status — 2026-07-29:** The fakewallet development scope is
> implemented and its cleanup milestones are reconciled in
> [`cleanup-2026-07-29.md`](cleanup-2026-07-29.md). The federation is **not
> production-ready**. In particular, bearer proofs still do not authenticate
> client intent against a Byzantine receiving member
> (`SEC-2026-07-17-01`); that protocol issue is explicitly deferred to a
> separate project and is not mitigated by the cleanup. The release limitations
> below remain authoritative even where older dated trackers contain unchecked
> or completed-looking checkboxes.
> The existing five-member Nocean testing federation was upgraded in place to
> cleanup commit `a7df24ebb` on 2026-07-29 with its DKG, journal, and wallet
> state preserved. This is an upgrade-compatibility smoke, not the multi-host or
> production release validation required below.

This is a living design, status, and development contract for federated Cashu support in CDK. Treat its safety and architecture rules as constraints, not suggestions. If a requested change conflicts with them, explain the conflict instead of implementing a workaround.

Sandbox test note: run localhost-listener and real-network tests with network permission enabled from the first attempt (an escalated `cargo test ...` invocation in Codex), because `127.0.0.1:0` bind failures without it are environmental rather than regressions.

## Grand vision

The project evolves Cashu from a single-operator Chaumian eCash mint into a federated, threshold-secured system whose independent members order state transitions through consensus and jointly issue, swap, and melt tokens using BLS threshold blind signatures. Federation mode should preserve the public Cashu wallet-to-mint protocol while preventing any single member from inflating or unilaterally corrupting mint state and remaining available while the required signing and consensus thresholds are met.

Every safety-critical operation must be ordered, durably journaled, replayable, and applied deterministically so honest members converge. Wallets must query multiple members, reject inconsistent views or bad shares, and aggregate valid shares into ordinary Cashu proofs. Standalone CDK behavior must remain unchanged unless federation mode is explicitly enabled.

## Scope

- Federate mint quote, issuance, swap, melt quote, and melt transitions; serve restore and check-state as threshold-authoritative reads of accepted state.
- Serialize safety-critical state transitions before returning usable signature shares or performing externally visible side effects.
- Centralize wallet request fan-out, response agreement, error handling, share validation, and signature aggregation.
- Use fakewallet as a normal local-development payment backend while exercising the real payment-observation quorum, DKG, consensus, and recovery paths.

## Current implementation state

- Federation setup uses a real dealer-free DKG ceremony with enrollment, roster agreement, identity binding, restartable rounds, transcript signatures, activation, and identical final public configuration export.
  Enrollment and roster assembly are currently file-mediated and assume an
  authenticated out-of-band operator ceremony. The planned signed,
  multi-operator setup workflow is specified in
  [`federated-cashu-multi-operator-setup-implementation-plan.md`](federated-cashu-multi-operator-setup-implementation-plan.md).
- Members run AlephBFT over authenticated private REST. Operations are durably
  journaled and deterministically applied. Sparse scheduled/recovery checkpoint
  quorums authenticate recovery frontiers; normal restart validates only the
  suffix after the trusted checkpoint. The final object model, ownership, lock
  order, and recovery sequences are documented in
  `docs/federated-cashu-checkpoint-architecture.md`; the original implementation
  record remains in `docs/federated-cdk-checkpoint-implementation-plan.md`, and
  the completed consolidation and final verification record is in
  `docs/federated-cashu-checkpoint-consolidation-plan.md`.
- Mint, swap, and melt flows are serialized before signing or payment side effects. Payment observations require quorum evidence and deterministic journal-time materialization.
- Federation-aware wallets require threshold-consistent views, reject invalid shares, aggregate BLS shares, and preserve ambiguous or unavailable outcomes in existing recoverable wallet flows.
- Keyset and session rotation, pending-operation recovery, restart/catch-up, member fault handling, and fail-closed readiness gates are implemented.
- Checkpoint-bound database-backup restore validation, genesis retention, and
  operator-only materialized-state audit recovery are documented in
  `docs/federated-cashu-backup-recovery.md`. No background full-state audit or
  peer physical-state snapshot protocol is part of version 1.
- Exact-checkpoint startup reads no historical journal/finalized prefix and
  performs no full materialized-state scan. Checkpoint-tail startup and peer
  catch-up use the same bounded suffix verifier. AlephBFT backup transfer is a
  separate, current-session, size-bounded recovery path and cannot advance mint
  state or checkpoint trust.
- The local harness starts a configurable number of members, completes real DKG, restarts into consensus, checks configuration and state convergence, and exercises mint, send/receive, restore, and melt flows.
- Standalone CDK remains available without federation features. Federation keysets are v3/BLS-only.

## Remaining release work

The implementation checklist is complete for the fakewallet federation scope.
That is a development-feature statement, not a real-money deployment claim.
The current disposition and proof for repository-local cleanup work is in
[`cleanup-2026-07-29.md`](cleanup-2026-07-29.md). Production readiness remains
blocked by:

- a separately authorized protocol design for proof-bound client intent
  (`SEC-2026-07-17-01`);
- a versioned journal/envelope and SQL migration before proof-`Y` conflict keys
  can be added safely;
- explicit payment-observation origin/readiness policy, independently
  verifiable backend evidence or a deliberately narrowed trust model, and
  uniform backend recovery identity;
- retry scheduling and per-member ingress fairness that preserve progress under
  sustained hostile traffic;
- service-owned supervision, cancellation, deadlines, and bounded concurrency
  for the transaction broadcaster and remaining background workers;
- a decision on synchronous federation keyset API semantics versus the durable
  asynchronous effective-keyset view;
- completion of the Bark/FROST release gate, including dependency availability,
  deterministic mint-and-melt regtest, SQLite/PostgreSQL parity, opt-in paid
  Signet coverage, and independent cryptographic/state review;
- multi-host deployment, upgrade, backup/restore, rotation, and incident drills;
- signed multi-operator enrollment, unanimous roster/policy approval, and
  setup-manifest distribution that remove centralized private-material
  generation and manual configuration exchange from the production setup UX;
  and
- sustained high-load CPU and p99 measurement plus asymmetric latency, loss,
  bandwidth, real TLS, multi-process, and real payment-backend performance
  validation.

The 2026-07-29 local screen retained an active AlephBFT cadence of 200 ms and a
network-pump width of one. It proved deterministic in-process correctness and a
matched local CPU screen only; it does not close the external performance gate.

## Cut-through compatibility policy

No federation mint or federation database format has been deployed. This is the first federation implementation, so federation development proceeds in cut-through mode.

- Do not preserve compatibility with earlier experimental federation protocols, APIs, wire formats, invitations, configurations, keyset metadata, database schemas, migrations, or fixtures.
- Change federation members, clients, schemas, tests, and generated fixtures together. Regenerate experimental federation databases and configurations instead of adding upgrade paths, adapters, dual reads, fallbacks, or legacy modes.
- This policy applies only to federation functionality. Preserve standalone CDK behavior and public Cashu NUT compatibility unless an intentional protocol change is explicitly documented.

BLS blind signatures are supported only for Cashu keyset v3, represented by `KeySetVersion::Version02` and keyset ID prefix `02`. Federation configuration, startup, signing, restore, and wallet validation must reject non-v3 keysets or mismatched aggregate keys, public shares, and local secret shares. Standalone mints remain unaffected.

## Design and architecture principles

Fedimint is the main reference for consensus, DKG, peer authentication, catch-up, state-machine boundaries, and threshold-signing safety. A sibling checkout is available at `../fedimint/`. Copy safety properties and lessons, not architecture or complexity that CDK does not need.

Prefer simplification and reuse before adding functions, types, or parallel implementations. Share domain behavior; separate federation orchestration. When federation and standalone operations have the same business semantics, reuse existing CDK validation, quote handling, proof checks, state transitions, error types, and persistence logic. Extract a small shared operation when that removes real duplication, but do not introduce speculative abstractions or conceal genuine semantic differences.

Coupling to existing CDK code must remain thin and explicit:

- The wallet federation connector, or an equivalent centralized module, owns member fan-out, threshold response and error agreement, fallback, signature-share verification and aggregation, and ambiguous-result handling. Issue, swap, melt, restore, and check-state flows should consume ordinary connector results and retain existing wallet saga and recovery behavior. Applications using the CDK wallet library should work almost unchanged except for federation-specific business operations such as importing or selecting a federation.
- The mint domain owns Cashu business validation and deterministic state application. Federation orchestration decides ordering, quorum authorization, replay, and when shared domain logic runs; it must not duplicate the standalone mint implementation.
- Consensus, DKG, authenticated member networking, journal/catch-up handling, readiness, rotation, and threshold-signing coordination belong in dedicated federation modules behind appropriate feature gates. Avoid scattering federation conditionals through standalone mint, wallet, HTTP, storage, or signatory code.
- Public HTTP handlers should remain thin adapters. AlephBFT orders operations but does not decide Cashu business validity. The federation journal is the authoritative operation history; local materialized state alone is not federation authorization. Private REST transports authenticated member traffic and must not become an alternate consensus path.

A reviewer familiar with non-federated CDK should be able to identify the reused domain code, the narrow federation integration surface, where ordering/quorum/replay are enforced, and which code is inactive without federation features.

Private signed REST is the mint-to-mint transport. Operators may provide stable federation URLs through HTTPS, VPNs, reverse tunnels, Tor onion services, or equivalent connectivity. Alternate NAT-traversing transports are out of scope.

## Safety invariants

- Consensus must precede signature-share creation and externally visible safety-critical state changes. No member may independently authorize issuance, swaps, or melts.
- Operations must have canonical encodings and stable IDs, apply deterministically and idempotently, and reproduce the same materialized state on replay.
- Payment side effects must use the designated replay-safe flow. A member-local payment result or database state is not quorum evidence.
- Threshold results require matching member views and valid shares. Minority disagreement is diagnostic and does not veto a valid threshold; if two incompatible payment-observation groups both reach threshold, the quote remains unresolved and must not produce spendable proofs or definitive success.
- Startup, catch-up, DKG, rotation, roster, keyset, transcript, and configuration mismatches must fail closed rather than defaulting, repairing, or falling back to standalone behavior.
- Local time, randomness, unordered data, network I/O, and member-local backend results must not influence deterministic application unless the relevant value was committed through consensus.

## Acceptable changes and prohibited shortcuts

It is acceptable to refactor behind existing interfaces, simplify or extract shared domain logic, add focused federation modules and database methods, improve fail-closed validation, add bounded diagnostics and tests, and regenerate undeployed experimental artifacts. Keep changes narrow and place decisions in their owning layer.

Do not add alternate production key-provisioning shortcuts, fake shares, automatic finalization, test consensus, or bypass flags. Do not return usable results before finality, accept the first successful response where threshold agreement is required, call payment backends during deterministic replay, weaken authentication or replay protection, or retain obsolete federation compatibility. Do not hide liveness problems with unbounded queues, retry storms, arbitrary timeout increases, serialized tests, or unmeasured periodic consensus traffic.

Prefer existing workspace facilities and domain types. New cryptographic or consensus dependencies require explicit justification. Builds without federation features must continue to work.

## Development, testing, and observability

- Commit coherent, independently testable changes using conventional commits. Keep code, migrations, tests, documentation, and verification results together and avoid unrelated cleanup.
- Run formatting and focused tests for every change. Shared-code changes must cover federation-enabled and standalone configurations; stateful changes need restart/replay coverage; distributed changes need adverse ordering or partial-failure coverage; bounded resources need saturation and cleanup tests.
- Never weaken assertions, increase timeouts, or serialize tests without evidence. Report commands run, meaningful results, and tests not run.
- Temporary instrumentation must be isolated from protocol behavior and removed unless it is documented, opt-in, bounded, and useful as low-overhead production observability.
- Status and metrics may expose non-secret member IDs, readiness, session or height, queue bounds, and digests. Logs and errors must remain bounded and must not expose private shares, coefficients, DKG private messages, credentials, payment secrets, or sensitive payloads.
- Never commit secrets, generated private member configs, private key shares, machine-specific paths, or personally identifiable local environment details. Deterministic fake keys are allowed only in clearly marked test fixtures.
