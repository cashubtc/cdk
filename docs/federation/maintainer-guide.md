# Federation guide for CDK maintainers

Read this alongside the [federation overview](../federation.md). The federation
extends the existing mint, database, payment and wallet interfaces. It adds
agreement about state changes and threshold custody; the ordinary Cashu
operations still own their validation and transactional effects.

## Concept map

| Familiar CDK concept | Federation integration | Where to start reviewing |
| --- | --- | --- |
| `MintBuilder` and mint startup | Explicit federation configuration and keyset selection; ordinary defaults stay ordinary | [Builder](../../crates/cdk/src/mint/builder.rs), [daemon configuration](../../crates/cdk-mintd/src/federation_configuration.rs) |
| Axum `MintState` and routes | `cdk-axum` holds routes and request authentication; the node is built by `FederationNode::prepare` | [Entrypoint](../../crates/cdk-axum/src/lib.rs), [node construction](../../crates/cdk-federation/src/construction.rs), [bootstrap](../../crates/cdk-federation/src/bootstrap.rs), [runtime](../../crates/cdk-federation/src/node_runtime.rs) |
| `MintDatabase` and `MintTransaction` | Federation traits participate in the same database transaction composition | [Traits](../../crates/cdk-common/src/database/mint/federation.rs), [SQL application](../../crates/cdk-sql-common/src/mint/federation/application.rs) |
| Quote, proof and signature records | The agreed operation is applied against the member's current transactional state | [Application](../../crates/cdk/src/mint/federation/application.rs), [admission](../../crates/cdk/src/mint/federation/configuration_and_admission.rs) |
| `MintPayment` and melt saga | Accepted plans and durable action identities authorize external work; evidence determines completion | [Payment observation](../../crates/cdk-federation/src/payment_observation.rs), [Bark runtime](../../crates/cdk-bark/src/runtime.rs) |
| `MintConnector` | `FederatedMintConnector` validates member responses and returns an ordinary aggregate Cashu response | [Wallet connector](../../crates/cdk/src/wallet/federation.rs), [registry](../../crates/cdk/src/wallet/federation/registry.rs) |
| Saga restart and reconciliation | Journal replay, checkpoint evidence and custody reconciliation restore readiness before new actions | [Startup](../../crates/cdk-federation/src/bootstrap.rs), [catch-up](../../crates/cdk-federation/src/checkpoint_catch_up.rs) |

`cashu` has one curve-aware `PublicKey`/`SecretKey` and always compiles
BLS12-381 v3 keysets; no feature changes a public type. Secp256k1-only
operations use fallible `try_*` methods. The
[isolated downstream check](../../scripts/check_standalone_api.py) builds an
ordinary consumer without workspace feature unification and checks that the
federation custody crates stay out of its dependency tree.

## Incoming payment and mint issuance

1. Start with quote creation in the [wallet connector](../../crates/cdk/src/wallet/federation.rs).
   Member ordering, timeout and fallback are shared mechanics. Mint-specific
   response validation and threshold visibility remain explicit. Once a valid
   quote has been created, a visibility failure does not authorize creating
   another quote.
2. Follow admission and ordering in the [operation pipeline](../../crates/cdk-federation/src/operation_pipeline.rs),
   then `apply_accepted_federation_operation` in
   [mint application](../../crates/cdk/src/mint/federation/application.rs).
   Agreement about an operation does not bypass its independent application
   checks.
3. Payment observers submit evidence through the federation path. For joint
   on-chain deposits, inspect [chain observations](../../crates/cdk/src/mint/federation/chain_observation.rs).
   For Bark-backed Lightning, follow [incoming orchestration](../../crates/cdk-bark/src/node/incoming.rs),
   including protected receive secrets and certified claim evidence.
4. Issuance consumes the paid quote state and persists member signature
   shares. The wallet verifies share metadata, member identity, keyset, amount
   and blinded output before selecting a compatible threshold and aggregating.

**Authority:** accepted operations and their transactional quote and signature
records determine paid and issued amounts. An HTTP response, local wallet scan
or worker wake does not establish payment authority.

**Transactions and external effects:** mint application commits quote and
signature effects through the existing database transaction interfaces. Bark
preparation, claim execution and chain access are external work tied to
accepted action identities. A failed external call cannot become quote credit.

**Recovery:** durable quote state, accepted Bark records, protected secret
storage and persisted shares support restart and duplicate requests. Workers
reconcile and observe accepted work; they do not infer fresh authorization from
a retry timer.

## Swap and ecash transfer

1. The wallet sends an ordinary swap through `FederatedMintConnector`. See
   [wallet collection](../../crates/cdk/src/wallet/federation.rs) for response
   validation and threshold completion.
2. The [operation pipeline](../../crates/cdk-federation/src/operation_pipeline.rs)
   admits and orders the authorized envelope. The
   [mint application](../../crates/cdk/src/mint/federation/application.rs)
   applies the swap against current proof states.
3. Proof validation and overlapping-input checks remain transactional. A losing
   conflicting operation is recorded as rejected, and replay continues beyond
   it. Do not move proof reservation ahead of independent proof verification.
