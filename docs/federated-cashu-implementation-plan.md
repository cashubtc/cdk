# Federated Cashu with CDK: implementation plan

This is a working checklist for adding federated Cashu mint support to CDK while keeping the public Cashu wallet-to-mint protocol as close as possible to today's NUT endpoints. The plan assumes BLS blind signatures are the federation signing scheme, with locally generated key shares first and real DKG later.

## Grand vision

The grand vision of this project is to evolve Cashu from a single-operator Chaumian eCash mint into a federated, threshold-secured eCash system where multiple mint members can run on independent servers, coordinate all state transitions through consensus, and jointly issue, swap, and melt Cashu tokens using BLS threshold blind signatures. Federation mode should preserve the existing Cashu wallet-to-mint protocol as much as possible while giving users stronger security and fault tolerance: no single operator should be able to inflate, censor, or unilaterally corrupt mint state, and the federation should continue operating when some members are offline as long as the signing and consensus thresholds are met. Every safety-critical mint operation must be serialized by the federation, stored in a shared consensus journal, replayable after outages, and applied deterministically so all honest members converge on the same database state. Wallets must be able to talk to multiple members, verify and aggregate threshold signature shares into ordinary Cashu proofs, and reject inconsistent federation metadata or bad shares. The system should start with static trusted-dealer key generation, fake-wallet payment backends, private REST networking, and locally testable consensus scaffolding, then grow toward durable storage, catch-up, member authentication, DKG, key rotation, and full mint/swap/melt integration. Federation is explicitly v3/BLS-only, standalone CDK behavior must remain unchanged, and the implementation should favor simple CDK-native designs while borrowing Fedimint's core safety lessons around consensus-before-signing, authenticated peers, deterministic state machines, and robust recovery.

## Scope

- Federate CDK mint issuance, swap, restore, check-state, and eventually melt flows.
- Let a federation-aware CDK wallet talk to multiple mint members and aggregate BLS blind signature shares into normal Cashu proofs.
- Serialize all safety-critical mint state transitions through consensus before any member returns a usable signature share.
- Keep existing standalone mint and wallet behavior unchanged unless federation mode is explicitly enabled.
- Start with fake wallet payment backends and trusted-dealer key shares, then add payment observation quorum and real DKG.

## Hard keyset constraint

BLS blind signatures are only supported for Cashu keyset v3, represented in CDK as `KeySetVersion::Version02` and serialized with keyset ID prefix `02`. Federation mode must therefore be v3/BLS-only.

- Federation configs must only create, import, advertise, and validate aggregate v3/BLS keysets.
- Federation startup must reject any aggregate keyset, member public share, or local secret share that is not tied to a v3/BLS keyset.
- Federated mint, swap, restore, and change-output signing paths must reject outputs targeting non-v3 keysets before consensus signing.
- Federation-aware wallets must reject member sets that advertise mismatched aggregate keysets or any non-v3 federation keyset.
- Standalone non-federated mints remain unaffected and may keep supporting whatever keyset versions CDK otherwise supports.

## Implementation philosophy

Fedimint should be treated as the main reference implementation for federation design questions, especially around consensus, DKG, peer authentication, catch-up, state-machine boundaries, and threshold signing safety. When there is ambiguity about which library, protocol pattern, or engineering method to use, inspect the Fedimint code and design first.

Cashu/CDK should still value simplicity over copying architecture wholesale. If a simpler design achieves the same safety property, preserves Cashu protocol compatibility, is easier to test, and fits CDK's existing structure better, prefer the simpler design. Copy invariants and lessons from Fedimint; do not blindly copy complexity.

The interface to existing CDK code should be as small and thin as practical. Prefer logically organized federation modules with well-separated concerns over scattering federation conditionals through existing mint, wallet, HTTP, storage, or signatory code. Existing code should expose narrow hooks or adapters where needed, and most federation-specific behavior should live behind explicit feature-gated abstractions so reviews can reason about ownership, invariants, and blast radius.

## Development workflow

Commit changes to Git regularly after each coherent section, milestone, or independently testable checkpoint. Each commit should leave the repository in a reconstructable state: code, migrations, tests, documentation, and test results should make it clear what changed and how to verify it. Prefer small conventional commits over large mixed changes so federation development can be audited, bisected, reverted, or resumed cleanly.

Never commit secrets, generated private member configs, private key shares, machine-specific paths, or personally identifiable local environment details. Test fixtures may include deterministic fake keys only when they are clearly marked as non-production material.

## Sources reviewed

CDK source areas:

- `crates/cdk/src/mint/issue/mod.rs`
- `crates/cdk/src/mint/swap/mod.rs`
- `crates/cdk/src/mint/swap/swap_saga/mod.rs`
- `crates/cdk/src/mint/melt/melt_saga/mod.rs`
- `crates/cdk/src/mint/verification.rs`
- `crates/cdk-common/src/database/mint/mod.rs`
- `crates/cdk-sql-common/src/mint/{proofs,quotes,signatures,saga}.rs`
- `crates/cdk-signatory/src/{signatory,db_signatory}.rs`
- `crates/cashu/src/dhke.rs`
- `crates/cashu/src/nuts/nut01/bls.rs`
- `crates/cdk/src/wallet/{issue,swap}/`
- `crates/cdk/src/wallet/mint_connector/`
- `crates/cdk-axum/src/{router_handlers,custom_handlers}.rs`

Fedimint source areas:

- [README](https://github.com/fedimint/fedimint)
- [fedimint-server-core/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server-core/src/lib.rs)
- [fedimint-core/src/transaction.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-core/src/transaction.rs)
- [fedimint-core/src/epoch.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-core/src/epoch.rs)
- [fedimint-server/src/consensus/engine.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/engine.rs)
- [fedimint-server/src/consensus/transaction.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/transaction.rs)
- [fedimint-server/src/consensus/aleph_bft/data_provider.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/consensus/aleph_bft/data_provider.rs)
- [modules/fedimint-mint-server/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/modules/fedimint-mint-server/src/lib.rs)
- [modules/fedimint-mint-common/src/lib.rs](https://github.com/fedimint/fedimint/blob/master/modules/fedimint-mint-common/src/lib.rs)
- [fedimint-server/src/config/dkg_g2.rs](https://github.com/fedimint/fedimint/blob/master/fedimint-server/src/config/dkg_g2.rs)

## Key observations

### CDK today

- `Mint::process_mint_request` validates quotes and outputs, calls `blind_sign`, then persists blinded messages, blind signatures, quote issuance, and completed operations in a database transaction.
- `Mint::process_swap_request` uses a saga: TX1 marks input proofs pending and stores blinded outputs, signing happens outside the DB transaction, then TX2 stores signatures and marks inputs spent.
- `MeltSaga` correctly treats external payments as non-transactional and uses write-ahead saga state before payment attempts. This is the right shape for a federated melt, but the state transitions need consensus.
- The mint database already has useful materialized state tables: `proof`, `blind_signature`, `mint_quote`, `melt_quote`, `saga_state`, and `completed_operations`.
- The wallet already depends on `MintConnector`, so federation can start as a new connector implementation without rewriting wallet business logic.
- The current BLS primitives can blind, sign, unblind, and verify BLS signatures, but CDK does not yet have threshold share generation, share verification, or interpolation.

### Fedimint patterns to copy

- Client value transfers are modeled as canonical transactions ordered by consensus. Module consensus items are reserved for facts needed to validate transactions, such as time, block height, fees, or external observations.
- The mint module spends input notes and creates output signature shares inside consensus transaction processing.
- Output outcomes contain blind signature shares and are read by clients only after the transaction is accepted.
- The consensus engine accepts an ordered stream of items, processes each item in a DB transaction, records accepted items, audits module balances, and checkpoints DB state.
- Fedimint uses AlephBFT for atomic broadcast and authenticated peer networking. We should use the same concept and can use the same `aleph-bft` crate, but CDK should not depend on Fedimint's full module stack.
- Fedimint DKG for G2 is a direct reference for later dealer-free setup. For CDK MVP, trusted-dealer key share generation is enough.

## Safety invariants

Every implementation phase must preserve these invariants:

- A member never returns a blind signature share unless the exact operation payload was accepted by consensus or was already accepted earlier.
- The operation hash commits to the complete request: quote IDs, input proofs, output blinded messages, amounts, keyset IDs, witnesses, NUT-20 signatures, payment method, and version.
- A quote cannot issue more aggregate output value than its consensus-tracked mintable amount.
- A proof can transition to spent exactly once in the consensus materialized state.
- A blinded output can be signed at most once per accepted operation and amount.
- Federation mode only advertises, accepts, and signs Cashu keyset v3 (`KeySetVersion::Version02`) aggregate BLS keysets.
- All honest members process the same ordered operation log into the same state digest.
- External payment status is never read directly inside deterministic consensus apply logic. It enters consensus as signed observations or designated payment events.
- Standalone CDK behavior remains available and covered by existing tests.

## Compatibility strategy

The public Cashu protocol should stay mostly unchanged:

- Existing routes remain: `/v1/mint/quote/{method}`, `/v1/mint/{method}`, `/v1/mint/{method}/batch`, `/v1/swap`, `/v1/melt/{method}`, `/v1/restore`, `/v1/checkstate`, `/v1/keys`, `/v1/keysets`, `/v1/info`.
- Federation-aware wallets use a new `FederatedMintConnector` that internally fans out requests to member URLs and aggregates BLS shares before returning normal `MintResponse` or `SwapResponse` to wallet sagas.
- Federation members may return `MintResponse` or `SwapResponse` whose `BlindSignature.c` is a BLS share, not a final signature. This is acceptable only when the caller is the federated connector and knows the member ID.
- Existing non-federated wallets should learn from mint info that a mint is federated and should refuse unless they support federation.
- Add an optional mint info extension, for example `nuts.federation` or a CDK-specific metadata field, containing federation ID, threshold, members, aggregate keysets, public key shares, consensus version, and operation-hash version.
- All members expose the same aggregate public Cashu v3/BLS keysets to clients. Member public key shares are extra federation metadata, not replacement NUT-01 keys.

## Architecture overview

Recommended new components:

- `cashu` threshold BLS module: key shares, public shares, share verification, interpolation, and aggregate public key derivation.
- `cdk-common` federation types: `FederationId`, `MemberId`, `ThresholdParams`, canonical operation types, operation hashes, accepted outcomes, state digests, and errors.
- `cdk-federation` crate: consensus engine, in-memory, private REST, and later NAT-traversing network transports, operation mempool, session outcomes, DB journal abstractions, and test harness.
- `cdk` mint federation mode: wraps mint operations with consensus submit/apply/await and uses a key-share signatory.
- `cdk` wallet federation connector: fans out requests, validates partial responses against public shares, aggregates threshold shares, and returns ordinary wallet responses.
- `cdk-mintd` federation config and commands: create local trusted-dealer configs, run member nodes, expose status, and start fake-wallet federation tests.

## Federation member identity model

Objective: ensure each mint only accepts federation traffic from configured federation partners.

Key material:

- Each member has a long-lived federation identity keypair used for mint-to-mint authentication.
- Each member also has a threshold BLS signing share for each federated Cashu v3 keyset.
- The federation has aggregate Cashu public keys derived from the DKG or trusted-dealer transcript.
- The member identity key is separate from the Cashu signing share. Network authentication must not reuse Cashu signing secret material.

Public roster:

- Every member stores the complete federation roster: federation ID, member IDs, partner public API URLs, private federation REST URLs, member identity public keys, BLS public shares, and aggregate Cashu keysets.
- Startup validation fails if the local member's private identity key does not match the configured member identity public key.
- Startup validation fails if the local BLS secret share does not match the configured BLS public share.
- Startup validation fails if the roster, aggregate keysets, thresholds, or DKG transcript hash differ from the configured federation.
- MVP membership is static. Unknown members are rejected rather than added dynamically.

Request authentication:

- Every private REST message is wrapped in a signed member envelope.
- The receiver verifies federation ID, sender member ID, receiver member ID, body hash, sequence or nonce, and signature against the sender's configured identity public key.
- Messages from unknown member IDs, unexpected public keys, wrong federation IDs, stale sequence numbers, or mismatched body hashes are rejected before they can enter consensus or mutate state.
- DKG and trusted-dealer setup output must bind member IDs to identity public keys and BLS public shares so a public share cannot be silently reassigned to another member.

## Core operation coverage matrix

Objective: make it explicit that federation support covers all three core Cashu eCash operations: minting, swapping, and melting.

Minting coverage:

- Covered by Milestone 8, Milestone 9, and Milestone 10.
- The wallet obtains a mint quote from one member, then submits the same quote ID and blinded outputs to all members through the federated connector.
- The federation serializes the exact mint request by operation hash before any member returns a signature share.
- Every member persists the same mint quote, payment observation, accepted output set, and deterministic signature shares.
- The wallet verifies member shares against public key shares, aggregates any valid threshold subset, unblinds the aggregate signatures, and stores ordinary Cashu proofs.
- Required happy-path tests: fake wallet marks the incoming invoice paid for all members, threshold shares aggregate, and the resulting proofs verify against the aggregate keyset.
- Required unhappy-path tests: unpaid quote, expired quote, amount mismatch, conflicting output set for the same quote, duplicate request replay, insufficient shares, invalid share, one slow member, one offline member that later catches up.

Swapping coverage:

- Covered by Milestone 6, Milestone 7, Milestone 11, and Milestone 12.
- The wallet sends the same input proofs and blinded outputs to all members.
- The federation serializes the exact swap request, verifies inputs once, moves inputs through pending or spent states consistently, and signs only the accepted output set.
- The operation hash commits to inputs, outputs, unit, fees, spending conditions, and signature flags so members cannot sign overlapping output windows.
- Required happy-path tests: valid inputs are spent, threshold output shares aggregate, and new proofs verify.
- Required unhappy-path tests: double spend, conflicting output set, insufficient input amount, fee mismatch, duplicate proof, pending proof conflict, P2PK or HTLC failure, `SIG_ALL` mismatch, and the sliding-window quorum-cover attack.

Melting coverage:

- Covered by Milestone 14, with earlier support from the deterministic state machine in Milestone 6 and the journal in Milestone 5.
- The wallet submits proofs and a melt quote. The federation serializes reserve, fake-wallet payment observation, finalization, and optional change signing.
- Inputs are not considered finally spent until the accepted melt state says the outgoing payment is final, while pending states remain durable and recoverable.
- With the fake wallet backend, all members can deterministically observe success for happy-path tests, and scripted failures or delays are used for unhappy-path tests.
- Required happy-path tests: fake wallet outgoing payment succeeds, all members finalize the melt, inputs become spent, and change outputs aggregate when present.
- Required unhappy-path tests: fake wallet failure, long pending payment, one member slow to observe, one member offline during payment, payment timeout, quote expiry, duplicate melt retry, crash after payment attempt, and rollback or retry according to the accepted state.

Restore and state-check coverage:

- Covered by Milestone 13.
- Restore and check-state are not value-changing operations, but they must read the federated database consistently and expose the aggregate keyset view expected by wallets.
- Required tests: restored signatures match deterministic shares or aggregate signatures, pending and spent states agree across members, and lagging members refuse to answer as authoritative until caught up.

## Incremental validation ladder

Objective: make the mint-side implementation testable in small, reversible steps before the whole federation exists.

Checkpoint 1: threshold BLS only.

- Implement trusted-dealer shares, share verification, interpolation, aggregate signatures, and wallet-side aggregation without changing mint business logic.
- Success criteria: 1-of-1 equals standalone BLS, 3-of-5 and 4-of-7 aggregate, invalid or duplicate shares fail, randomized property tests pass.

Checkpoint 2: static federation configuration.

- Generate local member configs, aggregate keysets, public shares, private shares, peer identities, and deterministic test fixtures.
- Success criteria: all generated members agree on federation ID and aggregate keyset IDs; each private share validates against public config; malformed configs fail before startup.

Checkpoint 3: operation hashing.

- Convert mint, swap, melt, payment observation, and restore request material into canonical operation encodings.
- Success criteria: semantically identical requests hash identically; reordered outputs or changed amounts hash differently; cross-language fixture vectors can be added later.

Checkpoint 4: networking smoke tests without consensus.

- Start N member processes with the planned private REST transport and exchange authenticated ping, status, and config-digest messages.
- Success criteria: members connect, reject unknown peers, report peer liveness, survive reconnects, and expose status without any mint operation enabled.

Checkpoint 5: consensus on a dummy log.

- Feed counter increments or no-op operations into the consensus engine, persist accepted sessions, and expose height and digest.
- Success criteria: all members apply the same ordered dummy operations, one member can restart and catch up, and state digest mismatch is detected.

Checkpoint 6: database journal and replay.

- Add the federation journal around an isolated test state machine before touching mint issue, swap, or melt.
- Success criteria: crash-before-apply, crash-after-apply, replay, checkpoint, and compaction tests pass across memory and SQLite.

Checkpoint 7: deterministic mint state-machine extraction.

- Refactor existing mint business logic so validation and state transitions can be run as deterministic apply functions.
- Success criteria: standalone mint behavior remains unchanged; federation mode can apply accepted operations without making HTTP responses or payment calls inside consensus apply.

Checkpoint 8: fake-wallet payment observations.

- Script fake incoming and outgoing payment outcomes as deterministic observations that can be proposed into consensus.
- Success criteria: success, failure, pending, delayed, duplicate, and conflicting observations are all serialized and produce the expected quote state.

Checkpoint 9: federated mint issuance.

- Enable mint quote creation, paid observation, accepted output set, share production, response caching, and wallet aggregation.
- Success criteria: one end-to-end fake-wallet mint succeeds; conflicting mint requests for the same quote fail; one offline member catches up and later serves the cached or regenerated share.

Checkpoint 10: federated swap.

- Enable proof verification, pending/spent transitions, accepted output set signing, and wallet aggregation for swap.
- Success criteria: basic swap succeeds; double spend fails; overlapping-window attack fails; state digests remain equal.

Checkpoint 11: federated melt.

- Enable melt reserve, fake outgoing payment observation, finalization, failure recovery, and optional change signing.
- Success criteria: successful melt finalizes across all members; failed or pending melt does not lose funds; retries are idempotent.

Checkpoint 12: network chaos and catch-up.

- Run the three core operations while members disconnect, restart, lag, and rejoin.
- Success criteria: caught-up members converge to the same digest, lagging members refuse signing until safe, and wallets can complete with any valid threshold subset.

## Fake wallet payment behavior matrix

Objective: prove payment handling is robust before connecting real Lightning backends.

Fake incoming payments for minting:

- Happy path: all members observe the mint quote as paid and the wallet receives aggregate signatures.
- One slow member: threshold members observe paid and sign; the slow member later records the same paid observation and catches up.
- One offline member: the operation finalizes with a threshold or consensus quorum; the offline member replays the accepted quote and payment observation before serving shares.
- Pending payment: members keep the quote pending, do not sign outputs, and later sign only after a paid observation reaches consensus.
- Failed payment: members mark the quote failed or unpaid according to the fake backend script and refuse issuance.
- Conflicting observation: one member proposes a different amount, unit, invoice, or quote state; consensus rejects or records the conflict without issuing signatures.
- Duplicate observation: repeated fake wallet notifications are idempotent and do not change the operation hash or issue twice.

Fake outgoing payments for melting:

- Happy path: all members accept the melt reserve, fake wallet reports success, inputs become spent, and change is signed.
- One slow member: threshold responses are enough for the wallet where safe, but the slow member catches up before serving authoritative state.
- One offline member: accepted melt state and payment observation are replayed on reconnect, and the member does not attempt a second outgoing fake payment.
- Long pending payment: inputs remain pending, no success response is returned as final, and retry or polling returns the same pending state.
- Failed payment: inputs are released or moved to the configured failed state consistently across all members, and no payment success is reported.
- Timeout then success: the final accepted observation determines state, and repeated wallet polling receives a consistent result.
- Crash after payment attempt: recovery checks accepted observations and idempotency keys before finalizing or retrying.

## Milestone 0: written RFC and feature gate

Objective: make the safety model explicit before code paths start diverging.

Implementation checklist:

- [ ] Add an RFC document under `docs/` describing the federated mint threat model.
- [ ] Define supported fault models: BFT consensus default, optional lower signing threshold, crash-only test mode.
- [ ] Decide the default threshold rule. Recommended: default to Fedimint-style `n - floor((n - 1) / 3)` for both consensus and signing until lower signing thresholds receive separate review.
- [ ] If supporting examples like 3-of-5 signing, define it as a separate `signature_threshold`, with consensus quorum still governed by the BFT engine.
- [ ] Add a `federation` Cargo feature that is off by default.
- [ ] Add `FederationMode::{Standalone, Member}` to mint setup without changing existing constructors initially.
- [ ] Document that signature shares must never be produced from pre-consensus local request state.

Tests and success criteria:

- [ ] Existing `cargo test --lib --workspace --exclude cdk-postgres` passes with the feature disabled.
- [ ] Enabling the feature compiles a stub crate or module with no behavior change.
- [ ] A review checklist exists for all future federation PRs: determinism, consensus-before-signing, operation hashing, state digest, attack tests.

## Milestone 1: threshold BLS blind signature primitives

Objective: add the cryptographic building blocks without touching mint state logic.

Implementation checklist:

- [ ] Add a separate file-backed module for threshold BLS under `crates/cashu/src/nuts/nut01/bls/` or an equivalent protocol-level location.
- [ ] Define `BlsSecretKeyShare`, `BlsPublicKeyShare`, `BlsAggregatePublicKey`, `BlsBlindSignatureShare`, and signer index types.
- [ ] Implement trusted-dealer polynomial key generation for BLS12-381 G2 public keys and G1 signature shares.
- [ ] Implement Lagrange interpolation over BLS12-381 scalar field for arbitrary member IDs.
- [ ] Implement blind signing with a secret share: `share = sk_i * blinded_message`.
- [ ] Implement share verification: `e(share, G2) == e(blinded_message, public_share_i)`.
- [ ] Implement aggregate blind signature construction from `t` valid shares.
- [ ] Verify aggregate blind signatures with existing `verify_bls_blind_signature`.
- [ ] Unblind aggregate signatures with existing BLS unblinding logic and verify final proofs with existing `verify_bls_message`.
- [ ] Add serialization for share types with explicit versioning and no secret key debug leakage.
- [ ] Add zeroization for secret shares where practical.

Tests and success criteria:

- [ ] Unit test 1-of-1 threshold produces the same final signature as the existing standalone BLS signer.
- [ ] Unit test 3-of-5 and 4-of-7 aggregate signatures verify under aggregate public keys.
- [ ] Unit test any valid threshold subset aggregates to the same final signature.
- [ ] Unit test insufficient shares cannot aggregate.
- [ ] Unit test duplicate signer IDs are rejected.
- [ ] Unit test invalid share fails verification before aggregation.
- [ ] Unit test share/order permutations produce identical aggregate signatures.
- [ ] Property test random messages, thresholds, and subsets.
- [ ] Existing NUT-00 BLS proof verification passes for aggregate proofs.

## Milestone 2: federation configuration and trusted-dealer setup

Objective: create deterministic federation configs and local key shares for test federations.

Implementation checklist:

- [ ] Add `FederationConfig` with federation ID, member set, consensus params, signature threshold, keyset metadata, supported units, and protocol versions.
- [ ] Add `MemberConfigPublic` with member ID, public API URL, private federation REST URL, P2P endpoint, federation identity public key, optional TLS/Noise/Iroh public key, and public BLS shares.
- [ ] Add `MemberConfigPrivate` with member ID, federation identity private key, P2P private key, local secret BLS shares, payment backend config, and DB path.
- [ ] Add `FederationRoster` mapping every member ID to its identity public key, endpoint set, and BLS public shares.
- [ ] Add `FederatedKeyset` with aggregate public keys per amount and public shares per member.
- [ ] Define deterministic member ordering and reject duplicate member IDs.
- [ ] Generate Cashu keyset IDs from aggregate public keys, not member share public keys.
- [ ] Force all generated federation keysets to Cashu keyset v3 (`KeySetVersion::Version02`).
- [ ] Add `cdk-mintd federation trusted-dealer` to generate N member config files for tests.
- [ ] Add `cdk-mintd federation inspect` to print non-secret config and keyset IDs.
- [ ] Add config validation to ensure each local identity private key matches the configured identity public key.
- [ ] Add config validation to ensure secret shares match declared public shares.
- [ ] Add config validation that every aggregate federation keyset is v3/BLS and reject v1/v2/non-BLS federation keysets.
- [ ] Add validation that all members agree on roster, identity public keys, aggregate keysets, thresholds, units, fees, and NUT settings.
- [ ] Reject startup if a member is missing any federation partner public key.

Tests and success criteria:

- [ ] Generated 5-member configs have identical federation ID and aggregate keyset IDs.
- [ ] Generated 5-member configs have identical roster hashes and complete partner identity public-key allowlists.
- [ ] Each member's private identity key verifies against its public identity key.
- [ ] Each member's private share verifies against its public share.
- [ ] A config with mismatched thresholds is rejected.
- [ ] A config with mismatched aggregate keyset is rejected.
- [ ] A config with any non-v3 federation aggregate keyset is rejected.
- [ ] A config with mismatched member identity public key is rejected.
- [ ] A config missing a federation partner public key is rejected.
- [ ] A config with duplicate member IDs is rejected.
- [ ] A config generated twice with the same deterministic test seed matches byte-for-byte except explicitly random fields.

## Milestone 3: canonical operation encoding and hashes

Objective: make every consensus operation uniquely and deterministically identified.

Implementation checklist:

- [ ] Define `FederatedMintOperation` enum with versioned variants.
- [ ] Start with variants: `CreateMintQuote`, `ObserveMintQuotePayment`, `IssueMint`, `Swap`, `RestoreRequest`, `MeltReserve`, `ObserveMeltPayment`, `FinalizeMelt`, `RollbackMelt`.
- [ ] Use a deterministic binary encoding for consensus data. Avoid raw JSON and unordered maps.
- [ ] Use `BTreeMap` and sorted vectors wherever ordering matters.
- [ ] Define `OperationHash = sha256("cdk-fed-op-v1" || encoded_operation)`.
- [ ] Define per-output hashes for blinded messages and per-input proof Y identifiers.
- [ ] Make operation IDs for federated consensus hash-based rather than UUIDv7-based.
- [ ] Keep wallet-local saga IDs as UUIDs if useful, but never use them as consensus identity.
- [ ] Define conflict keys per operation:
  - [ ] quote issuance: quote ID plus issued amount window.
  - [ ] swap: input proof Ys and output blinded messages.
  - [ ] melt reserve: quote ID plus input proof Ys.
  - [ ] restore: blinded messages only.
- [ ] Define a canonical `RequestTranscript` for NUT-20 and SIG_ALL verification.

Tests and success criteria:

- [ ] Encoding of a golden operation matches committed test vectors.
- [ ] Equivalent operations produce the same hash across processes.
- [ ] Reordered unordered fields still encode canonically.
- [ ] Changing one output blinded message changes the operation hash.
- [ ] Changing an input witness changes the operation hash.
- [ ] Fuzz decoding rejects unknown major versions and oversized payloads.

## Milestone 4: consensus engine abstraction

Objective: add an ordered log before integrating with mint business logic.

Implementation checklist:

- [ ] Create a `cdk-federation` consensus abstraction: `submit(item)`, `await_operation(hash)`, `accepted_stream()`, `state_digest()`.
- [ ] Add `ConsensusItem::{Operation(FederatedMintOperation), ModuleFact(...), SessionSignature(...), Default(...)}`.
- [ ] Add an in-memory single-node consensus engine for deterministic unit tests.
- [ ] Add an in-memory multi-node test engine that can reorder, duplicate, and delay messages.
- [ ] Add AlephBFT-backed engine for multi-member BFT ordering.
- [ ] Add a `DataProvider` equivalent that batches pending items, deduplicates operation hashes, and enforces max item size.
- [ ] Add a `FinalizationHandler` equivalent that applies ordered items to the DB transactionally.
- [ ] Add session outcomes signed by member identity keys.
- [ ] Store accepted items and signed session outcomes in the DB.
- [ ] Add operation outcome notification so public HTTP handlers can await acceptance.
- [ ] Add bounded mempool and admission checks to avoid DoS by large invalid operations.

Tests and success criteria:

- [ ] Single-node engine accepts operations in submit order.
- [ ] Multi-node in-memory engine converges on identical accepted sequences.
- [ ] Duplicate operation submissions are accepted once.
- [ ] Oversized consensus items are rejected before broadcast.
- [ ] Restarting a node mid-session replays accepted items without duplicating state changes.
- [ ] AlephBFT integration test finalizes sessions for 4 nodes and 5 nodes.
- [ ] Consensus halts or rejects when fewer than the required quorum is reachable.

## Milestone 5: federation DB journal and state digests

Objective: keep replicated operation history separate from materialized mint state.

Implementation checklist:

- [ ] Add DB traits for `FederationDatabase` and `FederationTransaction`.
- [ ] Add SQL migrations for SQLite and Postgres:
  - [ ] `federation_config`
  - [ ] `federation_session_outcome`
  - [ ] `federation_accepted_item`
  - [ ] `federation_operation`
  - [ ] `federation_operation_outcome`
  - [ ] `federation_signature_share`
  - [ ] `federation_payment_observation`
  - [ ] `federation_state_digest`
  - [ ] `federation_member_status`
- [ ] Keep existing mint tables as the materialized state used by existing APIs where possible.
- [ ] Add `signature_share` rows instead of overloading final `blind_signature` semantics for federated members.
- [ ] Add state digest calculation over consensus-visible tables only.
- [ ] Exclude local private config, local connection status, logs, and caches from the digest.
- [ ] Add an audit summary similar to Fedimint's balance audit after each accepted operation.
- [ ] Store DB checkpoints or lightweight digest history per session.

Tests and success criteria:

- [ ] Migrations apply cleanly on empty SQLite and Postgres databases.
- [ ] Migration tests cover rollback-compatible standalone mode.
- [ ] Applying the same operation log to two DBs yields identical state digests.
- [ ] Local-only fields do not change state digest.
- [ ] Negative balance audit causes a hard error in tests.
- [ ] Signature shares can be retrieved by operation hash and output index.

## Milestone 6: deterministic mint state machine extraction

Objective: refactor CDK mint operations so consensus can call deterministic apply functions.

Implementation checklist:

- [ ] Extract validation and apply logic from `process_mint_request` into `validate_issue_mint` and `apply_issue_mint`.
- [ ] Extract validation and apply logic from `SwapSaga::setup_swap` and `finalize` into `validate_swap` and `apply_swap`.
- [ ] Ensure apply functions receive all nondeterministic values as inputs: accepted time, operation hash, quote IDs, and payment observations.
- [ ] Remove calls to `unix_time()` from consensus apply paths except via consensus-provided accepted time.
- [ ] Remove UUID generation from consensus apply paths.
- [ ] Ensure apply paths do not spawn tasks or publish pubsub notifications until after commit.
- [ ] Keep existing standalone methods calling the refactored functions through a standalone adapter.
- [ ] Make `apply_issue_mint` update quote issuance, blinded message records, completed operation, and output signature-share request in one DB transaction.
- [ ] Make `apply_swap` verify inputs, verify spending conditions, check balance, mark inputs spent, record blinded outputs, record completed operation, and request signature shares in one DB transaction.
- [ ] Decide how to represent `Pending` in federated mode:
  - [ ] Swap should not use local pending state before consensus.
  - [ ] Melt reserve can use consensus-visible pending state between reserve and finalization.
- [ ] Add idempotency: if the exact same operation was already accepted, return the existing outcome.
- [ ] Add conflict detection: if a conflicting operation was accepted first, reject and record a conflict outcome where appropriate.

Tests and success criteria:

- [ ] Standalone mint tests still pass using the refactored code.
- [ ] Applying a valid issue operation twice is idempotent.
- [ ] Applying a same-quote different-output issue after the first issue fails.
- [ ] Applying a swap with already spent inputs fails.
- [ ] Applying a swap with duplicate outputs fails.
- [ ] Applying operations in the same order on multiple DB instances produces identical materialized state.
- [ ] Pubsub notifications fire only after commit in standalone and federation modes.

## Milestone 7: federated key-share signatory

Objective: let each member sign only with its own BLS share while advertising aggregate keys.

Implementation checklist:

- [ ] Add a `FederatedSignatory` or split the existing `Signatory` trait into final-signer and share-signer responsibilities.
- [ ] `keysets()` must return aggregate public keys and aggregate keyset IDs.
- [ ] `verify_proofs()` must verify proofs against aggregate public keys.
- [ ] `blind_sign_share()` must return member share signatures plus member ID.
- [ ] Add share verification against local public share before storing or returning a share.
- [ ] Prevent federated share signer from being used in standalone paths that expect final signatures.
- [ ] Ensure Cashu keyset v3 (`KeySetVersion::Version02`) BLS responses do not include DLEQ data.
- [ ] Reject all non-v3 keysets in federation signing paths unless a future protocol explicitly supports them.
- [ ] Persist signature shares in `federation_signature_share`, keyed by operation hash, output index, member ID, blinded message, amount, and keyset ID.
- [ ] Return existing share for idempotent repeat requests.

Tests and success criteria:

- [ ] A member cannot return a final signature in federation mode.
- [ ] A member's share verifies against its public share.
- [ ] Aggregate of shares returned by three members verifies as a normal Cashu blind signature when the configured threshold is three.
- [ ] Existing standalone `DbSignatory` behavior is unchanged.
- [ ] A federated member rejects non-BLS keysets for federation signing unless explicitly supported later.
- [ ] A federated member rejects v1/v2 keysets even if they are otherwise valid standalone Cashu keysets.

## Milestone 8: federated mint quote creation and payment observations

Objective: make all members agree on mint quotes and paid status.

Implementation checklist:

- [ ] In federation mode, `get_mint_quote` on the contacted member creates the payment request, then submits `CreateMintQuote` to consensus.
- [ ] `CreateMintQuote` includes quote ID, method, unit, amount, expiry, request string, request lookup ID, pubkey, extra JSON, origin member, and accepted time.
- [ ] All members persist the same quote after consensus accepts it.
- [ ] Add a payment observer task per member that watches unpaid quotes and submits `ObserveMintQuotePayment`.
- [ ] `ObserveMintQuotePayment` includes quote ID, payment ID, amount, member ID, observed time, and optional backend proof.
- [ ] Mark quote paid only after the configured observation policy is satisfied.
- [ ] For fake wallet MVP, allow all members to observe paid deterministically.
- [ ] For real Lightning/onchain, define observation policies:
  - [ ] shared backend or independently verifiable settlement: threshold matching observations.
  - [ ] origin-custodied invoice: origin observation plus policy-defined quorum acknowledgement.
  - [ ] onchain: consensus block height plus transaction proof.
- [ ] Make duplicate payment observations idempotent by payment ID.
- [ ] Ensure a malicious single member cannot mark a quote paid alone.

Tests and success criteria:

- [ ] Creating a quote on one member makes it visible on all members after consensus.
- [ ] Fake wallet payment observation marks the quote paid on all members.
- [ ] Duplicate observations do not double-count `amount_paid`.
- [ ] Conflicting observations for the same payment ID are rejected or quarantined.
- [ ] A member that did not create the quote can still serve quote status after consensus.
- [ ] Quote state digests match across all members after payment.

## Milestone 9: federated mint issuance

Objective: issue eCash from paid quotes only after the exact issuance request is accepted by consensus.

Implementation checklist:

- [ ] In federation mode, `process_mint_request` converts `MintInput` into an `IssueMint` operation and submits it to consensus.
- [ ] `IssueMint` includes all quote IDs, quote amounts, blinded outputs, NUT-20 signatures, and operation version.
- [ ] Consensus apply validates quote states, paid amounts, issued amounts, payment methods, units, v3/BLS output keysets, output uniqueness, and NUT-20 signatures.
- [ ] Consensus apply records issuance and signature-share rows in the same DB transaction.
- [ ] The public handler waits for the operation outcome and returns this member's signature shares as `MintResponse`.
- [ ] If the same operation was already accepted, return cached shares.
- [ ] If a conflicting operation spent the quote first, return the existing CDK error that best maps to issued or unbalanced quote.
- [ ] For batch minting, record one accepted operation and per-quote issued amounts.
- [ ] Ensure returned shares are ordered exactly like request outputs.

Tests and success criteria:

- [ ] Single quote paid by fake wallet mints successfully through 3-of-5 or configured threshold.
- [ ] Batch mint with two paid quotes mints once and updates both quotes.
- [ ] Replaying the same mint request returns the same aggregate final signature after wallet aggregation.
- [ ] Same quote with different outputs cannot obtain threshold shares for both output sets.
- [ ] On member crash after consensus acceptance but before HTTP response, retry returns stored shares.
- [ ] On member crash before consensus acceptance, retry submits the same operation hash.
- [ ] Existing NUT-20 locked quote tests pass in federated mode.

## Milestone 10: federated wallet connector and aggregation

Objective: make the wallet see normal Cashu responses while the connector handles member fanout.

Implementation checklist:

- [ ] Add `FederatedMintConnector` implementing `MintConnector`.
- [ ] Load federation config from mint info, invite code, or explicit builder config.
- [ ] Wrap one `HttpClient` per member.
- [ ] `get_mint_keys`, `get_mint_keysets`, and `get_mint_info` should query multiple members and require matching federation ID and aggregate keysets.
- [ ] `post_mint_quote` should create a quote through one healthy member, then optionally verify quorum visibility.
- [ ] `post_mint` and `post_batch_mint` should fan out the exact same request to all members concurrently.
- [ ] Validate each returned share against the member public share and corresponding blinded output.
- [ ] Aggregate the first threshold valid shares per output.
- [ ] Return normal aggregate `MintResponse` to the wallet saga.
- [ ] `post_swap` should do the same for swap signatures.
- [ ] Implement retry and timeout policy per member.
- [ ] Track per-member errors without leaking them into proof construction unless threshold cannot be reached.
- [ ] Add a clear error when too few valid shares arrive.
- [ ] Ensure wallet-local `validate_mint_response_signatures` validates aggregate signatures under aggregate keys after aggregation.

Tests and success criteria:

- [ ] Federated connector mints proofs accepted by existing wallet proof verification.
- [ ] Connector tolerates one offline member when threshold can still be met.
- [ ] Connector rejects a bad share from one member and still succeeds if enough good shares remain.
- [ ] Connector fails if responses have mismatched amounts or keyset IDs.
- [ ] Connector fails if members advertise different aggregate keysets or any non-v3 federation keyset.
- [ ] Existing wallet issue saga needs minimal changes beyond connector injection.

## Milestone 11: federated swap

Objective: prevent double-spends and mix-and-match output attacks for swaps.

Implementation checklist:

- [ ] In federation mode, public `/swap` submits a canonical `Swap` operation to consensus.
- [ ] `Swap` commits to all input proofs, all witnesses, all output blinded messages, and request version.
- [ ] Consensus apply verifies:
  - [ ] inputs are unique.
  - [ ] outputs are unique.
  - [ ] input keysets are known and not expired.
  - [ ] output keysets are active and not expired.
  - [ ] proofs verify under aggregate public keys.
  - [ ] P2PK, HTLC, and SIG_ALL spending conditions verify over the canonical transcript.
  - [ ] input value equals output value plus fees.
  - [ ] no input proof Y is already spent or pending in consensus state.
- [ ] Consensus apply marks inputs spent and records output signature shares in one DB transaction.
- [ ] Public handler waits for acceptance and returns this member's shares.
- [ ] Existing standalone saga remains available for non-federated mints.
- [ ] Add restore mapping from blinded output to operation hash and output index.

Tests and success criteria:

- [ ] Normal wallet swap succeeds through federated connector.
- [ ] Concurrent swaps spending the same input result in exactly one accepted swap.
- [ ] Replay of the same swap request is idempotent.
- [ ] Different output sets for the same inputs cannot both get threshold shares.
- [ ] P2PK and HTLC tests pass in federated mode.
- [ ] SIG_ALL tests prove the signature commits to the exact canonical operation.

## Milestone 12: explicit sliding-window attack regression suite

Objective: prove that threshold shares cannot be mixed across different output sets.

Implementation checklist:

- [ ] Build a malicious test wallet helper that can send different output windows to different members.
- [ ] Configure a 4-member test federation with signature threshold 3 if lower-threshold mode is supported.
- [ ] Create a paid quote for three notes and attempt the ABC, BCD, CDA, DAB pattern.
- [ ] Ensure each member submits its request to consensus and signs only accepted operations.
- [ ] Record which operation hash won consensus.
- [ ] Attempt to aggregate shares per output across all responses.
- [ ] Repeat the same test for swap inputs instead of mint quote issuance.
- [ ] Repeat with delayed members and duplicate submissions.

Tests and success criteria:

- [ ] Only one exact operation receives enough honest shares.
- [ ] The fourth unpaid output cannot be aggregated into a valid proof.
- [ ] Conflicting operations are rejected, not partially signed.
- [ ] State digests still match after the attack attempt.
- [ ] The same suite fails against an intentionally naive local-sign-before-consensus test double.

## Milestone 13: restore and check-state

Objective: support wallet recovery without reintroducing unsafe signing.

Implementation checklist:

- [ ] `checkstate` can remain a read query over local consensus materialized state.
- [ ] `restore` should never sign new outputs. It should return signature shares only for blinded messages already signed by accepted operations.
- [ ] Add `federation_signature_share` lookup by blinded message.
- [ ] If a member lacks a local share for an accepted output, it may regenerate it from the accepted operation and key share, then persist it.
- [ ] Federated connector calls restore on members, validates shares, aggregates threshold shares, and returns normal `RestoreResponse`.
- [ ] Recovery APIs must not leak member-specific private share data.

Tests and success criteria:

- [ ] Wallet can restore proofs after losing local DB using threshold member responses.
- [ ] Restore of unknown blinded message returns no signature share.
- [ ] Restore cannot be used to get shares for an output that never passed consensus.
- [ ] Checkstate returns identical states from all honest members.

## Milestone 14: melt reserve and payment finalization

Objective: federate melts without losing funds or allowing proof reuse.

Implementation checklist:

- [ ] Split melt into consensus-visible stages.
- [ ] `MeltQuoteCreate` persists the quote across members.
- [ ] `MeltReserve` consumes the wallet's melt request, validates inputs and change outputs, and marks inputs pending in consensus state.
- [ ] Exactly one payment actor should attempt the outgoing payment, selected deterministically from operation hash or configured as origin member.
- [ ] Before attempting payment, consensus must record `PaymentAttempted` or equivalent write-ahead state.
- [ ] Payment result enters consensus as `ObserveMeltPayment`.
- [ ] `FinalizeMelt` after threshold paid observations marks inputs spent, records quote paid, signs change outputs, records completed operation, and deletes pending melt state.
- [ ] `RollbackMelt` after threshold failed observations releases pending inputs and records quote failed.
- [ ] Unknown or pending backend states keep inputs pending until resolved or an admin policy handles timeout.
- [ ] Existing `MeltSaga` recovery concepts should be reused, but consensus state replaces member-local saga state as the source of truth.
- [ ] Internal melt-to-mint settlement must become a single consensus operation that updates both quote families atomically.

Tests and success criteria:

- [ ] Fake wallet melt succeeds and returns aggregated change signatures.
- [ ] Failed fake wallet melt releases inputs across all members.
- [ ] Pending fake wallet melt keeps inputs pending and later finalizes after observation.
- [ ] Crash after payment attempt but before finalize recovers by checking observations and finalizing.
- [ ] Two concurrent melts of the same proofs cannot both pay.
- [ ] Internal melt-to-mint updates the mint quote and melt quote in one accepted operation.

## Milestone 15: networking and peer authentication

Objective: run federation members as separate processes.

Implementation checklist:

- [ ] Add a P2P trait similar to Fedimint's `IP2PConnections`.
- [ ] Implement in-memory P2P for tests.
- [ ] Keep the transport behind a trait so in-memory, private REST, TCP/TLS or Noise, and Iroh can share consensus code.
- [ ] Implement private REST transport first using Axum/Reqwest for local, regtest, and reachable-server deployments.
- [ ] Keep federation REST endpoints separate from public Cashu wallet endpoints, for example under `/federation/v1/*`.
- [ ] Use signed, canonical member-to-member envelopes on every REST message: federation ID, sender member ID, receiver member ID, message kind, consensus height, nonce or sequence number, body hash, and signature.
- [ ] Verify every private REST envelope against the sender's identity public key from the local federation roster.
- [ ] Refuse private REST requests from member IDs that are not in the configured roster.
- [ ] Refuse private REST requests whose signing key does not match the configured public key for the claimed sender member ID.
- [ ] Do not support dynamic peer admission in MVP; member add/remove must be an explicit later governance operation.
- [ ] Make REST messages idempotent so retries after timeouts cannot duplicate consensus proposals, payment observations, or outgoing fake-wallet attempts.
- [ ] Add REST endpoints for peer status, proposal submission, consensus messages, signed session outcomes, catch-up ranges, and state-digest comparison.
- [ ] Optionally implement TCP/TLS or Noise transport with static peer identities if the consensus engine benefits from a lower-level stream API.
- [ ] Add an Iroh transport adapter for internet deployments where members cannot expose stable inbound P2P ports.
- [ ] Treat Iroh as the recommended NAT/firewall traversal transport, following the same broad deployment motivation as Fedimint, but do not make the first correctness milestones depend on it.
- [ ] Document that private REST alone requires reachable member endpoints or an external connectivity solution such as VPN, reverse tunnel, relay, Tor, or Iroh.
- [ ] Authenticate every peer message by member ID.
- [ ] Add bounded outbound and inbound queues.
- [ ] Add reconnect with exponential backoff.
- [ ] Add periodic config-digest, consensus-height, and state-digest gossip between peers.
- [ ] Add peer status metrics and admin status output.
- [ ] Separate public HTTP API URL from private P2P URL.
- [ ] Reject messages from unknown members or wrong federation IDs.
- [ ] Define maximum message sizes and backpressure behavior for consensus proposals, signature-share responses, and catch-up data.

Tests and success criteria:

- [ ] Five local processes discover peers and form consensus over private REST.
- [ ] In-memory P2P test can deterministically simulate partitions, dropped messages, reordered messages, and reconnects.
- [ ] REST retry test proves duplicate HTTP deliveries are idempotent.
- [ ] REST authentication test rejects wrong federation ID, wrong sender ID, stale sequence, invalid signature, and body-hash mismatch.
- [ ] REST allowlist test rejects a well-formed message signed by a valid key that is not in the federation roster.
- [ ] REST claimed-sender test rejects a message signed by member A's key while claiming member B's ID.
- [ ] Iroh adapter smoke test connects peers without direct inbound TCP reachability when the test environment supports it.
- [ ] Restarted member reconnects and catches up.
- [ ] Unknown peer messages are rejected.
- [ ] Queue overflow drops non-critical messages without crashing consensus.
- [ ] TLS, Noise, or signed-envelope identity mismatch prevents connection or message acceptance.
- [ ] Peer status shows connected, disconnected, lagging, catching-up, and halted states.

## Milestone 16: member startup, catch-up, and recovery

Objective: make nodes durable and restart-safe.

Implementation checklist:

- [ ] On startup, load federation config and validate key shares before serving HTTP.
- [ ] Start P2P and consensus before enabling signing endpoints.
- [ ] Replay accepted items since last checkpoint if needed.
- [ ] Persist last applied consensus session, item index, operation hash, outcome hash, and state digest.
- [ ] Persist signed session outcomes so lagging members can verify catch-up data without trusting one peer.
- [ ] On reconnect, request missing signed session outcomes and accepted items from peers starting at the last durable local height.
- [ ] Verify quorum or threshold signatures on catch-up data before replaying it locally.
- [ ] Replay accepted operations deterministically and recompute state digests after every session boundary.
- [ ] Regenerate deterministic signature shares from accepted operations when local share rows are missing.
- [ ] Compare local state digest to quorum state digest at session boundaries.
- [ ] Expose admin endpoint or CLI to show consensus height, last digest, member lag, and pending operations.
- [ ] Add DB checkpoint retention configuration.
- [ ] Add a safe read-only "catch-up" mode for lagging members.
- [ ] Prevent a lagging member from serving signing shares until it catches up to the operation's acceptance height.
- [ ] Prevent a lagging member from initiating fake-wallet outgoing payment attempts until it has replayed all accepted melt state.
- [ ] Add idempotency keys for fake wallet outgoing payments so recovery cannot pay twice.
- [ ] Add a manual halt and inspect mode for state digest mismatch.

Tests and success criteria:

- [ ] Crash before applying an item replays cleanly.
- [ ] Crash after applying but before HTTP response returns cached shares on retry.
- [ ] Crash after signing share but before storing share regenerates the same share.
- [ ] Lagging member refuses to sign until caught up.
- [ ] Member offline for 10 accepted sessions catches up and reaches the same state digest.
- [ ] Member offline during mint quote payment observation catches up and can later return the correct deterministic share.
- [ ] Member offline during swap cannot later accept a conflicting spend and catches up to spent state.
- [ ] Member offline during melt payment observation catches up without attempting a duplicate fake-wallet payment.
- [ ] Network partition heals and all non-faulty members converge or halt on digest mismatch.
- [ ] State digest mismatch is detected and halts the member in tests.

## Milestone 17: `cdk-mintd` integration and regtest tooling

Objective: make the federation easy to run locally.

Implementation checklist:

- [ ] Add `federation` section to mintd config.
- [ ] Add member-specific config file support.
- [ ] Add `just fed-regtest` or similar target that starts N fake-wallet mints and a test wallet.
- [ ] Add health commands: member status, consensus height, quote status across members, state digest comparison.
- [ ] Add log labels for federation ID, member ID, operation hash, and session.
- [ ] Add a fixture generator for 4-member and 5-member federations.
- [ ] Ensure no generated config intended for examples contains private production material.

Local multi-mint federation harness:

- [ ] Generate per-member local configs containing member ID, public wallet API URL, private federation REST URL, DB path, fake-wallet backend settings, federation public config path, and local private member config path.
- [ ] Assign deterministic localhost ports for every member, with separate public wallet API ports and private federation REST ports.
- [ ] Create isolated per-member data directories so databases, logs, and configs cannot overlap.
- [ ] Add `just fed-regtest` to launch 4-member and 5-member fake-wallet federations as separate `cdk-mintd` processes.
- [ ] Add `just fed-regtest-status` to show public URL, private federation URL, process status, consensus height, last state digest, lag, and member health.
- [ ] Add `just fed-regtest-stop-member MEMBER_ID` and `just fed-regtest-start-member MEMBER_ID` to test threshold liveness, reconnect, and catch-up.
- [ ] Add `just fed-regtest-clean` to remove generated local configs, DBs, logs, and process state.
- [ ] Add a local wallet federation config, invite file, or test connector config that points to the federation roster instead of a single mint URL.
- [ ] Make local logs identify federation ID, member ID, public port, private federation port, consensus height, operation hash, and DB path.
- [ ] Document how to run the local federation manually without `just`, including the exact `cdk-mintd` command shape for one member.
- [ ] Keep generated local private configs out of examples and Git; commit only templates, deterministic non-secret fixtures, and safe documentation.

Tests and success criteria:

- [ ] `just fed-regtest` starts a 5-member fake-wallet federation.
- [ ] A CDK wallet can mint, swap, restore, and check state against it.
- [ ] A CDK wallet can melt through the fake-wallet federation and receive aggregated change signatures when change is expected.
- [ ] Stopping one member still allows operations if threshold and consensus quorum allow.
- [ ] Restarting the stopped member makes it reconnect, catch up from peers, and reach the same state digest.
- [ ] State digest comparison reports all members equal after each test.
- [ ] Local private REST peer authentication rejects unknown members, wrong federation IDs, wrong claimed member IDs, and invalid signatures on localhost.
- [ ] Local status commands clearly show connected, offline, lagging, catching-up, and healthy members.
- [ ] `just fed-regtest-clean` leaves no running member processes and removes generated local state.

## Milestone 18: storage backend coverage

Objective: support the same storage expectations as CDK does today.

Implementation checklist:

- [ ] Implement federation DB traits for SQLite first.
- [ ] Implement Postgres after SQLite semantics are stable.
- [ ] Decide whether Redb is wallet-only and therefore out of scope for mint federation.
- [ ] Keep migrations additive and compatible with standalone mode.
- [ ] Add SQL lock ordering rules for all conflict keys to avoid deadlocks.
- [ ] Add database indexes for operation hash, quote ID, proof Y, blinded message, session index, and member ID.

Tests and success criteria:

- [ ] SQLite unit and integration tests pass.
- [ ] Postgres integration tests pass when a Postgres instance is available.
- [ ] Concurrent request stress tests do not deadlock.
- [ ] Query plans for hot paths use indexes.

## Milestone 19: real payment backend policy

Objective: move beyond fake wallet without centralizing payment trust accidentally.

Implementation checklist:

- [x] Write a payment trust model document before enabling real funds.
- [x] For mint quotes, decide whether payment verification is independently verifiable, shared-backend, or origin-custodied.
- [x] For Lightning BOLT11/BOLT12, define how members verify invoice settlement:
  - [x] shared node with read-only credentials for all members.
  - [x] origin member proves settlement to other members.
  - [x] threshold gateway or payment processor service.
- [x] For onchain quotes, use consensus block height and transaction proof observations.
- [x] Define observation quorum and conflict handling for each method.
- [x] Add limits for pending quotes and stale observations.
- [ ] Add admin tooling to inspect unresolved payment observations.

Tests and success criteria:

- [x] Fake wallet policy and real backend policy are separate code paths.
- [x] A single malicious observation cannot mark a quote paid.
- [ ] Conflicting payment amounts do not silently update quote state.
- [ ] Stale pending quotes are visible to operators and do not block consensus.

## Milestone 20: distributed key generation

Objective: replace trusted-dealer setup with dealer-free DKG.

Implementation checklist:

- [ ] Port or adapt Pedersen DKG for BLS G2, using Fedimint's DKG as a reference.
- [ ] Run DKG over authenticated P2P during federation setup, accepting DKG messages only from identity public keys in the federation roster.
- [ ] Include federation ID, member roster, member identity public keys, participant order, commitments, BLS public shares, and aggregate public keys in the DKG transcript.
- [ ] Require each participant to sign the final DKG transcript with its federation identity key.
- [ ] Commit hash of polynomial commitments before revealing commitments.
- [ ] Verify secret shares against public commitments.
- [ ] Derive public shares and aggregate public keys deterministically.
- [ ] Persist DKG transcript for audit and recovery.
- [ ] Persist the final roster hash and DKG transcript hash in each member config.
- [ ] Support key rotation through consensus as a federation operation.
- [ ] Keep trusted-dealer mode for tests only.

Tests and success criteria:

- [ ] DKG produces keys equivalent in behavior to trusted-dealer keygen.
- [ ] DKG rejects a participant whose identity public key is not in the configured roster.
- [ ] DKG rejects a transcript where a BLS public share is reassigned to another member ID.
- [ ] DKG transcript signatures verify for every member identity public key.
- [ ] Invalid share is detected and setup fails.
- [ ] Restart during setup can resume or safely restart.
- [ ] Key rotation creates a new aggregate keyset accepted by all members.
- [ ] Old keysets remain verifiable until final expiry.

## Milestone 21: key rotation and federation upgrades

Objective: keep long-running federations maintainable.

Implementation checklist:

- [ ] Add consensus operation `RotateKeyset`.
- [ ] New aggregate keysets become active only after consensus acceptance.
- [ ] Each member proves possession of the corresponding secret share before activation.
- [ ] Wallet sees NUT-02 keyset changes consistently across members.
- [ ] Add protocol version negotiation in federation metadata.
- [ ] Add migration plan for changing operation encoding versions.
- [ ] Add member add/remove as a later, explicit governance operation. Do not include in MVP.

Tests and success criteria:

- [ ] Wallet can mint with new active keyset after rotation.
- [ ] Old proofs remain spendable.
- [ ] Members with stale config refuse to join upgraded consensus.
- [ ] Mixed version tests reject unsupported major versions cleanly.

## Milestone 22: observability and operator safety

Objective: make failures diagnosable without exposing private user data.

Implementation checklist:

- [ ] Add metrics for consensus height, pending mempool items, accepted operations, rejected operations, signing share latency, aggregation failures, state digest mismatches, and peer connectivity.
- [ ] Add structured logs with short operation hashes, not full secrets or PII.
- [ ] Add admin command to compare state digests across members.
- [ ] Add admin command to list pending payment observations by quote ID.
- [ ] Add alert when a member is behind by more than configured sessions.
- [ ] Add alert when too many invalid operations are submitted by one client IP or auth token.
- [ ] Redact proof secrets, witnesses, invoices, local paths, and private keys from logs.

Tests and success criteria:

- [ ] Log snapshot tests verify redaction of secrets and private config.
- [ ] Metrics are emitted in fake regtest.
- [ ] Digest mismatch alert fires in an injected-divergence test.

## Milestone 23: security review checklist

Objective: make the design reviewable before mainnet use.

Implementation checklist:

- [ ] Review threshold choice and BFT assumptions.
- [ ] Review operation canonicalization and transcript binding.
- [ ] Review all paths that can call `blind_sign` or `blind_sign_share`.
- [ ] Ensure no public endpoint can bypass consensus in federation mode.
- [ ] Review NUT-20 signature verification for federated batch issuance.
- [ ] Review P2PK, HTLC, and SIG_ALL transcript construction.
- [ ] Review real payment observation policies.
- [ ] Review DB conflict keys and indexes.
- [ ] Review member identity authentication.
- [ ] Review denial-of-service surfaces: huge witnesses, huge batches, repeated invalid ops, slow consensus, and restore spam.

Tests and success criteria:

- [ ] Independent reviewer can trace every signature share to an accepted operation hash.
- [ ] Fuzzers cover operation decoding and request conversion.
- [ ] Attack regression suite passes.
- [ ] No known route signs directly in federation mode.

## MVP cut lines

MVP 1: local deterministic federation

- Trusted-dealer key shares.
- In-memory or single-process consensus.
- Fake wallet mint quote payment.
- Mint issuance and swap.
- Fake-wallet melt state-machine tests without networked member processes.
- Federated wallet connector aggregation.
- Sliding-window regression test.

MVP 2: networked fake-wallet federation

- Separate member processes.
- AlephBFT consensus.
- SQLite federation DB.
- Member restart and catch-up.
- Fake-wallet melt reserve, pending, failure, finalize, and change signing.
- Restore and check-state.
- Regtest tooling.

MVP 3: payments and production hardening

- Real payment observation policy.
- Postgres support.
- DKG.
- Key rotation.
- Full observability and security review.

## Initial implementation order

1. Add threshold BLS tests and types.
2. Add federation config and trusted-dealer generator.
3. Add canonical operation encoding and operation hash tests.
4. Add in-memory consensus and federation DB journal.
5. Refactor mint issue apply path.
6. Add federated signatory share storage.
7. Implement federated mint issuance end to end with fake wallet.
8. Implement federated wallet connector aggregation for mint.
9. Add sliding-window mint attack regression.
10. Refactor and implement federated swap.
11. Add sliding-window swap attack regression.
12. Add networked consensus and cdk-mintd fake federation harness.
13. Add fake-wallet melt reserve, pending, failure, finalize, and change signing.
14. Add restore/check-state.
15. Add network chaos, catch-up, and payment-observation recovery tests.
16. Add real payment observation policy and DKG.
