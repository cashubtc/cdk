# Federated Cashu Code Improvement TODO

This is the execution tracker for
[`docs/federated-cashu-code-improvement-opportunities.md`](federated-cashu-code-improvement-opportunities.md).
Read that document before changing code: it defines the intended ownership,
security boundaries, non-goals, tests, and completion criteria for every item.

This TODO covers federation code organization and consolidation outside the
checkpoint and catch-up subsystem. Checkpoint work belongs to
`docs/federated-cashu-checkpoint-consolidation-plan.md` and must not be folded
into these milestones.

Update checkboxes and the progress log as work lands. A milestone is complete
only when its required tests pass and its completion criteria in the detailed
document are satisfied. Prefer one reviewable commit for a mechanical move and
a separate commit for semantic consolidation.

## Security release gate — 2026-07-17

Detailed reference:
[`federated-cashu-security-audit-2026-07-17.md`](federated-cashu-security-audit-2026-07-17.md).

These items take precedence over the refactoring phases below. Do not mark the
federation production-ready or advertise Byzantine client-fund safety while a
critical item remains open.

### Critical

- [ ] **SEC-2026-07-17-01:** Add proof-bound client authorization over exact
  federated swap and melt intent, or explicitly narrow the threat model and
  deployment claims for ordinary bearer proofs.
- [ ] **SEC-2026-07-17-02:** Carry quorum-verifiable actual off-chain melt spend
  through payment observations and return unused fee reserve as change.
- [ ] **SEC-2026-07-17-03:** Reject semantically invalid member-authored
  operations per item without aborting a finalized unit or halting the live
  AlephBFT session.

### Warning

- [ ] **SEC-2026-07-17-04:** Reject Byzantine-unsafe production signing,
  consensus, payment-observation, checkpoint, and catch-up threshold profiles.
- [ ] **SEC-2026-07-17-05:** Replace externally driven unbounded AlephBFT queues
  with bounded backpressure and per-member request/byte admission.
- [ ] **SEC-2026-07-17-06:** Add proof-`Y` conflict keys to swap and melt
  operations while retaining quote conflicts for melt.

### Security gate verification

- [ ] Add the adversarial regression tests listed under every audit finding.
- [ ] Run the full federation unit and integration suites after the fixes.
- [ ] Run multi-member Byzantine tests covering malicious operation contents,
  proof-intent replacement, weak-threshold rejection, and inbound queue floods.
- [ ] Run restart and catch-up tests across deterministic operation rejections.
- [ ] Run a real-backend melt test proving exact fee refund after crash/replay.
- [ ] Update the public federation threat model and operator guidance.
- [ ] Re-review all six findings and record closure evidence in the audit or a
  dated follow-up audit.

## Status legend

- `[ ]` not started
- `[~]` in progress; include a progress-log entry
- `[x]` complete and verified
- `[!]` blocked or deliberately rejected; record why

---

## Phase A — Baseline and reviewable test files

Detailed reference: Opportunity 9.

- [ ] Record current production LOC, test LOC, test count, and representative
  federation test runtime separately.
- [ ] Inventory safety invariants and assign each a primary test suite.
- [ ] Extract the inline `custom_handlers` test module into a test directory
  without changing assertions.
- [ ] Split fixture setup from handler scenario tests.
- [ ] Confirm federation-enabled and federation-disabled handler tests still
  compile and pass.
- [ ] Record any test count change and map every removed duplicate to retained
  coverage.

Gate before Phase B:

- [ ] Public route safety tests remain discoverable.
- [ ] Production handler files no longer contain a multi-thousand-line inline
  test suite.
- [ ] Baseline measurements are recorded below.

---

## Phase B — Small public-route and status simplifications

Detailed reference: Opportunities 3 and 10.

### B1. NUT-19 cache mechanics

- [ ] Add one typed cache-through helper.
- [ ] Preserve separate `mint`, `mint_batch`, and `melt` namespaces.
- [ ] Migrate mint, batch mint, and melt cache entry points.
- [ ] Test hit, miss, malformed request, namespace isolation, and idempotent
  federation responses.

### B2. Quote parsing and response shaping

- [ ] Add one mint quote parser returning the existing Cashu request enum.
- [ ] Add one melt quote parser returning the existing Cashu request enum.
- [ ] Move method-specific response shaping into a focused helper module.
- [ ] Preserve assigned federation quote-ID behavior.
- [ ] Test Bolt11, Bolt12, on-chain, custom, and method mismatch paths.

### B3. Runtime status snapshot

- [ ] Introduce an internal `FederationNodeStatusSnapshot` or equivalent.
- [ ] Move whole-node status gathering out of the HTTP route.
- [ ] Convert the snapshot to the existing wire response at one boundary.
- [ ] Make readiness, status, and metrics consume the same component-state
  definitions.
