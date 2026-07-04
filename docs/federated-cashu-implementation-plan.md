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

## Current implementation status audit

Last code-confirmed audit: 2026-07-04. This checklist marks items as done only when the repository contains a concrete implementation or a concrete checked-in test for that item. Broad "test suite passes" checkboxes remain open unless the suite was run during the audit. Some completed items are implemented with CDK-native names and modules rather than the exact names originally proposed here; those are marked done when the safety property is present, with notes where the original mechanism is still missing.

Current confirmed shape:

- Core implemented areas: threshold BLS shares, static federation config, trusted-dealer setup, v3/BLS keyset enforcement, canonical operation hashes, in-memory consensus scaffold, private REST peer envelopes, federation journal persistence, mint/swap/melt/restore/check-state apply paths, wallet fanout and aggregation, payment observation policy/quarantine, local regtest tooling, DKG setup-route scaffold, keyset rotation, digest comparison, and several catch-up/recovery tests.
- Partial or intentionally deferred areas: production BFT ordering, AlephBFT integration, Iroh/NAT traversal, exact deterministic binary operation encoding, a separate `federation_signature_share` table, a separate `cdk-federation` crate, full dealer-free DKG replacing trusted-dealer setup, dynamic membership, protocol-version governance, comprehensive fuzzing, and a final mainnet security-review checklist.
- Test-suite status: this audit read code and tests but did not run the full workspace test suite.
- Latest checkpoint: seven-member/five-threshold in-memory consensus partition coverage now has a fast test gate, and generated live smoke scripts now pass a bounded `federation smoke-operations` timeout. A seven-member live smoke generated configs and started all members, but the baseline operation driver stayed CPU-bound long enough to require manual interruption, so profiling that hot path remains open before promoting seven-member live smokes to the regular matrix.

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

Database equality means equality of consensus-visible federation state, not byte-for-byte equality of each member's entire database file. Private member configuration, local peer state, local sequence trackers, logs, caches, metrics, and other node-local operational data must be excluded from state digests and row-level convergence checks. Any member whose consensus-visible state digest differs from quorum after deterministic replay must halt or enter manual inspection mode before serving signing shares, melt payment attempts, restore responses, or authoritative check-state responses.

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

- [x] Add an RFC document under `docs/` describing the federated mint threat model.
- [x] Define supported fault models: BFT consensus default, optional lower signing threshold, crash-only test mode.
- [x] Decide the default threshold rule. Recommended: default to Fedimint-style `n - floor((n - 1) / 3)` for both consensus and signing until lower signing thresholds receive separate review.
- [x] If supporting examples like 3-of-5 signing, define it as a separate `signature_threshold`, with consensus quorum still governed by the BFT engine.
- [x] Add a `federation` Cargo feature that is off by default.
- [x] Add `FederationMode::{Standalone, Member}` to mint setup without changing existing constructors initially.
- [x] Document that signature shares must never be produced from pre-consensus local request state.

Tests and success criteria:

- [ ] Existing `cargo test --lib --workspace --exclude cdk-postgres` passes with the feature disabled. Status: not re-run during the 2026-07-04 audit.
- [ ] Enabling the feature compiles a stub crate or module with no behavior change. Status: implementation exists, but this broad compile check was not re-run during the audit.
- [ ] A review checklist exists for all future federation PRs: determinism, consensus-before-signing, operation hashing, state digest, attack tests. Status: still needs a dedicated checklist document or PR template.

## Milestone 1: threshold BLS blind signature primitives

Objective: add the cryptographic building blocks without touching mint state logic.

Implementation checklist:

- [x] Add a separate file-backed module for threshold BLS under `crates/cashu/src/nuts/nut01/bls/` or an equivalent protocol-level location.
- [x] Define `BlsSecretKeyShare`, `BlsPublicKeyShare`, `BlsAggregatePublicKey`, `BlsBlindSignatureShare`, and signer index types.
- [x] Implement trusted-dealer polynomial key generation for BLS12-381 G2 public keys and G1 signature shares.
- [x] Implement Lagrange interpolation over BLS12-381 scalar field for arbitrary member IDs.
- [x] Implement blind signing with a secret share: `share = sk_i * blinded_message`.
- [x] Implement share verification: `e(share, G2) == e(blinded_message, public_share_i)`.
- [x] Implement aggregate blind signature construction from `t` valid shares.
- [x] Verify aggregate blind signatures with existing `verify_bls_blind_signature`.
- [x] Unblind aggregate signatures with existing BLS unblinding logic and verify final proofs with existing `verify_bls_message`.
- [x] Add serialization for share types with explicit versioning and no secret key debug leakage.
- [ ] Add zeroization for secret shares where practical. Status: no explicit zeroization implementation was found in the BLS share types during the audit.

Tests and success criteria:

- [x] Unit test 1-of-1 threshold produces the same final signature as the existing standalone BLS signer.
- [ ] Unit test 3-of-5 and 4-of-7 aggregate signatures verify under aggregate public keys. Status: 3-of-5 and 4-of-6 coverage exists; add an explicit 4-of-7 case if that exact acceptance criterion matters.
- [x] Unit test any valid threshold subset aggregates to the same final signature.
- [x] Unit test insufficient shares cannot aggregate.
- [x] Unit test duplicate signer IDs are rejected.
- [x] Unit test invalid share fails verification before aggregation.
- [x] Unit test share/order permutations produce identical aggregate signatures.
- [ ] Property test random messages, thresholds, and subsets. Status: deterministic multi-case tests exist, but no randomized/property test was found.
- [x] Existing NUT-00 BLS proof verification passes for aggregate proofs.

## Milestone 2: federation configuration and trusted-dealer setup

Objective: create deterministic federation configs and local key shares for test federations.

Implementation checklist:

