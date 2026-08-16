# Federated BDK and Bark backend decoupling plan

Date: 2026-08-16

Status: implemented locally; migration and full topology acceptance pending

## Decision

Federated BDK and federated Bark must be independently selectable payment
backends. They may reuse generic Bitcoin, chain-source, and FROST primitives,
but neither backend may require the other backend's feature, configuration,
application identity, wallet state, balance, UTXOs, or runtime.

The refactor must reduce long-term complexity. It must not introduce a generic
plugin framework, a permanent dual architecture, or new top-level Bark
consensus phases merely to model CPFP execution.

## Required outcome

The repository and runtime must support all three configurations:

| Configuration | Compiled features | Runtime payment methods | Required wallet applications |
|---|---|---|---|
| BDK only | `federation-bdk`; no `federation-bark` | On-chain | BDK payment application |
| Bark only | `federation-bark`; no `federation-bdk` | BOLT11 | Bark custody and Bark recovery applications |
| BDK and Bark | `federation-bdk,federation-bark` | On-chain and BOLT11 | Distinct BDK payment, Bark custody, and Bark recovery applications |

When both backends are enabled, they may share one sealed FROST root and one
chain-source implementation. They must use domain-separated application IDs,
derivation paths, address spaces, UTXO scopes, signing sessions, fee budgets,
and operator status.

## Why this work is required

The current implementation couples Bark recovery to the federated BDK payment
wallet:

- `FederationFrostWalletConfig` contains a mandatory
  `cdk_onchain_application` and adds Bark as an extension around it.
- initial Bark bootstrap is rejected unless the CDK on-chain wallet was also
  bootstrapped;
- emergency-exit requests allocate their destination from the BDK
  application's address space;
- the `cdk-mintd` `federation` feature enables `bdk`, while the corresponding
  Axum feature directly enables `cdk-bdk`;
- Bark maintenance orchestration is owned by `cdk-axum`, even though refresh,
  exit, and recovery are backend lifecycle work rather than HTTP concerns.

The live Signet federation also exposes two recovery defects that this plan
must cover with synthetic regression tests before code is rearranged:

- accepted receive-claim state fails member-local replay after restart;
- emergency exits stop at Bark's `awaiting-cpfp-broadcast` boundary because the
  federation facade calls the pause-capable exit progression path and no
  consensus-authorized component supplies the CPFP child.

The fail-closed degraded state is correct. Indefinite recovery without a path to
completion is not.

## Scope

### In scope

- split the compile-time feature graph so BDK and Bark are independently
  selectable;
- make the BDK FROST application optional in the public wallet config;
- give Bark a separately registered, threshold-controlled on-chain recovery
  application;
- make Bark emergency exits and CPFP use only Bark-owned recovery state;
- fix receive-claim restart replay;
- consolidate startup replay, ordinary recovery, operator retry, and automatic
  maintenance around one Bark recovery driver;
- remove Bark lifecycle ownership from `cdk-axum`;
- preserve a bounded, explicitly temporary compatibility path for the current
  public configuration;
- migrate or replace the live test federation through an explicit operator
  procedure; and
- delete the legacy BDK-backed Bark recovery path after migration evidence is
  complete.

### Out of scope

- changing public Cashu NUT-04 or NUT-05 behavior;
- changing standalone payment backends;
- replacing AlephBFT or altering consensus ordering;
- creating a generic payment-plugin framework;
- rewriting the complete Bark state machine;
- renumbering existing Bark semantic phase codes;
- merging BDK and Bark wallet balances when both are enabled; or
- preserving legacy BDK-backed Bark recovery indefinitely.

## Complexity budget

Complexity reduction is a release requirement, not an incidental benefit.

1. Record the baseline non-test production line count and dependency edges for
   every touched crate before implementation.
2. Do not add a new crate unless it replaces enough existing code to produce a
   smaller final dependency and ownership surface.
