# Federated Cashu Code Improvement Opportunities

This document is the detailed design and review reference for simplifying the
federated mint code outside the checkpoint and catch-up subsystem. The separate
execution checklist is
[`docs/federated-cashu-code-improvement-todo.md`](federated-cashu-code-improvement-todo.md).

The objective is to reduce accidental complexity and unnecessary duplication,
make ownership visible from the module tree, and keep production and test files
small enough to review. The desired result is not a generic federation
framework. It is a set of concrete components with narrow responsibilities and
explicit security boundaries.

This is a living document. Before implementing an item, verify its premise
against the current branch and record material design changes in the changelog.
The TODO document records execution status; this document explains why the
work exists and what the finished architecture should look like.

Related context:

- `docs/federated-cashu-implementation-plan.md` explains the larger federated
  CDK goal.
- `docs/federated-cashu-refactoring-plan.md` records earlier architectural
  decisions and completed refactors.
- `docs/federation-cleanup-todo.md` records protocol and safety cleanup.
- `docs/federated-cashu-checkpoint-consolidation-plan.md` exclusively owns
  checkpoint and catch-up consolidation.

---

## Executive assessment

The federation core is substantially better organized than its first version.
The in-memory consensus path has been removed, application transactions and
journal application states have been consolidated, materialized-state
projection is mint-owned, wallet member scheduling is shared, common types have
coarse domain modules, and AlephBFT has a session supervisor.

The largest remaining problem is **incomplete ownership**. Components have been
extracted, but callers still understand and assemble their internals:

- `FederationNodeRuntime` stores background worker objects, while startup
  launches detached tasks and retains no common lifecycle handle.
- Public HTTP handlers construct operations, submit consensus work, wait for
  local application, and recover results.
- The status route gathers state from nearly every runtime subsystem itself.
- Mint federation application remains concentrated in one large production
  file even though stable domain boundaries now exist.
- DKG planning, cryptographic phase evolution, persistence, and transport are
  divided across two drivers with overlapping phase models.
- The wallet-liveness scheduler also constructs and classifies Bitcoin
  transactions.
- Private REST routes repeat authentication and response mechanics.
- Shared wire types and the largest test suites remain difficult to navigate.

The best simplifications therefore move logic to its natural owner, delete
duplicate orchestration, and split files along real domain boundaries. Pure
file moves should precede behavior-sensitive consolidation so reviewers can
see semantic changes clearly.

---

## Scope and boundary conditions

Every opportunity in this document must preserve these conditions:

- Consensus ordering, operation IDs, conflict keys, canonical bytes, signing
  transcripts, and all persisted or wire formats remain unchanged unless a
  separate protocol change explicitly authorizes a version bump.
- Journal replay and deterministic application remain authoritative. A local
  scheduler, HTTP request, BDK wallet, or peer response must not gain authority
  over accepted state.
- Independent validation of untrusted member proposals remains independent
  from proposal construction.
- DKG secret material remains encrypted at rest, redacted from logs, and
  transported only under the existing confidentiality policy.
- Standard Cashu errors remain the business-error authority. Federation
  infrastructure must not create a second classifier that changes wallet saga
  behavior.
- Standalone mint behavior and feature-disabled builds remain unchanged.
- Existing safety coverage is moved or parameterized before duplicate tests
  are deleted.
- Checkpoint, suffix verification, catch-up, recovery checkpoint selection, and
  checkpoint transport are out of scope here.
- Refactors should prefer concrete structs and functions. Introduce a trait or
  effect model only when it removes multiple real implementations or makes an
  invalid state impossible.
- Do not replace several understandable files with one generic manager that
  merely renames the same complexity.

### File and module organization goals

These are review targets, not automatic lint failures:

- Production route files should normally stay below roughly 500–800 lines.
- A production domain file approaching 1,500 lines should be reviewed for more
  than one responsibility.
- Test files should be split when fixture setup or unrelated scenario families
  prevent local reasoning; line count alone is not a reason to delete tests.
- `mod.rs` and facade files should describe ownership and re-export stable APIs,
  not contain another large implementation.
- Test-only LOC and production LOC must always be reported separately.

---

## Current evidence

Approximate current sizes on the reviewed branch in July 2026:

| Area | Approximate size | Main concern |
|---|---:|---|
| `cdk-axum/src/custom_handlers.rs` | 10.3k lines; production ends near 1.2k | Public federation orchestration plus a very large inline test module |
| `cdk-axum/src/federation/tests.rs` | 25.2k lines | Fixtures and unrelated scenario families share one file |
| `cdk/src/mint/federation.rs` | 3.5k production lines | Operation construction, validation, application, and response recovery |
| `cdk-common/src/federation/transport.rs` | 3.3k production lines plus tests | Every private REST message family in one module |
| `cdk-common/src/federation/transaction_proposal.rs` | 2.6k production lines plus tests | Intent, proposal, signing, broadcast, settlement, and state types |
| `cdk-axum/src/federation/dkg_driver.rs` | About 1.6k production lines | Planning/execution boundary and a second FROST phase machine |
| `cdk-axum/src/federation/wallet_liveness.rs` | 1.1k lines | Scheduling plus transaction construction and RBF policy mechanics |

