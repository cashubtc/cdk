# Federated Cashu with CDK: implementation plan

This is a working checklist for adding federated Cashu mint support to CDK while keeping the public Cashu wallet-to-mint protocol as close as possible to today's NUT endpoints. The plan assumes BLS blind signatures are the federation signing scheme, with locally generated key shares first and real DKG later.

## Grand vision

The grand vision of this project is to evolve Cashu from a single-operator Chaumian eCash mint into a federated, threshold-secured eCash system where multiple mint members can run on independent servers, coordinate all state transitions through consensus, and jointly issue, swap, and melt Cashu tokens using BLS threshold blind signatures. Federation mode should preserve the existing Cashu wallet-to-mint protocol as much as possible while giving users stronger security and fault tolerance: no single operator should be able to inflate, censor, or unilaterally corrupt mint state, and the federation should continue operating when some members are offline as long as the signing and consensus thresholds are met. Every safety-critical mint operation must be serialized by the federation, stored in a shared consensus journal, replayable after outages, and applied deterministically so all honest members converge on the same database state. Wallets must be able to talk to multiple members, verify and aggregate threshold signature shares into ordinary Cashu proofs, and reject inconsistent federation metadata or bad shares. The system should start with static trusted-dealer key generation, fakewallet as a configured development payment backend, private REST networking, and locally testable consensus scaffolding, then grow toward durable storage, catch-up, member authentication, DKG, key rotation, and full mint/swap/melt integration. Federation is explicitly v3/BLS-only, standalone CDK behavior must remain unchanged, and the implementation should favor simple CDK-native designs while borrowing Fedimint's core safety lessons around consensus-before-signing, authenticated peers, deterministic state machines, and robust recovery.

## Scope

- Federate CDK mint issuance, swap, restore, check-state, and eventually melt flows.
- Let a federation-aware CDK wallet talk to multiple mint members and aggregate BLS blind signature shares into normal Cashu proofs.
- Serialize all safety-critical mint state transitions through consensus before any member returns a usable signature share.
- Keep existing standalone mint and wallet behavior unchanged unless federation mode is explicitly enabled.
- Start with fakewallet as a normal configured CDK payment backend for local development and trusted-dealer key shares, then add payment observation quorum and real DKG.

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

## Blocking bugs

If a bug or design issue blocks safe progress, record it here before continuing milestone work. Blocking bugs must be fixed before dependent milestones continue.

A blocker is only resolved when the fix is tested and the code is clean: no debug scaffolding, temporary naming, fake shortcuts, relaxed validation, one-off tools, extra logs, generated artifacts, or other traces of the workaround remain in production paths.

Current blocking bugs:

- None recorded.

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

Last code-confirmed audit: 2026-07-06. This checklist marks items as done only when the repository contains a concrete implementation or a concrete checked-in test for that item. Broad "test suite passes" checkboxes remain open unless the suite was run during the audit. Some completed items are implemented with CDK-native names and modules rather than the exact names originally proposed here; those are marked done when the safety property is present, with notes where the original mechanism is still missing.

Current confirmed shape:

- Core implemented areas: threshold BLS shares, static federation config, trusted-dealer setup, v3/BLS keyset enforcement, canonical operation hashes and request transcripts, in-memory consensus boundary, private REST peer envelopes, signed REST submit-operation consensus admission, route-level quote/payment/quarantine admission plus mint/swap/melt/restore/keyset-rotation admission before public append, private sequence persistence, or non-primary forwarding, private REST route-kind rejection before catch-up sequence persistence, private REST submit-response validation that binds top-level operation IDs to embedded journal outcomes, private REST AlephBFT finalized-unit acknowledgement validation that applies the same embedded-outcome binding, signed AlephBFT finalized-unit delivery over private REST with sender/unit-submitter binding, safe AlephBFT proposal preview/exact-drain handoff primitives, quorum-gated Axum finalized-unit broadcast/drain helper, invalid-ack rejection before quorum accounting, bounded AlephBFT driver tick/loop shell wired into router startup for `aleph_bft`, bounded remote finalized-unit delivery with quorum-stop behavior, AlephBFT data-provider empty-queue liveness coverage, AlephBFT finalized-unit empty-payload rejection, AlephBFT state-layer queue-and-wait submit path, AlephBFT public-submission fail-closed readiness while real finality is unavailable, public mint/swap/melt AlephBFT not-ready route coverage that proves halted routes do not queue work or register waiters, AlephBFT startup restore that preserves the unavailable halt over stale persisted healthy snapshots, typed retryable AlephBFT pending-finalization timeouts, public batch-mint retry after AlephBFT pending-finalization timeout returning the persisted accepted response without issuing twice, live duplicate public batch-mint requests joining one pending AlephBFT queue item and returning one accepted persisted response, AlephBFT queued submit-operation sequence handoff, configured AlephBFT pending-mempool admission backpressure with retained-queue finalization coverage, AlephBFT pending and drained-unit semantic conflict-key rejection for competing mint/melt quote operations before finality, item-level release of drained AlephBFT reservations when peer finality decides duplicate or conflicting items, bounded AlephBFT response-waiter accounting and timeout cleanup, duplicate pending-operation waiter joining, bounded accepted-response cache retention, durable-journal duplicate responses after response-cache eviction, lost waiter notification state, or post-admission acceptance races, retained AlephBFT finalized-item recovery validation for accepted unit/item order, gap-free unit finalization order, forward-only session order, unit body hashes, and item indexes, signed local-status exposure of AlephBFT adapter counters, expanded mintd AlephBFT status-line fields, structured sync lag/catch-up status, signed session-outcome exact-range quorum status, catch-up response-version rejection at the replay boundary, pending response waiters, response-cache size, response-cache hit counters, response-cache hit-rate status, duplicate-submission counters, federation journal persistence, indexed accepted-entry lookup by operation ID, dedicated federation signature-share and retained state-digest checkpoint persistence, durable AlephBFT finalized-item persistence, startup reload, and controlled finalized-unit application, operation-hash/output-index signature-share restore provenance, accepted-index and consensus-evidence signature-share provenance for mint, swap, restore, and melt change, wallet-facing public mint and swap responses read from persisted accepted signature-share rows in request-output order, accepted mint apply materialization rows for quote issuance, blinded secrets, completed operation response, blind signatures, and member-share provenance, accepted swap apply materialization rows for spent input proofs, proof/output operation linkage, completed operation response, blind signatures, and member-share provenance, swap malformed-proof, bad-proof-signature, duplicate-input, duplicate-output, P2PK-missing-witness, and unbalanced-output pre-state-change rejection before proof state, completed-operation, blind-signature, or federation-share persistence, melt bad-proof-signature, duplicate-input, P2PK-missing-witness, non-v3-change, and unbalanced-input rejection before proof state, saga, change-signature, federation-share, quote-state, or outgoing-payment side effects, accepted melt reserve/payment-attempt ordering that proves pending proof state, pending quote state, `PaymentAttempted` saga state, and accepted-operation provenance exist before the outgoing payment backend is called, live fake-wallet happy-path melt coverage that proves paid public melts spend inputs, persist change shares, clear saga state, and record a terminal melt payment observation on quote-status polling, accepted mint-output restore reads persisted signature shares by original operation/output index, exact duplicate accepted public mint/swap/melt requests return the same persisted response without appending another operation even after local response-cache eviction, duplicate failed public fake-wallet melt requests return `PaymentFailed` without appending another operation or attempting a second outgoing payment, public mint/swap/melt duplicate responses survive SQLite restart after runtime replay and in-memory catch-up replay, duplicate failed public fake-wallet melt retries also survive SQLite restart and in-memory catch-up without another payment attempt, explicit pure mint/swap/melt/restore/keyset-rotation and quote/payment-observation admission helpers, a single accepted-operation apply boundary used by journal replay/live finalization/catch-up, terminal-status guards for local rejected/failed journal entries, member-mode guards that reject direct standalone mint/swap/melt/raw-signing API paths, unpaid-quote, expired-quote, duplicate-output, and quote/output amount-mismatch pre-signing rejection for federated mint apply, mint/swap/melt/restore/check-state apply paths, deterministic single-member production melt payment attempts, wallet fanout and aggregation, wallet retry handling for pending/rate-limited members, mixed final/pending/rejected wallet fanout tolerance for mint/swap/melt, valid-subset wallet share aggregation that ignores bad shares when threshold remains reachable, wallet compatible-member keyset filtering before signing/state fanout, wallet-side non-v3 output rejection before member fanout for mint/batch mint/swap/restore/melt change, payment observation policy/quarantine, local regtest tooling, DKG setup-route scaffold, keyset rotation, digest comparison, and several catch-up/recovery tests.
- Partial or intentionally deferred areas: production BFT ordering, real AlephBFT finality replacing the private-REST finalized-unit broadcast stand-in, Iroh/NAT traversal, future deterministic binary operation encoding if a later operation version needs it, a separate `cdk-federation` crate, full dealer-free DKG replacing trusted-dealer setup, dynamic membership, protocol-version governance, comprehensive fuzzing, and a final mainnet security-review checklist.
- Test-suite status: focused federation replay, peer-sync, peer reconnect backoff, remote pubsub reconnect backoff, peer evidence/backoff, mintd peer-sync config wiring, SQLite federation storage/query plans, SQLite federation operation-ID journal lookup and indexed lookup query-plan coverage, SQLite empty federation migration application, live Postgres federation migration/storage coverage, feature-disabled SQLite legacy migration, standalone mint issue/swap regressions, standalone builder keyset-version regression, standalone DbSignatory regressions, session-outcome, trusted-dealer setup, trusted-dealer fakewallet regtest opt-in, deterministic accepted-time mint quote replay, operation balance audit, source- and manifest-enforced fakewallet coupling audits, deterministic trusted-dealer fixture generation, no-fake `cdk-mintd`, mintd startup-policy, payment-observation policy serialization, backend-agnostic payment-observation, payment-observation metadata/idempotence, backend-agnostic production payment-path audits, deterministic single-member production melt payment actor tests, mint storage hot-path index query-plan tests, always-on mint payment observer, durable state-digest checkpoint persistence and retention, backend-agnostic signature-share persistence, operation-hash/output-index restore provenance, accepted-index and consensus-evidence signature-share provenance regressions, accepted mint and swap apply materialization row regressions, federated swap malformed-proof, bad-proof-signature, duplicate-input/output, P2PK-missing-witness, and unbalanced-output no-side-effect regressions, federated melt bad-proof-signature, duplicate-input, P2PK-missing-witness, non-v3-change, and unbalanced-input no-side-effect regressions, accepted mint-output restore provenance regression, persisted public mint/swap share-response regressions, exact duplicate and after-cache-eviction public mint/swap/melt response regressions, public mint/swap/melt SQLite-restart duplicate response regressions, public mint/swap/melt catch-up duplicate response regressions, public mint/swap/melt AlephBFT not-ready fail-closed route regressions, public batch-mint AlephBFT pending-timeout retry and live pending duplicate-join no-double-issue regressions, federated mint expired-quote, duplicate-output, and amount-mismatch pre-signing no-side-effect regressions, public failed-melt duplicate no-second-payment regression, public failed-melt SQLite-restart and catch-up no-second-payment regressions, fakewallet coupling audit, wallet member request timeout/retry policy tests, wallet rate-limit retry regression, wallet mixed final/pending/rejected mint/swap/melt response regression, wallet slow/offline/bad-share 3-of-5 mint/swap/melt response regression, wallet compatible-member fanout filtering, wallet non-v3 mint/batch mint/swap/restore/melt-change output rejection before fanout, federation invite-code discovery tests, wallet quote-visibility and matching mint-quote status quorum tests, consensus test-module split coverage, canonical request transcript tests for NUT-20 and SIG_ALL, pure mint/swap/melt and quote/payment-observation admission helper regressions, route-level quote/payment admission regressions, route-level all-operation admission regressions, accepted-operation apply-boundary regressions, direct member-mode standalone-path guard regressions, public Axum route consensus-gating source audits, federated NUT-20 single and batch issuance tests, federated unpaid mint quote no-signing regression, P2PK/HTLC/SIG_ALL canonical swap transcript tests, NUT-19 retry backoff, federation protocol-version metadata, mixed-version runtime and wallet rejection, stale-upgrade consensus item and catch-up rejection, catch-up response-version replay rejection, signed catch-up bad-signature and truncated-range rejection, private REST catch-up wrong-kind sequence rejection, 100+ operation offline catch-up, explicit 4-of-7 threshold BLS aggregation, seeded randomized threshold BLS property coverage, witness-bound operation hashing, v1 canonical operation golden vectors, operation-decoding version rejection, operation-decoding fuzz target compilation, federated swap input-reuse attack regression, AlephBFT proposal preview/exact-drain regressions, AlephBFT empty-queue data-provider liveness regression, AlephBFT empty-finalized-unit rejection regression, private REST AlephBFT empty-finalized-unit route regression, Axum AlephBFT broadcast/drain and invalid-ack quorum regressions, signed AlephBFT finalized-unit sender-binding regression, bounded AlephBFT driver tick and startup-decision regressions, bounded remote finalized-unit delivery regressions, AlephBFT public-not-ready startup regressions, AlephBFT stale startup-status restore regressions, AlephBFT queue-and-wait submit regressions, retryable pending-finalization timeout regressions, AlephBFT queued sequence handoff regressions, AlephBFT pending-mempool backpressure and retained-finalization regressions, AlephBFT response-waiter backpressure and timeout cleanup regressions, duplicate pending-operation waiter join regressions, AlephBFT duplicate-after-cache-eviction, lost-waiter, and post-admission-race durable-journal response regressions, AlephBFT retained-finalized-item recovery body-hash/order/gap/session regressions, accepted-response cache retention regressions, private REST submit-response embedded-outcome mismatch regressions, private REST AlephBFT finalized-unit embedded-outcome mismatch regressions, response-cache hit-rate and duplicate-submission status regressions, structured sync lag status regressions, signed session-outcome range status regressions, AlephBFT local-status adapter counter regressions, expanded mintd AlephBFT status-line regression, mintd AlephBFT status-line regressions, and `cdk-axum` federation tests were run for recent checkpoints. Focused `cdk-axum` persisted public share tests, exact duplicate public mint/swap/melt response tests, public mint/swap/melt after-cache-eviction duplicate response tests, public mint/swap/melt SQLite-restart duplicate response tests, public mint/swap/melt catch-up duplicate response tests, public mint/swap/melt AlephBFT not-ready fail-closed route tests, public batch-mint AlephBFT pending-timeout retry and live pending duplicate-join tests, public failed-melt duplicate no-second-payment tests, public failed-melt SQLite-restart and catch-up no-second-payment tests, source/readiness tests, the non-primary forwarding fixture test, and `cargo check -p cdk{,-axum} --features federation` passed on 2026-07-06. The focused `cdk` melt admission filter `apply_federation_melt_rejects_`, swap admission filter `apply_federation_swap_rejects_`, accepted swap materialization filter `apply_federation_swap_persists_accepted_materialization_rows`, accepted mint materialization filter `apply_federation_mint_persists_accepted_materialization_rows`, and federated mint admission filters `apply_federation_mint_rejects_expired_quote_before_signing`, `apply_federation_mint_rejects_duplicate_outputs_before_signing`, and `apply_federation_batch_mint_rejects_quote_amount_mismatch_before_signing` passed on 2026-07-06. The focused `cdk-axum` filters `duplicate_failed_federation_melt`, `process_failed_melt_request_releases_inputs_without_change_shares`, `duplicate_federation_melt`, and `process_melt_request`, plus `cargo test -p cdk --features federation failed_melt -- --test-threads 1`, passed on 2026-07-06 after the failed-melt restart/catch-up retry fix. The focused `cdk-common` signed catch-up rejection filter `cargo test -p cdk-common --features federation catch_up_response_rejects -- --test-threads 1` passed on 2026-07-06. The focused `cdk` restore provenance test `cargo test -p cdk --features federation restore_returns_persisted_accepted_mint_shares_by_original_output_index -- --test-threads 1` passed on 2026-07-06. The broad feature-disabled library gate `cargo test --lib --workspace --exclude cdk-postgres` passed on 2026-07-05, and the broad feature-enabled compile gate `cargo check --workspace --all-targets --exclude cdk-postgres --features federation` passed on 2026-07-05.
- Latest checkpoint: Offline/catch-up melt payment-observation replay now proves a non-elected member can reserve an accepted melt without paying, apply a quorum of paid observations, spend inputs, sign change, clear saga state, return the persisted final melt/change response, and never call the outgoing payment backend.
- Focused checkpoint tests: `cargo test -p cdk --features federation catch_up_melt_payment_observations_finalize_without_duplicate_payment -- --test-threads 1` passed on 2026-07-06.

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
- `cdk-mintd` federation config and commands: create local trusted-dealer configs, run member nodes, expose status, and start fakewallet-backed development federation tests.

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
- The wallet submits proofs and a melt quote. The federation serializes reserve, development-backend payment observation, finalization, and optional change signing.
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