3. Reuse `FederationFrostApplicationConfig` and existing canonical federation
   envelopes. Do not add a parallel application registry or consensus channel.
4. Do not add new top-level Bark semantic phases for the ordinary CPFP path.
5. Keep all temporary compatibility behavior behind one adapter with an
   explicit removal milestone.
6. Use one recovery transition driver for startup replay, live maintenance,
   and exact-action operator retry.
7. After the legacy adapter is removed, target non-test production code at or
   below the recorded baseline. Any net increase requires a written accounting
   showing which duplicate path was removed and why the remaining code is
   necessary.
8. Moving a function or splitting a file does not count as simplification by
   itself. The final review must identify deleted branches, duplicate loops,
   obsolete types, and removed dependency edges.

## Safety invariants

The following constraints apply to every milestone:

- an exact accepted plan must precede every threshold signature, protected
  secret disclosure, external Ark request, or Bitcoin broadcast;
- deterministic apply may not perform chain I/O, Ark I/O, signing, randomness,
  local-clock reads, or member-local wallet reads;
- the accepted journal and deterministic Bark projection remain authority;
- Bark SQLite remains a rebuildable member-local execution cache;
- actor or signer failover may change the executor or attempt, not the accepted
  economic action;
- CPFP parent, funding inputs, outputs, fee, fee ceiling, package relationship,
  actor, and attempt must be independently validated;
- response loss may not create a second logical CPFP, exit drain, payment, or
  reservation;
- BDK-only builds contain no Bark types or Bark dependencies;
- Bark-only builds do not instantiate or reference the federated BDK payment
  backend;
- disabling `federation-bark` removes Bark code and dependency resolution; and
- no Bark-specific type may move into `cdk-common` or a general consensus
  module.

## Target ownership

```text
Federation consensus and generic FROST/Bitcoin primitives
├── BDK payment application
│   └── on-chain MintPayment adapter
└── Bark
    ├── Bark VTXO custody application
    ├── Bark on-chain recovery application
    ├── Bark MintPayment adapter
    └── Bark recovery driver
```

| Concern | Owner |
|---|---|
| Application registration and domain separation | Generic FROST configuration plus backend-owned registration code |
| BDK address allocation, UTXOs, proposals, and payments | Federated BDK backend |
| Bark VTXOs, Lightning actions, refresh, exit, and CPFP | `cdk-federation-bark` and `cdk-bark` |
| Bark background recovery lifecycle | `cdk-bark` |
| Starting and stopping backend workers | `cdk-mintd` |
| Public HTTP/Iroh routing and operator request parsing | `cdk-axum` |
| Ordering accepted Bark operations | Existing federation consensus runtime |

`cdk-axum` may call a narrow Bark operator entry point, but it must not own the
refresh/exit state machine or retry loop.

## Design

### Independent application registration

Keep the existing sealed FROST root and application derivation mechanism. Make
the BDK payment application optional and register two Bark-owned applications:

1. the existing Bark VTXO custody application; and
2. a Bark recovery application with a new domain-separated application label.

The Bark recovery application owns recovery addresses, fee-reserve UTXOs,
transaction signing sessions, and chain confirmation policy. Its configuration
belongs to `cdk-federation-bark`; general federation code sees only the existing
generic application registration type.

Do not rename a BDK payment application into a supposedly generic application
while retaining its BDK-specific state semantics. Shared types may be neutral;
wallet state must remain backend-owned.

### Independent feature graph

Split the current feature edges as follows:

- `federation` enables consensus, storage, FROST, and transport only;
- a new `federation-bdk` feature enables `cdk-bdk` and the on-chain payment
  adapter;
- `federation-bark` enables only Bark-specific crates plus the generic
  federation/FROST requirements it actually uses.

Preserve current default behavior during one transition release if required,
but tests must exercise an explicit Bark-only build in which `cdk-bdk` is not a
direct CDK backend dependency.

If Bark's upstream wallet uses the `bdk_wallet` library internally, that is an
implementation detail and does not enable the CDK federated BDK payment
backend.