Sizes are evidence of review cost, not proof of bad design. The opportunities
below additionally require duplicated workflow or misplaced ownership.

---

## Ranking

Impact measures reduction in operational risk, duplicate authority, review
surface, and future change cost. Cost includes implementation effort,
behavioral risk, and breadth of required tests.

| Rank | Opportunity | Potential impact | Cost | Recommended timing |
|---:|---|---|---|---|
| 1 | Supervise all federation background services | Very high | Medium | First architectural change |
| 2 | Move public federation workflows out of HTTP handlers | High | Medium | Early, after or with test extraction |
| 3 | Give runtime status one authoritative snapshot API | High | Low–medium | Early quick win |
| 4 | Split mint federation logic by business domain | High | Medium | After route ownership is clear |
| 5 | Consolidate the DKG state machine boundary | High | High | Isolated security-sensitive project |
| 6 | Consolidate on-chain transaction proposal construction | High | High | Isolated security-sensitive project |
| 7 | Add narrow private-route request helpers | Medium–high | Medium | After message classes remain explicit |
| 8 | Split common transport and transaction types by domain | Medium | Low–medium | Mechanical moves around other work |
| 9 | Consolidate federation test infrastructure | Medium | Medium | Start early, continue incrementally |
| 10 | Unify NUT-19 cache and quote parsing mechanics | Low–medium | Low | Early quick win |

The order above favors impact-to-cost. Opportunities 5 and 6 may produce more
long-term benefit than several smaller items, but their security-sensitive
state machines require dedicated characterization and review.

---

## Opportunity 1 — Supervise all federation background services

**Status:** Open  
**Impact / cost:** Very high / medium

### Current problem

`FederationNodeRuntime` owns values for the payment observer, exact-byte
transaction broadcaster, and wallet-liveness worker. Startup clones each value
and calls a `spawn(self, MintState)` method. Those methods call `tokio::spawn`
and discard the resulting `JoinHandle`. Peer sync and automatic DKG startup use
similar top-level spawning paths.

Consequences:

- There is no single place to prove that each long-lived service starts once.
- Shutdown cannot deterministically cancel and join every service.
- A worker can terminate after an initialization failure without a common
  failed state or readiness consequence.
- Tests cannot ask the node runtime which services are running or await clean
  termination.
- Startup knows the enablement conditions for several federation subsystems.
- AlephBFT has explicit supervision while adjacent federation work does not.

This is lifecycle duplication, not a need to merge the worker implementations.

### Target ownership

Keep each worker in its focused module, but add one concrete lifecycle owner:

```text
cdk-axum/src/federation/
  node_runtime.rs
  background_services.rs       # start/stop/join/status only
  payment_observation.rs       # payment observation work
  broadcast.rs                 # exact-byte broadcast work
  wallet_liveness.rs           # proposal/signing scheduling
  sync_loop.rs                 # peer synchronization work
  dkg_driver.rs                # DKG work
```

`FederationNodeRuntime` should hold a `FederationBackgroundServices` handle
instead of exposing independently spawnable worker values. The supervisor
should own:

- a cancellation signal;
- named task handles;
- per-service lifecycle state;
- idempotent start protection;
- deterministic shutdown and joining;
- initialization failure reporting;
- a compact status snapshot.

The worker modules should expose `run(...)` futures or one-round methods. They
should not decide whether to detach themselves from runtime ownership.

### Explicit service policy

The supervisor must keep enablement rules visible and typed:

- Payment observation requires an active federation config and payment
  processors.
- Transaction broadcasting requires an accepted FROST wallet and a local
  watch-only wallet.
- Wallet liveness additionally requires FROST signing support.
- Peer sync requires the configured production consensus/runtime path.
- Automatic DKG may run before the application federation is active and must
  therefore remain a distinct startup phase.

Do not make all services share one retry policy. Scheduling intervals, durable
replay, and failure semantics remain worker-specific.

### Implementation sequence

1. Characterize current enablement, first-tick, retry, and shutdown behavior.
2. Change worker `spawn` methods into owned `run` futures without changing loop
   bodies or intervals.
3. Add the supervisor and move current startup conditions into it.
4. Start and stop the supervisor through `FederationNodeRuntime`.
5. Surface failed/stopped service states in runtime readiness and local status
   where operationally meaningful.
6. Consider DKG ownership last because setup-mode DKG has a different lifetime.

### Required tests

- Every eligible service starts exactly once.
- Repeated start is idempotent or rejected deterministically.
- Disabled services do not start.
- Cancellation wakes interval and event-driven workers promptly.
- Shutdown joins every task.
- Initialization failure is visible and does not leave a false running state.
- One worker failure does not silently disappear.
- Restart rebuilds worker state from durable authority and does not duplicate a
  payment, proposal, signing attempt, or broadcast operation.
- Existing idle behavior and retry timing remain unchanged.

### Completion criteria

- No production federation worker calls detached `tokio::spawn` on itself.
- Startup does not individually clone and launch federation workers.
- `FederationNodeRuntime` is the discoverable owner of long-lived federation
  services.