Checkpoint 8: development-backend payment observations.

- Script fake incoming and outgoing payment outcomes as deterministic observations that can be proposed into consensus.
- Success criteria: success, failure, pending, delayed, duplicate, and conflicting observations are all serialized and produce the expected quote state.

Checkpoint 9: federated mint issuance.

- Enable mint quote creation, paid observation, accepted output set, share production, response caching, and wallet aggregation.
- Success criteria: one end-to-end fakewallet-backed development mint succeeds; conflicting mint requests for the same quote fail; one offline member catches up and later serves the cached or regenerated share.

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
- Failed payment: inputs are released or moved to the configured failed state consistently across all members, no payment success is reported, and exact duplicate failed melt retries do not attempt another outgoing payment.
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

- [x] Re-run `cargo test --lib --workspace --exclude cdk-postgres` with the feature disabled and record the result. Passed on 2026-07-05 after the NUT-10 test helper was made keyset-version aware.
- [x] Re-run a feature-enabled workspace compile/check and confirm standalone behavior remains unchanged. `cargo check --workspace --all-targets --exclude cdk-postgres --features federation` passed on 2026-07-05 after fixing the `melt-token` example secp256k1 signing key import and stabilizing `Transport::resolve_dns_txt` across feature combinations.
- [x] Add a dedicated review checklist document or PR template for future federation PRs: determinism, consensus-before-signing, operation hashing, state digest, attack tests.

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
- [x] Add zeroization for secret shares where practical.

Tests and success criteria:

- [x] Unit test 1-of-1 threshold produces the same final signature as the existing standalone BLS signer.
- [x] Unit test 3-of-5 aggregate signatures verify under aggregate public keys.
- [x] Unit test multi-threshold and order-permutation aggregation, including a 4-of-6 case.
- [x] Add an explicit 4-of-7 aggregate signature test if that exact threshold/member count remains a release criterion.
- [x] Unit test any valid threshold subset aggregates to the same final signature.
- [x] Unit test insufficient shares cannot aggregate.
- [x] Unit test duplicate signer IDs are rejected.
- [x] Unit test invalid share fails verification before aggregation.
- [x] Unit test share/order permutations produce identical aggregate signatures.
- [x] Add deterministic multi-case tests for messages, thresholds, and signer subsets.
- [x] Add randomized/property tests for random messages, thresholds, and signer subsets.
- [x] Existing NUT-00 BLS proof verification passes for aggregate proofs.

## Milestone 2: federation configuration and trusted-dealer setup

Objective: create deterministic federation configs and local key shares for test federations.

Implementation checklist:

- [x] Add `FederationConfig` with federation ID, member set, consensus params, signature threshold, keyset metadata, supported units, and protocol versions.
- [x] Add public member config data with member ID, public API URL, private federation REST URL, federation identity public key, and public BLS shares.
- [ ] Extend public member config with P2P endpoint and optional TLS/Noise/Iroh identity fields when non-REST transports are added.
- [x] Add private member config data with member ID, federation identity private key, local secret BLS shares, and config-file/runtime wiring.
- [ ] Extend private member config with P2P private key, payment backend config, and DB path fields if those remain member-private rather than mintd runtime config.
- [x] Represent the federation roster deterministically in `FederationConfig`, mapping member IDs to identity public keys, endpoint data, and BLS public shares.
- [ ] Add a dedicated `FederationRoster` wrapper only if future code needs a stronger type boundary than the ordered maps in `FederationConfig`.
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
- [x] Trusted-dealer tests cover deterministic IDs and validation for generated federation material.
- [x] Add a deterministic test-seed mode where a config generated twice matches byte-for-byte except explicitly random fields.

## Milestone 3: canonical operation encoding and hashes

Objective: make every consensus operation uniquely and deterministically identified.

Implementation checklist:

- [x] Define `FederatedMintOperation` enum with versioned variants.
- [x] Add versioned federation operation variants for mint quote, mint quote payment, mint issuance, swap, melt quote, melt quote payment, melt, restore, payment-observation quarantine, and keyset rotation.
- [x] Decide whether to further split melt into explicit reserve/finalize/rollback operation variants; operation version 1 keeps the current `Melt` reserve/execution operation plus accepted `MeltQuotePayment` observations and saga recovery, as documented in `docs/federated-cashu-melt-state-machine.md`.
- [x] Implement deterministic canonical JSON encoding with sorted object keys for consensus operation IDs.
- [x] Decide canonical sorted JSON remains the operation version 1 encoding; a deterministic binary format must use a future operation version if it becomes a protocol requirement.
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
- [x] Define a canonical `RequestTranscript` for NUT-20 and SIG_ALL verification.

Tests and success criteria:

- [x] Unit tests cover canonical operation encoding behavior.
- [x] Commit cross-version golden fixture vectors for canonical operation encoding.
- [x] Equivalent operations produce the same hash across processes.
- [x] Reordered unordered fields still encode canonically.
- [x] Changing one output blinded message changes the operation hash.
- [x] Add an explicit witness-mutation test proving that changing an input witness changes the operation hash.
- [x] Add size-limit tests for oversized consensus payloads.
- [x] Add fuzz coverage proving operation decoding rejects unknown major versions and malformed payloads.

## Milestone 4: consensus engine abstraction

Objective: add an ordered log before integrating with mint business logic.

Implementation checklist:

- [x] Add a consensus abstraction in `cdk-common::federation::consensus` with submit, journal, catch-up, and state-digest boundaries.
- [ ] Extract to a separate `cdk-federation` crate or add exact `await_operation`/`accepted_stream` APIs only if future integration needs that boundary.
- [x] Add `ConsensusItem::{Operation(FederatedMintOperation), ModuleFact(...), SessionSignature(...), Default(...)}`.
- [x] Add an in-memory single-node consensus engine for deterministic unit tests.
- [x] Add an in-memory multi-node test engine that can reorder, duplicate, and delay messages.
- [x] Add AlephBFT backend selector/configuration and an explicit `UnsupportedConsensusBackend` error.
- [ ] Implement the AlephBFT-backed engine for multi-member BFT ordering.
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
- [ ] AlephBFT integration test finalizes sessions for 4 nodes and 5 nodes.
- [x] Consensus halts or rejects when fewer than the required quorum is reachable.

## Milestone 4A: AlephBFT strict-order consensus and idempotent federation APIs

Objective: replace the development consensus path with a production BFT ordering
engine and make federated mint, swap, and melt APIs behave like Fedimint-style
submit-and-await APIs. Public wallet requests must only validate and enqueue
operations. All safety-critical state changes, signature-share creation, proof
spending, quote finalization, payment-attempt state, and response persistence
must happen only after the exact operation envelope comes out of AlephBFT in the
finalized order.

Fedimint lessons to adopt:

- Treat AlephBFT as an atomic broadcast engine, not as mint business logic.
- Submit paths run admission or dry-run validation only; they do not commit
  state and do not produce usable blind-signature shares.
- The ordered consensus stream is the only source of truth for applying
  transactions.
- Exact duplicate submissions are harmless and converge to the same operation
  hash, accepted outcome, and cached response.
- A member can receive a wallet request after it has already processed the same
  operation through consensus; the API must return the persisted result rather
  than trying to process the operation again.
- Signature-share endpoints are lookups of persisted consensus outcomes, never
  fresh local signing decisions.
- Waiting APIs must handle both races: the operation may already be accepted, or
  it may be accepted after the HTTP request starts waiting.
- Signed session outcomes, accepted item logs, and state digests are catch-up
  and replay evidence. A lagging member must not serve signing shares or
  authoritative state until replay has certified convergence.

Architecture checklist:

- [ ] Add a production `AlephBftFederationConsensus` backend behind the existing
  consensus abstraction, using Fedimint's AlephBFT integration as the reference
  for data provider, finalization handler, keychain, network, session outcome,
  crash recovery, and catch-up design.
- [ ] Keep Cashu/CDK simpler than Fedimint where possible: CDK needs strict
  operation ordering and deterministic mint execution, not Fedimint's full module
  system or wallet state-machine stack.
- [x] Define the AlephBFT unit payload as canonical CDK consensus items:
  `Operation(FederationOperation)`, payment observation facts, session
  signatures, default/compatibility items, and future module facts. This is now
  represented by `FederationAlephBftUnitPayload`.
- [x] Ensure every unit payload includes or commits to federation ID, consensus
  item version, operation hash, submitting member, accepted session/index, body
  hash, and protocol-version metadata. The payload includes member/session/unit
  metadata, current federation protocol versions, derived operation IDs, and a
  length-prefixed hash over the ordered canonical item body.
- [x] Reuse the existing signed private REST envelope as the authenticated
  admission and transport envelope where practical, but make it explicitly
  mappable into AlephBFT proposals without changing the canonical operation
  bytes. `FederationVerifiedOperationAdmission` now verifies the signed
  `SubmitOperation` REST envelope, enforces the expected message kind and
  configured sender roster, carries sender sequence metadata, computes the
  operation ID, and wraps the exact operation envelope as a consensus item
  without changing its canonical bytes. The Axum private operation route now
  uses this helper before forwarding or local submission so private REST
  transport and the future AlephBFT proposal path share one operation admission
  boundary.
- [ ] Add peer identity allowlist validation before any AlephBFT message,
  private REST submit, catch-up response, or signed session outcome is accepted.
  The existing signed private REST envelope verifies sender membership against
  `FederationConfig`, and signed session-outcome quorum verification rejects
  unknown signers. The AlephBFT adapter runtime now constructs a roster-bound
  finalization handler from the configured members and rejects finalized units
  whose `submitting_member` is not in the roster before advancing unit indexes,
  marking duplicates, appending journal entries, or recording finalized items.
  Signed submit-operation admission now reuses that private REST roster check
  before producing any consensus item. Signed private REST finalized-unit
  delivery now reuses the same envelope and sequence check before applying an
  externally finalized AlephBFT unit, and also rejects a mismatch between the
  authenticated sender and the unit's `submitting_member` before any finalized
  item, journal, sequence, or mint-state mutation. Private REST submit-operation
  responses are also validated against both the submitted operation ID and the
  embedded accepted journal outcome operation ID before the peer client accepts
  them. AlephBFT finalized-unit acknowledgement responses now apply the same
  embedded-outcome validation to each nested submit response before the peer
  client counts a remote member as having accepted the unit. Future non-REST
  AlephBFT wire messages still need to reuse the same roster check at the
  networking boundary.