- [x] Add `FederationConfig` with federation ID, member set, consensus params, signature threshold, keyset metadata, supported units, and protocol versions.
- [ ] Add `MemberConfigPublic` with member ID, public API URL, private federation REST URL, P2P endpoint, federation identity public key, optional TLS/Noise/Iroh public key, and public BLS shares. Status: public wallet URL, private federation REST URL, identity key, and BLS public shares exist; P2P/TLS/Noise/Iroh identity fields remain deferred.
- [ ] Add `MemberConfigPrivate` with member ID, federation identity private key, P2P private key, local secret BLS shares, payment backend config, and DB path. Status: member ID, identity secret, local secret BLS shares, and config-file wiring exist; P2P private key is not implemented.
- [ ] Add `FederationRoster` mapping every member ID to its identity public key, endpoint set, and BLS public shares. Status: roster behavior is implemented as ordered maps in `FederationConfig`; no separate `FederationRoster` type was found.
- [x] Add `FederatedKeyset` with aggregate public keys per amount and public shares per member.
- [x] Define deterministic member ordering and reject duplicate member IDs.
- [x] Generate Cashu keyset IDs from aggregate public keys, not member share public keys.
- [x] Force all generated federation keysets to Cashu keyset v3 (`KeySetVersion::Version02`).
- [x] Add `cdk-mintd federation trusted-dealer` to generate N member config files for tests.
- [x] Add `cdk-mintd federation inspect` to print non-secret config and keyset IDs.
- [x] Add config validation to ensure each local identity private key matches the configured identity public key.
- [x] Add config validation to ensure secret shares match declared public shares.
- [x] Add config validation that every aggregate federation keyset is v3/BLS and reject v1/v2/non-BLS federation keysets.
- [x] Add validation that all members agree on roster, identity public keys, aggregate keysets, thresholds, units, fees, and NUT settings.
- [x] Reject startup if a member is missing any federation partner public key.

Tests and success criteria:

- [x] Generated 5-member configs have identical federation ID and aggregate keyset IDs.
- [x] Generated 5-member configs have identical roster hashes and complete partner identity public-key allowlists.
- [x] Each member's private identity key verifies against its public identity key.
- [x] Each member's private share verifies against its public share.
- [x] A config with mismatched thresholds is rejected.
- [x] A config with mismatched aggregate keyset is rejected.
- [x] A config with any non-v3 federation aggregate keyset is rejected.
- [x] A config with mismatched member identity public key is rejected.
- [x] A config missing a federation partner public key is rejected.
- [x] A config with duplicate member IDs is rejected.
- [ ] A config generated twice with the same deterministic test seed matches byte-for-byte except explicitly random fields. Status: trusted-dealer tests cover deterministic IDs and validation, but the generator currently creates fresh random identity/secret material for normal output.

## Milestone 3: canonical operation encoding and hashes

Objective: make every consensus operation uniquely and deterministically identified.

Implementation checklist:

- [x] Define `FederatedMintOperation` enum with versioned variants.
- [ ] Start with variants: `CreateMintQuote`, `ObserveMintQuotePayment`, `IssueMint`, `Swap`, `RestoreRequest`, `MeltReserve`, `ObserveMeltPayment`, `FinalizeMelt`, `RollbackMelt`. Status: implemented variants use CDK names (`MintQuote`, `MintQuotePayment`, `Mint`, `Swap`, `MeltQuote`, `MeltQuotePayment`, `Melt`, `Restore`, `PaymentObservationQuarantine`, `KeysetRotation`); exact reserve/finalize/rollback split remains partially represented through melt quote/payment states and saga recovery.
- [ ] Use a deterministic binary encoding for consensus data. Avoid raw JSON and unordered maps. Status: deterministic canonical JSON is implemented and tested; binary encoding remains open.
- [x] Use `BTreeMap` and sorted vectors wherever ordering matters.
- [x] Define `OperationHash = sha256("cdk-fed-op-v1" || encoded_operation)`.
- [x] Define per-output hashes for blinded messages and per-input proof Y identifiers.
- [x] Make operation IDs for federated consensus hash-based rather than UUIDv7-based.
- [x] Keep wallet-local saga IDs as UUIDs if useful, but never use them as consensus identity.
- [x] Define conflict keys per operation:
  - [x] quote issuance: quote ID plus issued amount window.
  - [x] swap: input proof Ys and output blinded messages.
  - [x] melt reserve: quote ID plus input proof Ys.
  - [x] restore: blinded messages only.
- [ ] Define a canonical `RequestTranscript` for NUT-20 and SIG_ALL verification. Status: no dedicated federation transcript type was found.

Tests and success criteria:

- [ ] Encoding of a golden operation matches committed test vectors. Status: unit tests cover canonical behavior, but no committed cross-version fixture vector was found.
- [x] Equivalent operations produce the same hash across processes.
- [x] Reordered unordered fields still encode canonically.
- [x] Changing one output blinded message changes the operation hash.
- [ ] Changing an input witness changes the operation hash. Status: operation hashes include request payloads, but an explicit witness-mutation test was not found.
- [ ] Fuzz decoding rejects unknown major versions and oversized payloads. Status: size-limit tests exist; fuzz coverage for decoding/version rejection remains open.

## Milestone 4: consensus engine abstraction

Objective: add an ordered log before integrating with mint business logic.

Implementation checklist:

- [ ] Create a `cdk-federation` consensus abstraction: `submit(item)`, `await_operation(hash)`, `accepted_stream()`, `state_digest()`. Status: a consensus abstraction exists in `cdk-common::federation::consensus`; it is not a separate `cdk-federation` crate and does not expose exactly this interface.
- [x] Add `ConsensusItem::{Operation(FederatedMintOperation), ModuleFact(...), SessionSignature(...), Default(...)}`.
- [x] Add an in-memory single-node consensus engine for deterministic unit tests.
- [x] Add an in-memory multi-node test engine that can reorder, duplicate, and delay messages.
- [ ] Add AlephBFT-backed engine for multi-member BFT ordering. Status: backend kind exists, but selecting AlephBFT currently returns `UnsupportedConsensusBackend`.
- [x] Add a `DataProvider` equivalent that batches pending items, deduplicates operation hashes, and enforces max item size.
- [x] Add a `FinalizationHandler` equivalent that applies ordered items to the DB transactionally.
- [x] Add session outcomes signed by member identity keys.
- [x] Store accepted items and signed session outcomes in the DB.
- [x] Add operation outcome notification so public HTTP handlers can await acceptance.
- [x] Add bounded mempool and admission checks to avoid DoS by large invalid operations.

Tests and success criteria:

- [x] Single-node engine accepts operations in submit order.
- [x] Multi-node in-memory engine converges on identical accepted sequences.
- [x] Duplicate operation submissions are accepted once.
- [x] Oversized consensus items are rejected before broadcast.
- [x] Restarting a node mid-session replays accepted items without duplicating state changes.
- [ ] AlephBFT integration test finalizes sessions for 4 nodes and 5 nodes. Status: not implemented.
- [x] Consensus halts or rejects when fewer than the required quorum is reachable.

## Milestone 5: federation DB journal and state digests

Objective: keep replicated operation history separate from materialized mint state.

Implementation checklist:

- [x] Add DB traits for `FederationDatabase` and `FederationTransaction`. Status: implemented as `FederationJournalDatabase` and `FederationJournalTransaction`.
- [ ] Add SQL migrations for SQLite and Postgres:
  - [ ] `federation_config`. Status: no dedicated table found; federation config remains file/runtime config.
  - [x] `federation_session_outcome`. Status: implemented as `federation_session_outcomes`.
  - [x] `federation_accepted_item`. Status: implemented as `federation_journal_entries`.
  - [x] `federation_operation`. Status: operation envelopes are persisted in `federation_journal_entries`.
  - [x] `federation_operation_outcome`. Status: operation application status is persisted in `federation_journal_entries`.
  - [ ] `federation_signature_share`. Status: no separate table found; federated shares are currently stored through existing blind-signature rows.
  - [x] `federation_payment_observation`. Status: implemented as `federation_payment_observations`.
  - [ ] `federation_state_digest`. Status: state digests are computed and reported, but no dedicated digest-history table was found.
  - [ ] `federation_member_status`. Status: private REST sequence watermarks are persisted in `federation_member_sequences`; member status is otherwise runtime/admin state.
- [x] Keep existing mint tables as the materialized state used by existing APIs where possible.
- [ ] Add `signature_share` rows instead of overloading final `blind_signature` semantics for federated members. Status: still open; current code reuses existing `blind_signature` storage for member shares.
- [x] Add state digest calculation over consensus-visible tables only.
- [x] Exclude local private config, local connection status, logs, and caches from the digest.
- [ ] Add an audit summary similar to Fedimint's balance audit after each accepted operation. Status: no balance-audit equivalent was found.
- [x] Store DB checkpoints or lightweight digest history per session. Status: accepted order digests and signed session outcomes are persisted; a dedicated checkpoint-retention mechanism remains open.

Tests and success criteria:

- [ ] Migrations apply cleanly on empty SQLite and Postgres databases. Status: migrations exist for both backends, but this audit did not run migration tests.
- [ ] Migration tests cover rollback-compatible standalone mode. Status: not re-run during this audit.
- [x] Applying the same operation log to two DBs yields identical state digests.
- [x] Local-only fields do not change state digest.
- [ ] Negative balance audit causes a hard error in tests. Status: no balance-audit test found.
- [ ] Signature shares can be retrieved by operation hash and output index. Status: no separate signature-share table/API found; existing blind-signature lookup is by blinded secret.

## Milestone 6: deterministic mint state machine extraction

Objective: refactor CDK mint operations so consensus can call deterministic apply functions.

Implementation checklist:

- [ ] Extract validation and apply logic from `process_mint_request` into `validate_issue_mint` and `apply_issue_mint`. Status: federation has `apply_federation_mint`, but the standalone method was not refactored into the exact proposed functions.
- [ ] Extract validation and apply logic from `SwapSaga::setup_swap` and `finalize` into `validate_swap` and `apply_swap`. Status: federation has `apply_federation_swap`, but the standalone saga was not refactored into the exact proposed functions.
- [ ] Ensure apply functions receive all nondeterministic values as inputs: accepted time, operation hash, quote IDs, and payment observations. Status: operation IDs/hashes, quote IDs, and payment observations are explicit; no dedicated consensus-provided accepted-time field was confirmed.
- [ ] Remove calls to `unix_time()` from consensus apply paths except via consensus-provided accepted time.
- [x] Remove UUID generation from consensus apply paths.
- [x] Ensure apply paths do not spawn tasks or publish pubsub notifications until after commit.
- [ ] Keep existing standalone methods calling the refactored functions through a standalone adapter. Status: standalone behavior remains, but no shared standalone adapter around the exact new functions was found.
- [x] Make `apply_issue_mint` update quote issuance, blinded message records, completed operation, and output signature-share request in one DB transaction. Status: implemented as `apply_federation_mint`, using existing blind-signature storage for member shares.
- [x] Make `apply_swap` verify inputs, verify spending conditions, check balance, mark inputs spent, record blinded outputs, record completed operation, and request signature shares in one DB transaction. Status: implemented as `apply_federation_swap`.
- [x] Decide how to represent `Pending` in federated mode:
  - [x] Swap should not use local pending state before consensus.
  - [x] Melt reserve can use consensus-visible pending state between reserve and finalization.
- [x] Add idempotency: if the exact same operation was already accepted, return the existing outcome.
- [x] Add conflict detection: if a conflicting operation was accepted first, reject and record a conflict outcome where appropriate.

Tests and success criteria:

- [ ] Standalone mint tests still pass using the refactored code. Status: not re-run during this audit.
- [x] Applying a valid issue operation twice is idempotent.
- [x] Applying a same-quote different-output issue after the first issue fails.
- [x] Applying a swap with already spent inputs fails.
- [x] Applying a swap with duplicate outputs fails.
- [x] Applying operations in the same order on multiple DB instances produces identical materialized state.
- [ ] Pubsub notifications fire only after commit in standalone and federation modes. Status: federation swap publishes after commit; no broad test covering all modes was found.

## Milestone 7: federated key-share signatory

Objective: let each member sign only with its own BLS share while advertising aggregate keys.

Implementation checklist:

- [ ] Add a `FederatedSignatory` or split the existing `Signatory` trait into final-signer and share-signer responsibilities. Status: member-share signing exists on federation config/private config helpers; no split signatory trait or `FederatedSignatory` type was found.
- [x] `keysets()` must return aggregate public keys and aggregate keyset IDs.
- [x] `verify_proofs()` must verify proofs against aggregate public keys.
- [x] `blind_sign_share()` must return member share signatures plus member ID.
- [x] Add share verification against local public share before storing or returning a share.
- [x] Prevent federated share signer from being used in standalone paths that expect final signatures.
- [x] Ensure Cashu keyset v3 (`KeySetVersion::Version02`) BLS responses do not include DLEQ data.
- [x] Reject all non-v3 keysets in federation signing paths unless a future protocol explicitly supports them.
- [ ] Persist signature shares in `federation_signature_share`, keyed by operation hash, output index, member ID, blinded message, amount, and keyset ID. Status: no separate table exists; current persistence uses existing blind-signature rows.
- [x] Return existing share for idempotent repeat requests.

Tests and success criteria:

- [x] A member cannot return a final signature in federation mode.
- [x] A member's share verifies against its public share.
- [x] Aggregate of shares returned by three members verifies as a normal Cashu blind signature when the configured threshold is three.
- [ ] Existing standalone `DbSignatory` behavior is unchanged. Status: no federation code path appears to modify standalone signatory behavior, but this broad regression test was not re-run during the audit.
- [x] A federated member rejects non-BLS keysets for federation signing unless explicitly supported later.
- [x] A federated member rejects v1/v2 keysets even if they are otherwise valid standalone Cashu keysets.