### Bark-owned recovery and CPFP

Keep the existing semantic lifecycle:

```text
ExitPlanned -> ExitRegistered -> ExitPublished -> ExitConfirmed
```

Enrich those accepted operations rather than adding ordinary-path top-level
phases:

- `ExitPlanned` authorizes the recovery destination, parent/package policy,
  fee ceiling, recovery wallet scope, actor, and attempt;
- `ExitRegistered` binds the exact exit parents, prepared CPFP package, and
  Bark recovery fee-input reservations before signing or broadcast;
- `ExitPublished` binds the exact signed CPFP child and drain publication
  evidence; and
- `ExitConfirmed` retains distinct-member confirmation observations and
  terminal accounting.

If an exact CPFP child cannot be safely prepared before `ExitRegistered`, stop
the milestone for design review. Do not silently add multiple new phases or
allow an unaccepted broadcast. A fee replacement, changed input, or actor
attempt may use an additional versioned retry transition because it represents
a genuinely changed irreversible artifact.

The BDK payment wallet must never fund, reserve, or sign a Bark CPFP. Bark-only
operators must fund the Bark recovery application's bounded fee reserve through
an explicit operator flow.

### One recovery driver

Replace the divergent orchestration paths with one backend-owned driver that:

1. reads an accepted Bark projection;
2. determines the next permitted recovery step without side effects;
3. returns either no work, an accepted-operation proposal, or an exact
   already-authorized execution request;
4. executes an external effect only after its accepted authorization is
   supplied;
5. returns canonical evidence for admission; and
6. replays the same accepted entries during restart without external effects.

The driver owns refresh, failed-send revocation, incoming-claim recovery,
emergency exit, CPFP, and confirmation progression. `cdk-mintd` owns its task
lifecycle. `cdk-axum` retains only request parsing and the optional exact-action
retry route.

Avoid a broad callback or plugin framework. If a submission boundary is
required, define the smallest Bark-specific interface that submits an existing
`FederationOperationEnvelope` and returns its accepted result.

### Replay compatibility

Existing accepted operation bytes and phase codes must remain decodable. New
fields require a versioned Bark payload or a new Bark application/config epoch;
they may not alter the meaning of an existing v1 byte sequence.

The current production-like federation configuration remains a legacy input,
not the target architecture. Compatibility must be isolated behind one
`LegacyBdkRecoveryApplication` adapter or equivalent narrow conversion. New
federations must not emit that form.

## Milestones

### Milestone 0: freeze evidence and baseline complexity

Tasks:

- add sanitized regression fixtures reproducing receive-claim replay failure;
- add a synthetic exit fixture that reaches `awaiting-cpfp-broadcast` without
  copying private deployment databases or identifiers;
- prove the current worker retries indefinitely without producing a CPFP;
- record production/test line counts and direct feature/dependency edges;
- document the current public-config version and application IDs; and
- verify the working tree contains no deployment inventory or private state.

Exit criteria:

- both defects fail deterministically under local tests;
- fixtures contain no server, account, wallet-secret, invoice, or live
  transaction data; and
- the complexity baseline is committed with the plan implementation ledger.

### Milestone 1: freeze the independent configuration contract

Tasks:

- update `docs/federated-cashu-implementation-plan.md` and the Bark-FROST plan;
- make the BDK application optional in the FROST wallet config;
- define the Bark recovery application registration and derivation domain;
- define validation for BDK-only, Bark-only, and combined configurations;
- forbid overlapping application IDs, derivation domains, and wallet scopes;
  and
- define the legacy-config decoder without permitting new legacy configs.

Primary areas:

- `crates/cdk-federation-bark/src/config.rs`;
- generic FROST wallet configuration;
- public/private federation configuration validation; and
- configuration encoding tests.

Exit criteria:

- all three configurations round-trip and validate;
- Bark-only public configuration contains no BDK payment application;
- combined configuration proves domain and scope separation; and
- old configuration bytes still decode with their original meaning.

### Milestone 2: split features and DKG/bootstrap wiring

Tasks:

- remove the `federation -> bdk` feature implication;
- add the narrow `federation-bdk` feature edges required by the payment
  adapter;
- remove `cdk-bdk` from the base Axum federation feature;
- make BDK and Bark bootstrap arguments independent;
- generate the Bark recovery application whenever Bark is configured;
- remove the error requiring the CDK recovery wallet for Bark bootstrap; and
- ensure setup/status output names each enabled application without exposing
  private material.

Primary areas:

- root and crate `Cargo.toml` files;
- `crates/cdk-mintd/src/federation_dkg_status.rs`;
- `crates/cdk-mintd/src/config.rs`; and
- mintd startup validation.

Exit criteria:

- BDK-only, Bark-only, and combined minimal builds pass;
- `cargo tree` for Bark-only has no `cdk-bdk` CDK backend edge;
- disabling `federation-bark` does not resolve Bark dependencies; and
- each DKG/bootstrap combination either succeeds or returns one precise
  configuration error.

### Milestone 3: repair replay through one Bark driver

Tasks:

- extract the current receive, send, revocation, refresh, and exit replay order
  into one Bark-owned driver;
- make startup reconciliation and post-application reconciliation call that
  driver;
- make exact-action operator retry use the same transition selection;
- repair accepted receive-claim materialization after restart;
- keep replay free of Ark, chain, signing, random, and clock side effects; and
- remove the replaced reconciliation branches.

Primary areas:

- `crates/cdk-bark/src/`;
- `crates/cdk-axum/src/lib.rs` startup reconciliation; and
- Bark operation-pipeline reconciliation.

Exit criteria:

- clean database rebuild reaches the accepted frontier;
- restart succeeds at every receive/send/revocation/refresh/exit boundary;
- replaying the same journal twice produces the same digest and no external
  calls; and
- the old duplicate replay paths are deleted.

### Milestone 4: implement Bark-owned exit and CPFP recovery

Tasks:

- allocate the exit destination from the Bark recovery application;
- prepare and validate CPFP parent/child packages against accepted policy;
- reserve only Bark recovery fee UTXOs;
- perform threshold signing only after accepted authorization;
- make publication and response-loss retries idempotent;
- support bounded fee replacement and signer/actor retry;
- confirm the final drain into Bark recovery custody; and
- clear `recovery_required` only after accepted terminal recovery.

Primary areas:

- `crates/cdk-federation-bark/src/operation.rs` and projection validation;
- `crates/cdk-bark/src/runtime.rs` and smaller recovery modules;
- Bark's restricted federation facade; and
- Bark-specific SQL projection code.

Exit criteria:

- the synthetic CPFP fixture reaches `ExitConfirmed`;
- no BDK payment UTXO, address, signer, or database is accessed;
- crash/restart before and after CPFP signing, broadcast, and response loss is
  safe;
- insufficient fee reserve degrades with an actionable bounded status rather
  than looping silently; and
- the normal recovery path adds no top-level Bark semantic phase.

### Milestone 5: remove Bark maintenance ownership from Axum

Tasks:

- move the retry loop, backoff, transition selection, and effect orchestration
  into the Bark driver;
- start and stop the driver from `cdk-mintd`;
- retain only operator authentication, request parsing, and response conversion
  in Axum;
- reuse the same driver entry point for automatic and operator-triggered work;
  and
- delete `cdk-axum` Bark maintenance branches and duplicate submit helpers.

Exit criteria:

- `cdk-axum` contains no Bark refresh/exit phase match;
- one bounded worker exists per configured Bark backend;
- stopping HTTP listeners does not define Bark maintenance lifecycle; and
- worker shutdown and restart do not lose accepted work.

### Milestone 6: migration and temporary compatibility

Tasks:

- determine whether the current federation supports an authenticated
  application/config epoch upgrade without a new DKG;
- if supported, propose and verify the exact public-config transition before
  changing any host;
- otherwise create a new federation rather than rewriting accepted config;
- provide a read-only preflight showing recovery balances, pending actions,
  application IDs, and quorum readiness;
- migrate one member at a time while retaining quorum;
- verify replay and readiness before continuing; and
- mark the legacy adapter for removal immediately after migration evidence is
  complete.

Exit criteria:

- no in-place mutation of accepted v1 bytes or private shares;
- all members agree on the new config/application epoch;
- the Bark recovery application is funded for bounded CPFP operation;
- BDK and Bark status expose separate balances and application scopes; and
- rollback criteria and backup verification are documented before rollout.

### Milestone 7: delete legacy coupling and close the complexity budget

Tasks:

- delete the legacy BDK-backed Bark recovery adapter;
- delete obsolete configuration fields, validation branches, DKG coupling,
  tests, and documentation;
- remove unused dependency edges;
- recount production lines and ownership surfaces; and
- run a final architecture review against this plan.

Exit criteria:

- no Bark module references `cdk_onchain_application`, the federated BDK
  payment adapter, or BDK wallet state;
- no new configuration can produce the legacy layout;
- final non-test production code is at or below the baseline, or a reviewed
  exception identifies unavoidable safety code and the larger duplicate path
  that was removed;
- only one Bark recovery driver and one replay implementation remain; and
- the implementation contract describes the final architecture rather than
  the migration state.

## Verification matrix

### Compile and dependency gates

- format and strict Clippy for every changed crate;
- minimal BDK-only mintd build;
- minimal Bark-only mintd build;
- combined BDK/Bark mintd build;
- workspace checks with `federation-bark` disabled;
- SQLite and PostgreSQL feature combinations; and
- dependency-tree assertions proving the intended optional edges.

### Deterministic unit and integration tests

- application-ID and derivation-domain separation;
- independent configuration encoding and validation;
- exact CPFP package validation and mutation rejection;
- insufficient fee-reserve handling;
- actor and signer attempt replacement;
- duplicate proposal, signature, broadcast, and observation handling;
- receive-claim replay from a clean database;
- exit replay at every accepted phase;
- two-clean-database materialized-state digest equality;
- checkpoint restore during Bark recovery; and
- no Bark side effects during deterministic apply or replay.

### Regtest acceptance

Run each topology from fresh state and after restart:

1. BDK only: on-chain mint and melt;
2. Bark only: Lightning mint and melt;
3. Bark only: refresh, forced emergency exit, CPFP, drain, and confirmation;
4. combined: concurrent BDK and Bark operations with distinct balances;
5. one member offline while quorum continues;
6. actor failure before and after every irreversible Bark step; and
7. rolling restart followed by zero-lag convergence.

### Live Signet acceptance

After local gates pass and the operator explicitly authorizes spending Signet
funds:

- deploy one member at a time while retaining consensus quorum;
- require identical public-config and binary hashes;
- complete one BDK on-chain mint/melt with Bark disabled at runtime;
- complete one Bark Lightning mint/melt with BDK payments disabled at runtime;
- complete or safely simulate the bounded Bark recovery path as permitted by
  the test-fund budget;
- require all members ready, converged, and free of unresolved recovery alerts;
  and
- verify CPU, memory, database growth, and operation latency remain within the
  recorded baseline.

## Rollout and rollback

1. Land regression fixtures before behavior changes.
2. Land configuration/feature separation with legacy decoding but no live
   activation.
3. Land the single recovery driver and CPFP implementation.
4. Pass the complete local verification matrix.
5. Produce a redacted migration preflight and independently verify backups.
6. Upgrade the live federation only through a reviewed config/application
   transition or replace it with a fresh federation.
7. Observe a full Signet test cycle and rolling restart.
8. Remove the legacy adapter in the next immediate change; do not leave it as a
   permanent compatibility mode.