- [ ] Add a Fedimint-like data-provider loop that drains a bounded pending queue,
  deduplicates operation hashes locally per session, enforces max byte limits,
  and batches items without busy loops. The synchronous
  `FederationAlephBftUnitDataProvider` primitive now performs bounded,
  byte-limited, duplicate-aware unit construction; non-destructive previews; and
  exact-match draining so transient broadcast failures do not force a driver to
  consume the queue before it has actually finalized or durably handed off the
  proposal. Router-created AlephBFT federation state now starts the bounded CDK
  driver loop, which calls this primitive through the safe preview/broadcast/
  exact-drain path; the remaining production work is replacing the private-REST
  finalized-unit stand-in with real AlephBFT finality.
  `FederationAlephBftConsensusAdapter` now wires the primitive to the CDK-side
  operation submission boundary so tests can verify queued operations become
  canonical unit payloads. The backend adapter runtime can now select the
  AlephBFT adapter, validate roster, threshold, and local-member membership
  before queueing, and expose explicit preview/drain/finalize methods for the
  future AlephBFT task. Axum now has a focused broadcast round that exercises
  that handoff over signed private REST and retains the queued proposal when
  remote acknowledgements do not reach the configured consensus threshold.
- [ ] Add a Fedimint-like finalization handler that converts finalized AlephBFT
  units into an ordered stream consumed by exactly one deterministic CDK apply
  loop. The `FederationAlephBftFinalizationHandler` primitive now verifies
  finalized unit payloads, assigns accepted item indexes, preserves
  session/unit metadata, rejects wrong federation/session/replayed units, and
  flags duplicate operation IDs; the production AlephBFT adapter still needs to
  feed this stream from the live network task. The current
  `FederationAlephBftConsensusAdapter` accepts externally finalized units,
  validates them, appends operation outcomes to the in-memory accepted-operation
  journal, and records finalized item metadata. Axum now exposes a controlled
  finalized-unit apply path that persists the finalized-item delta and newly
  appended journal entries atomically before swapping runtime state, applying
  materialized mint changes, and notifying response waiters. The private REST
  federation router now exposes a signed finalized-unit delivery endpoint that
  feeds this path and atomically stores the peer sequence with accepted
  finalized-unit state. A local driver-facing drain/apply helper now drains a
  previewed local unit and finalizes it only on a cloned runtime, persists the
  finalized items and journal entries first, and swaps runtime state only after
  storage succeeds.
  Generic dispatch-and-finalize calls for the AlephBFT adapter fail closed with
  an external-finalization-required error so local queueing cannot be mistaken
  for BFT acceptance.
- [ ] Store accepted session number, item index, operation hash, operation
  outcome hash, state digest, and signed session outcome in the federation
  journal before considering an item durable. `FederationJournalEntry` now
  stores operation outcome hashes, post-append state digests, and optional
  accepted-item consensus evidence with session, accepted item index, finalized
  unit position, submitting member, and member unit index. SQLite/Postgres
  journal persistence now round-trips those fields. Full signed session-outcome
  quorum linkage remains in the existing session-outcome table; core consensus
  now exposes `apply_catch_up_response_with_session_outcome_quorum` so replay can
  require quorum-signed evidence before appending peer entries. Axum REST peer
  sync now collects matching session outcomes from other peers for the exact
  catch-up range, applies through the quorum-verified replay primitive, and
  persists the accepted range plus evidence in one database transaction before
  reopening readiness. Startup replay now checks persisted contiguous
  session-outcome quorum height after materializing durable entries, blocks
  public value-changing routes when evidence is missing, and restores healthy
  status when quorum evidence already covers the replayed height. A final audit
  remains once the production BFT backend owns durable finalized-item recovery.
- [x] Persist enough AlephBFT unit state and accepted item state to recover after
  crashes without replaying an item twice or losing a finalized item. The
  in-memory adapter now has a tested recovery constructor from retained
  finalized items that rebuilds journal evidence, finalized-unit replay guards,
  duplicate-operation tracking, next item indexes, and next unit-finalization
  indexes while rejecting corrupted retained metadata, split or interleaved
  retained units, non-contiguous unit-finalization order, non-contiguous item
  indexes inside a retained unit, accepted-order item-index mismatches, and
  retained unit body hashes that no longer match the retained item body. Empty
  finalized units are rejected before the finalization handler mutates unit
  replay guards or indexes because they cannot be represented in the retained
  finalized-item log used for recovery. The shared mint database
  trait now persists `FederationAlephBftFinalizedItem` rows, SQLite/Postgres
  migrations create a durable accepted-item table with session/unit and
  operation-ID indexes, SQLite round-trip tests reload the persisted log into
  adapter recovery, and Postgres compiles the same persistence regression.
  Axum startup now loads this finalized-item log when `aleph_bft` is selected,
  rebuilds the recovered journal, compares it against any stored journal rows,
  replays unapplied operations into mint state, and rejects synchronous submits
  with an external-finalization error until the live AlephBFT driver exists.
  Newly applied externally finalized units are persisted through the same
  finalized-item store and journal transaction before local runtime mutation.
- [ ] On startup, rebuild the consensus runtime from durable state, fetch missing
  signed session outcomes or journal ranges from peers, replay unapplied items,
  and only then enable public federation mint/swap/melt/restore/check-state
  routes. The in-memory path already restores persisted journal/session evidence
  and readiness; the AlephBFT path now restores local finalized-item state and
  can durably apply externally finalized units. Router startup now starts the
  bounded AlephBFT driver loop for `aleph_bft` but keeps public value-changing
  submissions halted until real finality is available, and stale persisted
  healthy sync snapshots cannot reopen those routes on restart; remaining
  production work is peer fetch and real AlephBFT finality before public routes
  can be enabled for production use.
- [x] If the AlephBFT backend is configured but unavailable, fail startup or keep
  federation routes in a not-ready state; never silently fall back to local
  signing or single-member processing. `cdk-mintd` rejects unavailable
  `aleph_bft` startup, and Axum-created AlephBFT federation state starts
  `halted`/not-ready so public wallet submissions fail before queue admission.
  Startup restore preserves that backend-imposed halt over stale persisted
  healthy snapshots.
- [ ] Add backpressure and admission limits for pending wallet submissions,
  consensus messages, catch-up ranges, and response waiters. The AlephBFT submit
  path now enforces a configurable `pending_mempool_item_limit` before committing
  a newly accepted operation to the live runtime; full queues fail fast without
  growing the pending set, duplicate pending operation IDs continue to collapse
  onto the existing entry, and competing pending or drained mint/melt quote
  operations with the same semantic conflict key are rejected before finality.
  Drained local units now remain counted as pending finalization until finality
  decides their exact operation ID or semantic conflict key; peer-finalized
  duplicate/conflicting items release only the affected local reservation and
  keep unrelated drained items reserved.
  It also enforces a configurable
  `response_waiter_limit` before swapping new pending operations into the live
  runtime; waiter slots are released on timeout or cancellation, and signed local
  status reports the pending waiter count. Existing coverage also includes size
  limits, private REST inbound/outbound concurrency limits, catch-up page
  limits, bounded driver ticks, and a configurable per-unit finalized-unit
  remote delivery cap that stops after quorum and fails before networking if the
  cap is below the remote quorum requirement. Remaining work: extend the same
  bounded-queue discipline to the live AlephBFT transport once real finality
  replaces the private-REST broadcast stand-in.
- [ ] Expose status for AlephBFT session, round, consensus height, pending item
  count, finalized item count, signed session outcome height, catch-up state,
  member lag, response-cache hit rate, and duplicate submissions. The
  `FederationAlephBftAdapterStatus` snapshot now exposes the adapter-visible
  subset through signed local status and `cdk-mintd federation status`: backend,
  federation, local member, session, next local unit index, next unit
  finalization index, next global item index, queued item count, drained
  pending-finalization item count, finalized item count, accepted operation
  count, latest accepted operation index, and roster-bound member count.
  `cdk-mintd federation status` now prints all of those adapter-visible fields
  instead of only the older session/queue/finalized/accepted subset. Signed
  local status and `cdk-mintd federation status` now also expose
  response-cache lookup/hit counters,
  response-cache hit rate in basis points, duplicate-submission plus duplicate
  pending-waiter join counters, structured sync peer/operation-count fields for
  lagging, catch-up, session-outcome verification, and materialized-state
  mismatch states, and signed
  session-outcome quorum range fields (`from_index`, `latest_index`, operation
  count, and signer set). Full round and production AlephBFT consensus-height
  status still need to be wired once the production AlephBFT task owns those
  concepts.

Mint-side restructuring checklist:

- [x] Create explicit admission validation functions for federated operations:
  `validate_federated_mint_submission`, `validate_federated_swap_submission`,
  `validate_federated_melt_submission`, and equivalent quote/payment
  observation admission checks. Current code provides the mint, swap, melt,
  quote, payment-observation, and quarantine admission helpers.
- [x] Admission validation may read local consensus-visible state to reject
  malformed or obviously invalid requests, but it must not commit state, mark
  proofs pending, mark proofs spent, mutate quotes, attempt payments, or create
  signature shares. Current mint/swap/melt helpers are pure request-local
  checks and are exercised before operation-envelope construction. Quote,
  payment-observation, and quarantine helpers now validate operation-local quote
  IDs, method names, observer roster membership, and value consistency without
  touching quote/proof/payment state.
- [x] Keep final consensus apply stricter than admission validation. Admission
  checks are mempool policy; accepted-order apply is the authoritative validity
  decision against the current ordered state.
- [x] Add a single deterministic accepted-operation entrypoint such as
  `apply_accepted_federation_operation` and require journal replay, live
  finalization, and catch-up to use the same path.
- [x] Audit public Axum routes and `Mint::process_*` federation branches so no
  federated mint, swap, melt, restore, or change-output path can call standalone
  signing or mutation helpers before consensus acceptance. Current coverage
  wires quote/payment/quarantine plus mint/swap/melt/restore/keyset-rotation
  admission through public submit, private submit-with-sequence, and non-primary
  forwarding before append, sequence persistence, or peer forwarding. Public
  Axum mint/swap/melt handlers are source-audited to try federation submission
  before standalone fallback, restore is source-audited to read persisted
  federation shares without direct signing, and member-mode `Mint` guards reject
  direct standalone mint, swap, melt, and raw blind-signing calls.
- [ ] Split or wrap existing standalone mint issuance logic into reusable
  validation and apply helpers where this reduces duplication, while keeping
  standalone non-federation behavior unchanged.
- [ ] Split or wrap existing swap saga logic so federation admission can verify
  proofs, spending conditions, output validity, and balance without creating
  local pending state; accepted consensus apply marks inputs spent and stores
  output signature shares atomically.
- [ ] Split or wrap existing melt saga logic so federation admission validates
  quote/proofs/change outputs without attempting an external payment; accepted
  consensus apply records reserve/payment-attempt state, and accepted payment
  observations drive finalize or rollback.
- [ ] Treat external payment reads and payment attempts as non-deterministic side
  effects outside the pure apply path. Their results must enter consensus as
  signed observations or deterministic payment-attempt/finalization operations.
- [ ] Select the real outgoing melt payment actor deterministically from accepted
  consensus state, and use an idempotency key so duplicate wallet requests,
  retries, crashes, or catch-up never make two outgoing payments.
- [ ] Ensure no blind-signature share is created until the accepted operation
  hash, output index, blinded message, amount, keyset ID, member ID, and
  accepted session/index are known and persisted together. Current coverage
  includes an unpaid-quote regression proving federated mint apply fails before
  storing blind signatures or federation signature-share rows, plus checked
  accepted-share provenance constructors used by mint, swap, and melt-change
  signing paths that reject operation-ID mismatches before persistence. Public
  mint and swap handlers now read wallet-facing responses from those persisted
  rows after accepted apply.
- [x] Make all federation output paths v3/BLS-only: admission rejects non-v3
  output keysets, accepted apply rechecks the same condition, and startup
  rejects non-v3 aggregate federation keysets. Current coverage includes
  mint-side runtime rejection for mint/swap/restore/melt change and wallet-side
  rejection before fanout for mint, batch mint, swap, restore, and melt change.
  Fresh targeted coverage also proves common federation config rejects v1, v2,
  and non-BLS aggregate keysets, Axum public routes reject non-v3 mint, swap,
  restore, and melt-change outputs before consensus work, mintd federation
  startup rejects non-v3/non-BLS aggregate configs, and standalone
  non-federated mint builders still honor legacy keyset-version settings.