## Milestone 8: federated mint quote creation and payment observations

Objective: make all members agree on mint quotes and paid status.

Implementation checklist:

- [x] In federation mode, `get_mint_quote` on the contacted member creates the payment request, then submits `CreateMintQuote` to consensus. Status: implemented as a `MintQuote` federation operation.
- [x] `CreateMintQuote` includes quote ID, method, unit, amount, expiry, request string, request lookup ID, pubkey, extra JSON, origin member, and accepted time. Status: implemented through Cashu quote request/response payloads; accepted time is implicit in the accepted operation/journal rather than a dedicated field in this exact variant.
- [x] All members persist the same quote after consensus accepts it.
- [ ] Add a payment observer task per member that watches unpaid quotes and submits `ObserveMintQuotePayment`. Status: payment observations exist and are submitted/processed; no always-on per-member unpaid-quote watcher task was found.
- [x] `ObserveMintQuotePayment` includes quote ID, payment ID, amount, member ID, observed time, and optional backend proof. Status: implemented as `MintQuotePayment` with observer, state, paid/issued amounts, method, and optional payment proof; no dedicated observed-time field was found.
- [x] Mark quote paid only after the configured observation policy is satisfied.
- [x] For fake wallet MVP, allow all members to observe paid deterministically.
- [ ] For real Lightning/onchain, define observation policies:
  - [x] shared backend or independently verifiable settlement: threshold matching observations.
  - [x] origin-custodied invoice: origin observation plus policy-defined quorum acknowledgement.
  - [x] onchain: consensus block height plus transaction proof.
- [x] Make duplicate payment observations idempotent by payment ID.
- [x] Ensure a malicious single member cannot mark a quote paid alone.

Tests and success criteria:

- [x] Creating a quote on one member makes it visible on all members after consensus.
- [x] Fake wallet payment observation marks the quote paid on all members.
- [x] Duplicate observations do not double-count `amount_paid`.
- [x] Conflicting observations for the same payment ID are rejected or quarantined.
- [x] A member that did not create the quote can still serve quote status after consensus.
- [x] Quote state digests match across all members after payment.

## Milestone 9: federated mint issuance

Objective: issue eCash from paid quotes only after the exact issuance request is accepted by consensus.

Implementation checklist:

- [x] In federation mode, `process_mint_request` converts `MintInput` into an `IssueMint` operation and submits it to consensus. Status: implemented as a `Mint` federation operation.
- [x] `IssueMint` includes all quote IDs, quote amounts, blinded outputs, NUT-20 signatures, and operation version. Status: quote IDs and blinded outputs are included through `MintRequest`/`BatchMintRequest`; quote amounts are validated from accepted quote state.
- [x] Consensus apply validates quote states, paid amounts, issued amounts, payment methods, units, v3/BLS output keysets, output uniqueness, and NUT-20 signatures.
- [x] Consensus apply records issuance and signature-share rows in the same DB transaction. Status: signature shares are recorded via existing blind-signature rows, not a separate federation share table.
- [x] The public handler waits for the operation outcome and returns this member's signature shares as `MintResponse`.
- [x] If the same operation was already accepted, return cached shares.
- [x] If a conflicting operation spent the quote first, return the existing CDK error that best maps to issued or unbalanced quote.
- [x] For batch minting, record one accepted operation and per-quote issued amounts.
- [x] Ensure returned shares are ordered exactly like request outputs.

Tests and success criteria:

- [x] Single quote paid by fake wallet mints successfully through 3-of-5 or configured threshold.
- [x] Batch mint with two paid quotes mints once and updates both quotes.
- [x] Replaying the same mint request returns the same aggregate final signature after wallet aggregation.
- [x] Same quote with different outputs cannot obtain threshold shares for both output sets.
- [x] On member crash after consensus acceptance but before HTTP response, retry returns stored shares.
- [x] On member crash before consensus acceptance, retry submits the same operation hash.
- [ ] Existing NUT-20 locked quote tests pass in federated mode. Status: no explicit federated NUT-20 locked quote regression was confirmed during the audit.

## Milestone 10: federated wallet connector and aggregation

Objective: make the wallet see normal Cashu responses while the connector handles member fanout.

Implementation checklist:

- [x] Add `FederatedMintConnector` implementing `MintConnector`.
- [ ] Load federation config from mint info, invite code, or explicit builder config. Status: explicit builder/config construction exists; automatic mint-info or invite loading remains open.
- [x] Wrap one `HttpClient` per member.
- [x] `get_mint_keys`, `get_mint_keysets`, and `get_mint_info` should query multiple members and require matching federation ID and aggregate keysets.
- [ ] `post_mint_quote` should create a quote through one healthy member, then optionally verify quorum visibility. Status: quote creation through first healthy member exists; quorum visibility verification remains open.
- [x] `post_mint` and `post_batch_mint` should fan out the exact same request to all members concurrently.
- [x] Validate each returned share against the member public share and corresponding blinded output.
- [x] Aggregate the first threshold valid shares per output.
- [x] Return normal aggregate `MintResponse` to the wallet saga.
- [x] `post_swap` should do the same for swap signatures.
- [ ] Implement retry and timeout policy per member. Status: first-healthy fallback and threshold fanout exist; no explicit timeout/retry policy object was found.
- [x] Track per-member errors without leaking them into proof construction unless threshold cannot be reached.
- [x] Add a clear error when too few valid shares arrive.
- [x] Ensure wallet-local `validate_mint_response_signatures` validates aggregate signatures under aggregate keys after aggregation.

Tests and success criteria:

- [x] Federated connector mints proofs accepted by existing wallet proof verification.
- [x] Connector tolerates one offline member when threshold can still be met.
- [x] Connector rejects a bad share from one member and still succeeds if enough good shares remain.
- [x] Connector fails if responses have mismatched amounts or keyset IDs.
- [x] Connector fails if members advertise different aggregate keysets or any non-v3 federation keyset.
- [x] Existing wallet issue saga needs minimal changes beyond connector injection.

## Milestone 11: federated swap

Objective: prevent double-spends and mix-and-match output attacks for swaps.

Implementation checklist:

- [x] In federation mode, public `/swap` submits a canonical `Swap` operation to consensus.
- [x] `Swap` commits to all input proofs, all witnesses, all output blinded messages, and request version.
- [ ] Consensus apply verifies:
  - [x] inputs are unique.
  - [x] outputs are unique.
  - [x] input keysets are known and not expired.
  - [x] output keysets are active and not expired.
  - [x] proofs verify under aggregate public keys.
  - [ ] P2PK, HTLC, and SIG_ALL spending conditions verify over the canonical transcript. Status: spending-condition verification is called; a dedicated federated canonical-transcript test was not confirmed.
  - [x] input value equals output value plus fees.
  - [x] no input proof Y is already spent or pending in consensus state.
- [x] Consensus apply marks inputs spent and records output signature shares in one DB transaction.
- [x] Public handler waits for acceptance and returns this member's shares.
- [x] Existing standalone saga remains available for non-federated mints.
- [x] Add restore mapping from blinded output to operation hash and output index. Status: restore can recover accepted blind-signature rows by blinded output; no separate operation-hash/output-index share table exists.

Tests and success criteria:

- [x] Normal wallet swap succeeds through federated connector.
- [x] Concurrent swaps spending the same input result in exactly one accepted swap.
- [x] Replay of the same swap request is idempotent.
- [x] Different output sets for the same inputs cannot both get threshold shares.
- [ ] P2PK and HTLC tests pass in federated mode. Status: standalone spending-condition tests exist; federated-mode coverage was not confirmed.
- [ ] SIG_ALL tests prove the signature commits to the exact canonical operation. Status: standalone SIG_ALL tests exist; federated canonical operation coverage remains open.

## Milestone 12: explicit sliding-window attack regression suite

Objective: prove that threshold shares cannot be mixed across different output sets.

Implementation checklist:

- [x] Build a malicious test wallet helper that can send different output windows to different members.
- [x] Configure a 4-member test federation with signature threshold 3 if lower-threshold mode is supported.
- [x] Create a paid quote for three notes and attempt the ABC, BCD, CDA, DAB pattern.
- [x] Ensure each member submits its request to consensus and signs only accepted operations.
- [x] Record which operation hash won consensus.
- [x] Attempt to aggregate shares per output across all responses.
- [ ] Repeat the same test for swap inputs instead of mint quote issuance. Status: no explicit sliding-window swap attack regression was confirmed.
- [x] Repeat with delayed members and duplicate submissions.

Tests and success criteria:

- [x] Only one exact operation receives enough honest shares.
- [x] The fourth unpaid output cannot be aggregated into a valid proof.
- [x] Conflicting operations are rejected, not partially signed.
- [x] State digests still match after the attack attempt.
- [x] The same suite fails against an intentionally naive local-sign-before-consensus test double.

## Milestone 13: restore and check-state

Objective: support wallet recovery without reintroducing unsafe signing.

Implementation checklist:

- [x] `checkstate` can remain a read query over local consensus materialized state.
- [x] `restore` should never sign new outputs. It should return signature shares only for blinded messages already signed by accepted operations.
- [ ] Add `federation_signature_share` lookup by blinded message. Status: lookup uses existing blind-signature storage; no separate federation share table exists.
- [x] If a member lacks a local share for an accepted output, it may regenerate it from the accepted operation and key share, then persist it.
- [x] Federated connector calls restore on members, validates shares, aggregates threshold shares, and returns normal `RestoreResponse`.
- [x] Recovery APIs must not leak member-specific private share data.

Tests and success criteria:

- [x] Wallet can restore proofs after losing local DB using threshold member responses.
- [x] Restore of unknown blinded message returns no signature share.
- [x] Restore cannot be used to get shares for an output that never passed consensus.
- [x] Checkstate returns identical states from all honest members.

## Milestone 14: melt reserve and payment finalization

Objective: federate melts without losing funds or allowing proof reuse.

Implementation checklist:

- [x] Split melt into consensus-visible stages.
- [x] `MeltQuoteCreate` persists the quote across members. Status: implemented as `MeltQuote`.
- [x] `MeltReserve` consumes the wallet's melt request, validates inputs and change outputs, and marks inputs pending in consensus state. Status: implemented through `Melt` plus saga/pending state.
- [ ] Exactly one payment actor should attempt the outgoing payment, selected deterministically from operation hash or configured as origin member. Status: fake-wallet happy path is covered; deterministic single-actor production policy remains open.
- [x] Before attempting payment, consensus must record `PaymentAttempted` or equivalent write-ahead state.
- [x] Payment result enters consensus as `ObserveMeltPayment`. Status: implemented as `MeltQuotePayment`.
- [x] `FinalizeMelt` after threshold paid observations marks inputs spent, records quote paid, signs change outputs, records completed operation, and deletes pending melt state.
- [x] `RollbackMelt` after threshold failed observations releases pending inputs and records quote failed.
- [x] Unknown or pending backend states keep inputs pending until resolved or an admin policy handles timeout.
- [x] Existing `MeltSaga` recovery concepts should be reused, but consensus state replaces member-local saga state as the source of truth.
- [ ] Internal melt-to-mint settlement must become a single consensus operation that updates both quote families atomically. Status: not confirmed.

Tests and success criteria:

- [x] Fake wallet melt succeeds and returns aggregated change signatures.
- [x] Failed fake wallet melt releases inputs across all members.
- [x] Pending fake wallet melt keeps inputs pending and later finalizes after observation.
- [x] Crash after payment attempt but before finalize recovers by checking observations and finalizing.
- [x] Two concurrent melts of the same proofs cannot both pay.
- [ ] Internal melt-to-mint updates the mint quote and melt quote in one accepted operation. Status: not confirmed.

## Milestone 15: networking and peer authentication

Objective: run federation members as separate processes.

Implementation checklist:

- [ ] Add a P2P trait similar to Fedimint's `IP2PConnections`. Status: private REST client/server and `cdk_http_client::Transport` are used; no Fedimint-style P2P trait was found.
- [x] Implement in-memory P2P for tests. Status: implemented as an in-memory consensus/network simulator, not a separate P2P transport.
- [ ] Keep the transport behind a trait so in-memory, private REST, TCP/TLS or Noise, and Iroh can share consensus code. Status: HTTP transport is abstracted; no shared P2P transport abstraction for all listed transports was found.
- [x] Implement private REST transport first using Axum/Reqwest for local, regtest, and reachable-server deployments.
- [x] Keep federation REST endpoints separate from public Cashu wallet endpoints, for example under `/federation/v1/*`.
- [x] Use signed, canonical member-to-member envelopes on every REST message: federation ID, sender member ID, receiver member ID, message kind, consensus height, nonce or sequence number, body hash, and signature.
- [x] Verify every private REST envelope against the sender's identity public key from the local federation roster.
- [x] Refuse private REST requests from member IDs that are not in the configured roster.
- [x] Refuse private REST requests whose signing key does not match the configured public key for the claimed sender member ID.
- [x] Do not support dynamic peer admission in MVP; member add/remove must be an explicit later governance operation.
- [x] Make REST messages idempotent so retries after timeouts cannot duplicate consensus proposals, payment observations, or outgoing fake-wallet attempts.
- [x] Add REST endpoints for peer status, proposal submission, consensus messages, signed session outcomes, catch-up ranges, and state-digest comparison.
- [ ] Optionally implement TCP/TLS or Noise transport with static peer identities if the consensus engine benefits from a lower-level stream API.
- [ ] Add an Iroh transport adapter for internet deployments where members cannot expose stable inbound P2P ports.
- [x] Treat Iroh as the recommended NAT/firewall traversal transport, following the same broad deployment motivation as Fedimint, but do not make the first correctness milestones depend on it.
- [x] Document that private REST alone requires reachable member endpoints or an external connectivity solution such as VPN, reverse tunnel, relay, Tor, or Iroh.
- [x] Authenticate every peer message by member ID.
- [ ] Add bounded outbound and inbound queues. Status: consensus item size/admission limits exist; no general bounded REST queue/backpressure layer was confirmed.
- [ ] Add reconnect with exponential backoff. Status: periodic sync loop exists; exponential backoff was not confirmed.
- [x] Add periodic config-digest, consensus-height, and state-digest gossip between peers.
- [ ] Add peer status metrics and admin status output. Status: admin status output exists; dedicated metrics coverage remains open.
- [x] Separate public HTTP API URL from private P2P URL.
- [x] Reject messages from unknown members or wrong federation IDs.
- [x] Define maximum message sizes and backpressure behavior for consensus proposals, signature-share responses, and catch-up data.

Tests and success criteria:

- [ ] Five local processes discover peers and form consensus over private REST. Status: local process tooling and smoke scripts exist, but this was not re-run during the audit.
- [x] In-memory P2P test can deterministically simulate partitions, dropped messages, reordered messages, and reconnects.
- [x] REST retry test proves duplicate HTTP deliveries are idempotent.
- [x] REST authentication test rejects wrong federation ID, wrong sender ID, stale sequence, invalid signature, and body-hash mismatch.
- [x] REST allowlist test rejects a well-formed message signed by a valid key that is not in the federation roster.
- [x] REST claimed-sender test rejects a message signed by member A's key while claiming member B's ID.
- [ ] Iroh adapter smoke test connects peers without direct inbound TCP reachability when the test environment supports it. Status: not implemented.
- [x] Restarted member reconnects and catches up.
- [x] Unknown peer messages are rejected.
- [ ] Queue overflow drops non-critical messages without crashing consensus. Status: size/admission tests exist; queue-overflow behavior was not confirmed.
- [x] TLS, Noise, or signed-envelope identity mismatch prevents connection or message acceptance. Status: signed-envelope identity mismatch is tested; TLS/Noise transports are not implemented.
- [x] Peer status shows connected, disconnected, lagging, catching-up, and halted states.

## Milestone 16: member startup, catch-up, and recovery

Objective: make nodes durable and restart-safe.

Implementation checklist:

- [x] On startup, load federation config and validate key shares before serving HTTP.
- [x] Start P2P and consensus before enabling signing endpoints. Status: implemented for private REST/consensus runtime, not a non-REST P2P transport.
- [x] Replay accepted items since last checkpoint if needed.
- [x] Persist last applied consensus session, item index, operation hash, outcome hash, and state digest.
- [x] Persist signed session outcomes so lagging members can verify catch-up data without trusting one peer.
- [x] On reconnect, request missing signed session outcomes and accepted items from peers starting at the last durable local height.
- [x] Verify quorum or threshold signatures on catch-up data before replaying it locally.
- [x] Replay accepted operations deterministically and recompute state digests after every session boundary.
- [x] Regenerate deterministic signature shares from accepted operations when local share rows are missing.
- [x] Compare local state digest to quorum state digest at session boundaries.
- [x] Expose admin endpoint or CLI to show consensus height, last digest, member lag, and pending operations.
- [ ] Add DB checkpoint retention configuration. Status: no configurable checkpoint retention was confirmed.
- [x] Add a safe read-only "catch-up" mode for lagging members.
- [x] Prevent a lagging member from serving signing shares until it catches up to the operation's acceptance height.
- [x] Prevent a lagging member from initiating fake-wallet outgoing payment attempts until it has replayed all accepted melt state.
- [x] Add idempotency keys for fake wallet outgoing payments so recovery cannot pay twice.
- [x] Add a manual halt and inspect mode for state digest mismatch.

Tests and success criteria:

- [x] Crash before applying an item replays cleanly.
- [x] Crash after applying but before HTTP response returns cached shares on retry.
- [x] Crash after signing share but before storing share regenerates the same share.
- [x] Lagging member refuses to sign until caught up.
- [x] Member offline for 10 accepted sessions catches up and reaches the same state digest.
- [x] Member offline during mint quote payment observation catches up and can later return the correct deterministic share.
- [x] Member offline during swap cannot later accept a conflicting spend and catches up to spent state.
- [x] Member offline during melt payment observation catches up without attempting a duplicate fake-wallet payment.
- [x] Network partition heals and all non-faulty members converge or halt on digest mismatch.
- [x] State digest mismatch is detected and halts the member in tests.

## Milestone 16A: state convergence and catch-up certification

Objective: prove that any honest federation member that was offline, restarted, or lagging can replay consensus history and converge to the same consensus-visible mint state as the rest of the federation, while refusing unsafe service until catch-up is certified.

Implementation checklist:

- [x] Define the exact consensus-visible database surface: tables and columns included in federation state equality.
- [x] Define the exact local-only database surface: private config, peer connection state, sequence trackers, logs, caches, local metrics, process metadata, and other fields excluded from equality.
- [x] Implement a canonical state snapshot/export over consensus-visible rows.
- [x] Implement a state digest over the canonical snapshot.
- [ ] Store state digest history at consensus session boundaries. Status: signed session outcomes include digests and current digests are reported; no dedicated digest-history table or retention policy was confirmed.
- [x] Store last applied session, item index, operation hash, outcome hash, and state digest.
- [x] Implement an admin command to compare state digests across members.
- [x] Implement an optional admin command to diff consensus-visible rows when digests differ.
- [x] Require catch-up data to include quorum-signed session outcomes or equivalent authenticated evidence.
- [x] Reject catch-up ranges with missing, duplicated, reordered, tampered, or conflicting entries.
- [x] Replay accepted operations deterministically from the last durable local height.
- [x] Recompute state digest after replay and compare it to quorum-reported digest.
- [x] Put lagging members into safe catch-up mode.
- [x] In catch-up mode, reject signing shares, melt payment attempts, and authoritative wallet reads.
- [x] Exit catch-up mode only after local height and state digest match quorum evidence.
- [x] Halt or quarantine the member if local replay produces a digest mismatch.
- [x] Regenerate deterministic signature shares from accepted operations if local share rows are missing.
- [x] Ensure fake-wallet outgoing payment idempotency prevents duplicate payments after catch-up or restart.