Rollback must restore binaries and member-local execution caches without
rewriting accepted journal history. A public-config/application transition is
rolled back only by its defined consensus mechanism; it is never reverted by
editing files independently on members.

## Acceptance criteria

The plan is complete only when all of the following are true:

- federated BDK and Bark can each compile and run without the other payment
  backend;
- Bark has a separately scoped threshold recovery application;
- BDK and Bark never share wallet balances, UTXO reservations, addresses, or
  signing sessions;
- no Bark-specific code exists in `cdk-common` or the general consensus engine;
- CPFP uses existing exit phases in the ordinary path and is consensus-safe,
  restart-safe, and response-loss-safe;
- accepted receive-claim state replays from clean local storage;
- one Bark recovery driver serves startup replay, automatic maintenance, and
  operator retry;
- `cdk-axum` no longer owns Bark maintenance state transitions;
- BDK-only, Bark-only, and combined regtest suites pass;
- the live deployment is migrated explicitly or replaced, never silently
  mutated;
- temporary legacy compatibility is deleted; and
- the final complexity accounting demonstrates fewer dependency edges,
  duplicate paths, and owners, with non-test production code at or below the
  recorded baseline unless a reviewed safety exception is documented.

## Implementation ledger

Implementation snapshot (2026-08-16):

- The pre-change baseline is the current branch `HEAD`. Across the touched
  crates it contains 235,696 production-like Rust source lines; the working
  tree contains 240,106. This conservative count excludes dedicated test and
  benchmark paths but includes inline `#[cfg(test)]` modules. The raw increase
  of 4,410 lines is chiefly the consensus-validated Bark recovery/CPFP path,
  actor failover, and its tests; the shared Bitcoin wallet implementation was
  moved rather than duplicated. This does not yet satisfy the plan's optional
  at-or-below-baseline target and requires review as a safety-code exception or
  a later simplification pass.
- The backend dependency edges are removed: a Bark-only mintd graph cannot
  resolve the `cdk-bdk` package, and a BDK-only graph cannot resolve
  `cdk-federation-bark`. Bark uses the neutral `cdk-bitcoin-wallet` primitives
  directly for its independently scoped recovery application.
- Synthetic federation smoke, setup, DKG, configuration, replay, exact CPFP,
  mutation-rejection, actor/signer failover, and backend unit suites pass. The
  repository does not currently provide a fresh five-member federated Bark
  regtest topology; its deployment helper is Signet-only. Therefore the full
  regtest and live Signet acceptance sections remain open.
- No live configuration, host, database, or deployment was changed by this
  implementation pass.

| Milestone | Status | Evidence | Commit |
|---|---|---|---|
| 0. Evidence and baseline | Complete | Sanitized deterministic replay/recovery fixtures; source and dependency baseline above; no deployment inventory added | Uncommitted |
| 1. Configuration contract | Complete | Optional BDK application; distinct Bark custody and recovery registrations; round-trip and separation tests | Uncommitted |
| 2. Features and bootstrap | Complete | BDK-only, Bark-only, combined, and wallet-free feature checks; negative dependency-tree assertions | Uncommitted |
| 3. Single replay/recovery driver | Complete | `cdk-bark` owns replay and recovery selection; startup, automatic, and operator paths share it | Uncommitted |
| 4. Bark-owned CPFP recovery | Code complete; topology acceptance pending | Exact accepted packages, Bark-only recovery wallet/reservations, FROST authorization, idempotent publication, actor/signer retry tests | Uncommitted |
| 5. Axum ownership removal | Complete | No production Axum match on Bark refresh/exit phases; mintd owns worker lifecycle | Uncommitted |
| 6. Migration | Not started | Requires an explicit reviewed live rollout and Signet authorization | |
| 7. Legacy deletion and complexity closeout | In progress | BDK-backed Bark path and dependency edges removed; raw source count is above baseline and full acceptance remains open | Uncommitted |