- [ ] Add an operation response cache keyed by operation hash and response type
  for mint, swap, melt, restore, and quote state responses that need idempotent
  public API behavior. The local accepted-operation notification cache now
  retains a bounded `response_cache_limit` of accepted responses for idempotent
  waiters and status visibility. Active waiters keep a per-operation response
  copy so cache eviction cannot race a notified request. AlephBFT duplicate
  submits now also fall back to the durable accepted journal after cache
  eviction, returning a duplicate response without requeueing work or adding a
  waiter. Public mint, swap, and melt route coverage now bounds the cache to
  one accepted response, evicts the first operation with a second accepted
  operation, retries the first request, and verifies the wallet-facing response
  is rebuilt from persisted accepted state without appending a third operation.
  Future typed response caches can build on this once separate terminal melt
  result and share-set response types need longer retention than the local
  notification cache.
- [ ] Add a waiter registry keyed by operation hash so duplicate HTTP requests
  join the same pending consensus wait rather than submitting unbounded copies.
  AlephBFT submits now check the existing operation notification registry after
  durable journal duplicate detection but before consensus admission. If the
  same operation is already pending locally, the duplicate request joins the
  existing waiter instead of re-entering queue admission; private REST sequence
  metadata is still retained for the finalized operation.
- [x] Define retryable timeout semantics: if consensus has not finalized before
  the public API timeout, return a clear pending/retryable error without
  changing operation state; retrying the exact request must later return the
  finalized response.
  AlephBFT submits now return a typed `OperationPendingFinalization` federation
  error instead of a generic string timeout. That error maps to Cashu's
  retryable pending error response, remains non-definitive for wallet recovery,
  leaves the pending operation queued, and a later exact retry can join the
  existing operation and receive the finalized response once the accepted unit is
  applied.
- [x] Ensure accepted conflicting operations produce deterministic rejected
  outcomes that are persisted and returned idempotently where appropriate.
  AlephBFT-finalized operations that conflict with an existing journal conflict
  key now append a `Rejected` journal entry, preserve operation-id duplicate
  replay, rebuild from durable entries without reserving the rejected conflict
  key, and return a rejected submit response instead of aborting finalized-unit
  replay.
- [x] Reject terminal rejected or failed submit-operation responses before
  wallet-facing handlers read persisted mint, swap, or melt state. Submit
  responses now expose a shared accepted-status guard that allows only accepted,
  applying, or applied journal entries through public and forwarded submit
  boundaries. Rejected and failed finalized outcomes become a typed
  `RejectedOperation` federation error, while the direct in-memory sequence
  path still records private REST sequence metadata for terminal duplicate
  responses before returning the error.

Federated public API checklist:

- [ ] `post_mint` and `post_batch_mint` compute the canonical mint operation
  hash, check for an accepted/cached response, run admission validation if
  needed, submit the operation to AlephBFT, await the accepted outcome, and
  return this member's persisted signature shares in normal Cashu response
  shape. Current in-memory/public route coverage returns persisted accepted
  rows after submit, verifies the response against the stored share record, and
  proves an exact duplicate accepted request returns the same persisted response
  without appending a second operation, including after local response-cache
  eviction. SQLite restart coverage now rebuilds `MintState`, replays the
  persisted runtime state, retries the same public batch mint request, and
  returns the persisted shares without appending another operation. In-memory
  catch-up coverage now applies peer-accepted quote, payment, and mint
  operations before the public request arrives and returns the persisted shares
  without appending another operation. AlephBFT not-ready coverage now proves
  the public batch mint route fails closed before queueing work, appending a
  journal entry, or registering a waiter while real finality is unavailable.
  Pending and AlephBFT-finality variants remain open.
- [ ] `post_swap` computes the canonical swap operation hash, checks for a
  cached response, validates the request without pending/spent mutations,
  submits to AlephBFT, awaits accepted apply, and returns persisted output
  shares in request-output order. Current in-memory/public route coverage
  returns persisted accepted rows after submit, verifies the response against
  the stored share record, and proves an exact duplicate accepted request
  returns the same persisted response without appending a second operation,
  including after local response-cache eviction. SQLite restart coverage now
  rebuilds `MintState`, replays the persisted runtime state, retries the same
  public swap request, and returns the persisted shares without appending
  another operation. In-memory catch-up coverage now applies a peer-accepted
  swap before the public request arrives and returns the persisted shares
  without appending another operation. AlephBFT not-ready coverage now proves
  the public swap route fails closed before queueing work, appending a journal
  entry, or registering a waiter while real finality is unavailable. Pending and
  AlephBFT-finality variants remain open.
- [ ] `post_melt` computes the canonical melt operation hash, checks for cached
  reserve/final response, validates proofs and change outputs without payment
  side effects, submits to AlephBFT, awaits accepted reserve/finalization
  according to the configured fake or real payment observation policy, and
  returns the same response on duplicate requests. Current in-memory/public
  route coverage proves an exact duplicate accepted melt with change returns the
  same persisted paid response and change signatures without appending a second
  operation, including after local response-cache eviction. SQLite restart
  coverage now rebuilds `MintState`, replays the persisted runtime state,
  retries the same melt request, and returns the persisted paid response plus
  change signatures without appending another operation. In-memory catch-up
  coverage now applies a peer-accepted melt before the public request arrives
  and returns the persisted paid response plus change signatures without
  appending another operation. AlephBFT not-ready coverage now proves the public
  melt route fails closed before queueing work, appending a journal entry,
  registering a waiter, or moving the melt quote out of `Unpaid` while real
  finality is unavailable. Failed fake-wallet coverage now proves an exact
  duplicate failed melt returns `PaymentFailed`, keeps inputs released, appends
  no second operation, and does not attempt a second outgoing payment. SQLite
  restart and in-memory catch-up coverage now prove that exact duplicate failed
  melts remain terminal and do not attempt another outgoing payment after
  recovery. AlephBFT-finality variants remain open.
- [ ] Mint quote creation and melt quote creation remain idempotent around quote
  IDs and quote request payloads; duplicate quote operations return the
  accepted quote response, while conflicting quote payloads are rejected.
  Current route coverage proves AlephBFT-not-ready public mint/melt quote
  routes fail closed before creating local quote state, appending journal
  entries, queueing work, or registering waiters. Consensus-level
  duplicate/conflict coverage exists for quote operations; AlephBFT pending
  queue coverage now rejects competing mint/melt quote operations with the same
  quote conflict key before finality. Public reusable quote-ID duplicate/conflict
  behavior remains open where protocols expose client-selected quote IDs.
- [x] Payment observation submission is idempotent by payment ID, quote ID,
  observer, method, amount, state, and proof material. Operation IDs now
  exclude observer-local `observed_time` metadata for mint and melt payment
  observations, while changed payment proof material still produces a distinct
  conflicting operation ID. Covered by
  `payment_observation_operation_ids_ignore_observed_time`.
- [x] Restore and check-state remain read-only, but they must refuse
  authoritative answers from lagging members and must only return shares or
  states derived from accepted consensus-visible state. Public Axum restore and
  check-state handlers now verify federation sync readiness after auth and
  before local mint reads; the focused lagging-member regression proves both
  routes fail closed while lagging and resume once healthy.
- [x] Public APIs must support the race where the accepted operation arrives
  between admission validation and queue submission: the submit path should then
  return the cached accepted response. Submit responses now recompute the
  embedded journal entry envelope's operation ID before any accepted response is
  returned to wallet-facing mint, swap, or melt handlers. The focused in-memory
  submit regression now validates an operation, accepts it through a competing
  path, then submits the same envelope through the wallet path and proves the
  durable duplicate response is returned without appending or applying a second
  journal entry.
- [x] Public APIs must support the race where the accepted operation arrives
  before the wallet reaches this member: the handler should compute the same
  operation hash and immediately return the persisted response. AlephBFT submit
  coverage now proves this for an already accepted duplicate after the bounded
  notification response cache has evicted the original response. Public mint,
  swap, and melt handler coverage now proves the same persisted wallet-facing
  response behavior after local response-cache eviction. The mint, swap, and
  melt catch-up regressions now explicitly certify the stronger
  accepted-before-request case where peer-accepted operations are replayed
  before the public request reaches this member and the handler returns
  persisted state without a second append.

Wallet checklist:

- [ ] Keep the CDK wallet simple. Do not add a Fedimint-style persistent client
  state-machine stack unless a later requirement proves it necessary.
- [x] The federated connector continues to fan out the same mint, swap, and melt
  request to all configured members, accepts threshold compatible responses,
  validates member shares, aggregates BLS shares, and returns ordinary Cashu
  wallet results. The connector now keeps all successful value-operation
  responses for aggregation, validates each member's full response, ignores bad
  signature shares when a threshold-compatible valid subset remains available,
  and returns normal aggregate `MintResponse`, `SwapResponse`, and melt-change
  responses.
- [x] The connector treats member timeouts and pending/retryable responses as
  normal retry inputs, not as proof construction failures unless threshold
  cannot be reached. Pending quote and pending-finalization errors remain
  non-definitive, and HTTP 429 rate limits are now classified as retryable so a
  temporarily rate-limited member can still contribute a threshold share after a
  configured retry.
- [ ] The connector rejects members advertising mismatched federation IDs,
  mismatched aggregate keysets, mismatched protocol versions, or non-v3/BLS
  aggregate federation keysets before submitting value-changing requests.
  Current code excludes members with mismatched or non-v3 active aggregate
  keysets from mint, batch mint, swap, melt, restore, check-state, and mint-info
  fanout, fails before signing if a threshold compatible subset cannot be
  reached, rejects unsupported public-config protocol versions at connector
  construction, and rejects non-v3 mint, batch mint, swap, restore, and melt
  change outputs before contacting any members. Explicit per-member federation
  ID and protocol-version advertisements still need a Cashu info extension
  before this item can close.
- [x] The connector tolerates a member that already processed the operation and
  another member that is still waiting, as long as a threshold of valid final
  responses can be collected. The wallet regression
  `federated_mint_connector_tolerates_mixed_final_pending_and_rejected_value_responses`
  now covers mint, swap, and melt fanout with one already-final member, one
  retryable pending-finalization member that later returns a final response,
  and one definitive rejecting member.

AlephBFT and consensus unit tests:

- [ ] A 4-member AlephBFT test finalizes ordered sessions with one faulty or
  offline member and all honest members store identical accepted item order.
- [ ] A 5-member AlephBFT test finalizes ordered sessions with one faulty or
  offline member and all honest members store identical accepted item order.
- [ ] Duplicate proposals from multiple members produce at most one applied
  operation outcome and identical idempotent responses.
- [ ] Delayed, reordered, and duplicated AlephBFT network messages do not change
  the final accepted order seen by honest members.
- [ ] Byzantine equivocation or malformed unit data from one member is rejected
  or ignored without causing honest members to diverge.
- [ ] Consensus stalls safely when fewer than the BFT quorum are connected and
  resumes when enough members return.
- [ ] Restart during unit creation, after finalization before apply, and after
  apply before response persistence all recover to the same journal and state
  digest.
- [x] Catch-up from signed session outcomes and journal ranges rejects bad
  signatures, wrong federation IDs, wrong protocol versions, truncated ranges,
  and state-digest mismatches. Covered by
  `catch_up_response_rejects_bad_session_outcome_signature`,
  `catch_up_response_rejects_truncated_signed_session_outcome_range`,
  `catch_up_response_rejects_tampered_session_outcome`,
  `dummy_consensus_rejects_wrong_federation_catch_up_response`,
  `dummy_consensus_rejects_future_catch_up_response_version`,
  `dummy_consensus_rejects_future_operation_version_catch_up_response`,
  `dummy_consensus_rejects_tampered_catch_up_digest_chain`, and
  `rest_peer_client_detects_catch_up_materialized_state_mismatch`.
- [x] The data provider never spins at 100 percent CPU when the pending queue is
  empty, full, or repeatedly receiving duplicate operations. Direct
  data-provider coverage now proves repeated empty polls do not advance unit
  indexes or create phantom work; existing size-limit, duplicate-operation,
  semantic-conflict, pending-mempool backpressure, and idle-driver tests cover
  the full/duplicate/conflicting cases and the Axum tick surface.
- [x] Bounded queues apply backpressure under load and do not drop finalized
  consensus items. The pending-mempool overflow regression now rejects only the
  new operation, keeps the already admitted unit previewable, lets duplicate
  submissions join the existing pending waiter, finalizes the retained unit
  through the bounded driver, and verifies durable finalized-item metadata plus
  the applied journal entry.
- [x] AlephBFT finalized-unit responses and waiters report post-apply terminal
  status, not the pre-apply append status. Covered by
  `federation_aleph_bft_finalized_unit_reports_state_rejected_swap`, which
  finalizes valid, state-conflicting, and later valid swaps in one unit and
  proves the loser response is `Rejected` while valid siblings are `Applied`.

Mint operation tests:

- [ ] Admission validation for mint rejects malformed requests, unpaid quotes,
  expired quotes, amount mismatches, duplicate outputs, NUT-20 failures, and
  non-v3/BLS keysets without writing quote issuance or signature-share rows.
  Current apply-layer coverage proves unpaid quotes, expired quotes, duplicate
  outputs, batch quote/output amount mismatches, and NUT-20 missing/mutated
  signatures fail before quote issuance, blind-signature storage,
  completed-operation storage, or federation share persistence; non-v3 output
  rejection is covered before consensus submission.
- [ ] Accepted mint apply persists quote issuance, blinded messages, member
  signature shares, completed operation response, and state digest atomically.
  Current direct coverage proves accepted mint apply persists quote issuance,
  the blinded output row, the completed operation row, the blind signature, the
  member signature-share provenance, and the applied journal status; stronger
  failure-injection coverage for all-or-nothing atomic rollback and
  materialized state-digest coupling remains open.
