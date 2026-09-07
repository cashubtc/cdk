# Federation walkthroughs for CDK maintainers

Read this alongside the [federation guide](federation.md). The federation feature extends the existing mint, database, payment, and wallet interfaces. It adds agreement about state changes and threshold custody; the ordinary Cashu operations still own their validation and transactional effects.

## Concept map

| Familiar CDK concept | Federation integration | Where to start reviewing |
| --- | --- | --- |
| `MintBuilder` and mint startup | Explicit federation configuration/keyset selection; ordinary defaults remain ordinary | [Builder](../crates/cdk/src/mint/builder.rs), [daemon configuration](../crates/cdk-mintd/src/federation_configuration.rs) |
| Axum `MintState` and routes | Federation modules own admission, bootstrap, synchronization, and member services | [Entrypoint](../crates/cdk-axum/src/lib.rs), [bootstrap](../crates/cdk-axum/src/federation/bootstrap.rs), [runtime](../crates/cdk-axum/src/federation/node_runtime.rs) |
| `MintDatabase` and `MintTransaction` | Federation traits participate in the same database transaction composition | [Traits](../crates/cdk-common/src/database/mint/federation.rs), [SQL application](../crates/cdk-sql-common/src/mint/federation/application.rs) |
| Quote, proof, and signature records | The agreed operation is applied against the member's current transactional state | [Application](../crates/cdk/src/mint/federation/application.rs), [admission](../crates/cdk/src/mint/federation/configuration_and_admission.rs) |
| `MintPayment` and melt saga | Accepted plans and durable action identities authorize external work; evidence determines completion | [Payment observation](../crates/cdk-axum/src/federation/payment_observation.rs), [Bark runtime](../crates/cdk-bark/src/runtime.rs) |
| `MintConnector` | `FederatedMintConnector` validates member responses and returns an ordinary aggregate Cashu response | [Wallet connector](../crates/cdk/src/wallet/federation.rs) |
| Saga restart and reconciliation | Journal replay, checkpoint evidence, and custody reconciliation restore readiness before new actions | [Startup](../crates/cdk-axum/src/federation/bootstrap.rs), [catch-up](../crates/cdk-axum/src/federation/checkpoint_catch_up.rs) |

The ordinary key implementation and its original methods remain available when `cashu/federation` is disabled. Federation selects curve-aware key implementations. Shared code uses fallible `try_*` operations at that boundary. The [isolated downstream check](../scripts/check_standalone_api.py) exercises ordinary APIs and serialization without workspace feature unification.

## Incoming payment and mint issuance

1. Start with quote creation in the [wallet connector](../crates/cdk/src/wallet/federation.rs). Member ordering, timeout, and fallback are shared mechanics. Mint-specific response validation and threshold visibility remain explicit. Once a valid quote has been created, a visibility failure does not authorize creating another quote.
2. Follow admission and ordering in the [operation pipeline](../crates/cdk-axum/src/federation/operation_pipeline.rs), then `apply_accepted_federation_operation` in [mint application](../crates/cdk/src/mint/federation/application.rs). Agreement about an operation does not bypass its independent application checks.
3. Payment observers submit evidence through the federation path. For joint on-chain deposits, inspect [chain observations](../crates/cdk/src/mint/federation/chain_observation.rs). For Bark-backed Lightning, follow [incoming orchestration](../crates/cdk-axum/src/federation/bark_incoming.rs), including protected receive secrets and certified claim evidence.
4. Issuance consumes the paid quote state and persists member signature shares. The wallet verifies share metadata, member identity, keyset, amount, and blinded output before selecting a compatible threshold and aggregating.

**Authority:** accepted operations and their transactional quote/signature records determine paid and issued amounts. An HTTP response, local wallet scan, or worker wake does not establish payment authority.

**Transactions and external effects:** mint application commits quote/signature effects through the existing database transaction interfaces. Bark preparation, claim execution, and chain access are external work tied to accepted action identities. A failed external call cannot be converted into successful quote credit.

**Recovery:** durable quote state, accepted Bark records, protected secret storage, and persisted shares support restart and duplicate requests. Workers reconcile and observe accepted work; they do not infer fresh authorization from a retry timer.

## Swap and ecash transfer

1. The wallet sends an ordinary swap through `FederatedMintConnector`. See [wallet collection](../crates/cdk/src/wallet/federation.rs) for response validation and threshold completion.
2. The [operation pipeline](../crates/cdk-axum/src/federation/operation_pipeline.rs) admits and orders the authorized envelope. The [mint application](../crates/cdk/src/mint/federation/application.rs) applies the swap against current proof states.
3. Proof validation and overlapping-input checks remain transactional. A losing conflicting operation is recorded as rejected; replay must continue beyond it. Do not move proof reservation ahead of independent proof verification.
4. Each member persists its resulting shares. The connector returns a verified aggregate to the ordinary wallet receive/send flow. Request-local verified-share reuse is bound to the original configuration, members, outputs, response content, and validation mode.