- Consensus and payment safety remain in durable state, not supervisor memory.

---

## Opportunity 2 — Move public federation workflows out of HTTP handlers

**Status:** Open  
**Impact / cost:** High / medium

### Current problem

`custom_handlers.rs` mixes four different concerns:

1. HTTP authentication, method selection, and body parsing.
2. Cashu request/response variant conversion.
3. NUT-19 request caching.
4. Federation workflow orchestration.

The federation-specific helpers process mint input, wait for quote state,
construct mint and melt operations, submit them, recover persisted responses,
and interpret terminal melt state. Consequently the HTTP layer is a second
federation edge runtime and must change whenever operation workflow changes.

### Target ownership

Introduce a concrete public value-operation service owned by the Axum
federation boundary:

```text
cdk-axum/src/
  custom_handlers/
    mod.rs                     # route functions and HTTP concerns
    quote_parsing.rs           # JSON -> Cashu request enums
    response.rs                # Cashu response enum -> JSON
    cache.rs                   # NUT-19 route cache wrapper
    tests/
  federation/
    public_value_operations.rs # federation submit/wait/recover workflows
```

Do not create a generic route framework. A small concrete service may expose
methods such as:

```rust,ignore
create_mint_quote(...)
issue(...)
create_melt_quote(...)
melt(...)
swap(...)
```

The exact signatures should use existing Cashu request and response types. The
service owns federation-versus-local workflow selection only where that avoids
duplicating route behavior. HTTP headers, authentication, path method checks,
and JSON response shapes stay in handler modules.

### Desired handler shape

```text
authenticate and validate route method
    -> parse one Cashu request enum
    -> execute local/federated value operation
    -> serialize response
    -> apply NUT-19 caching at the route boundary
```

The service should use existing operation builders and
`submit_federation_envelope`; it should not reimplement admission, journal
application, or wallet recovery.

### Implementation sequence

1. Extract the inline test module without semantic changes.
2. Move pure request parsing and response shaping into small files.
3. Move existing federation helper functions as a unit into
   `public_value_operations.rs`.
4. Replace direct `MintState` choreography with focused service calls.
5. Only after behavior is stable, consolidate shared mint/batch-mint paths and
   replace polling with an existing notification primitive if one already
   carries the required state transition.

### Required tests

- Federation feature on and off.
- Mint, batch mint, melt, and swap public routes.
- Bolt11, Bolt12, on-chain, and custom quote methods.
- Assigned federation quote IDs and mismatched method rejection.
- Duplicate request after cache eviction and restart.
- Pending and terminal melt behavior.
- Fail-closed behavior while the member is not ready.
- Existing HTTP status codes and JSON shapes.

### Completion criteria

- Route files do not construct federation operations or wait for consensus
  application directly.
- Federation public workflow is discoverable under `federation/`.
- Standalone behavior remains a normal direct mint path, not an emulation of
  federation behavior.
- No new public API or wire type is introduced merely for the refactor.

---

## Opportunity 3 — Give runtime status one authoritative snapshot API

**Status:** Open  
**Impact / cost:** High / low–medium

### Current problem

`post_local_status` gathers peer status, materialized rows, sync state,
checkpoint state, payment reports, mempool size, notification statistics,
AlephBFT state, invalid-operation alerts, on-chain wallets, recovery,
wallet-liveness state, and operator state. It then constructs the common wire
response and updates metrics.

This makes an HTTP route understand the composition of nearly the entire node.
Readiness, metrics, operator APIs, and tests can drift because there is no
single runtime snapshot.

### Target ownership

Add a crate-local runtime snapshot under `node_status.rs` or `status.rs`:

```rust,ignore
struct FederationNodeStatusSnapshot { /* runtime-owned fields */ }

impl FederationNodeRuntime {
    async fn status_snapshot(
        &self,
        mint: &Mint,
        include_materialized_rows: bool,
    ) -> Result<FederationNodeStatusSnapshot, FederationRuntimeError>;
}
```

The snapshot is an internal ownership type. Conversion to
`FederationLocalStatusResponse` may remain at the HTTP boundary if the common
type is strictly a wire contract. Prometheus should consume the same snapshot
or its wire projection rather than independently querying state.

### Rules

- Preserve the current consistency level; do not imply that separately locked
  subsystems form an atomic distributed snapshot.
- Run independent reads concurrently only after proving they do not contend on
  the same transaction or lock order.
- Keep expensive materialized rows explicitly opt-in.
- Distinguish unavailable, disabled, stopped, and failed components rather than
  representing all of them as an absent optional field.

### Required tests

- Disabled/standalone runtime snapshot.
- Setup/DKG-only runtime snapshot.
- Active healthy runtime.
- Lagging, catching-up, and halted states.
- Failed background service visibility after Opportunity 1.
- Materialized rows excluded and included.
- HTTP conversion and Prometheus labels preserve existing behavior.

### Completion criteria

- Status routes verify requests and serialize results; they do not assemble
  runtime internals.
- Readiness and status consume the same component state definitions.
- Runtime status can be tested without constructing an HTTP request.

---