4. Each member persists its resulting shares. The connector returns a verified
   aggregate to the ordinary wallet receive and send flow. Request-local reuse
   of verified shares is bound to the original configuration, members, outputs,
   response content and validation mode.

**Authority:** spent-proof state and persisted operation results establish
whether value moved. A threshold response must still contain a compatible set
of cryptographically valid shares.

**Transactions and external effects:** consuming input proofs and producing
signature results follow the mint transaction boundary. A swap does not start a
Lightning or Bitcoin payment.

**Recovery:** retries retrieve the durable operation result or restore
signatures for the persisted blinded outputs. Conflicting or insufficient
member responses do not authorize releasing uncertain proofs.
[Swap tests](../../crates/cdk/src/mint/federation/tests/swap.rs) and the
[wallet adversarial tests](../../crates/cdk/src/wallet/federation/tests.rs)
cover the two sides of this boundary.

## Outgoing payment and change

1. An outgoing quote and melt enter through the ordinary wallet API, with
   federation agreement and transactional application layered onto the
   existing melt workflow. See
   [application and payment probing](../../crates/cdk/src/mint/federation/application.rs).
2. The accepted action selects the external worker and binds its payment
   identity. For on-chain custody, follow
   [transaction state](../../crates/cdk/src/mint/federation/transaction_state.rs),
   [FROST signing](../../crates/cdk-federation/src/frost_signing.rs) and
   [broadcast observation](../../crates/cdk-federation/src/broadcast.rs).
   Transaction construction and independent signer validation remain distinct.
3. For Bark-backed Lightning, follow
   [outgoing orchestration](../../crates/cdk-bark/src/node/outgoing.rs).
   Selected observers inspect the exact accepted payment before initiating it.
   Only the typed missing outcome permits initiation; pending, failed
   observation or timeout is not missing.
4. Certified terminal evidence advances the accepted state. Signature
   collection verifies actual change, including a valid prefix of requested
   blank change outputs. A restarted connector without the original blinded
   outputs omits unverified change and lets wallet restore recover it from
   persisted outputs.

**Authority:** accepted plans, single-use nonce state, independent signing
checks and verified terminal evidence control custody and settlement. Local
task ownership and retry settings provide no payment authority.

**Transactions and external effects:** state changes are persisted before
external initiation, signing or broadcast. Network calls do not belong inside
database or consensus critical sections.

**Recovery:** uncertain outcomes remain pending. Restart inspects the same
durable action, transaction or payment identity. Recovery must not create a new
payment because an earlier worker stopped.

## Startup, failure, restart and catch-up

1. [Bootstrap](../../crates/cdk-federation/src/bootstrap.rs) verifies stored
   checkpoint evidence, reconstructs consensus and sequencing, and reconciles
   local custody. A complete member context groups the required handles; setup
   and active membership are distinct.
2. [Node construction](../../crates/cdk-federation/src/construction.rs) returns
   prepared services without starting them.
   [Federation services](../../crates/cdk-federation/src/services.rs) start the
   workers under the runtime-owned task group. BDK chain sync starts here too
   and shares cancellation and the shutdown grace period. Setup does not start
   payment workers. An unavailable wallet or a member awaiting quorum is a
   readiness condition.
3. Essential-worker failure cancels sibling work and propagates to the serving
   owner. Shutdown stops scheduling, waits for tracked work, then aborts and
   joins work that exceeds the grace period. AlephBFT shutdown is part of the
   service lifetime.
4. [Checkpoint catch-up](../../crates/cdk-federation/src/checkpoint_catch_up.rs)
   validates evidence and replays the required suffix;
   [retention](../../crates/cdk-federation/src/retention.rs) installs a state
   transfer when peers have pruned it.
   [Runtime components](../../crates/cdk-federation/src/runtime_components.rs)
   document lock order and separate readiness, application, recovery and
   wallet ownership.
5. A member resumes authoritative work only when its evidence and state meet
   the readiness conditions. A materialized-state audit mismatch stays latched.

**Authority:** verified checkpoint lineage, ordered operations and
materialized-state agreement establish the recovered state. Candidate creation
alone is not a trusted checkpoint.

**Transactions and external effects:** replay uses the application transaction
interfaces. Custody reconciliation observes and materializes already accepted
plans; it preserves durable external identities and nonce safety.

**Recovery owner:** the member service owns worker lifetime; mint application
owns database effects; the custody runtime owns external reconciliation. No
payment observer or generic restart loop owns all three.

## Rules for federation changes

- **Standalone stays unchanged.** Shared code changes only when federation
  needs it, with standalone behaviour covered by tests; no federation
  conditionals in unrelated standalone paths.
- **Consensus before value.** No share, spend or quote state change before the
  exact operation is accepted. Every share traces to an accepted operation and
  output.
- **Deterministic application.** No backend calls, local time, randomness or
  network I/O in apply; nondeterministic inputs enter as accepted operations.
  Evidence must be accepted at or before the operation it authorizes.
- **Idempotent recovery.** Retries, duplicates, replay, catch-up and restart
  converge on the original durable result without a second signature, spend,
  payment attempt or broadcast.