- [ ] Same quote plus same outputs is idempotent before submission, while
  pending, after acceptance, after restart, and after catch-up. Current public
  route coverage proves live AlephBFT duplicates join one pending queue item
  and return the same accepted response without issuing twice, and proves the
  after-acceptance duplicate returns the same persisted `MintResponse` without
  appending another operation, including after local response-cache eviction,
  SQLite restart/runtime replay, and in-memory catch-up replay.
- [x] Same quote plus different outputs is rejected before any threshold of
  shares can be collected. Covered at the consensus-conflict layer by
  `runtime_config_rejects_sliding_window_mint_output_attack` and at the public
  route/storage layer by
  `batch_mint_rejects_same_quote_with_different_outputs_without_signing`, which
  proves a conflicting output set appends no second mint operation and stores no
  signature share for the conflicting blinded output.
- [x] A mint operation already accepted by consensus before the HTTP request
  reaches a member returns the same persisted `MintResponse`. Covered by
  `duplicate_federation_mint_after_catch_up_returns_persisted_shares`, which
  applies peer-accepted mint quote, payment observation, and mint operations
  before calling the public batch mint handler.
- [x] A mint request that times out while pending can be retried and later
  returns the accepted response without issuing twice. Covered by
  `batch_mint_pending_aleph_bft_retry_returns_persisted_response_without_issuing_twice`,
  which proves the first public batch mint times out while queued without
  issuing a share, the exact retry waits for finality, the accepted response is
  read from persisted shares, and duplicate accepted retries do not append or
  issue again.
- [x] Restore of accepted mint outputs returns persisted or deterministically
  regenerated shares tied to the original operation hash and output index.
  Covered by
  `restore_returns_persisted_accepted_mint_shares_by_original_output_index`,
  which restores a subset of accepted mint outputs in request order and verifies
  each returned signature share by original operation ID, accepted index,
  consensus evidence, blinded secret, and output index.

Swap operation tests:

- [ ] Admission validation for swap rejects malformed proofs, duplicate inputs,
  duplicate outputs, bad signatures, P2PK/HTLC/SIG_ALL failures, balance/fee
  errors, non-v3/BLS output keysets, and obviously spent inputs without marking
  proofs pending or spent.
  Current apply-layer coverage proves oversized/malformed proofs, invalid
  aggregate proof signatures, duplicate inputs, duplicate outputs, P2PK missing
  witnesses, unbalanced outputs, and SIG_ALL output mutation fail before proof
  state storage, blind-signature storage, completed-operation storage, or
  federation share persistence; non-v3 output rejection is covered before
  consensus submission; conflicting spent-input reuse is covered by the
  input-reuse attack regression.
- [ ] Accepted swap apply marks inputs spent and stores output signature shares
  in one deterministic DB transaction.
  Current direct coverage proves accepted swap apply persists the spent input
  proof state, proof operation linkage, blinded output operation linkage,
  completed operation row, blind signature, member signature-share provenance,
  and applied journal status; stronger failure-injection coverage for
  all-or-nothing atomic rollback and materialized state-digest coupling remains
  open.
- [ ] Exact duplicate swaps return the same persisted shares before submission,
  while pending, after acceptance, after restart, and after catch-up. Current
  public route coverage proves the after-acceptance duplicate returns the same
  persisted `SwapResponse` and does not append another operation, including
  after local response-cache eviction, SQLite restart/runtime replay, and
  in-memory catch-up replay.
- [x] Concurrent conflicting swaps spending the same proof result in exactly one
  accepted spend and deterministic rejected outcomes for the losers. Covered at
  the deterministic apply layer by
  `apply_federation_swap_rejects_sliding_window_input_reuse_attack` and at the
  public route race layer by
  `concurrent_conflicting_swaps_spend_one_input_once_without_signing_losers`,
  which sends concurrent same-proof/different-output swap requests and proves
  one applies, losers are terminal rejected/failed journal outcomes, and loser
  outputs remain unsigned.
- [ ] The overlapping-window quorum-cover attack fails for mint issuance and
  swap: different output sets cannot be mixed into threshold aggregate notes.
- [x] A swap accepted by consensus before the wallet reaches a member returns
  the same persisted `SwapResponse`. Covered by
  `duplicate_post_swap_after_catch_up_returns_persisted_signature_shares`,
  which applies the peer-accepted swap before calling the public swap handler.

Melt operation tests:

- [ ] Admission validation for melt rejects malformed proofs, duplicate inputs,
  bad spending conditions, invalid quote state, invalid change outputs,
  non-v3/BLS change keysets, and balance/fee errors without marking proofs spent
  and without attempting payment.
  Current apply-layer coverage proves invalid aggregate proof signatures,
  duplicate inputs, P2PK missing witnesses, non-v3 change outputs, and
  unbalanced inputs fail before proof state storage, saga storage, quote-state
  mutation, change signature storage, federation share persistence, or outgoing
  payment attempts. Broader invalid quote-state and malformed-content coverage
  remains open.
- [x] Accepted melt reserve records pending inputs and payment-attempt intent
  before any external outgoing payment can be attempted. Covered by
  `accepted_federation_melt_persists_reserve_before_payment_attempt`, whose
  payment backend checks during `make_payment` that proofs are pending, the
  quote is pending, the melt saga is `PaymentAttempted`, and the saga is tied
  to the accepted federation operation; non-elected real-policy reserve remains
  covered by `real_policy_non_elected_melt_member_reserves_without_payment_attempt`.
- [x] Fake-wallet happy path records successful outgoing payment observations,
  finalizes the melt, marks inputs spent, signs change outputs when present, and
  returns the same response to every duplicate API request. Covered by
  `successful_federation_melt_quote_check_records_terminal_payment_observation`
  for the live public happy path plus terminal quote-status observation row, by
  `process_melt_request_returns_federated_change_shares` for change-share
  verification, and by the duplicate public melt tests for direct, cache-evicted,
  SQLite-restarted, and catch-up replayed idempotent responses.
- [x] Fake-wallet failure path records failed observations, rolls back or
  releases pending inputs according to the accepted state machine, and returns
  deterministic idempotent errors. Covered by
  `process_failed_melt_request_releases_inputs_without_change_shares`,
  `duplicate_failed_federation_melt_returns_payment_failed_without_second_payment`,
  `duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment`,
  `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`,
  `failed_federation_melt_quote_check_records_terminal_unpaid_observation`, and
  `get_check_melt_custom_quote_skips_unattempted_unpaid_observation`.
- [x] Long-pending fake-wallet path keeps inputs pending, returns retryable
  pending status, and later finalizes or rolls back only after an accepted
  observation. Covered by
  `pending_federated_melt_public_quote_check_observes_only_terminal_settlement`,
  `process_pending_melt_request_persists_federation_operation_id`,
  `pending_federated_melt_quote_check_later_finalizes_with_change`,
  `pending_federated_melt_verified_paid_finalizes_with_change`, and
  `pending_federated_melt_check_error_keeps_recovery_state`.
- [x] Only the deterministic payment actor attempts a real outgoing melt payment,
  and retries/crashes cannot cause duplicate outgoing payments. Covered by
  `melt_payment_actor_selection_is_deterministic_for_real_policies`,
  `real_policy_non_elected_melt_member_reserves_without_payment_attempt`,
  `accepted_federation_melt_persists_reserve_before_payment_attempt`,
  `duplicate_pending_federation_melt_returns_pending_without_second_payment`,
  `duplicate_pending_federation_melt_after_sqlite_restart_avoids_second_payment`,
  `duplicate_failed_federation_melt_returns_payment_failed_without_second_payment`,
  `duplicate_failed_federation_melt_after_sqlite_restart_stays_failed_without_payment`,
  and `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`.
- [x] A member offline during payment observation catches up without attempting a
  duplicate payment and then returns the same final melt/change response.
  Covered by
  `catch_up_melt_payment_observations_finalize_without_duplicate_payment`,
  `duplicate_federation_melt_after_catch_up_returns_persisted_response`, and
  `duplicate_failed_federation_melt_after_catch_up_stays_failed_without_payment`.

Race and wallet scenario tests:

- [ ] Many concurrent identical mint requests sent to all members produce one
  accepted operation, one response cache entry per member, and threshold
  aggregate proofs. Member-local public route idempotency is covered by
  `concurrent_identical_value_routes_are_idempotent`; multi-member wallet
  fanout with aggregate proof assertions remains open.
- [ ] Many concurrent identical swap requests sent to all members produce one
  accepted operation, one input spend, one response cache entry per member, and
  threshold aggregate proofs. Member-local public route idempotency is covered
  by `concurrent_identical_value_routes_are_idempotent`; multi-member wallet
  fanout with aggregate proof assertions remains open.
- [ ] Many concurrent identical melt requests sent to all members produce one
  accepted reserve/finalization path and at most one outgoing payment attempt.
  Member-local public route idempotency and single fake-wallet payment-attempt
  behavior are covered by `concurrent_identical_value_routes_are_idempotent`;
  multi-member wallet fanout remains open.
- [ ] Conflicting mint, swap, and melt requests submitted concurrently from
  different members are serialized by AlephBFT and converge on identical
  accepted/rejected outcomes on all honest members.
- [x] The accepted operation arrives between admission validation and queue
  submission; the handler returns the cached accepted response without
  resubmitting a conflicting operation. Covered by
  `submit_federation_envelope_returns_duplicate_when_acceptance_races_submission`
  for the in-memory submit path and
  `aleph_bft_duplicate_after_admission_race_returns_journal_duplicate` for the
  AlephBFT queue path.
- [x] The accepted operation arrives before the HTTP request reaches the member;
  the handler returns immediately from durable state. Covered for mint, swap,
  and melt by the public catch-up duplicate-response regressions.
- [x] The member crashes after accepted apply but before waking HTTP waiters;
  retry returns the persisted response. Covered by
  `aleph_bft_duplicate_after_lost_waiters_returns_journal_duplicate`, which
  clears in-memory notification/cache state after finality and proves a retry
  returns the durable journal duplicate without registering a waiter.
- [x] The member is catching up while a wallet request arrives; value-changing
  routes return not-ready or retryable errors until catch-up certification
  succeeds. Covered by
  `value_routes_fail_closed_while_catching_up_and_resume_when_healthy`, which
  marks a federation member `CatchingUp`, proves mint, swap, and melt reject
  before journal, queue, or waiter mutation, then marks healthy and verifies the
  same operations complete.
- [x] A wallet can mint, swap, melt, restore, and check state with only a
  threshold of live reachable members while one member is offline. Covered by
  `federated_mint_connector_uses_threshold_live_members_for_wallet_operations`
  in a 3-of-5 federation where one member fails key discovery and the wallet
  completes mint, swap, melt with change, restore, and check-state from the
  reachable threshold.
- [x] A wallet tolerates one slow member, one offline member, and one bad
  signature share as long as enough valid shares are returned. Covered for
  mint, swap, and melt change by
  `federated_mint_connector_tolerates_slow_offline_and_bad_share_members` in a
  3-of-5 federation.
- [ ] A full fake-backend scenario runs mint -> swap -> melt with member
  restarts, delayed payment observation, duplicate wallet retries, catch-up, and
  final state-digest equality.

Success criteria:

- [ ] No federated mint, swap, melt, restore, or change-output path can return a
  usable signature share unless the exact operation was accepted by AlephBFT or
  was already accepted earlier and retrieved from durable state. Current
  coverage includes submit-response validation that rejects tampered embedded
  journal envelopes before accepted responses can be used for share-returning
  public paths.
- [ ] Every safety-critical federation mutation is reachable from ordered
  consensus apply, replay, or catch-up; no public route commits those mutations
  directly.
- [ ] Mint, swap, and melt public APIs are idempotent across duplicate requests,
  retries, timeouts, accepted-before-request races, member restarts, and catch-up.
- [ ] Wallet behavior remains close to current CDK behavior: request, wait for
  compatible member responses, verify shares, aggregate threshold BLS signatures,
  and return ordinary Cashu proofs/responses.
- [ ] AlephBFT-backed 4-member and 5-member integration tests pass for mint,
  swap, melt, restore, check-state, restart, offline member, catch-up, duplicate
  submission, and conflicting operation scenarios.
- [ ] Standalone non-federated mint and wallet tests remain unchanged, including
  non-federation keyset support.

## Milestone 5: federation DB journal and state digests

Objective: keep replicated operation history separate from materialized mint state.

Implementation checklist:

- [x] Add DB traits for federation journal/database transactions, implemented as `FederationJournalDatabase` and `FederationJournalTransaction`.
- [x] Add SQL migrations for SQLite and Postgres:
  - [x] `federation_config`, storing a durable public config digest snapshot and rejecting conflicting database reuse at federation startup.
  - [x] `federation_session_outcomes`.
  - [x] `federation_journal_entries` for accepted items, operation envelopes, and operation outcomes.
  - [x] `federation_signature_share`, implemented as `federation_signature_shares`.
  - [x] `federation_payment_observations`.
  - [x] Dedicated `federation_state_digest` digest-history table.
  - [x] `federation_member_sequences` for private REST sequence watermarks.
  - [x] Dedicated `federation_member_status` table if runtime/admin state should become persistent.
- [x] Keep existing mint tables as the materialized state used by existing APIs where possible.
- [x] Add `signature_share` rows instead of overloading final `blind_signature` semantics for federated members.
- [x] Add state digest calculation over consensus-visible tables only.
- [x] Exclude local private config, local connection status, logs, and caches from the digest.
- [x] Add an audit summary similar to Fedimint's balance audit after each accepted operation.
- [x] Persist accepted order digests and signed session outcomes as lightweight replay/catch-up evidence.
- [x] Add configurable DB checkpoint retention and/or dedicated digest-history storage, satisfied for now by dedicated digest-history storage.