## Opportunity 4 — Split mint federation logic by business domain

**Status:** Open  
**Impact / cost:** High / medium

### Current problem

`cdk/src/mint/federation.rs` still owns runtime configuration, operation
construction, submission validation, accepted-operation dispatch, mint quote
application, payment observations, mint signing, swap application, melt
application, retry equivalence, and response recovery. Several focused
submodules already exist for address allocation, chain observation,
transaction policy/state, and projection, proving that the domain boundaries
are stable.

The remaining file is difficult to review because changes to independent value
operations share imports, helpers, and one implementation surface.

### Target module tree

```text
cdk/src/mint/federation/
  mod.rs                    # facade, mode, dispatch, shared invariants
  runtime_config.rs         # validated config and operation authorization
  operation_builder.rs      # public operation-envelope constructors
  mint.rs                   # quote, payment observation, issuance apply
  melt.rs                   # quote, payment observation, payment/final apply
  swap.rs                   # swap validation and apply
  signing.rs                # shared output-share creation/recovery
  payment_observation.rs    # shared observation checks only
  address_allocation.rs
  chain_observation.rs
  projection.rs
  transaction_policy.rs
  transaction_state.rs
```

`mod.rs` should contain the exhaustive operation dispatch so adding an
operation still produces a compiler-visible ownership decision. It should not
contain every applier implementation.

### Consolidation opportunities inside the split

Only consolidate helpers when mint and swap/melt genuinely share the same
rule:

- prior accepted/applied operation lookup;
- quote parsing and immutable-term comparison;
- persisted signature-share reconstruction;
- blinded-output share generation;
- transaction commit/rollback finalization already introduced by the earlier
  application-transaction work;
- deterministic retry equivalence.

Keep these explicit and operation-specific:

- Mint quote versus melt quote state transitions.
- Incoming payment quorum versus outgoing payment evidence.
- Melt external-payment saga boundaries.
- Balance and fee rules.
- Standard Cashu rejection construction.

Avoid an `OperationApplier` trait or generic state-machine engine. There is one
mint implementation and the important differences are security rules, not
boilerplate.

### Implementation sequence

1. Add characterization tests for dispatch and every operation kind.
2. Move code one domain at a time with no logic edits.
3. Reduce imports and make helpers private to their natural module.
4. Consolidate only helpers that remain byte-for-byte or rule-for-rule
   duplicates after the move.
5. Update module documentation with authority and replay guarantees.

### Required tests

- Golden operation IDs and canonical bytes remain unchanged.
- Accepted-operation dispatch covers every variant.
- Mint, swap, and melt idempotency and persisted response recovery.
- Deterministic rejection and transient failure behavior.
- Application transaction rollback and replay.
- Materialized projection remains exhaustive.
- Federation-disabled and standalone mint builds.

### Completion criteria

- No production mint federation file combines mint, swap, and melt appliers.
- `mod.rs` remains a small facade and exhaustive dispatcher.
- Shared helpers encode truly shared invariants rather than hiding domain
  differences.
- No serialization, database, or error behavior changes during pure moves.

---

## Opportunity 5 — Consolidate the DKG state machine boundary

**Status:** Open  
**Impact / cost:** High / high

### Current problem

The DKG design already separates a durable-record planner in `cdk-common` from
an Axum driver, but the separation is incomplete:

- Common `FederationDkgDriverRecords` and `FederationDkgDriverPlan` model the
  dealer-free Cashu keyset ceremony.
- Axum defines another `FrostDkgPhase` and maps it into the common driver phase.
- Axum's `run_frost_dkg_tick` combines cryptographic state evolution,
  encrypted-state persistence, record loading, outbound delivery, delivery
  accounting, and phase decisions.
- BLS/Cashu DKG and FROST wallet DKG are coordinated by the same outer driver
  but have different cryptographic records and completion conditions.

This is not a reason to merge the two cryptographic protocols. It is a reason
to give both the same explicit planner/executor boundary.

### Target boundary

```text
durable records + validated ceremony config
    -> pure phase planner
    -> typed next action(s)
    -> Axum persistence/transport executor
    -> newly durable records
    -> planner again
```

Possible concrete modules:

```text
cdk-common/src/federation/
  dkg.rs                    # ceremony records and transcript validation
  dkg_driver.rs             # outer combined ceremony plan
  frost_dkg.rs              # FROST record types and cryptographic validation
  frost_dkg_driver.rs       # local FROST phase planning/evolution

cdk-axum/src/federation/
  dkg_driver.rs             # bounded orchestration loop
  dkg_delivery.rs           # authenticated concurrent delivery and retry stats
  dkg_persistence.rs        # atomic durable effects
  dkg_routes.rs             # thin private REST boundary
```

The first safe simplification is to remove the mirror `FrostDkgPhase` and use
one shared phase vocabulary. The deeper step is to move local FROST transition
logic behind a typed plan/result so Axum no longer decides cryptographic phase
progress while performing I/O.

### Typed actions, without a generic effect framework

The planner may return a small DKG-specific enum such as:

```rust,ignore
enum FederationFrostDkgAction {
    CreateRoundOne,
    AdvanceRoundTwo,
    FinalizeRoot,
    BroadcastRoundOne,
    DeliverRoundTwo,
    BroadcastConfirmation,
    Complete,
    Wait,
}
```

Exact types must preserve secret ownership. Secret-bearing results must not
derive `Serialize`, `Clone`, or verbose `Debug` unless explicitly safe.

### Rules that must remain explicit

- Readiness is all-member and ceremony-bound.
- Records from another federation, ceremony, member, epoch, or keyset spec are
  rejected.
- Secret shares retain receiver binding and confidentiality requirements.
- Durable local encrypted state is written before relying on later delivery.
- Earlier artifacts may be rebroadcast idempotently for lagging members.
- Completion requires the existing transcript and all-member confirmation
  rules.
- Delivery failure classification must stay typed; strings are diagnostics,
  never phase control flow.

### Implementation sequence

1. Freeze golden tests for durable records, restart at every phase, and
   transcript mismatch behavior.
2. Replace `FrostDkgPhase` with the common phase vocabulary.
3. Extract delivery tracking and concurrent fanout from cryptographic phase
   evolution.
4. Introduce the FROST-specific planner/result boundary.
5. Move atomic persistence of each transition into `dkg_persistence.rs`.
6. Reduce the Axum driver to load -> plan -> execute -> report.
7. Split large tests by planner, persistence, route, and live delivery.

### Required tests

- Restart before and after every durable phase transition.
- Duplicate and reordered authenticated deliveries.
- Cross-ceremony, cross-member, and cross-federation rejection.
- Missing, corrupt, or undecryptable local state fails closed.
- Mismatched public-root confirmations never complete.
- Delivery to all members, including self, remains idempotent.
- HTTPS/loopback confidentiality policy remains unchanged.
- Cancellation does not release the bounded CPU-work permit early.
- Debug and error output never exposes secret state.
- Existing combined BLS plus FROST ceremony completes with identical config.

### Completion criteria

- Axum does not maintain a second phase model.
- Cryptographic progress decisions are testable without HTTP.
- Transport delivery does not mutate phase implicitly.
- BLS and FROST remain separate cryptographic domains coordinated by one clear
  outer plan.

---

## Opportunity 6 — Consolidate on-chain transaction proposal construction

**Status:** Open  
**Impact / cost:** High / high

### Current problem

The accepted on-chain wallet model is intentionally spread across trust
boundaries, but some business logic is in the wrong layer:

- `cdk-common` defines intent, proposal, signing, broadcast, settlement, and
  complete projected-state types in one large module.
- Mint `transaction_policy.rs` independently validates untrusted proposals.
- Mint `transaction_state.rs` applies and persists accepted state.
- Axum `wallet_liveness.rs` schedules work **and** selects accepted UTXOs,
  adapts a BDK PSBT, classifies recipient/change outputs, allocates fees, builds
  replacements, validates the proposal, and submits it.

The liveness worker should decide when and which member attempts work. It
should not be the primary home of transaction-construction rules.

### Target ownership

Preserve three distinct layers:

1. **Local wallet adapter:** asks BDK for a candidate transaction using local
   chain/fee data. This input is untrusted.
2. **Proposal constructor:** converts the candidate plus accepted state into a
   canonical `FederationTransactionProposal`, classifies every output, and
   rejects anything not authorized.
3. **Independent proposal validator:** revalidates the proposal on every member
   using only accepted state and public policy.

Suggested layout:

```text
cdk/src/mint/federation/
  transaction_builder.rs       # candidate -> canonical proposal
  transaction_policy.rs        # independent untrusted proposal validation
  transaction_state.rs         # accepted projection and persistence

cdk-axum/src/federation/
  wallet_liveness.rs           # leases, retries, work selection
  wallet_proposer.rs           # BDK adapter and fee-source I/O
  frost_signing.rs
  broadcast.rs
```

The constructor should accept an explicit candidate/draft type containing the
unsigned transaction, selected previous outputs, requested change allocation,
and local fee observation. It must not accept arbitrary proposer claims as
already validated facts.

### Shared versus independent logic

Construction and validation may share pure primitives for:

- checked input/output sums;
- recipient output matching;
- change output classification;
- fee contribution allocation;
- transaction weight and count limits;
- RBF fee delta arithmetic.

They must not collapse into one call that validates only what the proposer just
constructed. Every receiving member must continue to execute an independent
top-level validation path.

### Type organization

After behavior is stable, split `transaction_proposal.rs` behind stable facade
re-exports:

```text
transaction/
  mod.rs
  intent.rs
  proposal.rs
  authorization.rs
  signing.rs
  broadcast.rs
  settlement.rs
  state.rs
```

Do not change serde tags, version constants, hash domains, or canonical bytes
during this move.

### Implementation sequence

1. Add golden proposal, authorization, signed-transaction, and state bytes.
2. Extract pure arithmetic/output-classification helpers with adversarial
   tests.
3. Move BDK-specific I/O into `wallet_proposer.rs`.
4. Introduce the mint-owned proposal constructor.
5. Reduce wallet liveness to work selection, lease/retry, and submission.
6. Split common transaction types mechanically.
7. Re-evaluate whether broadcaster and signing coordinator share only
   scheduling utilities; do not merge their state machines.