- **Explicit crash boundaries.** Domain effects and journal status commit
  together; external effects need deterministic actor selection, a durable
  claim and a consensus-controlled finalization.
- **Fail closed** on conflicting keysets, config digests, journal order,
  evidence, identities, signatures, state digests and readiness.
- **Bounded work.** Queues, caches, retries, waiters, payloads, pages and
  diagnostics have explicit limits; reads are keyed or ranged, not proportional
  to history (see [storage and retention](storage-and-retention.md#readers-and-retained-history)).
- **Typed errors with fixed reasons.** Federation errors carry fixed text plus
  public protocol values; foreign error text is redacted where it is logged or
  replicated.
- **Cut-through.** Federation formats change in place with regenerated golden
  vectors; no compatibility shims for undeployed formats.
- **No secrets in logs, fixtures or commits**: identity keys, key shares, DKG
  private messages, credentials, payment secrets and private configs.

## Review checklist

- Does the change preserve standalone behaviour and non-federation builds?
- Can any path create or expose a signature share without accepted operation
  provenance?
- Does every operation keep its canonical encoding, stable ID, complete value
  commitment, bounded size and conflict handling?
- Is application free of backend calls, local time, randomness and other
  uncommitted nondeterminism?
- Are payment observations temporally authorized, quorum-checked and replay
  safe, including after their validity window?
- Can retries, cancellation, restart, catch-up, state transfer or rotation
  duplicate value, lose pending work or serve stale state?
- Does deletion below the retention floor leave every digest and every later
  decision unchanged?
- Do wallet responses require valid threshold agreement, including matching
  definitive errors?
- Do stateful changes test replay and restart, and distributed changes test
  adverse ordering and partial failure?
- Are queues, caches, retries, waiters, payloads and diagnostics bounded?

## Tests

Keep transport admission, mint application, SQL rollback, cryptographic
verification, network disagreement and process restart scenarios distinct;
they exercise different trust boundaries even when they share fixtures.
Fixture consolidation should make each scenario's policy and injected failures
more visible. Record retained coverage before deleting a test.

Notable suites:

- `cdk` `mint::federation::tests::parity`: standalone and federated swap, mint
  and melt agree on every compared dimension.
- `cdk` `mint::federation::tests::replay_window`: replays of every operation
  kind after its validity window.
- `cdk-axum` `federation::tests::retention` and `state_transfer`: pruning,
  state transfer, terminal entities, stale-member halt and reset.
- `cdk-common` storage conformance (SQLite and PostgreSQL), including
  `federation_hot_reads_stay_bounded_as_history_grows`.
- `cdk-common` `tests/federation_setup_transcript_vectors.rs`: setup wire
  vectors.

Federation and FROST decoders have a fuzz corpus: `just fuzz-federation-security
<seconds> <jobs>` runs it locally; normal CI exercises the decoder seeds, and
the longer campaign is the manually dispatched `federation-security-fuzz`
workflow.

## Supported feature combinations

`flake.nix` lists the supported crate and feature combinations as four
clippy-and-test groups, which the CI `clippy-and-test` matrix runs on pushes,
releases and manual runs (pull requests run a smaller subset plus workspace
clippy and unit tests):

- `federation-domain`: `cdk-common` with `federation,test`, the Bark domain
  crates, the SQL storage crates with `federation-bark`, `cdk` with and without
  Bark, and the `cdk` wallet with Iroh;
- `federation-custody`: `cdk-frost`, `cdk-frost-bip32`, `cdk-frost-bark` and
  `cdk-bitcoin-wallet`;
- `federation-runtime`: `cdk-federation` with all features, and `cdk-axum` with
  `federation`, `federation-bdk`, `federation-bark`, and all three with `iroh`;
- `federation-binaries`: `cdk-mintd` in its production and test feature sets,
  `cdk-cli` with `federation,iroh`, `cdk-ffi` with and without `federation`, and
  `cdk-fed-tui`.

These groups run with a 16 MiB test-thread stack (`RUST_MIN_STACK`). Two
feature sets are deliberately outside the matrix: `cdk-postgres`'s federation
features (they gate the PostgreSQL federation store, whose tests need a
running server) and `cdk-axum` with `prometheus` alone (covered through the
production `cdk-mintd` entry). The wallet front ends enable
`cdk-sqlite/federation`, because `cdk-common/federation` enables the mint store,
which must then implement the federation traits.

Local fake-wallet federations come from
`just` recipes that run `cdk-mintd federation test-trusted-dealer` and start
members through `scripts/start_federation_test_member.py`, which imports their
settings through the real configuration commands.
`scripts/run_federation_setup_matrix.sh` (with `run_test_federation_dkg.sh`)
runs the multi-operator setup ceremony at several sizes, and
`scripts/run_iroh_federation_e2e.sh` runs a federation over Iroh.

Bark is pinned to an exact reviewed Git revision of `callebtc/bark` in
`crates/cdk-bark/Cargo.toml`; Cargo fetches it, and no sibling checkout is
needed.