**Authority:** spent-proof state and persisted operation results establish whether value moved. A threshold response must still contain a compatible set of cryptographically valid shares.

**Transactions and external effects:** consuming input proofs and producing signature results follow the mint transaction boundary. A swap does not initiate a Lightning or Bitcoin payment.

**Recovery:** retries retrieve the durable operation result or restore signatures for the persisted blinded outputs. Conflicting or insufficient member responses do not authorize releasing uncertain proofs. Existing [swap tests](../crates/cdk/src/mint/federation/tests/swap.rs) and [wallet adversarial tests](../crates/cdk/src/wallet/federation/tests.rs) cover separate sides of this boundary.

## Outgoing payment and change

1. An outgoing quote and melt enter through the ordinary wallet API, with federation agreement and transactional application layered onto the existing melt workflow. See [application and payment probing](../crates/cdk/src/mint/federation/application.rs).
2. The accepted action selects the external worker and binds its payment identity. For on-chain custody, follow [transaction state](../crates/cdk/src/mint/federation/transaction_state.rs), [FROST signing](../crates/cdk-axum/src/federation/frost_signing.rs), and [broadcast observation](../crates/cdk-axum/src/federation/broadcast.rs). Transaction construction and independent signer validation remain distinct.
3. For Bark-backed Lightning, follow [outgoing orchestration](../crates/cdk-axum/src/federation/bark_outgoing.rs). Selected observers inspect the exact accepted payment before initiating it. Only the typed missing outcome permits initiation; pending, failed observation, or timeout is not missing.
4. Certified terminal evidence advances the accepted state. Signature collection verifies actual change, including a valid prefix of requested blank change outputs. A restarted connector without the original blinded outputs omits unverified change and lets wallet restore recover it from persisted outputs.

**Authority:** accepted plans, single-use nonce state, independent signing checks, and verified terminal evidence control custody and settlement. Local task ownership and retry settings provide no payment authority.

**Transactions and external effects:** state changes are persisted before external initiation/signing/broadcast. Network calls do not belong inside database or consensus critical sections. Focused Bark action reads support certificate lookup without reading unrelated history; validation and recovery keep full snapshots when needed.

**Recovery:** uncertain outcomes remain pending. Restart inspects the same durable action, transaction, or payment identity. Recovery must not create a new payment merely because an earlier worker stopped.

## Startup, failure, restart, and catch-up

1. [Bootstrap](../crates/cdk-axum/src/federation/bootstrap.rs) verifies stored checkpoint evidence, reconstructs consensus/sequencing, and reconciles local custody. A complete member context groups the required handles; setup and active membership are distinct.
2. Shared entrypoints attach routes and return prepared services directly. [Federation services](../crates/cdk-axum/src/federation/services.rs) start workers under the runtime-owned task group. BDK chain sync starts here too, after construction, and shares cancellation and the shutdown grace period. Setup does not start payment workers. An unavailable wallet or a member awaiting quorum is a readiness condition, not an incomplete required-handle combination.
3. Essential-worker failure cancels sibling work and propagates to serving ownership. Shutdown stops scheduling, waits for tracked work, and then aborts and joins work exceeding the bounded grace period. AlephBFT shutdown remains part of the service lifetime.
4. [Checkpoint catch-up](../crates/cdk-axum/src/federation/checkpoint_catch_up.rs) validates evidence and replays the required suffix. [Runtime components](../crates/cdk-axum/src/federation/runtime_components.rs) document lock order and separate readiness, application, recovery, and wallet ownership.
5. A member resumes authoritative work only when its evidence and state meet the readiness conditions. A materialized-state audit mismatch stays latched; changing a message or restarting a task does not clear it.

**Authority:** verified checkpoint lineage, ordered operations, and materialized-state agreement establish the recovered state. Candidate creation alone is not a trusted checkpoint.

**Transactions and external effects:** replay uses the application transaction interfaces. Custody reconciliation observes and materializes already accepted plans; it must preserve durable external identities and nonce safety.

**Recovery owner:** the member service/runtime owns worker lifetime; mint application owns database effects; the custody runtime owns external reconciliation. Neither a payment observer nor a generic task restart loop owns all three decisions.

## Reviewing tests and changes

Keep transport admission, mint application, SQL rollback, cryptographic verification, network disagreement, and process restart scenarios distinct. They exercise different trust boundaries even when they use similar fixtures. Fixture consolidation should make the scenario's policy and injected failures more visible. Record retained coverage before deleting a test.

The [implementation ledger](bls-federation-maintainability-plan-2026-09-07.md) tracks commands, measurements, limitations, and final payment verification. A passing unit suite does not establish local payment or value-conservation results.