### Required tests

- Candidate containing an unknown input.
- Missing, duplicated, or substituted recipient output.
- Extra unallocated output.
- Wrong or dust change.
- Input/output and fee overflow.
- Fee cap, weight, input/output count, and confirmation-target limits.
- First transaction and bounded RBF replacement.
- Replacement lineage mismatch and insufficient fee increase.
- Different members independently accept the same proposal bytes.
- Malicious locally constructed candidate is rejected by the independent
  validator.
- Restart between proposal, signing, broadcast intent, observation, and
  settlement.

### Completion criteria

- `wallet_liveness.rs` is recognizably a scheduler, not a transaction policy
  implementation.
- Proposal construction has one domain owner.
- Independent validation remains mandatory and separately tested.
- Common transaction modules are navigable without changing wire contracts.

---

## Opportunity 7 — Add narrow private-route request helpers

**Status:** Open, deliberately narrower than the previously proposed generic
extractor
**Impact / cost:** Medium–high / medium

### Current problem

DKG, FROST signing, status, operator, AlephBFT, and other private handlers
repeat combinations of:

```text
acquire admission permit
verify federation/member/signature/message kind
check readiness
decode and validate payload
perform operation
record or atomically persist sequence
sign response
map errors
```

The common authentication is duplicated, but sequence semantics are not
uniform. DKG mutations use durable atomic replay protection, read/liveness
messages use bounded in-memory windows, and operation submission relies on
canonical operation identity. One universal extractor would hide these
security differences.

### Target design

Add a small `private_request.rs` module containing only shared mechanics and
typed verified context:

```rust,ignore
struct VerifiedPrivateRequest<'a, T> {
    sender: MemberId,
    sequence: u64,
    kind: FederationRestMessageKind,
    payload: &'a T,
    permit: OwnedSemaphorePermit,
}
```

Use narrow entry points or policies for established message classes:

- authenticated read/liveness request;
- ephemeral FROST mutation whose sequence is recorded after success;
- durable DKG mutation whose replay watermark is committed atomically with its
  record;
- operation submission, which remains separate;
- high-frequency Aleph network traffic, which retains its specialized path.

Response signing can be shared because it does not decide sequence
consumption. The call that consumes or persists a sequence must remain visible
at the handler or domain persistence boundary.

### Implementation sequence

1. Write a message-kind classification table in module documentation.
2. Extract response signing and common identity/signature verification.
3. Migrate one read route and one FROST route.
4. Verify that ordering of validation, effect, and sequence recording is
   unchanged.
5. Migrate similar routes; leave DKG atomic persistence and operation admission
   specialized.
6. Stop if the helper increases type parameters or hides route semantics
   without deleting meaningful code.

### Required tests

- Unknown member, wrong federation, wrong kind, and invalid signature.
- Admission saturation and closed semaphore.
- Replay and equivocation in each message class.
- Invalid payload does not consume a sequence where current semantics forbid
  it.
- Successful mutation records the sequence at the same acceptance point.
- Response is signed for the verified requester and original sequence.
- DKG record and replay watermark remain atomic.

### Completion criteria

- Common signed-envelope verification is not copied route by route.
- Sequence consumption remains explicit and class-specific.
- No generic middleware commits durable state before domain validation.

---

## Opportunity 8 — Split common transport and transaction types by domain

**Status:** Open  
**Impact / cost:** Medium / low–medium

### Current problem

`cdk-common/src/federation/transport.rs` contains the signed request envelope,
body hashes, signatures, sequence windows, FROST signing requests, DKG
requests, status responses, wallet operator requests, operation submission,
AlephBFT messages, and checkpoint/catch-up messages. Canonical signing helpers
sit far from several message families.

The module is logically a facade but physically one implementation file. A
change to one private protocol requires navigating unrelated types and tests.

### Target module tree

Preserve `cdk_common::federation::*` re-exports while using internal modules:

```text
federation/transport/
  mod.rs                 # curated facade and shared version constants
  envelope.rs            # hashes, signatures, signed request
  replay.rs              # bounded sequence trackers/windows
  status.rs              # peer/local/operator status wire messages
  dkg.rs                 # DKG private messages
  frost.rs               # FROST signing private messages
  operation.rs           # submit operation request/response
  aleph.rs               # activity/network/backup messages
  wallet.rs              # operator wallet requests/responses
```

Checkpoint and catch-up transport types must remain untouched until the
checkpoint consolidation plan chooses their final home. They may temporarily
remain in a residual file.

Keep canonical signing transcript construction next to each message family or
in `envelope.rs` only when it is genuinely identical. Do not introduce dynamic
message registries or trait objects.

### Mechanical-move rules

- No serde rename/tag/default changes.
- No version or hash-domain changes.
- No public path changes unless all downstream crates are updated in the same
  commit and compatibility is explicitly accepted.
- Move golden tests with the owning message family.
- `mod.rs` must not become another multi-thousand-line implementation.

### Required tests