Tests and success criteria:

- [x] Re-run migration tests proving federation migrations apply cleanly on an empty SQLite mint database while preserving standalone mint tables.
- [x] Re-run migration tests proving federation migrations apply cleanly on an empty Postgres mint database with a live Postgres instance. `cargo test -p cdk-postgres --features federation federation_migrations_apply_to_empty_postgres_and_keep_standalone_tables -- --test-threads 1` passed against the local Postgres helper on 2026-07-05.
- [x] Re-run migration tests covering feature-disabled SQLite legacy/standalone migration compatibility.
- [x] Applying the same operation log to two DBs yields identical state digests.
- [x] Local-only fields do not change state digest.
- [x] Negative balance audit causes a hard error in tests.
- [x] Signature shares can be retrieved by operation hash and output index from a dedicated federation share API/table.

## Milestone 6: deterministic mint state machine extraction

Objective: refactor CDK mint operations so consensus can call deterministic apply functions.

Implementation checklist:

- [x] Add deterministic federated mint apply path, implemented as `apply_federation_mint`.
- [ ] Refactor standalone `process_mint_request` into shared `validate_issue_mint` and `apply_issue_mint` functions if shared standalone/federation code is still desired.
- [x] Add deterministic federated swap apply path, implemented as `apply_federation_swap`.
- [ ] Refactor standalone `SwapSaga::setup_swap` and `finalize` into shared `validate_swap` and `apply_swap` functions if shared standalone/federation code is still desired.
- [x] Pass operation hashes/IDs, quote IDs, and payment observations explicitly into federation apply paths.
- [x] Add a consensus-provided accepted-time input if any apply path needs accepted time.
- [x] Remove calls to `unix_time()` from consensus apply paths except via consensus-provided accepted time.
- [x] Remove UUID generation from consensus apply paths.
- [x] Ensure apply paths do not spawn tasks or publish pubsub notifications until after commit.
- [ ] Keep existing standalone methods calling shared refactored functions through a standalone adapter if the shared functions are introduced.
- [x] Make `apply_federation_mint` update quote issuance, blinded message records, completed operation, and output member-share records in one DB transaction using existing blind-signature storage.
- [x] Make `apply_federation_swap` verify inputs, verify spending conditions, check balance, mark inputs spent, record blinded outputs, record completed operation, and request signature shares in one DB transaction.
- [x] Decide how to represent `Pending` in federated mode:
  - [x] Swap should not use local pending state before consensus.
  - [x] Melt reserve can use consensus-visible pending state between reserve and finalization.
- [x] Add idempotency: if the exact same operation was already accepted, return the existing outcome.
- [x] Add conflict detection: if a conflicting operation was accepted first, reject and record a conflict outcome where appropriate.

Tests and success criteria:

- [x] Re-run standalone mint tests after the federation refactor.
- [x] Applying a valid issue operation twice is idempotent.
- [x] Applying a same-quote different-output issue after the first issue fails.
- [x] Applying a swap with already spent inputs fails.
- [x] Applying a swap with duplicate outputs fails.
- [x] Applying operations in the same order on multiple DB instances produces identical materialized state.
- [x] Federation swap pubsub notifications fire after commit.
- [x] Add broad post-commit pubsub tests for standalone mode and all federation operations.

## Milestone 7: federated key-share signatory

Objective: let each member sign only with its own BLS share while advertising aggregate keys.

Implementation checklist:

- [x] Add federation member-share signing helpers on federation config/private config data.
- [ ] Introduce a `FederatedSignatory` type or split the existing `Signatory` trait if the helper approach becomes too implicit.
- [x] `keysets()` must return aggregate public keys and aggregate keyset IDs.
- [x] `verify_proofs()` must verify proofs against aggregate public keys.
- [x] `blind_sign_share()` must return member share signatures plus member ID.
- [x] Add share verification against local public share before storing or returning a share.
- [x] Prevent federated share signer from being used in standalone paths that expect final signatures.
- [x] Ensure Cashu keyset v3 (`KeySetVersion::Version02`) BLS responses do not include DLEQ data.
- [x] Reject all non-v3 keysets in federation signing paths unless a future protocol explicitly supports them.
- [x] Persist signature shares in `federation_signature_share`, keyed by operation hash, output index, member ID, blinded message, amount, and keyset ID.
- [x] Persist accepted journal index and consensus evidence with signature-share records so shares can be traced to accepted ordering provenance.
- [x] Build accepted signature-share records through checked constructors that reject mismatched share-set and journal/provenance operation IDs before persistence.
- [x] Return existing share for idempotent repeat requests.

Tests and success criteria:

- [x] A member cannot return a final signature in federation mode.
- [x] A member's share verifies against its public share.
- [x] Aggregate of shares returned by three members verifies as a normal Cashu blind signature when the configured threshold is three.
- [x] Re-run regression coverage proving existing standalone `DbSignatory` behavior is unchanged.
- [x] A federated member rejects non-BLS keysets for federation signing unless explicitly supported later.
- [x] A federated member rejects v1/v2 keysets even if they are otherwise valid standalone Cashu keysets.

## Milestone 8: federated mint quote creation and payment observations

Objective: make all members agree on mint quotes and paid status.

Implementation checklist:

- [x] In federation mode, `get_mint_quote` on the contacted member creates the payment request, then submits a `MintQuote` federation operation to consensus.
- [x] `MintQuote` carries quote ID and Cashu quote request/response payloads, including method, unit, amount, expiry, request, lookup, pubkey, and extra fields carried by those types.
- [ ] Add explicit origin-member and accepted-time fields to the mint quote operation if they are required outside the journal envelope.
- [x] All members persist the same quote after consensus accepts it.
- [x] Payment observation operations are submitted and processed through consensus.
- [x] Add an always-on payment observer task per member that watches unpaid quotes and submits mint quote payment observations.
- [x] `MintQuotePayment` carries observer, method, state, amount paid, amount issued, and optional payment proof.
- [x] Add explicit payment ID and observed-time fields if optional payment proof plus journal ordering are insufficient.
- [x] Mark quote paid only after the configured observation policy is satisfied.
- [x] For fake wallet MVP, allow all members to observe paid deterministically.
- [x] For real Lightning/onchain, define observation policies:
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

- [x] In federation mode, `process_mint_request` converts `MintInput` into a `Mint` federation operation and submits it to consensus.
- [x] `Mint` includes quote IDs and blinded outputs through `MintRequest`/`BatchMintRequest`; quote amounts are validated from accepted quote state.
- [ ] Add explicit quote amount fields to the operation payload only if future review decides the current quote-state validation is insufficient.
- [x] Consensus apply validates quote states, paid amounts, issued amounts, payment methods, units, v3/BLS output keysets, output uniqueness, and NUT-20 signatures.
- [x] Consensus apply records issuance and member signature shares in the same DB transaction via dedicated federation signature-share storage plus the current wallet-facing blind-signature compatibility rows.
- [x] Move member signature shares to a dedicated `federation_signature_share` table if that table is added.
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
- [x] Unpaid quote minting fails before blind-signature or federation signature-share storage.
- [x] On member crash after consensus acceptance but before HTTP response, retry returns stored shares.
- [x] On member crash before consensus acceptance, retry submits the same operation hash.
- [x] Add or re-run explicit federated NUT-20 locked quote regression tests.

## Milestone 10: federated wallet connector and aggregation

Objective: make the wallet see normal Cashu responses while the connector handles member fanout.

Implementation checklist:

- [x] Add `FederatedMintConnector` implementing `MintConnector`.
- [x] Load federation config through explicit builder/config construction.
- [x] Load federation config automatically from mint info or invite codes.
- [x] Wrap one `HttpClient` per member.
- [x] `get_mint_keys`, `get_mint_keysets`, and `get_mint_info` should query multiple members and require matching federation ID and aggregate keysets.
- [x] `post_mint_quote` creates a quote through the first healthy member.
- [x] Optionally verify quorum visibility after quote creation.
- [x] `post_mint` and `post_batch_mint` should fan out the exact same request to all members concurrently.
- [x] Validate each returned share against the member public share and corresponding blinded output.
- [x] Aggregate the first threshold valid shares per output.
- [x] Return normal aggregate `MintResponse` to the wallet saga.
- [x] `post_swap` should do the same for swap signatures.
- [x] Implement first-healthy fallback and threshold fanout error collection.
- [x] Add an explicit retry and timeout policy per member.
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
- [x] Consensus apply verifies:
  - [x] inputs are unique.
  - [x] outputs are unique.
  - [x] input keysets are known and not expired.
  - [x] output keysets are active and not expired.
  - [x] proofs verify under aggregate public keys.
  - [x] Existing spending-condition verification is called inside federated swap apply.
  - [x] Add dedicated federated canonical-transcript tests for P2PK, HTLC, and SIG_ALL.
  - [x] input value equals output value plus fees.
  - [x] no input proof Y is already spent or pending in consensus state.
- [x] Consensus apply marks inputs spent and records output signature shares in one DB transaction.
- [x] Public handler waits for acceptance and returns this member's shares.
- [x] Existing standalone saga remains available for non-federated mints.
- [x] Restore can recover accepted blind-signature rows by blinded output.
- [x] Add explicit operation-hash/output-index restore mapping if the dedicated signature-share table is added.

Tests and success criteria:

- [x] Normal wallet swap succeeds through federated connector.
- [x] Concurrent swaps spending the same input result in exactly one accepted swap.
- [x] Replay of the same swap request is idempotent.
- [x] Different output sets for the same inputs cannot both get threshold shares.
- [x] Add or re-run P2PK and HTLC tests in federated mode.
- [x] Add federated SIG_ALL tests proving the signature commits to the exact canonical operation.

## Milestone 12: explicit sliding-window attack regression suite

Objective: prove that threshold shares cannot be mixed across different output sets.

Implementation checklist:

- [x] Build a malicious test wallet helper that can send different output windows to different members.
- [x] Configure a 4-member test federation with signature threshold 3 if lower-threshold mode is supported.
- [x] Create a paid quote for three notes and attempt the ABC, BCD, CDA, DAB pattern.
- [x] Ensure each member submits its request to consensus and signs only accepted operations.
- [x] Record which operation hash won consensus.
- [x] Attempt to aggregate shares per output across all responses.
- [x] Repeat the same sliding-window attack test for swap inputs instead of mint quote issuance.
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
- [x] Restore lookup uses existing blind-signature storage by blinded message.
- [x] Add `federation_signature_share` lookup by blinded message if the dedicated federation share table is added.
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
- [x] `MeltQuote` persists the quote across members.
- [x] Federated melt request validates inputs and change outputs and uses consensus-visible saga/pending state.
- [x] Keep the current federated melt request name for operation version 1; document `FederationOperation::Melt` as the accepted reserve/execution operation rather than a terminal finality operation.
- [x] Fake-wallet happy path covers outgoing payment completion.
- [x] Exactly one production payment actor should attempt the outgoing payment, selected deterministically from operation hash or configured as origin member.
- [x] Before attempting payment, consensus must record `PaymentAttempted` or equivalent write-ahead state.
- [x] Payment result enters consensus as `MeltQuotePayment`.
- [x] `FinalizeMelt` after threshold paid observations marks inputs spent, records quote paid, signs change outputs, records completed operation, and deletes pending melt state.
- [x] `RollbackMelt` after threshold failed observations releases pending inputs and records quote failed.
- [x] Unknown or pending backend states keep inputs pending until resolved or an admin policy handles timeout.
- [x] Existing `MeltSaga` recovery concepts should be reused, but consensus state replaces member-local saga state as the source of truth.
- [x] Internal melt-to-mint settlement must become a single consensus operation that updates both quote families atomically.

Tests and success criteria:

- [x] Fake wallet melt succeeds and returns aggregated change signatures.
- [x] Failed fake wallet melt releases inputs across all members, and exact duplicate failed public retries return `PaymentFailed` without attempting a second outgoing payment, including after SQLite restart and in-memory catch-up.
- [x] Pending fake wallet melt keeps inputs pending and later finalizes after observation.
- [x] Crash after payment attempt but before finalize recovers by checking observations and finalizing.
- [x] Two concurrent melts of the same proofs cannot both pay.
- [x] Internal melt-to-mint updates the mint quote and melt quote in one accepted operation.

## Milestone 15: networking and peer authentication

Objective: run federation members as separate processes.

Implementation checklist:

- [x] Use private REST client/server and `cdk_http_client::Transport` for MVP federation traffic.
- [ ] Add a Fedimint-like P2P trait similar to `IP2PConnections` when adding non-REST transports.
- [x] Implement an in-memory consensus/network simulator for tests.
- [ ] Implement a separate in-memory P2P transport if future consensus code needs it.
- [x] Abstract HTTP transport for the private REST peer client.
- [ ] Add a shared transport abstraction spanning in-memory, REST, TCP/TLS or Noise, and Iroh if those transports share consensus code.
- [x] Implement private REST transport first using Axum/Reqwest for local, regtest, and reachable-server deployments.
- [x] Keep federation REST endpoints separate from public Cashu wallet endpoints, for example under `/federation/v1/*`.
- [x] Use signed, canonical member-to-member envelopes on every REST message: federation ID, sender member ID, receiver member ID, message kind, consensus height, nonce or sequence number, body hash, and signature.
- [x] Verify every private REST envelope against the sender's identity public key from the local federation roster.
- [x] Refuse private REST requests from member IDs that are not in the configured roster.
- [x] Refuse private REST requests whose signing key does not match the configured public key for the claimed sender member ID.
- [x] Do not support dynamic peer admission in MVP; member add/remove must be an explicit later governance operation.
- [x] Make REST messages idempotent so retries after timeouts cannot duplicate consensus proposals, payment observations, or configured development-backend outgoing attempts.
- [x] Add REST endpoints for peer status, proposal submission, consensus messages, signed session outcomes, catch-up ranges, and state-digest comparison.
- [ ] Optionally implement TCP/TLS or Noise transport with static peer identities if the consensus engine benefits from a lower-level stream API.
- [ ] Add an Iroh transport adapter for internet deployments where members cannot expose stable inbound P2P ports.
- [x] Treat Iroh as the recommended NAT/firewall traversal transport, following the same broad deployment motivation as Fedimint, but do not make the first correctness milestones depend on it.
- [x] Document that private REST alone requires reachable member endpoints or an external connectivity solution such as VPN, reverse tunnel, relay, Tor, or Iroh.
- [x] Authenticate every peer message by member ID.
- [x] Add consensus item size/admission limits.
- [x] Add bounded outbound and inbound REST queues and backpressure.
- [x] Add a periodic peer sync loop.
- [x] Add reconnect with exponential backoff.
- [x] Add periodic config-digest, consensus-height, and state-digest gossip between peers.
- [x] Add admin peer status output.
- [x] Add peer status metrics.
- [x] Separate public HTTP API URL from private P2P URL.
- [x] Reject messages from unknown members or wrong federation IDs.
- [x] Define maximum message sizes and backpressure behavior for consensus proposals, signature-share responses, and catch-up data.

Tests and success criteria:

- [x] Local process tooling and smoke scripts exist for multi-member private REST federation runs.
- [x] Re-run and record a five-local-process private REST consensus smoke test.
- [x] In-memory P2P test can deterministically simulate partitions, dropped messages, reordered messages, and reconnects.
- [x] REST retry test proves duplicate HTTP deliveries are idempotent.
- [x] REST authentication test rejects wrong federation ID, wrong sender ID, stale sequence, invalid signature, and body-hash mismatch.
- [x] REST allowlist test rejects a well-formed message signed by a valid key that is not in the federation roster.
- [x] REST claimed-sender test rejects a message signed by member A's key while claiming member B's ID.
- [ ] Iroh adapter smoke test connects peers without direct inbound TCP reachability when the test environment supports it.
- [x] Restarted member reconnects and catches up.
- [x] Unknown peer messages are rejected.
- [x] Size/admission tests cover oversized consensus items.
- [x] Queue overflow drops non-critical messages without crashing consensus.
- [x] Signed-envelope identity mismatch prevents message acceptance.
- [ ] Add TLS/Noise identity mismatch tests if those transports are implemented.
- [x] Peer status shows connected, disconnected, lagging, catching-up, and halted states.

## Milestone 16: member startup, catch-up, and recovery

Objective: make nodes durable and restart-safe.

Implementation checklist:

- [x] On startup, load federation config and validate key shares before serving HTTP.
- [x] Start private REST/consensus runtime before enabling federation signing endpoints.
- [ ] Revisit startup sequencing when non-REST P2P transports are implemented.
- [x] Replay accepted items since last checkpoint if needed.
- [x] Persist last applied consensus session, item index, operation hash, outcome hash, and state digest.
- [x] Persist signed session outcomes so lagging members can verify catch-up data without trusting one peer.
- [x] On reconnect, request missing signed session outcomes and accepted items from peers starting at the last durable local height.
- [x] Verify quorum or threshold signatures on catch-up data before replaying it locally.
- [x] Replay accepted operations deterministically and recompute state digests after every session boundary.
- [x] Regenerate deterministic signature shares from accepted operations when local share rows are missing.
- [x] Compare local state digest to quorum state digest at session boundaries.
- [x] Expose admin endpoint or CLI to show consensus height, last digest, member lag, and pending operations.
- [x] Add DB checkpoint retention configuration.
- [x] Add a safe read-only "catch-up" mode for lagging members.
- [x] Prevent a lagging member from serving signing shares until it catches up to the operation's acceptance height.
- [x] Prevent a lagging member from initiating configured development-backend outgoing payment attempts until it has replayed all accepted melt state.
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
- [x] Member offline during melt payment observation catches up without attempting a duplicate configured-backend payment.
- [x] Network partition heals and all non-faulty members converge or halt on digest mismatch.
- [x] State digest mismatch is detected and halts the member in tests.

## Milestone 16A: state convergence and catch-up certification

Objective: prove that any honest federation member that was offline, restarted, or lagging can replay consensus history and converge to the same consensus-visible mint state as the rest of the federation, while refusing unsafe service until catch-up is certified.

Implementation checklist:

- [x] Define the exact consensus-visible database surface: tables and columns included in federation state equality.
- [x] Define the exact local-only database surface: private config, peer connection state, sequence trackers, logs, caches, local metrics, process metadata, and other fields excluded from equality.
- [x] Implement a canonical state snapshot/export over consensus-visible rows.
- [x] Implement a state digest over the canonical snapshot.
- [x] Include state digests in signed session outcomes and admin status reporting.
- [x] Add a dedicated digest-history table and retention policy for consensus session boundaries.
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
- [x] Ensure configured-backend outgoing payment idempotency prevents duplicate payments after catch-up or restart.

Tests and success criteria:

- [x] Member offline for 10 accepted operations catches up and reaches the same state digest.
- [x] Long-partition tooling exists for extended offline/catch-up scenarios.
- [x] Add or re-run an explicit 100+ accepted-operation offline catch-up test.
- [x] Member offline across minting catches up and can return the correct deterministic mint share.
- [x] Member offline across swapping catches up to spent input state and rejects conflicting swaps.
- [x] Catch-up replay continues past a deterministic rejected conflicting swap
  and still applies later independent swaps. Covered by
  `catch_up_marks_conflicting_swap_rejected_and_continues_replay`.
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
- [x] Add `just fed-regtest` or similar target that starts N local federation mints with fakewallet configured and a test wallet.
- [x] Add health commands: member status, consensus height, quote status across members, state digest comparison.
- [x] Add log labels for federation ID, member ID, operation hash, and session.
- [x] Add a fixture generator for 4-member and 5-member federations.
- [x] Ensure no generated config intended for examples contains private production material.

Local multi-mint federation harness:

- [x] Generate per-member local configs containing member ID, public wallet API URL, private federation REST URL, DB path, configured backend settings, federation public config path, and local private member config path.
- [x] Assign deterministic localhost ports for every member, with separate public wallet API ports and private federation REST ports.
- [x] Create isolated per-member data directories so databases, logs, and configs cannot overlap.
- [x] Add `just fed-regtest` to launch 4-member and 5-member federations with fakewallet configured as the backend as separate `cdk-mintd` processes.
- [x] Add `just fed-regtest-status` to show public URL, private federation URL, process status, consensus height, last state digest, lag, and member health.
- [x] Add `just fed-regtest-stop-member MEMBER_ID` and `just fed-regtest-start-member MEMBER_ID` to test threshold liveness, reconnect, and catch-up.
- [x] Add `just fed-regtest-clean` to remove generated local configs, DBs, logs, and process state.
- [x] Add a local wallet federation config, invite file, or test connector config that points to the federation roster instead of a single mint URL.
- [x] Make local logs identify federation ID, member ID, public port, private federation port, consensus height, operation hash, and DB path.
- [x] Document how to run the local federation manually without `just`, including the exact `cdk-mintd` command shape for one member.
- [x] Keep generated local private configs out of examples and Git; commit only templates, deterministic non-secret fixtures, and safe documentation.

Tests and success criteria:

- [x] `just fed-regtest` target and generated smoke scripts exist for development-backend federation runs.
- [x] Re-run `just fed-regtest` and record that it starts a 5-member federation with fakewallet configured as the backend.
- [x] Connector and local smoke coverage exist for mint, swap, restore, and check-state flows.
- [x] Re-run full regtest proving a CDK wallet can mint, swap, restore, and check state against the federation.
- [x] Code and tests exist for development-backend federated melt and aggregated change signatures.
- [x] Re-run full regtest proving a CDK wallet can melt through the development-backend federation and receive aggregated change signatures when change is expected.
- [x] Stop/start tooling and catch-up tests exist for threshold liveness and member recovery.
- [x] Re-run regtest proving stopping one member still allows operations if threshold and consensus quorum allow.
- [x] Re-run regtest proving restarting the stopped member reconnects, catches up from peers, and reaches the same state digest.
- [x] State digest comparison command exists.
- [x] Re-run regtest proving state digest comparison reports all members equal after each test.
- [x] Local private REST peer authentication rejects unknown members, wrong federation IDs, wrong claimed member IDs, and invalid signatures on localhost.
- [x] Local status commands clearly show connected, offline, lagging, catching-up, and healthy members.
- [x] `just fed-regtest-clean` target exists.
- [x] Re-run `just fed-regtest-clean` and verify it leaves no running member processes or generated local state.

## Milestone 18: storage backend coverage

Objective: support the same storage expectations as CDK does today.

Implementation checklist:

- [x] Implement federation DB traits for SQLite first.
- [x] Implement Postgres after SQLite semantics are stable.
- [x] Decide whether Redb is wallet-only and therefore out of scope for mint federation.
- [x] Keep migrations additive and compatible with standalone mode.
- [x] Conflict keys and concurrent request stress tests exist.
- [x] Document SQL lock ordering rules for all conflict keys to avoid deadlocks.
- [x] Add federation journal/payment/session indexes for operation/session/payment hot paths.
- [x] Review and add exact indexes for quote ID, proof Y, blinded message, and dedicated signature-share rows after the final federation schema settles.

Tests and success criteria:

- [x] Re-run SQLite unit and integration tests.
- [x] Re-run Postgres integration tests with a live Postgres instance. `cargo test -p cdk-postgres --features federation -- --test-threads 1` passed 138 tests against the local Postgres helper on 2026-07-05.
- [x] Concurrent request stress tests do not deadlock.
- [x] Verify query plans for hot paths use indexes.

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

- [x] Add DKG setup phases, commitments/reveals, secret-share contributions, transcript signatures, and local member keyset-share derivation.
- [ ] Complete a production-ready dealer-free Pedersen DKG replacement for BLS G2, using Fedimint's DKG as a reference.
- [x] Run DKG over authenticated private REST setup routes, accepting DKG messages only from identity public keys in the federation roster.
- [x] Include federation ID, member roster, member identity public keys, participant order, commitments, BLS public shares, and aggregate public keys in the DKG transcript.
- [x] Require each participant to sign the final DKG transcript with its federation identity key. Private REST transcript-signature acceptance now also requires the receiver to have a locally derived DKG keyset-share record and rejects transcript hashes that do not match that local final public config before advancing the sender sequence.
- [x] Commit hash of polynomial commitments before revealing commitments.
- [x] Verify secret shares against public commitments.
- [x] Derive public shares and aggregate public keys deterministically.
- [x] Persist DKG transcript for audit and recovery.
- [x] Persist the final roster hash and DKG transcript hash in each member config.
- [x] Support key rotation through consensus as a federation operation.
- [ ] Make trusted-dealer mode test-only once production DKG is complete.

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

- [x] Add consensus operation `KeysetRotation`.
- [x] New aggregate keysets become active only after consensus acceptance.
- [x] Validate configured key shares against public shares.
- [x] Add an explicit rotation-time proof-of-possession gate before activating a new keyset.
- [x] Wallet sees NUT-02 keyset changes consistently across members.
- [x] Add protocol version negotiation in federation metadata.
- [x] Add migration plan for changing operation encoding versions.
- [x] Add member add/remove as a later, explicit governance operation. Do not include in MVP.

Tests and success criteria:

- [x] Wallet can mint with new active keyset after rotation.
- [x] Old proofs remain spendable.
- [x] Config mismatch validation exists for federation membership/configuration.
- [x] Add stale-upgrade consensus tests proving members with stale config refuse to join upgraded consensus.
- [x] Version fields and some version checks exist for federation data.
- [x] Add mixed-version federation tests proving unsupported major versions are rejected cleanly.

Operation encoding migration plan:

- Treat `FederationProtocolVersions.operation`, `consensus_item`, `rest_api`, and `session_outcome` as hard compatibility gates. A member must reject startup config, wallet metadata, private REST payloads, consensus items, session outcomes, operation admission, runtime replay, and catch-up material when any advertised or embedded major version differs from the current build.
- For a future operation encoding change, introduce the new decoder and encoder behind a new `FEDERATION_OPERATION_VERSION` plus golden vectors before any federation advertises it. The operation hash transcript must remain domain-separated by the versioned envelope bytes so old and new encodings cannot collide.
- For a future consensus item encoding change, bump `FEDERATION_CONSENSUS_ITEM_VERSION`, add item-level admission tests for `Operation`, `ModuleFact`, `Default`, and any production BFT items, and require stale members to reject queue admission before dispatch.
- Rollout must be all-or-nothing for a federation roster until dynamic upgrade governance exists: generate a new public config advertising the new versions, require all members to load the same config, refuse mixed-version rosters at startup, and keep old members halted rather than participating in consensus.
- Catch-up/replay is the last safety gate. Persisted journal entries and peer catch-up responses must be revalidated through the embedded operation envelope version and session-outcome version before updating local journal state or materialized mint state.
- Every encoding migration needs checked-in canonical vectors, stale-config startup tests, consensus item admission rejection tests, catch-up/replay rejection tests, and wallet metadata rejection tests before release.