- [ ] Test disabled, setup-only, healthy, lagging, catching-up, and halted
  states.

Gate before Phase C:

- [ ] Public HTTP and status response contracts are unchanged.
- [ ] Standalone and federation feature builds pass.

---

## Phase C — Runtime and public-workflow ownership

Detailed reference: Opportunities 1, 2, and 4.

### C1. Background service supervision

- [ ] Characterize current service enablement, retry timing, and first-tick
  behavior.
- [ ] Convert payment observer self-spawn into an owned run future.
- [ ] Convert transaction broadcaster self-spawn into an owned run future.
- [ ] Convert wallet-liveness self-spawn into an owned run future.
- [ ] Bring peer-sync task ownership under the same runtime lifecycle.
- [ ] Add a concrete background-service supervisor with cancellation, named
  handles, start-once protection, join, and status.
- [ ] Start and stop services through `FederationNodeRuntime`.
- [ ] Surface service initialization/termination failures appropriately.
- [ ] Decide and document whether setup-mode DKG belongs to this supervisor or
  a separate pre-activation lifecycle owner.
- [ ] Test start once, disabled services, failure visibility, cancellation,
  clean join, and durable restart behavior.

### C2. Public federation value operations

- [ ] Create a focused `public_value_operations` federation module.
- [ ] Move mint-input federation processing out of HTTP handlers.
- [ ] Move mint/melt quote operation submission out of HTTP handlers.
- [ ] Move persisted response recovery and terminal melt interpretation out of
  HTTP handlers.
- [ ] Keep authentication, method checks, headers, and JSON at the route
  boundary.
- [ ] Preserve all operation builders, admission, and replay authorities.
- [ ] Test feature on/off, fail-closed readiness, duplicate/restart, and
  terminal melt behavior.

### C3. Mint federation domain split

- [ ] Add characterization tests for exhaustive accepted-operation dispatch.
- [ ] Move runtime config and operation builders into focused modules.
- [ ] Move mint quote/payment/issuance application into `mint.rs`.
- [ ] Move melt quote/payment/final application into `melt.rs`.
- [ ] Move swap application into `swap.rs`.
- [ ] Move genuinely shared signature-share logic into `signing.rs`.
- [ ] Keep operation-specific state transitions and rejection rules explicit.
- [ ] Leave a small facade and exhaustive dispatcher in `mod.rs`.
- [ ] Verify golden operation IDs, retry equivalence, rollback, replay, and
  materialized projection.

Gate before Phase D:

- [ ] No federation worker detaches itself from runtime ownership.
- [ ] Public route files do not construct or await federation operations.
- [ ] No mint federation file combines mint, swap, and melt appliers.

---

## Phase D — Common modules and test-suite organization

Detailed reference: Opportunities 8 and 9.

### D1. Non-checkpoint transport split

- [ ] Freeze JSON and signing-transcript golden tests.
- [ ] Extract signed envelope/hash/signature types.
- [ ] Extract replay-window types.
- [ ] Extract status/operator messages.
- [ ] Extract DKG messages.
- [ ] Extract FROST signing messages.
- [ ] Extract operation-submission messages.
- [ ] Extract AlephBFT messages.
- [ ] Extract wallet operator messages.
- [ ] Preserve stable federation facade re-exports.
- [ ] Leave checkpoint/catch-up messages untouched pending their own plan.

### D2. Test-suite split and deduplication

- [ ] Create narrowly scoped member, state, payment, transport, Aleph, and
  assertion fixtures only where at least two suites need them.
- [ ] Split `federation/tests.rs` by route, runtime, consensus, DKG, wallet,
  operations, and recovery ownership.
- [ ] Relocate swap route tests to the owning route/federation suite.
- [ ] Relocate multi-member wallet fanout tests to wallet or integration-test
  ownership.
- [ ] Table-drive the largest mint/swap/melt recovery and duplicate families.
- [ ] Remove secondary duplicate tests only after recording their primary
  replacement.
- [ ] Compare focused and full-suite runtime with the baseline.

Gate before Phase E:

- [ ] Wire formats, hashes, signatures, and public type paths are unchanged.
- [ ] No common non-checkpoint transport file mixes unrelated protocol
  families.
- [ ] Safety coverage and test ownership are documented.

---

## Phase E — DKG consolidation

Detailed reference: Opportunity 5.

Treat this as a dedicated security-sensitive project.

- [ ] Add restart characterization at every BLS and FROST DKG phase.
- [ ] Add golden durable-record and transcript tests.
- [ ] Replace the Axum-local `FrostDkgPhase` mirror with one shared vocabulary.
- [ ] Extract concurrent delivery and delivery accounting from cryptographic
  phase evolution.
- [ ] Introduce a FROST-specific pure planner/action boundary.
- [ ] Move atomic DKG state transitions into the persistence boundary.
- [ ] Reduce the Axum driver to load, plan, execute, and report.
- [ ] Preserve encryption, receiver binding, all-member confirmation, and
  earlier-artifact rebroadcast.