- Golden JSON and canonical signing bytes for every message family.
- Signature verification and wrong-kind rejection.
- Replay-window boundary behavior.
- Cross-crate public API compilation.
- Request size limits remain attached to the same routes.

### Completion criteria

- No non-checkpoint transport file mixes DKG, FROST, status, operation, and
  Aleph message implementations.
- Stable facade paths remain available.
- A protocol reviewer can find a request, transcript, validation, and tests in
  one domain module.

---

## Opportunity 9 — Consolidate federation test infrastructure

**Status:** Open  
**Impact / cost:** Medium / medium

### Current problem

`custom_handlers.rs` contains a multi-thousand-line inline test module with a
large fixture prelude. `federation/tests.rs` exceeds 25k lines and combines fake
transport, storage setup, member factories, Aleph sessions, DKG, route tests,
fault injection, operation application, restart, and end-to-end value flows.

Large tests are not inherently redundant, but the present organization causes:

- repeated fake wallets and state builders;
- fixtures that are hard to discover outside their original file;
- near-copy mint/swap/melt recovery tests;
- uncertainty about which suite owns an invariant;
- misleading production-versus-test line counts;
- expensive review because unrelated scenarios share imports and helpers.

### Target test organization

Use small, composable support modules rather than one all-purpose cluster:

```text
cdk-axum/src/test_support/
  federation/
    mod.rs
    members.rs          # configs and member identities
    mint_state.rs       # DB-backed state construction
    payments.rs         # focused fake payment backends
    transport.rs        # authenticated fake peer transport and faults
    aleph.rs            # live-session helpers
    assertions.rs       # convergence and durable-state assertions

cdk-axum/src/federation/tests/
  mod.rs
  routes.rs
  runtime.rs
  consensus.rs
  dkg.rs
  wallet.rs
  operations.rs
  recovery.rs

cdk-axum/src/custom_handlers/tests/
  mod.rs
  mint.rs
  melt.rs
  quote_methods.rs
  cache.rs
```

Fixture layers should be opt-in. A handler unit test should not start a live
Aleph cluster, and a consensus test should not need a fake Lightning backend
unless the invariant requires it.

### Test ownership matrix

Assign one primary home to each class:

| Invariant | Primary home |
|---|---|
| HTTP method, status, and JSON contracts | Handler route tests |
| NUT-19 cache behavior | Handler cache tests |
| Wallet member fanout and threshold aggregation | `cdk` wallet federation tests or wallet integration tests |
| Private authentication and replay | Federation route tests |
| Consensus ordering and duplicate admission | Consensus/operation tests |
| Restart and durable replay | Federation recovery tests |
| DKG phase and transcript rules | Common planner plus Axum DKG integration tests |
| Full multi-member value flow | A small number of federation E2E tests |

Secondary suites should keep only a smoke test when crossing a real boundary.

### Parameterization candidates

Table-drive repeated families across:

- operation: mint, swap, melt;
- disruption: response-cache eviction, SQLite restart, peer catch-up;
- outcome: accepted, deterministic rejection, transient apply failure;
- duplicate timing: pending, accepted-not-applied, applied-not-published,
  finalized;
- federation size and threshold where the behavior is otherwise identical.

Keep separate tests where operation semantics differ, particularly external
melt payment and terminal observation behavior.

### Implementation sequence

1. Extract `custom_handlers` tests mechanically.
2. Inventory fixtures and invariants before deleting anything.
3. Create only the smallest shared fixtures needed by at least two suites.
4. Split `federation/tests.rs` by ownership without changing assertions.
5. Parameterize the largest exact-copy families.
6. Relocate swap tests and wallet-fanout tests to their natural owner.
7. Delete duplicate coverage only after the ownership matrix names the primary
   test.
8. Record test runtime before and after.

### Required checks

- Test count changes are explained.
- Every deleted test maps to retained coverage.
- Failure messages identify operation, disruption, and member.
- No fixture stores secret material in snapshots or logs.
- Running a focused module does not require the complete federation E2E suite.
- Production and test LOC are reported separately.

### Completion criteria

- Production modules contain no multi-thousand-line inline test suites.
- No single test file owns unrelated DKG, HTTP, consensus, and wallet families.
- Shared fixtures remove real duplication without becoming a mandatory global
  harness.
- Safety coverage is easier to find than before the move.

---

## Opportunity 10 — Unify NUT-19 cache and quote parsing mechanics

**Status:** Open  
**Impact / cost:** Low–medium / low

### Current problem

Mint, batch-mint, and melt cache entry points repeat:

```text
validate -> calculate key -> cache lookup -> execute on miss -> cache write
```

Mint and melt quote handlers also contain parallel method matches that parse
Bolt11, Bolt12, on-chain, and custom request variants before invoking parallel
mint APIs. Federation quote-ID assignment and readiness checks are interleaved
with those arms.

The duplication is small compared with the runtime work, but it is on public
value routes where drift in authentication, cache namespaces, or response
serialization is costly.

### Target design

Under `custom_handlers/` add:

- `cache.rs` with a small typed cache-through helper;
- `quote_parsing.rs` with `parse_mint_quote_request` and
  `parse_melt_quote_request` returning existing Cashu enums;