Tests and success criteria:

- [x] Member offline for 10 accepted operations catches up and reaches the same state digest.
- [ ] Member offline for 100 or more accepted operations catches up and reaches the same state digest. Status: long-partition tooling exists; no explicit 100+ operation test was confirmed.
- [x] Member offline across minting catches up and can return the correct deterministic mint share.
- [x] Member offline across swapping catches up to spent input state and rejects conflicting swaps.
- [x] Member offline across melting catches up without attempting a duplicate outgoing payment.
- [x] Member offline across pending-to-paid melt finalization catches up to paid state.
- [x] Member offline across pending-to-failed melt finalization catches up to rolled-back state.
- [x] Member offline across payment-observation conflict or quarantine catches up to the quarantined state.
- [x] Restart after journal append but before apply replays cleanly.
- [x] Restart after apply but before HTTP response returns cached or regenerated outcome.
- [x] Restart after signing share but before storing share regenerates the same share.
- [x] Lagging member refuses mint, swap, melt, restore, and check-state authority until caught up.
- [x] Catch-up rejects tampered operation IDs.
- [x] Catch-up rejects reordered ranges.
- [x] Catch-up rejects missing entries.
- [x] Catch-up rejects duplicate entries.
- [x] Catch-up rejects ranges signed by insufficient quorum.
- [x] Catch-up rejects ranges signed by unknown members.
- [x] Catch-up halts on state digest mismatch.
- [x] SQLite and Postgres produce the same state digest from the same accepted operation log.
- [x] Optional row-level diff shows the exact consensus-visible row mismatch when digest comparison fails.
- [x] Generated regtest stops one member, runs many mint, swap, melt, restore, check-state, payment-observation, and quarantine operations, restarts the member, waits for catch-up, and verifies height plus state digest equality across all members.

## Milestone 17: `cdk-mintd` integration and regtest tooling

Objective: make the federation easy to run locally.

Implementation checklist:

- [x] Add `federation` section to mintd config.
- [x] Add member-specific config file support.
- [x] Add `just fed-regtest` or similar target that starts N fake-wallet mints and a test wallet.
- [x] Add health commands: member status, consensus height, quote status across members, state digest comparison.
- [x] Add log labels for federation ID, member ID, operation hash, and session.
- [x] Add a fixture generator for 4-member and 5-member federations.
- [x] Ensure no generated config intended for examples contains private production material.

Local multi-mint federation harness:

- [x] Generate per-member local configs containing member ID, public wallet API URL, private federation REST URL, DB path, fake-wallet backend settings, federation public config path, and local private member config path.
- [x] Assign deterministic localhost ports for every member, with separate public wallet API ports and private federation REST ports.
- [x] Create isolated per-member data directories so databases, logs, and configs cannot overlap.
- [x] Add `just fed-regtest` to launch 4-member and 5-member fake-wallet federations as separate `cdk-mintd` processes.
- [x] Add `just fed-regtest-status` to show public URL, private federation URL, process status, consensus height, last state digest, lag, and member health.
- [x] Add `just fed-regtest-stop-member MEMBER_ID` and `just fed-regtest-start-member MEMBER_ID` to test threshold liveness, reconnect, and catch-up.
- [x] Add `just fed-regtest-clean` to remove generated local configs, DBs, logs, and process state.
- [x] Add a local wallet federation config, invite file, or test connector config that points to the federation roster instead of a single mint URL.
- [x] Make local logs identify federation ID, member ID, public port, private federation port, consensus height, operation hash, and DB path.
- [x] Document how to run the local federation manually without `just`, including the exact `cdk-mintd` command shape for one member.
- [x] Keep generated local private configs out of examples and Git; commit only templates, deterministic non-secret fixtures, and safe documentation.

Tests and success criteria:

- [ ] `just fed-regtest` starts a 5-member fake-wallet federation. Status: target and generated smoke script exist, but this was not re-run during the audit.
- [ ] A CDK wallet can mint, swap, restore, and check state against it. Status: connector and local smoke coverage exist, but this full regtest was not re-run during the audit.
- [ ] A CDK wallet can melt through the fake-wallet federation and receive aggregated change signatures when change is expected. Status: code/tests exist, but this full regtest was not re-run during the audit.
- [ ] Stopping one member still allows operations if threshold and consensus quorum allow. Status: stop/start tooling and catch-up tests exist, but this exact regtest was not re-run during the audit.
- [ ] Restarting the stopped member makes it reconnect, catch up from peers, and reach the same state digest. Status: catch-up tests and tooling exist, but this exact regtest was not re-run during the audit.
- [ ] State digest comparison reports all members equal after each test. Status: digest comparison command exists, but this exact regtest was not re-run during the audit.
- [x] Local private REST peer authentication rejects unknown members, wrong federation IDs, wrong claimed member IDs, and invalid signatures on localhost.
- [x] Local status commands clearly show connected, offline, lagging, catching-up, and healthy members.
- [ ] `just fed-regtest-clean` leaves no running member processes and removes generated local state. Status: clean target exists, but it was not re-run during the audit.

## Milestone 18: storage backend coverage

Objective: support the same storage expectations as CDK does today.

Implementation checklist:

- [x] Implement federation DB traits for SQLite first.
- [x] Implement Postgres after SQLite semantics are stable.
- [ ] Decide whether Redb is wallet-only and therefore out of scope for mint federation. Status: no explicit decision note was found.
- [x] Keep migrations additive and compatible with standalone mode.
- [ ] Add SQL lock ordering rules for all conflict keys to avoid deadlocks. Status: conflict keys and concurrency tests exist; a documented SQL lock-ordering rule set was not confirmed.
- [x] Add database indexes for operation hash, quote ID, proof Y, blinded message, session index, and member ID. Status: federation journal/payment/session indexes exist; exact index coverage should still be reviewed alongside the missing signature-share table.

Tests and success criteria:

- [ ] SQLite unit and integration tests pass. Status: not re-run during this audit.
- [ ] Postgres integration tests pass when a Postgres instance is available. Status: not re-run during this audit.
- [x] Concurrent request stress tests do not deadlock.
- [ ] Query plans for hot paths use indexes. Status: indexes exist, but no query-plan verification was confirmed.

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
- [x] Add admin tooling to inspect unresolved payment observations.

Tests and success criteria:

- [x] Fake wallet policy and real backend policy are separate code paths.
- [x] A single malicious observation cannot mark a quote paid.
- [x] Conflicting payment amounts do not silently update quote state.
- [x] Stale pending quotes are visible to operators and do not block consensus.

## Milestone 20: distributed key generation

Objective: replace trusted-dealer setup with dealer-free DKG.

Implementation checklist:

- [ ] Port or adapt Pedersen DKG for BLS G2, using Fedimint's DKG as a reference. Status: DKG setup phases and share aggregation scaffold exist, but a complete production-ready dealer-free Pedersen DKG replacement is not yet confirmed.
- [x] Run DKG over authenticated P2P during federation setup, accepting DKG messages only from identity public keys in the federation roster. Status: implemented over authenticated private REST setup routes.
- [x] Include federation ID, member roster, member identity public keys, participant order, commitments, BLS public shares, and aggregate public keys in the DKG transcript.
- [x] Require each participant to sign the final DKG transcript with its federation identity key.
- [x] Commit hash of polynomial commitments before revealing commitments.
- [x] Verify secret shares against public commitments.
- [x] Derive public shares and aggregate public keys deterministically.
- [x] Persist DKG transcript for audit and recovery.
- [x] Persist the final roster hash and DKG transcript hash in each member config.
- [x] Support key rotation through consensus as a federation operation.
- [ ] Keep trusted-dealer mode for tests only. Status: trusted-dealer setup remains the primary local generator path.

Tests and success criteria:

- [x] DKG produces keys equivalent in behavior to trusted-dealer keygen.
- [x] DKG rejects a participant whose identity public key is not in the configured roster.
- [x] DKG rejects a transcript where a BLS public share is reassigned to another member ID.
- [x] DKG transcript signatures verify for every member identity public key.
- [x] Invalid share is detected and setup fails.
- [x] Restart during setup can resume or safely restart.
- [x] Key rotation creates a new aggregate keyset accepted by all members.
- [x] Old keysets remain verifiable until final expiry.

## Milestone 21: key rotation and federation upgrades

Objective: keep long-running federations maintainable.

Implementation checklist:

- [x] Add consensus operation `RotateKeyset`. Status: implemented as `KeysetRotation`.
- [x] New aggregate keysets become active only after consensus acceptance.
- [ ] Each member proves possession of the corresponding secret share before activation. Status: key-share validation exists, but an explicit rotation-time proof-of-possession gate was not confirmed.
- [x] Wallet sees NUT-02 keyset changes consistently across members.
- [ ] Add protocol version negotiation in federation metadata. Status: no full negotiation mechanism confirmed.
- [ ] Add migration plan for changing operation encoding versions. Status: no dedicated migration plan confirmed.
- [x] Add member add/remove as a later, explicit governance operation. Do not include in MVP.

Tests and success criteria:

- [x] Wallet can mint with new active keyset after rotation.
- [x] Old proofs remain spendable.
- [ ] Members with stale config refuse to join upgraded consensus. Status: config mismatch validation exists; stale-upgrade consensus tests were not confirmed.
- [ ] Mixed version tests reject unsupported major versions cleanly. Status: version fields and some version checks exist; mixed-version federation tests remain open.

## Milestone 22: observability and operator safety

Objective: make failures diagnosable without exposing private user data.

Implementation checklist:

- [ ] Add metrics for consensus height, pending mempool items, accepted operations, rejected operations, signing share latency, aggregation failures, state digest mismatches, and peer connectivity. Status: admin/status reporting exists; comprehensive metrics coverage was not confirmed.
- [x] Add structured logs with short operation hashes, not full secrets or PII.
- [x] Add admin command to compare state digests across members.
- [x] Add admin command to list pending payment observations by quote ID.
- [x] Add alert when a member is behind by more than configured sessions.
- [ ] Add alert when too many invalid operations are submitted by one client IP or auth token. Status: not confirmed.
- [x] Redact proof secrets, witnesses, invoices, local paths, and private keys from logs.

Tests and success criteria:

- [x] Log snapshot tests verify redaction of secrets and private config.
- [ ] Metrics are emitted in fake regtest. Status: not confirmed.
- [x] Digest mismatch alert fires in an injected-divergence test.

## Milestone 23: security review checklist

Objective: make the design reviewable before mainnet use.

Implementation checklist:

- [ ] Review threshold choice and BFT assumptions. Status: defaults and validation are implemented; formal review remains open.
- [ ] Review operation canonicalization and transcript binding. Status: canonical JSON hashing exists; binary encoding and dedicated SIG_ALL transcript review remain open.
- [ ] Review all paths that can call `blind_sign` or `blind_sign_share`. Status: no final review checklist/signoff found.
- [ ] Ensure no public endpoint can bypass consensus in federation mode. Status: public handlers are wired through federation paths, but final security review remains open.
- [ ] Review NUT-20 signature verification for federated batch issuance. Status: federated NUT-20 coverage not confirmed.
- [ ] Review P2PK, HTLC, and SIG_ALL transcript construction. Status: federated transcript tests remain open.
- [x] Review real payment observation policies.
- [ ] Review DB conflict keys and indexes. Status: conflict keys and indexes exist, but missing signature-share table and query-plan review remain open.
- [x] Review member identity authentication.
- [ ] Review denial-of-service surfaces: huge witnesses, huge batches, repeated invalid ops, slow consensus, and restore spam. Status: size limits exist; full DoS review remains open.

Tests and success criteria:

- [ ] Independent reviewer can trace every signature share to an accepted operation hash. Status: operation IDs are tied to share generation, but lack of a separate signature-share table means this needs final review.
- [ ] Fuzzers cover operation decoding and request conversion. Status: no federation operation fuzz target confirmed.
- [ ] Attack regression suite passes. Status: attack tests exist, but the full suite was not re-run during this audit.
- [ ] No known route signs directly in federation mode. Status: code paths appear consensus-gated, but final security-review signoff remains open.

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
15. Add state convergence, catch-up certification, and database-equivalence gates.
16. Add network chaos, catch-up, and payment-observation recovery tests.
17. Add real payment observation policy and DKG.