- [ ] Replace string-based decision inputs with typed errors where decisions
  require classification.
- [ ] Split planner, persistence, route, and live-delivery tests.

Gate before Phase F:

- [ ] Axum maintains no second DKG phase model.
- [ ] Cryptographic phase decisions are testable without HTTP.
- [ ] Restart, duplicate, reorder, mismatch, confidentiality, and secret
  redaction tests pass.

---

## Phase F — On-chain transaction construction

Detailed reference: Opportunity 6.

Treat this as a separate security-sensitive project from DKG.

- [ ] Freeze proposal, authorization, signed-transaction, settlement, and state
  golden bytes.
- [ ] Extract checked arithmetic, output classification, fee contribution, and
  RBF arithmetic as pure helpers.
- [ ] Move BDK-specific candidate creation into a wallet adapter module.
- [ ] Introduce a mint/domain-owned candidate-to-proposal constructor.
- [ ] Keep independent untrusted-proposal validation as a separate top-level
  path.
- [ ] Reduce wallet liveness to leases, retries, work selection, signing
  scheduling, and submission.
- [ ] Mechanically split intent, proposal, authorization, signing, broadcast,
  settlement, and state types behind stable re-exports.
- [ ] Test adversarial inputs/outputs, fee limits, RBF lineage, independent
  member validation, and restart at every durable stage.

Gate before Phase G:

- [ ] Wallet liveness no longer owns transaction-policy implementation.
- [ ] Construction cannot bypass independent validation.
- [ ] Wire, hash, and persistence formats are unchanged.

---

## Phase G — Narrow private-route cleanup

Detailed reference: Opportunity 7.

- [ ] Document message classes and their exact sequence-consumption semantics.
- [ ] Extract common signed-envelope identity/signature verification.
- [ ] Extract common private response signing.
- [ ] Add a narrow authenticated-read helper.
- [ ] Add a narrow ephemeral FROST-mutation helper if it deletes real
  duplication without hiding sequence recording.
- [ ] Keep durable DKG mutation/replay persistence specialized and atomic.
- [ ] Keep operation submission and Aleph network traffic specialized.
- [ ] Test replay/equivocation and validation-before-sequence behavior for each
  class.
- [ ] Stop and reject the abstraction if it obscures acceptance points.

Completion gate:

- [ ] Common authentication mechanics are shared.
- [ ] Sequence consumption remains explicit.
- [ ] No generic middleware performs premature durable mutation.

---

## Final verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo check --workspace --all-targets`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --lib --workspace --exclude cdk-postgres`
- [ ] `cargo test -p cdk-axum`
- [ ] `cargo test -p cdk --features federation`
- [ ] `cargo test -p cdk-common`
- [ ] Relevant SQLite restart tests pass.
- [ ] Relevant PostgreSQL parity tests pass where persistence changed.
- [ ] Real-Aleph multi-member value, DKG, and on-chain wallet flows pass.
- [ ] Standalone mint and feature-disabled behavior remains unchanged.
- [ ] Production and test LOC are reported separately.
- [ ] Detailed document and module docs match the implemented architecture.

---

## Baseline measurements

Fill this before Phase A is completed.

| Measurement | Baseline | Final | Notes |
|---|---:|---:|---|
| Federation production LOC | TBD | TBD | Exclude tests/examples/benches |
| Federation test LOC | TBD | TBD | Report separately |
| `custom_handlers` production LOC | About 1.2k | TBD | Verify exact boundary |
| `custom_handlers` test LOC | About 9.0k | TBD | Current inline module |
| Axum federation test LOC | About 25.2k | TBD | Current monolithic suite |
| Focused Axum federation test runtime | TBD | TBD | Record command and hardware-neutral context |
| Full relevant workspace test runtime | TBD | TBD | Record command |

---

## Progress log

| Date | Phase/item | Status | Commit or PR | Notes |
|---|---|---|---|---|
| 2026-07-17 | Tracker created | Complete | Documentation only | Detailed opportunities expanded; implementation not started. |

---

## Rejected or deferred ideas

Record rejected implementation ideas here so they are not repeatedly proposed.

| Date | Idea | Decision and reason |
|---|---|---|
| 2026-07-17 | Rebuild wallet quorum collection around a generic policy trait | Rejected: scheduling is already shared and endpoint agreement rules are intentionally concrete. |
| 2026-07-17 | Introduce raw/setup/active validated-config wrappers throughout the stack | Deferred: previous review found more conversion code without eliminating a current validation bypass. |
| 2026-07-17 | One universal private REST extractor | Rejected: sequence persistence and acceptance points differ by message class. |
| 2026-07-17 | Generic operation-applier trait | Rejected: it would hide security-relevant differences between operation domains. |