## Milestone 22: observability and operator safety

Objective: make failures diagnosable without exposing private user data.

Implementation checklist:

- [x] Add admin/status reporting for consensus height, digests, lag, and peer state.
- [x] Add comprehensive metrics for consensus height, pending mempool items, accepted operations, rejected operations, signing share latency, aggregation failures, state digest mismatches, and peer connectivity.
- [x] Add structured logs with short operation hashes, not full secrets or PII.
- [x] Add admin command to compare state digests across members.
- [x] Add admin command to list pending payment observations by quote ID.
- [x] Add alert when a member is behind by more than configured sessions.
- [x] Add alert when too many invalid operations are submitted by one client IP or auth token.
- [x] Redact proof secrets, witnesses, invoices, local paths, and private keys from logs.

Tests and success criteria:

- [x] Log snapshot tests verify redaction of secrets and private config.
- [x] Metrics are emitted in fake regtest.
- [x] Digest mismatch alert fires in an injected-divergence test.

## Milestone 23: production cleanup and payment-backend isolation

Objective: remove MVP/dev scaffolding from production federation semantics before real-funds deployment.

Implementation checklist:

- [x] Ensure federation crates and production federation modules do not depend on, import, or semantically special-case `cdk-fake-wallet`.
- [x] Keep `FakeWallet` only as a normal CDK mint payment backend used by `cdk-mintd` for local development, smoke tests, and regtest.
- [x] Rename or replace any federation policy type, method, error, config field, or test helper whose production meaning is currently expressed as `FakeWallet`.
- [x] Replace implicit fakewallet payment policy defaults with explicit payment observation policy configuration at the mintd startup boundary.
- [x] Reject production federation startup when a config relies on implicit fakewallet defaults, single-observation dev policies, or test-only payment assumptions.
- [x] Model deterministic single-observation behavior, if still needed for local tests, as an explicit dev/test payment observation policy rather than as fakewallet-specific federation logic.
- [x] Make federation minting and melting depend only on generic payment observations, quote state transitions, and existing CDK payment backend traits.
- [x] Make federated minting and melting use the mint's configured payment backend in the same way standalone mint mode does; the federation layer must not know whether the backend is CLN, LND, LNBits, LDK Node, fakewallet, or a future backend.
- [x] Derive payment observations from generic quote/payment state returned by the configured backend, not from backend-specific federation branches.
- [x] Split trusted-dealer federation key/config generation from local fakewallet regtest config generation.
- [x] Allow `cdk-mintd federation trusted-dealer` or its successor setup command to generate member identities, public federation config, private member configs, BLS shares, and aggregate keysets without enabling the `fakewallet` feature.
- [x] Keep fakewallet-specific member config generation in a dev/regtest wrapper command, generated smoke harness, or `just` target rather than in the key generation path.
- [x] Add federation config for peer sync interval, peer request timeout, operation submission timeout, catch-up limits, digest recheck attempts, digest recheck delay, session outcome evidence chunk size, and related retry policy.
- [x] Add wallet federation connector configuration for per-member fallback timeout, member request timeout, member request retries, quote-status polling timeout, and fanout concurrency.
- [x] Replace production reads from in-memory federation journal/runtime state with durable mint database reads for peer status, catch-up responses, state digests, and session outcomes.
- [x] On `cdk-mintd` startup, reconstruct federation runtime state from the database and replay unapplied accepted journal entries before serving signing shares or authoritative status.
- [x] Ensure a crash after consensus acceptance but before deterministic apply recovers from the database and converges to the same state digest.
- [ ] Split large federation modules into logical submodules when useful, following existing CDK layout conventions.
- [ ] Suggested mint split: config/runtime, apply, mint, swap, melt, restore/check-state, payment observations, keysets, and tests.
- [ ] Suggested wallet split: connector, member fanout, share aggregation, keyset validation, quote/status aggregation, melt/change aggregation, and tests.
- [ ] Suggested Axum split: routes, peer client, sync loop, status, catch-up, DKG/setup routes, payment-observation routes, and tests. The peer-sync policy defaults, validation, env-disable parsing, interval setup, and reconnect backoff now live in `crates/cdk-axum/src/federation/sync_config.rs` while `FederationPeerSyncConfig` remains re-exported from the parent module. The signed private REST peer client now lives in `crates/cdk-axum/src/federation/peer_client.rs`, covering outbound admission, signed request construction, endpoint building, per-operation timeouts, DKG/status/submit/catch-up calls, and process-wide REST sequence reservation. Peer sync orchestration now lives in `crates/cdk-axum/src/federation/sync_loop.rs`, covering the periodic sync loop, reconnect-aware sync rounds, signed session-outcome evidence collection, primary forwarding, and peer-client catch-up/materialized-digest checks. The private DKG setup route handlers now live in `crates/cdk-axum/src/federation/dkg_routes.rs`, covering commitment, reveal, secret-share, and transcript-signature request validation/persistence while the parent module keeps router assembly and shared signed-envelope helpers. The signed catch-up journal route now lives in `crates/cdk-axum/src/federation/catch_up_routes.rs`, covering sequence persistence, bounded durable journal responses, materialized state digest attachment, and signed session-outcome evidence. The private status and local-status route handlers plus `FederationLocalStatusResponse` now live in `crates/cdk-axum/src/federation/status_routes.rs`, covering signed status sequence persistence, consensus-visible peer status reads, sync readiness, quorum evidence, materialized-row status responses, and local-status metrics. Payment-observation report assembly now lives in `crates/cdk-axum/src/federation/payment_observation.rs`, covering journal-derived observation/quarantine summaries and quote-age enrichment for local-status responses.
- [ ] Move large inline federation test fixtures, local connectors, and fakewallet helpers into dedicated test modules or integration-test helpers. The `cdk-common` consensus tests and fixtures have been extracted to `federation/consensus/tests.rs`, the wallet local member connector fixtures now live in `wallet/federation/tests/local_connector.rs`, the mint federation tests now live in `mint/federation/tests.rs`, the wallet federation connector tests now live in `wallet/federation/tests.rs`, the Axum private REST federation tests now live in `federation/tests.rs`, the Axum private operation route tests now live in `federation/tests/private_operation_route.rs`, the Axum DKG setup route tests now live in `federation/tests/dkg_setup_routes.rs`, the Axum DKG transcript-signature route tests now live in `federation/tests/dkg_transcript_signature.rs`, the mintd federation setup/smoke-artifact tests now live in `federation_setup/core_tests.rs` plus `federation_setup/tests.rs`, the smoke-operation argument tests now live in `federation_smoke/tests.rs`, the mintd status command tests now live in `federation_status/tests.rs`, the mintd DKG-status tests now live in `federation_dkg_status/tests.rs`, and the mintd quarantine, digest-compare, payment-observations, and inspect command tests now live in their sibling `tests.rs` modules; any remaining smoke/helper splits remain open.
- [x] Gate any remaining smoke/regtest-only federation helpers behind test-only code or explicit dev features.
- [x] Audit `cdk`, `cdk-common`, `cdk-axum`, `cdk-mintd`, and storage crates for accidental fakewallet coupling in federation code.
- [x] Audit generated example configs so fakewallet-backed development federation examples are clearly marked development-only and cannot be mistaken for production templates.
- [x] Audit timeout constants, fallback behavior, and member retry policies introduced for smoke tests, and make production-relevant values configurable.
- [x] Remove stale "scaffolding", "placeholder", and "MVP wiring" labels from production federation modules once the corresponding feature is implemented; keep such language only in explicit dev/test code.
- [ ] Make fakewallet federation regtest and smoke tooling look and run more like existing CDK tests where practical, with `cargo test` coverage for core flows and `just` targets as convenience wrappers. Smoke-operation argument tests now cover zero mint amounts plus the wallet database and private config prerequisites for resumable, quarantine, and keyset-rotation scenarios before generated scripts reach networked member operations, and `just fed-regtest-check` runs the fast smoke/setup cargo-test gate without starting member processes.
- [x] Remove or rewrite any comments, docs, metrics, or status labels that imply fakewallet is part of federation rather than a replaceable mint backend.
- [x] Confirm standalone fakewallet mint behavior remains unchanged outside federation mode.

Tests and success criteria:

- [x] `rg "cdk_fake_wallet|FakeWallet|fake_wallet|fake-wallet"` in production federation modules only finds test-only code, mintd backend setup, smoke tooling, or documentation that explicitly marks fakewallet as development-only.
- [x] A federation built with a real payment backend can start without enabling the `fakewallet` feature.
- [x] `cdk-mintd` can generate trusted-dealer federation configs with `federation,sqlite` features and without the `fakewallet` feature.
- [x] Federated mint and melt tests pass through the same federation payment-observation path with fakewallet and with a backend-agnostic mock payment backend.
- [x] A production-mode federation config without an explicit payment observation policy fails startup with a clear error.
- [x] A production-mode federation config with a dev/test single-observation policy fails startup unless an explicit development override is set.
- [x] Custom peer sync, catch-up, digest recheck, operation submission, and wallet fallback timeouts are honored in unit tests.
- [x] Restart tests prove accepted-but-unapplied federation operations are replayed from the database and converge to the same state digest.
- [x] Private REST status, catch-up, digest, and session outcome tests read from durable database state rather than an in-memory journal scaffold.
- [x] Fake-wallet regtest still passes through the normal CDK payment backend interface.
- [x] Mint, melt, and swap federation tests still pass after fakewallet-specific naming is removed from federation semantics.
- [x] Module split/refactor keeps focused federation tests passing without behavioral changes. `cargo test -p cdk-common --features federation federation::consensus::tests -- --test-threads 1` passed 33 tests after extracting the consensus test module.
- [x] `rg "scaffold|placeholder|MVP"` in production federation modules only finds intentional dev/test documentation or open future-work comments.
- [x] Standalone fakewallet mint tests remain unchanged and prove fakewallet is still available as a non-federation development backend.
- [x] Documentation states that federation is payment-backend agnostic and fakewallet is only a development backend for mintd/regtest.

## Milestone 24: security review checklist

Objective: make the design reviewable before mainnet use.

Implementation checklist:

- [x] Implement default threshold rules and validation.
- [x] Complete formal review of threshold choice and BFT assumptions.
- [x] Implement canonical JSON operation hashing.
- [x] Complete formal review of operation canonicalization and transcript binding, including the binary-encoding decision and dedicated SIG_ALL transcript behavior.
- [x] Complete final review of all paths that can call `blind_sign` or `blind_sign_share`.
- [x] Public handlers are wired through federation paths in federation mode.
- [x] Complete final security review proving no public endpoint can bypass consensus in federation mode.
- [x] Complete final review and/or tests for NUT-20 signature verification in federated batch issuance.
- [x] Complete final review and/or tests for P2PK, HTLC, and SIG_ALL transcript construction.
- [x] Review real payment observation policies.
- [x] Implement DB conflict keys and initial federation indexes.
- [x] Complete final DB conflict-key and index review after the signature-share schema and query-plan checks are done.
- [x] Review member identity authentication.
- [x] Implement size/admission limits for oversized consensus items.
- [x] Complete full DoS review for huge witnesses, huge batches, repeated invalid ops, slow consensus, and restore spam.

Tests and success criteria:

- [x] Operation IDs are tied to share generation in federation apply paths.
- [x] Ensure an independent reviewer can trace every signature share to an accepted operation hash, output index, keyset, member ID, accepted journal index, and consensus evidence after the signature-share persistence decision is finalized.
- [x] Add federation operation fuzz targets for operation decoding and request conversion.
- [x] Attack regression tests exist.
- [x] Re-run and record the full attack regression suite.
- [x] Code paths appear consensus-gated in federation mode.
- [x] Add executable source audit proving production Axum public handlers do not call direct signing helpers and preserve consensus-before-response ordering for mint, swap, and melt.
- [x] Complete final security-review signoff that no route signs directly in federation mode.

## MVP cut lines

MVP 1: local deterministic federation

- Trusted-dealer key shares.
- In-memory or single-process consensus.
- Fake wallet mint quote payment.
- Mint issuance and swap.
- Fake-wallet melt state-machine tests without networked member processes.
- Federated wallet connector aggregation.
- Sliding-window regression test.

MVP 2: networked development-backend federation

- Separate member processes.
- AlephBFT strict-order consensus and idempotent mint/swap/melt APIs from Milestone 4A.
- SQLite federation DB.
- Member restart and catch-up.
- Fake-wallet melt reserve, pending, failure, finalize, and change signing.
- Restore and check-state.
- Regtest tooling.

MVP 3: payments and production hardening

- Real payment observation policy.
- Production cleanup and payment-backend isolation.
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
12. Add Milestone 4A AlephBFT strict-order consensus, idempotent federation APIs, and the cdk-mintd fake federation harness.
13. Add development-backend melt reserve, pending, failure, finalize, and change signing.
14. Add restore/check-state.
15. Add state convergence, catch-up certification, and database-equivalence gates.
16. Add network chaos, catch-up, and payment-observation recovery tests.
17. Add real payment observation policy and DKG.
18. Add production cleanup and payment-backend isolation.