- `response.rs` for method-specific JSON response shaping.

Keep three named public cache entry points. The helper must preserve distinct
namespaces (`mint`, `mint_batch`, and `melt`) and must not cache a response under
a key derived from a different request representation.

Normalize the request once:

```text
validate route method and auth
    -> assign optional federation quote ID
    -> parse existing Cashu request enum
    -> call value-operation service
    -> serialize method-specific response
```

### Required tests

- Cache hit and miss for all three routes.
- Namespace isolation.
- Malformed body and method mismatch do not poison cache entries.
- Bolt11, Bolt12, on-chain, and custom quote round trips.
- Federation assigned quote ID is identical before and after extraction.
- Cached federation responses preserve idempotency and signature shares.
- Standalone behavior remains unchanged.

### Completion criteria

- Cache entry points share one cache-through mechanic.
- Quote parsing has one mint and one melt owner.
- Response shaping remains explicit rather than erased behind untyped JSON.

---

## Cross-cutting implementation rules

### Prefer moves before consolidation

For Opportunities 2, 4, 8, and 9, land or review mechanical moves separately
from behavior changes. A reviewer should be able to distinguish changed
ownership from changed protocol or business rules.

### Use structured errors only at decision boundaries

Introduce typed errors where code decides retryability, readiness, HTTP status,
or lifecycle state. Do not conduct a repository-wide replacement of every
diagnostic string. In particular:

- DKG delivery classification should consume typed transport errors.
- Background service failures should expose service and lifecycle state.
- Standard Cashu errors must pass through unchanged.
- Display strings must not become control flow.

### Preserve exhaustive matches

Operation dispatch, materialized projection, status conversion, and transaction
state transitions should retain exhaustive enum matches. Do not replace useful
compiler checks with registries, string maps, or default arms.

### Keep facades small

Stable re-exports from `cdk_common::federation` and `cdk::mint` are valuable.
Their facade modules should document dependencies and expose types, while
implementations live in focused files.

### Add size checks as review guidance, not architecture

An optional development script may report production and test LOC separately
and list files over review thresholds. It must not reward arbitrary splitting
or count generated code as hand-maintained product logic.

---

## Consolidations deliberately not recommended

### Do not rebuild wallet quorum collection

The wallet already has one concurrent member scheduler with endpoint-specific
collectors. Mint signatures, melt responses, restore, quote status, and
check-state have different agreement rules. Keep those policies concrete. Only
extract a further helper when two policies demonstrably make the same decision.

### Do not introduce a broad validated-config type hierarchy

Full validation already occurs at import/startup boundaries and expensive
derived values are cached. Earlier review found setup/active/raw wrapper types
would spread conversions across more crates without eliminating a current
bypass.

### Do not use one universal private request extractor

Authentication is shared; replay and sequence persistence are not. Opportunity
7 intentionally uses narrow helpers and leaves acceptance points explicit.

### Do not introduce a generic operation-applier framework

Mint, swap, melt, payment observation, transaction, and keyset rotation have
different security rules. Focused modules plus exhaustive dispatch give better
reviewability than a trait hierarchy.

### Do not mix checkpoint consolidation into these moves

Checkpoint and catch-up types are changing under their own plan. Combining the
projects would make review, rollback, and security analysis unnecessarily
difficult.

---

## Recommended execution order

```text
Phase A — establish reviewable files and baseline
  extract custom_handlers tests
  record invariant ownership and test runtime
  add optional prod/test size reporting

Phase B — low-risk duplication removal
  unify cache-through mechanics
  extract quote parsing and response shaping
  add runtime status snapshot

Phase C — ownership corrections
  supervise federation background services
  move public federation workflows out of handlers
  split mint federation by value-operation domain

Phase D — mechanical common/test organization
  split non-checkpoint transport modules
  split large federation test suites
  table-drive proven duplicate families

Phase E — security-sensitive domain consolidation
  consolidate DKG planner/executor boundary
  consolidate transaction proposal construction

Phase F — narrow route cleanup
  add class-specific private request helpers after DKG and runtime ownership
  have stabilized
```

Each phase should leave the tree buildable and testable. Do not combine DKG and
on-chain transaction consolidation in one review.

---

## Verification matrix

Run the smallest relevant set after each mechanical move, then the broader
matrix before declaring an opportunity complete:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --lib --workspace --exclude cdk-postgres
cargo test -p cdk-axum
cargo test -p cdk --features federation
cargo test -p cdk-common
```

Add focused SQLite restart and real-Aleph multi-member tests for lifecycle,
operation, DKG, and transaction changes. PostgreSQL parity should be exercised
where the affected persistence code has a PostgreSQL implementation.

---

## Changelog

| Date | Note |
|---|---|
| 2026-07-13 | Initial custom-handler and test-organization opportunities. |
| 2026-07-17 | Expanded into a ranked federation-wide consolidation design covering runtime services, HTTP workflows, status ownership, mint domains, DKG, on-chain transactions, private routes, common transport, tests, and route mechanics; checkpoint/catch-up remains separately owned. |
