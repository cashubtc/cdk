# Federated CDK BDK-FROST on-chain wallet implementation plan

This document is the working implementation checklist for adding a Bitcoin
on-chain wallet jointly controlled by a CDK mint federation. It is intentionally
separate from the general federated Cashu implementation plan: the Cashu BLS
keysets secure e-cash issuance, while this plan introduces an independent
secp256k1 FROST root key hierarchy whose first application is receiving and
spending Bitcoin through the federated CDK BDK backend. The DKG result is
application-neutral so the same threshold shares can later be used, through
reviewed application adapters, by other payment backends and compatible FROST
applications without reconstructing the root secret.

The target design uses BDK as a watch-only descriptor wallet, chain index,
transaction builder, and PSBT engine. AlephBFT orders and authorizes every
safety-critical wallet state transition. The CDK on-chain FROST adapter produces
BIP340-compatible threshold signatures only for transactions already accepted
by consensus; future adapters must enforce their own registered authorization
contracts. No member, transaction proposer, signing coordinator, chain-data
provider, application adapter, or public API server is individually trusted
with custody authority.

## Integration constraints

Existing federation mint business logic, consensus ordering, payment
observations, quote handling, sagas, recovery, and catch-up are expected to be
complete and must be changed as little as possible. Integrate the federated BDK
wallet through the existing payment and federation interfaces instead of
reimplementing or broadly refactoring mint behavior. If implementation reveals
a genuine missing piece outside the FROST BDK wallet, record the gap in this
document and attempt the smallest focused fix in the owning shared domain,
covered by standalone and federation regression tests.

Keep the secp256k1 FROST DKG and reusable key manager isolated in dedicated,
neatly organized modules with narrow typed interfaces. Reuse only the existing
ceremony transport, authentication, persistence, roster, and activation
infrastructure; do not mix FROST cryptographic state with Cashu BLS shares or
scatter FROST conditionals through existing mint code. Federation setup is
successful only when its independent Cashu BLS DKG and FROST root DKG both
complete, verify, persist, and activate successfully. Failure or mismatch in
either result, including a FROST signing threshold different from the
federation BLS `signature_threshold`, must fail closed and must not leave the
federation partially ready.

## Encountered bugs and things to remember

> Keep this section below 500 words. Record the milestone, symptom, verified
> cause, fix or open question, and preserving regression. Move long
> investigations to issues, benchmarks, or commit descriptions.

- Cashu BLS shares and member identity keys cannot become Bitcoin custody
  shares. Reuse ceremony/authentication infrastructure only; persist the
  untweaked FROST root package, apply BIP32/BIP341 at the application boundary,
  and never reconstruct the group secret.
- BDK's custom signer API is synchronous. Do not hide asynchronous FROST rounds
  or consensus waits inside `Wallet::sign()`.
- Never call `reveal_next_address()` independently in a public request handler.
  Consensus allocates address indexes; replay and catch-up reveal through an
  accepted target index idempotently.
- FROST nonces are one-time secrets. Persist reservation/consumption before
  exposing commitments, and never reuse a nonce after uncertainty or restart.
- A transaction proposer is untrusted. Consensus must accept the exact unsigned
  transaction before any member releases a signature share.
- Milestone 4 requires every DKG phase's outbound batch to be acknowledged
  before advancement. Keep confidential round-two retry payloads transient and
  durable records sealed/context-bound; preserve partition, response-loss, and
  ciphertext tests.
- Milestone 13 key-manager construction is a restart boundary and burns every
  nonterminal nonce record. Wire one long-lived manager per participant process;
  never reconstruct it per REST request or payment backend.
- Application derivation, authorization, nonces, transcripts, descriptors, and
  signing targets must bind the application key epoch. Preserve wrong-epoch
  rejection and epoch-separated descriptor vectors.
- Milestone 15 found that a pre-validation signed-transaction conflict key let
  an invalid ordered record block a later valid signature. Reserve no such key;
  accepted-state verification makes the first valid record durable and rejects
  later differing replays.
- Milestone 17 found that the generic elected-member payment observer would
  still call `make_payment` for a FROST on-chain melt. Bypass that side effect
  only for the joint wallet and derive its normal paid observation from the
  confirmed consensus transaction receipt; preserve every standalone path.
- Milestone 18 found that FROST routes and wallet side-effect loops checked
  local authorization or BDK health but not federation catch-up readiness.
  Gate commitments, shares, observations, and broadcast on the existing sync
  state before any nonce creation or consensus submission.
- Milestone 19 found that an active nonce slot required the failed coordinator
  to abort before failover. Persist a per-slot attempt high-water mark, burn a
  superseded nonce atomically, permit exact consumed commitment/share replay,
  and reject future or delayed attempts; preserve the reopen and coordinator-
  loss regressions.
- Milestone 25 found a failed-melt catch-up test whose fixed operation elected
  the other member, so the local observer correctly made no payment. Select a
  locally elected operation deterministically in that fixture; no mint business
  logic change was needed, and the focused catch-up regression now passes.
- Milestone 25 found vulnerable Hickory in BIP353 while Tor trusted resolver
  JSON. One transport-neutral resolver now builds bounded proofs over ordinary
  or Tor HTTPS and validates DNSSEC locally; preserve live smoke coverage.

## 1. Problem and setting

Federated CDK already coordinates Cashu mint state and BLS blind-signature
shares, but Bitcoin held by an on-chain payment backend is a separate custody
problem. The current `cdk-bdk` backend derives a local BIP84 wallet from one
mnemonic, constructs transactions with BDK, signs them locally, and broadcasts
them. Running that backend independently on every federation member would
create unrelated wallets and would not give the federation joint custody.
Electing one member to own the BDK mnemonic would preserve a single custodian
and make that member a liveness bottleneck.

The federation instead needs one logical Bitcoin wallet whose public receiving
keys are known by every member, whose private signing authority is distributed,
and whose transaction state is agreed through the same consensus system that
protects Cashu liabilities. At least the configured signing threshold must
cooperate to produce a valid Bitcoin signature. Fewer members must learn
nothing sufficient to spend. Every member must independently reject a proposal
that pays the wrong destination, spends an unauthorized UTXO, overpays fees,
creates invalid change, or was not accepted by federation consensus.

The intended deployment supports on-chain Cashu mint quotes and melt quotes:

- Incoming Bitcoin deposits receive unique federation-controlled addresses.
- Confirmed deposits become payment observations and can satisfy mint quotes.
- Outgoing on-chain melts become authorized payment intents.
- One member may construct a candidate transaction, but all members validate
  it and AlephBFT orders the exact accepted transaction.
- A threshold of members performs two-round FROST signing for each Taproot
  key-path input.
- Any member may finalize and broadcast the fully signed transaction.
- Every member can recover the same wallet state after restart or catch-up.

## 2. Goals

- Give the federation threshold control of Bitcoin without placing the full
  private key on any member.
- Produce ordinary BIP340 Taproot key-path signatures and compact P2TR spends.
- Reuse BDK for descriptors, address discovery, transaction graphs, coin
  selection, PSBT construction, fee estimation, and chain synchronization.
- Reuse CDK's existing `MintPayment`, on-chain quote, payment observation,
  AlephBFT journal, private signed REST, catch-up, database, and saga plumbing.
- Preserve standalone `cdk-bdk` behavior and existing non-federated mints.
- Make progress through bounded milestones that can be tested independently.
- Keep coordinators replaceable and untrusted; safety must not depend on which
  member proposes a transaction, collects shares, or broadcasts it.
- Support Bitcoin Core RPC and Esplora-compatible lightweight chain sources.
- Establish explicit key epochs so future wallet-key and federation-member
  rotations can be implemented without redesigning every persisted record.
- Produce a versioned, application-neutral secp256k1 FROST root key epoch whose
  shares can be reused by future payment backends and compatible FROST
  applications through domain-separated derivation and authorization adapters.

## 3. Non-goals for the first production release

- Deriving the Bitcoin key from the Cashu BLS key shares.
- Reconstructing or exporting the aggregate FROST private key.
- Hardened BIP32 derivation below the distributed account-level key.
- General dynamic federation membership or same-key addition of new members.
- Trusting mempool observations as final payment confirmation.
- Depending on deterministic BDK coin selection across all members.
- Supporting arbitrary Taproot script trees in the first release.
- Hardware-security-module support before the software share lifecycle is
  correct and covered by fault-injection tests.
- Silent payments, payjoin, coinjoin, or arbitrary PSBT co-signing interfaces.
- Cross-input signature aggregation.
- Implementing non-CDK payment backends or arbitrary third-party FROST
  applications in the first production release; the first release must provide
  and test the reusable root-key boundary they will consume.
- A plaintext secret-share export or unrestricted generic signing API. Reuse is
  provided through the participant-local key manager or an explicitly designed,
  encrypted migration format and application policy.

## 4. Security requirements

### Custody and key separation

- The secp256k1 FROST root key must be generated independently from Cashu BLS
  keysets and federation identity keys.
- Each member stores only its own canonical root key share and public group
  metadata. Application key packages are derived views and must not require a
  new durable copy or reconstruction of the root secret.
- The group secret must never exist in one process, database, log, test output,
  configuration export, or support bundle.
- DKG private messages must use authenticated confidential channels and be
  bound to federation ID, root key epoch, ceremony ID, sender, and receiver.
- Public root configuration must bind the DKG protocol and version, FROST
  ciphersuite, threshold, participant identifiers, verification shares,
  untweaked group key, root key epoch, and transcript hash.
- Application configuration must separately bind its application identifier,
  derivation/tweak version, derived public key package, and application-specific
  data such as chain code, descriptors, network, and policy.
- FROST identifiers must have a canonical one-to-one mapping to CDK member IDs.

### Application reuse and authorization

- Every use of a root share must be assigned a stable, versioned application
  identifier and a domain-separated derivation and signing context.
- A participant-local key manager owns the root share and all nonce state. An
  application adapter requests derived public data, commitments, and signature
  shares without receiving the plaintext root share.
- Each application adapter must independently authorize the exact signing
  transcript before the key manager releases a signature share. The CDK on-chain
  adapter requires an accepted AlephBFT transaction; future applications must
  define an equally explicit authorization policy.
- The key manager must not expose a sign-arbitrary-digest operation. Requests
  must bind the root epoch, application ID, application key epoch, derivation
  context, protocol/ciphersuite version, and application authorization proof.
- Reusing one root across applications creates a shared compromise and rotation
  domain. Public additive child derivation provides namespace separation, not
  compromise isolation, so derived aggregate private keys must never be
  reconstructed or exported.
- Applications that require access to a complete derived private key, use an
  incompatible curve or signing scheme, or cannot satisfy the shared lifecycle
  rules require a separate DKG.

### Consensus before signing

- No member may release a FROST signature share for an unsigned transaction
  that has not reached accepted AlephBFT consensus.
- Consensus must commit to the complete unsigned transaction and every value
  needed to calculate its Taproot sighashes.
- Members must derive the signing digest themselves from the accepted
  transaction; a coordinator-supplied digest alone is insufficient.
- Conflicting proposals for the same payment batch or reserved UTXO set must be
  serialized by conflict keys and cannot both reach signing.
- Catch-up and replay must never execute an outgoing payment side effect twice.

### Transaction validation

- Every input must be a currently spendable federation UTXO in accepted state.
- Every recipient output must correspond exactly to an authorized payment
  intent, including amount, network, and destination script.
- Change must pay a consensus-allocated internal federation address.
- No unknown or unauthorized outputs, inputs, PSBT fields, sighash modes, or
  scripts may be accepted.
- Fees, feerate, dust, transaction weight, locktime, sequence, RBF, batching,
  and maximum-input policies must be explicit and consensus-versioned.
- Transaction validation must use integer satoshi accounting and checked
  arithmetic.

### Nonce and signing-session safety

- Each application signing slot and signing attempt must use fresh FROST nonce
  material; a CDK transaction uses a separate slot for every input.
- A nonce is durably reserved before its commitment leaves the member.
- Reserved nonces are burned after uncertainty, abort, timeout, restart, signer
  substitution, or any round-two attempt.
- Signing messages must bind federation ID, root key epoch, application ID,
  application key epoch, accepted operation ID, transaction ID, input index,
  sighash type, attempt number, and selected signer set.
- Members must verify all round-one commitments and the complete signing
  package before producing round-two shares.
- Invalid shares must identify and isolate the offending response without
  weakening the required threshold.

### Chain and payment safety

- BDK local chain state is an observation cache, not federation consensus.
- Deposits become payable only after the configured confirmation rule and the
  required federation observation policy are satisfied.
- Reorganizations must reverse only state that is explicitly designed to be
  reversible; already issued Cashu liability requires conservative handling
  and operator-visible insolvency protection.
- A shared public Esplora service is a correlated dependency and must not be
  mistaken for independent quorum evidence.
- Broadcast errors must distinguish deterministic rejection, retryable
  transport failure, already-known success, and unresolved ambiguity.

### Availability and recovery

- The wallet remains usable while fewer than the allowed number of members are
  offline, provided consensus and signing thresholds remain available.
- Proposer, signing-coordinator, chain-source, and broadcaster roles must all
  support failover without changing the accepted transaction.
- All operation phases must be restart-safe, idempotent, and reconstructable
  from durable state.
- A caught-up member must verify wallet state through the existing federation
  catch-up and materialized-state digest mechanisms before signing.

## 5. Code requirements

- Keep standalone `CdkBdk` behavior unchanged unless explicitly configured for
  federation mode.
- Do not fork BDK. Use upstream descriptor, transaction graph, PSBT, and chain
  APIs behind narrow CDK-owned adapters.
- Do not implement FROST primitives from scratch. Use a reviewed
  BIP340/BIP341-compatible library, initially evaluating
  `frost-secp256k1-tr`.
- Keep a CDK-owned canonical representation of the untweaked root share and
  public package. Treat library key packages as adapters, not as the durable or
  cross-application interchange format.
- Put root-share storage, application registration, derivation, authorization,
  and nonce ownership behind a reusable federation FROST key-manager interface;
  do not make them responsibilities of `cdk-bdk`.
- Isolate ciphersuite conversion, key derivation tweaks, nonce handling,
  signing-package validation, and signature aggregation behind typed APIs.
- Do not put network waits or AlephBFT submission inside BDK's synchronous
  signer traits.
- Keep canonical consensus types in `cdk-common`; keep mint business validation
  in `cdk`; keep HTTP transport and live runtime ownership in `cdk-axum`; keep
  daemon wiring in `cdk-mintd`.
- Prefer extracting reusable watch-only BDK services over copying the current
  receive, send, fee, sync, storage, and recovery implementations.
- Persist consensus state through existing database traits and implement SQLite
  and PostgreSQL consistently.
- Include new materialized wallet state in federation state digests and catch-up
  verification.
- Use versioned canonical encodings for every consensus operation and signing
  transcript.
- Redact secret shares, nonce material, PSBT proprietary secrets, local paths,
  credentials, and sensitive transaction context from logs and debug output.
- Add feature flags and configuration validation so incomplete federation
  on-chain support fails closed.
- Use focused instrumentation while investigating performance, then remove it
  or retain only bounded production-grade metrics.
- Keep public APIs documented and follow workspace lint, formatting, error,
  tracing, and import conventions.
- Commit each coherent, verified milestone using conventional commits; do not
  combine protocol, schema, refactor, and documentation changes unnecessarily.

## 6. Existing CDK and BDK plumbing

The implementation should reuse these existing surfaces rather than create a
parallel payment system:

- `crates/cdk-bdk/src/lib.rs`: `CdkBdk`, persisted BDK wallet, background
  synchronization, `MintPayment` implementation, and local mnemonic setup.
- `crates/cdk-bdk/src/receive/`: tracked receive addresses and durable receive
  intents.
- `crates/cdk-bdk/src/send/`: payment intents, transaction batching,
  build/sign/broadcast progression, compensation, and recovery.
- `crates/cdk-bdk/src/chain/`: Bitcoin Core RPC and Esplora sync, fee lookup,
  and transaction broadcast.
- `crates/cdk-common/src/payment.rs`: `MintPayment`, incoming/outgoing on-chain
  options, payment identifiers, status, and event stream contracts.
- `crates/cdk-common/src/federation/`: roster, threshold parameters, DKG
  transport, authenticated REST payloads, operations, journal application
  state, state digests, and catch-up types.
- `crates/cdk/src/mint/federation.rs`: deterministic federation business
  application, effective config, payment actor selection, and quote lifecycle.
- `crates/cdk-axum/src/federation/`: AlephBFT runtime, private signed REST,
  operation admission, finalization, activity wake, catch-up, and payment
  observations.
- `crates/cdk-mintd/src/setup.rs`: BDK payment backend configuration and daemon
  assembly.
- `crates/cdk-sql-common`, `cdk-sqlite`, and `cdk-postgres`: durable mint and
  federation state.
- `crates/cdk-prometheus`: federation and payment-backend metrics.

The current BDK backend hard-codes mnemonic-derived BIP84 external/internal
descriptors, calls `Wallet::build_tx()`, calls `Wallet::sign()` locally, applies
the resulting unconfirmed transaction, persists batch state before broadcast,
and reconciles confirmations. The federated implementation should preserve the
useful saga and recovery structure while replacing local key ownership and the
single-call signing phase.

## 7. Target architecture

### Authority boundaries

1. **BDK watch-only engine:** derives public scripts, tracks wallet-relevant
   transactions, proposes coin selection, builds PSBTs, calculates fees, and
   finalizes signed transactions.
2. **Federation wallet state machine:** owns accepted address allocations,
   deposit observations, UTXOs, payment intents, reservations, accepted
   transactions, replacements, and terminal outcomes.
3. **AlephBFT:** orders all safety-critical wallet operations and resolves
   conflicts.
4. **Federation FROST key manager:** stores one application-neutral root share,
   derives application packages, owns one-time nonces across all applications,
   and releases shares only through a registered authorization adapter.
5. **CDK on-chain FROST adapter:** derives the BDK wallet key hierarchy and
   authorizes signing only for transactions accepted by AlephBFT.
6. **Private REST transport:** carries authenticated DKG and FROST round
   messages. These messages do not create a second consensus protocol.
7. **Chain sources:** provide untrusted local observations and broadcast
   transport. Consensus determines which observations affect liabilities.

### Key hierarchy

- One independent FROST DKG creates an untweaked, application-neutral
  secp256k1 root key for a root key epoch.
- A stable application identifier and versioned public derivation convention
  produce a CDK on-chain application key package from that root without
  reconstructing it.
- The CDK application package adds its public chain code and non-hardened
  derivation convention to produce separate external and internal keychains
  suitable for BDK wildcard descriptors.
- Consensus allocates monotonically increasing indexes per CDK application key
  epoch and keychain.
- Every member uses the same watch-only descriptors and can independently
  derive every accepted deposit and change address.
- Root-to-application derivation, threshold BIP32 child-share derivation,
  parity normalization, and BIP341 Taproot tweaking must have a written order
  and cross-implementation test vectors before use with value.
- The root key manager remains reusable; BDK descriptors, Bitcoin network,
  transaction policy, and AlephBFT authorization belong only to the CDK
  on-chain application package.

### Outgoing transaction flow

1. Consensus records authorized on-chain payment intents.
2. Any eligible proposer selects a bounded batch and builds a candidate PSBT.
3. Every member independently validates the candidate against accepted state
   and policy.
4. AlephBFT accepts one exact unsigned transaction and reserves its inputs.
5. Members exchange FROST round-one commitments for each input.
6. A coordinator forms a signing package bound to the accepted transaction.
7. A threshold returns verified round-two signature shares.
8. Any member aggregates signatures, finalizes the PSBT, persists the signed
   transaction, and broadcasts it.
9. Chain observations advance the transaction through mempool, confirmation,
   replacement, or failure states.
10. Existing melt payment observation and saga behavior receives the resulting
    canonical status without a separate wallet-recovery system.

## 8. Milestone checklist

### Milestone 0: freeze the protocol model and baseline existing behavior

**Objective:** establish reviewed boundaries and reproducible baselines before
changing production behavior.

**Work:**

- [x] Write an architecture decision record defining BDK as watch-only engine,
  AlephBFT as authorization, and external asynchronous FROST as signing.
- [x] Decide and document the reusable-root boundary: the DKG produces an
  untweaked secp256k1 FROST root epoch, while CDK on-chain and future consumers
  use registered, domain-separated application packages.
- [x] Document the compatibility boundary: reuse requires compatible
  secp256k1 Schnorr/FROST algebra and a reviewed adapter; incompatible curves,
  schemes, private-key export requirements, or lifecycle policies require a
  separate DKG.
- [x] Document the threat model, tolerated faulty-member counts, consensus
  threshold, Bitcoin signing threshold, and whether the two thresholds must be
  equal.
- [x] Decide the first-release Bitcoin networks and confirmation policies.
- [x] Inventory every `CdkBdk` responsibility and map it to reusable,
  standalone-only, or federated-only ownership.
- [x] Record baseline standalone BDK mint and melt behavior, database rows,
  transaction bytes, recovery behavior, and timing on regtest.
- [x] Record baseline federation on-chain behavior or explicit unsupported
  errors before adding the new backend.
- [x] Define feature flags and configuration names without enabling incomplete
  behavior.
- [x] Review the local Fedimint wallet implementation for transaction proposal,
  signature collection, UTXO state, fee consensus, and recovery lessons while
  preferring simpler CDK-native boundaries.

**Success criteria:**

- [x] Reviewers can identify one owner for every address, UTXO, proposal,
  signature, broadcast, and confirmation transition.
- [x] The signing threshold and consensus threshold relationship is explicit
  and cannot silently drift through configuration.
- [x] Reviewers can identify which state belongs to the reusable root key
  manager and which belongs only to the CDK on-chain application.
- [x] Baseline commands and results are captured in a dedicated benchmark or
  test note.
- [x] No production behavior has changed.

**Required tests:**

- [x] Run focused `cdk-bdk` unit tests.
- [x] Run existing standalone on-chain integration tests.
- [x] Run relevant federation mint/melt and payment-observation tests.
- [x] Verify formatting and checks for all crates expected to be touched.

**Milestone record:** completed on 2026-07-13. See
`docs/federated-cashu-bdk-frost-architecture-decision.md` for the frozen
ownership, threshold, root/application, feature, and configuration boundaries,
and `docs/federated-cashu-bdk-frost-baseline.md` for commands, results, timings,
database shapes, transaction-byte measurements, recovery coverage, and current
unsupported federation-on-chain semantics.

### Milestone 1: extract a reusable BDK watch-only engine

**Objective:** separate descriptor/chain/transaction functionality from local
mnemonic ownership without changing standalone behavior.

**Work:**

- [x] Introduce a typed descriptor configuration accepted by the reusable BDK
  engine.
- [x] Support loading and creating a persisted watch-only BDK wallet from
  external and internal public descriptors.
- [x] Extract chain sync, fee estimation, wallet persistence, transaction graph,
  receive scanning, and broadcast behind reusable components.
- [x] Separate PSBT construction from PSBT signing and finalization.
- [x] Keep the existing mnemonic/BIP84 constructor as a thin standalone adapter.
- [x] Keep existing `MintPayment` behavior and event semantics unchanged for
  standalone `CdkBdk`.
- [x] Define a narrow interface for applying accepted address indexes and
  accepted signed transactions to BDK local state.
- [x] Prevent watch-only code paths from calling `Wallet::sign()`.
- [x] Avoid copying send/receive saga code; extract only abstractions proven
  necessary by the federated caller.

**Success criteria:**

- [x] A watch-only wallet can derive addresses, sync, list UTXOs, build a PSBT,
  accept externally supplied signatures, finalize, and broadcast.
- [x] Standalone BDK produces equivalent behavior and passes all prior tests.
- [x] The watch-only database contains no mnemonic, xprv, FROST share, or nonce.
- [x] No async federation operation is hidden inside a BDK signer callback.

**Required tests:**

- [x] Create/load/restart watch-only external and internal descriptors.
- [x] Reveal addresses through a target index and verify idempotent persistence.
- [x] Build an unsigned PSBT without private keys.
- [x] Insert an externally generated valid signature and finalize the PSBT.
- [x] Reject missing, malformed, wrong-key, and wrong-sighash signatures.
- [x] Run all existing `cdk-bdk` tests unchanged.
- [x] Run standalone BDK regtest mint, melt, restart, and rebroadcast tests.

**Milestone record:** completed on 2026-07-13. `BdkWalletEngine` now owns
descriptor validation, watch-only persistence, one-shot chain sync, fee/UTXO
snapshots, address-index application, PSBT construction, external Taproot
verification/finalization, accepted-transaction application, and broadcast.
The existing mnemonic-backed `CdkBdk` is a local-signing adapter over the same
engine. The focused watch-only suite passed 5 tests, the full crate passed 104
tests, the SQLite Bitcoin Core regtest suite passed 14 tests, and no-dependency
clippy passed with warnings denied. Workspace dependency clippy remains blocked
by pre-existing warnings in `cashu`; no unrelated lint changes were included.

### Milestone 2: select FROST and define the reusable root-key boundary

**Objective:** define a portable secp256k1 FROST root representation and prove
that reviewed application adapters can produce Bitcoin-valid signatures from
it without reconstructing or exporting the root secret.

**Work:**

- [x] Evaluate `frost-secp256k1-tr` version, maintenance, audit history,
  serialization stability, MSRV, dependencies, DKG, refresh, and Taproot tweak
  support.
- [x] Specify a CDK-owned canonical root key package containing the root key
  epoch, DKG protocol/version, ciphersuite, threshold, participant identifier,
  local untweaked signing share, participant verification shares, untweaked
  group key, and transcript binding.
- [x] Define strict construction of library-specific key packages from the
  canonical root package without using a reconstructed aggregate secret.
- [x] Determine exactly when the selected library performs even-Y normalization,
  ciphersuite `post_dkg`, BIP341 tweaks, or other application processing. None
  of those processed packages may silently replace the canonical root package.
- [x] Define a reusable participant-local key-manager API for application
  registration, public derivation, nonce commitment, authorized signature-share
  production, and aggregate verification.
- [x] Confirm signatures verify under rust-bitcoin/secp256k1 BIP340 verification,
  not only the FROST library's verifier.
- [x] Add a dedicated crypto module or crate with typed member identifiers,
  key packages, public packages, commitments, signature shares, and aggregate
  signatures.
- [x] Domain-separate the root DKG, CDK application derivation, nonce, signing,
  and transcript hashes.
- [x] Domain-separate the root DKG from every application and require an
  application ID and derivation version on every derived package and signing
  session.
- [x] Implement strict serialization wrappers with bounded input sizes.
- [x] Ensure all secret-bearing types redact `Debug` and are zeroized where the
  selected libraries support it.
- [x] Define errors that distinguish malformed input, invalid share, wrong
  participant, wrong session, insufficient threshold, and internal failure.
- [x] Prohibit any production API that reconstructs the group secret.
- [x] Prohibit plaintext root-share export and sign-arbitrary-digest APIs while
  leaving a versioned encrypted migration path as an explicit future design
  boundary.

**Success criteria:**

- [x] A threshold signature generated by the wrapper verifies as an ordinary
  BIP340 signature under the aggregate x-only key.
- [x] The canonical root package is not tied to BDK descriptors, Bitcoin
  network, AlephBFT operations, or a library's post-DKG serialization.
- [x] Two application adapters can derive different domain-separated public
  packages and sign through the same root shares while using one key-manager
  nonce namespace and without reconstructing a private key.
- [x] Fewer than threshold shares cannot produce a signature through any public
  API.
- [x] Serialized messages round-trip canonically and reject trailing or
  oversized data.
- [x] Secret material does not appear in normal logs or debug formatting.

**Required tests:**

- [x] Trusted-dealer test harness for 2-of-3, 3-of-5, 8-of-11, and 14-of-20.
- [x] Valid signing with different threshold signer subsets.
- [x] Threshold-minus-one failure.
- [x] Invalid participant identifier and duplicate participant rejection.
- [x] Invalid commitment and signature-share rejection.
- [x] Wrong message, wrong group key, wrong signer set, and wrong ciphersuite
  rejection.
- [x] BIP340 interoperability test using rust-bitcoin/secp256k1 verification.
- [x] Canonical root-package test vectors imported by two independent adapter
  implementations, including at least one non-BDK test adapter.
- [x] Application-domain separation tests: the same request under another
  application ID, derivation version, or authorization policy is rejected.
- [x] Serialization, fuzz, and property tests for all public wire wrappers.

**Milestone record:** completed on 2026-07-13. The new isolated `cdk-frost`
crate pins `frost-secp256k1-tr` 3.0.0 behind a CDK-owned canonical untweaked
root format, additive application boundary, typed authorization adapter,
single-use nonce API, strict public wire encodings, and BIP340 aggregation. It
contains no BDK, Bitcoin-network, Cashu mint, AlephBFT, database, or transport
dependency and exposes no root reconstruction, plaintext share export, or
direct digest-signing API. The 9-test suite covers 2-of-3, 3-of-5, 8-of-11,
14-of-20, signer subsets, threshold-minus-one, invalid participants,
commitments and shares, cross-session/application/version failures, wrong
message/group/signer set/ciphersuite, stable cross-adapter vectors, redaction,
and rust-bitcoin BIP340 verification. The dedicated FROST wire fuzz target
compiles. See `docs/federated-cashu-frost-crypto-review.md` for dependency,
post-DKG/even-Y/tweak, compromise-domain, and residual-audit findings. The
upstream audit did not cover the Taproot ciphersuite, so milestone 3 focused
review and later independent security gates remain mandatory. `cargo check
--workspace` passes. The broader pre-existing `--all-targets` check still fails
in untouched federation test fixtures whose quote initializers lag their current
Cashu response types; this milestone does not mask or expand that unrelated
failure.

### Milestone 3: specify and prove application-scoped threshold derivation

**Objective:** derive a CDK on-chain application key and BDK-compatible external
and internal Taproot keychains from the reusable root shares without
reconstructing either the root or a derived aggregate private key.

**Work:**

- [x] Assign the CDK on-chain backend a stable application ID and write the exact
  root-to-application derivation, application public key, public chain-code,
  non-hardened child derivation, x-only normalization, and Taproot tweak
  specification.
- [x] Define the complete derivation order: untweaked root group key,
  application-domain derivation, account/application group key, non-hardened
  BIP32 child, even-Y normalization at specified boundaries, then BIP341
  output-key tweak.
- [x] Define external branch `0`, internal branch `1`, allowed index range, and
  invalid-child handling.
- [x] Prove or source the rule for applying child derivation tweaks to FROST
  signing and verification shares.
- [x] Determine how `frost-secp256k1-tr` post-DKG processing interacts with
  root-to-application derivation, child derivation, and Taproot tweaking; do not
  proceed on an assumption or apply its post-DKG output as the root package.
- [x] Construct a canonical public extended key representation accepted by BDK
  watch-only wildcard descriptors.
- [x] Implement derivation through a narrow typed API rather than scattered
  scalar arithmetic.
- [x] Produce stable test vectors containing public config, branch/index,
  application ID/version, application public key, address, child verification
  shares, sighash, aggregate signature, and final verification result.
- [x] Document that public additive application/child derivation does not isolate
  compromise: exposure of a complete derived private key can compromise the
  root and every application that uses it.
- [x] Obtain focused cryptographic review before connecting derived keys to
  wallet value.

**Success criteria:**

- [x] Every member derives identical public addresses from public config alone.
- [x] Every member derives an identical CDK application public package from the
  canonical root public package and CDK application metadata.
- [x] Every member derives the same child verification package.
- [x] A threshold using derived child shares signs for the corresponding BDK
  P2TR descriptor key.
- [x] Signatures fail for the adjacent branch, index, epoch, or Taproot tweak.
- [x] Signatures and derived keys fail across application IDs and application
  derivation versions.
- [x] No hardened child derivation is accepted.

**Required tests:**

- [x] External and internal derivation vectors at indexes 0, 1, boundary values,
  and representative high indexes.
- [x] Independent BDK descriptor address equals CDK derivation result.
- [x] Child signature verifies against BDK's derived output key.
- [x] Wrong branch/index/chain code/network/root epoch/application key epoch
  rejection.
- [x] Invalid-child retry behavior is deterministic across all members.
- [x] Property test that arbitrary valid indexes produce matching public and
  threshold-private derivation results.
- [x] Cross-version vector test preventing accidental derivation changes.
- [x] Cross-application vector test proving that a second test application can
  reuse the root package while producing a distinct key and rejecting CDK
  signing transcripts.

**Milestone record:** completed on 2026-07-13. The application-neutral
`cdk-frost` boundary now accepts a bounded additive signing plan and binds its
hash into nonce commitments and signature shares. The isolated
`cdk-frost-bip32` adapter defines the versioned CDK application, deterministic
chain code, non-hardened `0/*` and `1/*` BIP32 derivation, BDK descriptors,
BIP340/BIP341 order, and deterministic invalid-child retry. Ten generic FROST
tests and four Bitcoin adapter tests pass, including BDK address equality,
256 property indexes, stable vectors, threshold Taproot signing, and domain
separation. Focused implementation review and residual risks are recorded in
`docs/federated-cashu-frost-bip32-derivation.md`; it is explicitly not an
independent external audit, and later security gates remain open.

### Milestone 4: run DKG for an application-neutral FROST root key epoch

**Objective:** run a real secp256k1 FROST DKG using existing authenticated
ceremony plumbing, persist portable untweaked root shares, and keep BLS, root
FROST, and derived application products independent.

**Work:**

- [x] Add FROST root-key ceremony identifiers and explicit root-key-epoch
  metadata.
- [x] Extend or compose the existing DKG phase driver with FROST-specific round
  messages rather than overloading BLS payloads.
- [x] Bind every DKG message to federation ID, roster, threshold, member ID,
  ceremony ID, root key epoch, DKG protocol/version, and ciphersuite.
- [x] Reuse signed private REST sender/receiver authentication and replay
  protection.
- [x] Persist local secret packages before acknowledging receipt or advancing a
  phase.
- [x] Verify all proofs, commitments, participant sets, and resulting public
  packages independently at every member.
- [x] Require transcript equality and threshold/member activation confirmation.
- [x] Produce and persist the canonical untweaked root public package and one
  canonical local root share per member before constructing any CDK, BDK,
  BIP32, BIP341, or other application-specific package.
- [x] Do not treat a ciphersuite `post_dkg` result or opaque library serialization
  as the root DKG result. Use only reviewed conversion code to construct such
  application packages from the canonical root package.
- [x] Store the root share in the participant-local federation FROST key manager,
  not in `cdk-bdk` or a payment-backend-specific configuration.
- [x] Add startup validation matching private share, public verification share,
  untweaked group key, threshold, roster, transcript, DKG/ciphersuite version,
  and root epoch.
- [x] Keep BLS DKG behavior and Cashu keysets unchanged.

**Success criteria:**

- [x] Fresh 3-of-5, 8-of-11, and 14-of-20 ceremonies converge on identical
  canonical public root configuration.
- [x] Every member receives a distinct valid local share and no other member's
  share.
- [x] Restart at every persisted phase resumes without regenerating or changing
  the final key.
- [x] One invalid or equivocated package prevents activation and identifies the
  faulty sender.
- [x] BLS and FROST transcript domains cannot be confused.
- [x] The same activated root epoch can initialize the CDK on-chain adapter and
  a second test adapter without mutating the root package or creating an
  application-owned durable copy of the canonical root share.

**Required tests:**

- [x] Happy-path DKG at quick and production-size profiles.
- [x] Offline member before threshold completion.
- [x] Duplicate, reordered, delayed, replayed, and wrong-recipient messages.
- [x] Wrong federation, ceremony, epoch, roster, threshold, and ciphersuite.
- [x] Invalid proof/commitment/share and sender equivocation.
- [x] Crash/restart after every DKG round and before/after activation.
- [x] Transcript mismatch and activation mismatch fail closed.
- [x] Pre/post-processing regression test proving that even-Y normalization,
  Taproot tweaks, and application derivation do not mutate the stored root
  package.
- [x] Existing BLS DKG regression suite remains unchanged.

**Milestone record:** completed on 2026-07-13. `cdk-frost` now owns the
dealer-free secp256k1 root ceremony, canonical conversion, and encryption-only
restart boundary. Federation code composes distinct signed FROST round-one,
round-two, and all-member root-confirmation messages with the existing BLS DKG,
using the exact BLS `signature_threshold`; the terminal ceremony transition
reopens the local sealed share and requires both independently valid results.
Confidential round-two retry contributions are sealed at rest, and
SQLite/PostgreSQL share one equivocation-safe record model. Focused design,
test evidence, and residual risks are recorded in
`docs/federated-cashu-frost-dkg.md`; this is not an external audit, and later
security gates remain open.

### Milestone 5: define root and CDK application configuration and persistence

**Objective:** separate reusable root-key state from CDK on-chain application
state and make both explicit, versioned, validated, and catch-up safe.

**Work:**

- [x] Add versioned public root config containing root epoch, DKG protocol and
  ciphersuite versions, threshold, participant verification shares, untweaked
  aggregate key, participant mapping, and transcript binding.
- [x] Add private root config containing only the local member's canonical root
  share, its verification binding, and sealed-storage metadata.
- [x] Add a separately versioned CDK on-chain application config containing its
  application ID and key epoch, referenced root epoch, derivation version,
  derived aggregate/verification keys, chain code, descriptors or descriptor
  commitment, network, confirmation policy, and fee policy version.
- [x] Define active, retiring, and retired states independently for root epochs
  and application key epochs, plus an explicit registry of applications using
  each root epoch.
- [x] Add startup fail-closed validation for descriptor/group-key mismatch,
  wrong network, wrong root share, root/application derivation mismatch,
  incomplete transcript, unregistered application, or unsupported protocol
  version.
- [x] Add database records for root config snapshots, application config
  snapshots, registered consumers, and active root/application epochs.
- [x] Include public root and CDK application configs and active epochs in the
  federation config digest.
- [x] Include materialized wallet records in state snapshots and catch-up
  verification.
- [x] Ensure private shares and nonce records are excluded from public snapshots
  and operator status output.
- [x] Define clean undeployed-schema migrations for SQLite and PostgreSQL.

**Success criteria:**

- [x] Every honest member loads the same public wallet configuration and BDK
  descriptors.
- [x] A copied private root config from another member is rejected.
- [x] Public root plus application config is sufficient to derive addresses but
  insufficient to sign.
- [x] Adding a registered test consumer does not change the root package or CDK
  descriptors.
- [x] Wallet config divergence changes state/config digests and blocks readiness.

**Required tests:**

- [x] Root public/private and application config round trips and canonical
  digest vectors.
- [x] Wrong member, epoch, network, threshold, descriptor, transcript, and group
  key startup rejection.
- [x] Wrong application ID, root reference, derivation version, derived key, and
  unregistered-consumer rejection.
- [x] Missing private share and extra unknown participant rejection.
- [x] SQLite/PostgreSQL schema and migration tests.
- [x] Snapshot/catch-up equality with wallet config present.
- [x] Secret-redaction tests for debug, JSON, status, and support output.

**Milestone record:** completed on 2026-07-13. Public root, local sealed
root, CDK application, consumer registry, lifecycle, and active-selector
configuration now live behind an isolated federation FROST config boundary.
Dual-DKG completion atomically materializes the public/private root snapshots;
startup validates the complete public derivation, exact persisted records, and
reopens only the local member-bound sealed share before readiness. Public wallet
configuration participates in the federation config digest and deterministic
materialized-state projection, while private roots and nonces remain outside
public snapshots and status fingerprints. SQLite and PostgreSQL share one
undeployed schema and generic equivocation-safe persistence implementation.

### Milestone 6: initialize identical watch-only BDK wallets on every member

**Objective:** give each member the same descriptor wallet and independently
verifiable local chain view without signing capability.

**Work:**

- [x] Instantiate BDK external/internal Taproot descriptors from accepted public
  wallet config.
- [x] Persist BDK wallet state in a federation-wallet-specific namespace.
- [x] Verify loaded descriptors against public config on every startup.
- [x] Add configurable Bitcoin Core RPC and Esplora chain sources per member.
- [x] Keep chain source configuration local and outside consensus while keeping
  network and confirmation policy in consensus config.
- [x] Expose bounded sync health, tip, last-success time, and source type without
  exposing queried addresses or credentials.
- [x] Prevent the federated BDK instance from loading local mnemonic signers.
- [x] Define behavior while BDK is behind consensus or chain tip.

**Success criteria:**

- [x] All members derive the same script at every revealed index.
- [x] Members may use different chain-source implementations and still converge
  after seeing the same chain.
- [x] Federated startup fails if a descriptor or network differs.
- [x] Standalone and federated BDK databases cannot be accidentally opened as
  each other.

**Required tests:**

- [x] Five independent watch-only wallet instances derive identical scripts.
- [x] Restart, empty database restore, and descriptor mismatch.
- [x] Mixed Bitcoin Core RPC and Esplora regtest sync.
- [x] One unavailable chain source does not corrupt local or consensus state.
- [x] No-signing-key assertions on persisted BDK data.
- [x] Standalone BDK regression tests.

**Milestone record:** completed on 2026-07-13. `cdk-bdk` now owns an
isolated federated watch-only runtime with scope-bound database markers,
descriptor/network verification on every open, no mnemonic path, continuous
member-local Bitcoin Core or Esplora synchronization, fail-closed readiness,
and bounded redacted health. Daemon wiring contributes only local source,
credential, interval, and storage settings; consensus remains authoritative for
the descriptors, network, confirmation policy, application epoch, and namespace
commitment. Five independent Regtest-network wallets using mixed source types
derive identical scripts and converge after the same chain transaction; both
one-shot adapters are exercised under outage to prove failures preserve the
wallet graph. All 109 `cdk-bdk` tests pass, including the unchanged standalone
backend regression coverage.

### Milestone 7: consensus-controlled deposit address allocation

**Objective:** allocate unique incoming addresses deterministically and
idempotently through the federation journal.

**Work:**

- [x] Add a canonical address-allocation operation keyed by quote ID and CDK
  application key epoch.
- [x] Store separate monotonic external and internal next-index counters in
  materialized consensus state.
- [x] Atomically map each on-chain mint quote to one external derivation index.
- [x] Return the existing mapping for duplicate/replayed quote requests.
- [x] Reject attempts to remap one quote or reuse one index for another quote.
- [x] Apply accepted indexes to BDK with `reveal_addresses_to` semantics.
- [x] Include allocation counters and mappings in state digest and catch-up.
- [x] Define exhaustion and maximum-index behavior.
- [x] Ensure public handlers never advance a local index before consensus.

**Success criteria:**

- [x] Concurrent requests at different members receive distinct addresses.
- [x] The same quote request at every member returns one identical address.
- [x] A member catching up across an index gap reveals every required script and
  reconstructs the same quote mapping.
- [x] Restart cannot reuse or skip an accepted index incorrectly.

**Required tests:**

- [x] Sequential, concurrent, duplicate, replayed, and conflicting allocation.
- [x] Allocation with one member offline and later catch-up.
- [x] Crash before consensus, after acceptance, before BDK persistence, and after
  BDK persistence.
- [x] External/internal keychain separation.
- [x] Index-boundary and exhaustion tests.
- [x] Materialized-state digest mismatch on altered mapping or counter.

**Milestone record:** completed on 2026-07-13. A versioned canonical
address-allocation operation now lets accepted journal order assign indexes;
proposers never carry or mutate a local cursor. One atomically persisted,
strictly validated federation projection owns independent external/internal
counters and immutable quote mappings per CDK application epoch, rejects gaps,
reuse, remapping, inactive epochs, and indexes beyond `2^31 - 1`, and contributes
stable counter/mapping rows to catch-up state digests. The isolated BDK adapter
reconciles accepted max-plus-one counters with `reveal_addresses_to` semantics
at startup, live apply, and catch-up. The public allocation boundary first
recovers an existing accepted mapping or reaches consensus, then and only then
reveals and returns the address. Five members, alternating authenticated
proposers, offline catch-up, every crash boundary, external/internal separation,
restart gaps, corruption, exhaustion, and changed-state digests are covered;
all 347 `cdk-common` federation tests, 110 `cdk-bdk` tests, and focused CDK
allocation tests pass. Existing mint quote, saga, payment, and issuance business
logic was not changed.

### Milestone 8: chain observations and consensus UTXO materialization

**Objective:** turn local BDK observations into an agreed, conservative wallet
UTXO state.

**Work:**

- [x] Define canonical deposit observation records containing outpoint, txout,
  allocated address reference, block hash/height, and observer member.
- [x] Validate observed scripts and amounts against accepted address allocation.
- [x] Reuse or extend federation payment-observation quorum rules for confirmed
  on-chain deposits.
- [x] Define agreement requirements for block identity, height, transaction, and
  confirmation depth.
- [x] Materialize UTXO states such as observed, confirmed, spendable, reserved,
  spent, reorged, and quarantined.
- [x] Make duplicate observations idempotent and conflicting evidence explicit.
- [x] Define conservative reorg handling and liability alarm behavior.
- [x] Include accepted UTXOs and observation evidence in state digest and
  catch-up snapshots.
- [x] Keep raw local BDK observations outside consensus until submitted as
  bounded canonical evidence.

**Success criteria:**

- [x] One member or one chain source cannot make a deposit spendable alone when
  policy requires a quorum.
- [x] Honest members converge on identical spendable UTXOs despite observation
  timing differences.
- [x] Conflicting block/transaction evidence cannot silently mutate a UTXO.
- [x] Catch-up reconstructs the exact wallet UTXO set.

**Required tests:**

- [x] Confirmed deposit with threshold matching observations.
- [x] Unconfirmed, insufficient-confirmation, and threshold-minus-one evidence.
- [x] Duplicate and reordered observations.
- [x] Conflicting amount, script, outpoint, block hash, and height.
- [x] Reorg before minting and reorg after liability issuance.
- [x] One stale, lying, offline, or delayed observer.
- [x] Members using a shared source versus independent sources are reported
  accurately in diagnostics.
- [x] Snapshot/catch-up and restart convergence.

**Milestone record:** completed on 2026-07-13. A bounded, author-bound chain
observation operation carries an allocated address reference, exact outpoint and
`TxOut`, optional block hash/height/depth evidence, observer, and opaque
independent/shared source classification. The isolated versioned UTXO state
validates the allocation and independently derived P2TR script, reuses the mint
payment-observation member quorum, and requires both confirmation depth and
matching observers before `spendable`; one member remains only `confirmed`.
Immutable amount, script, allocation, block, and height conflicts quarantine,
while one matching block-disconnect report immediately fails closed to
`reorged`; a sticky alarm records reorg/quarantine after liability issuance.
Observed, confirmed, spendable, reserved, spent, reorged, and quarantined states
are defined, with reservation/spend transitions intentionally owned by the
later accepted-transaction milestones. UTXOs and every evidence provenance row
participate in state digests and generic catch-up. Five persisted mint members,
one offline catch-up, restart, replay, timing/depth variation, shared/independent
source diagnostics, conflicts, and pre/post-liability reorgs are covered. All
352 `cdk-common` federation tests and focused CDK chain-observation tests pass;
the federation-enabled Axum crate compiles. No existing mint business logic was
changed beyond narrow admission, apply dispatch, and deterministic projection.

### Milestone 9: integrate incoming on-chain mint quotes

**Objective:** satisfy existing Cashu on-chain mint quotes from the federated
wallet without introducing a parallel quote or recovery model.

**Work:**

- [x] Route federation on-chain incoming-payment creation through consensus
  address allocation.
- [x] Preserve existing `CreateIncomingPaymentResponse`, quote ID, amount,
  expiry, and notification contracts.
- [x] Associate confirmed accepted UTXOs with the correct mint quote.
- [x] Submit normal federation mint payment observations only after accepted
  confirmation policy is satisfied.
- [x] Prevent one deposit from paying multiple quotes unless the protocol
  explicitly supports and accounts for that case.
- [x] Preserve overpayment, underpayment, duplicate payment, and multiple-UTXO
  policy explicitly.
- [x] Reuse existing mint quote state and wallet saga behavior.
- [x] Keep standalone on-chain mint quote behavior unchanged.

**Success criteria:**

- [x] A federation address can receive a regtest payment and satisfy exactly one
  on-chain mint quote after required confirmation and observation quorum.
- [x] The wallet receives ordinary Cashu mint quote states and can mint BLS
  e-cash through the existing federation connector.
- [x] Restart and member failover do not create a second address or quote.

**Required tests:**

- [x] Happy-path on-chain quote, deposit, confirmation, observation, and mint.
- [x] Slow confirmation and member observation delay.
- [x] Underpayment, overpayment, duplicate transaction, and multiple UTXOs.
- [x] Wrong address, wrong network, and unrelated wallet transaction.
- [x] Reorg before payment acceptance.
- [x] Interrupted wallet/client mint recovery.
- [x] One member offline during allocation and payment observation.
- [x] Standalone BDK mint regressions.

**Milestone record:** completed on 2026-07-13. Federation on-chain quote
creation now substitutes only the incoming-payment address step: consensus
allocates the derivation index, the handler independently derives and verifies
the exact P2TR address, and the ordinary mint-quote operation persists the
existing request and response types. The isolated BDK adapter reports owned
outputs with bounded, opaque independent/shared source identity; the existing
payment-observation worker translates only accepted allocations into canonical
chain evidence and submits ordinary federation quote-payment observations after
confirmation and member quorum. Quote credit is the exact cumulative satoshi
value of immutable accepted outpoints, committed by a stable payment ID and
attributed to one quote. NUT-30 quotes are amountless, so partial and additional
payments increase cumulative credit, duplicates are idempotent, and issuance
cannot exceed the ordinary quote accounting; every contributing UTXO is marked
as a liability atomically with quote credit. A pre-acceptance reorg blocks
credit, while a post-liability reorg raises the sticky wallet alarm and blocks
further credit. The existing quote state, notifications, NUT-20 authorization,
BLS issuance, recovery, and standalone BDK flows remain in place; no parallel
quote or saga was introduced. The only shared-domain gap was support for
cumulative payment-observation revisions: a narrow conflict-key and SQL-index
migration now permits `PaymentId`-backed on-chain revisions and quorum selects
each member's latest accepted report, while Bolt11 behavior is unchanged. No
new federation operation payload or envelope bump was required. Focused
five-member delay/offline, wrong-address/network, duplicate, multiple-UTXO,
reorg, issuance, and restart tests pass, along with all 353 `cdk-common`
federation tests, 116 CDK federation mint tests, and 110 `cdk-bdk` tests.

### Milestone 10: define payment intents, batches, and transaction proposals

**Objective:** represent outgoing payments and candidate PSBTs as bounded,
canonical, consensus-validatable data.

**Work:**

- [x] Define canonical on-chain payment intent with quote ID, destination script,
  amount, maximum fee, fee tier, creation time, and policy version.
- [x] Define deterministic batch eligibility and intent ordering independent of
  proposer-local hash-map or database iteration order.
- [x] Define a proposal operation containing batch ID, ordered intent IDs,
  selected inputs with trusted previous outputs, recipient outputs, change
  index, unsigned transaction, fee data, and PSBT commitment.
- [x] Set strict limits for intents, inputs, outputs, transaction weight, PSBT
  bytes, and operation bytes.
- [x] Canonicalize or exclude non-consensus PSBT metadata.
- [x] Add conflict keys for payment intents, batches, UTXOs, and replacement
  lineage.
- [x] Bind every proposal to root epoch, CDK application key epoch, and
  transaction policy version.
- [x] Keep proposer identity for diagnostics without granting it authority.

**Success criteria:**

- [x] Equivalent proposals encode identically and hash identically.
- [x] Oversized or ambiguous proposals fail before consensus admission.
- [x] Two proposals spending the same UTXO or intent cannot both be accepted.
- [x] Proposal types contain enough information for every member to recompute
  all Taproot sighashes without trusting proposer-supplied digests.

**Required tests:**

- [x] Canonical encoding and stable hash vectors.
- [x] Reordered intents, inputs, outputs, and PSBT maps.
- [x] Duplicate intent/input/output and conflicting batch tests.
- [x] Size, count, weight, and amount bounds.
- [x] Wrong epoch, policy version, network, and malformed transaction.
- [x] Property and fuzz tests for proposal decoding and conflict keys.

**Milestone record:** completed on 2026-07-13. A new isolated
`cdk-common::federation` transaction-proposal module defines versioned payment
intents, deterministic eligible batches, and bounded unsigned transaction
proposals without touching mint business logic or standalone BDK. Intent IDs
commit to the quote, exact destination script and amount, maximum fee, selected
fee tier, creation time, Bitcoin network, root/application epochs, and policy
version. Batches retain the complete intents, reject mixed scopes, late or
duplicate quotes, and sort by fee tier, creation time, and quote before deriving
a stable batch ID. Proposals normalize input and recipient metadata to the exact
transaction positions; carry every previous `TxOut`, recipient assignment,
internal change index, checked fee/feerate, proposer, replacement lineage, and
exact unsigned transaction; and pin a stable hash vector. The PSBT commitment
is derived only from the unsigned transaction and ordered witness UTXOs, which
is sufficient to recompute every Taproot key-spend sighash. Unknown,
proprietary, signer-controlled, or proposer-local PSBT maps are excluded and
rejected instead of becoming consensus data. Explicit limits cover intents,
inputs, outputs, transaction weight, PSBT consensus bytes, and canonical
proposal bytes. Quote, intent-assignment, batch, UTXO, and replacement-lineage
conflict keys are ready for the Milestone 12 operation admission/apply step; no
partially handled operation variant or envelope-version bump was introduced in
this type-definition milestone. Randomized canonicalization properties and a
raw JSON libFuzzer target cover decoding, hashing, and conflict-key generation.
All 361 `cdk-common` federation tests pass, the fuzz target compiles, and the
workspace compiles across all targets.

### Milestone 11: implement independent transaction-policy validation

**Objective:** let every member validate an untrusted BDK-built proposal without
requiring deterministic coin selection.

**Work:**

- [x] Verify every input is an accepted spendable UTXO for the selected epoch.
- [x] Verify inputs are not reserved, spent, reorged, quarantined, or assigned
  to another accepted transaction.
- [x] Verify each payment intent is pending, included exactly once, and matches
  one recipient output exactly.
- [x] Reject all unauthorized recipient and data-carrying outputs.
- [x] Derive and verify the change script from the proposed internal index.
- [x] Verify checked input/output sums, exact fee, feerate bounds, dust, weight,
  maximum fee, and per-intent fee allocation.
- [x] Verify version, locktime, sequences, RBF policy, sighash type, and Taproot
  key-path-only policy.
- [x] Verify PSBT previous-output data and strip/reject prohibited proprietary
  or signer-controlled fields.
- [x] Recompute transaction ID and each input sighash locally.
- [x] Return deterministic Cashu/federation errors for business-invalid
  proposals and retryable errors for unavailable local verification data
  (v1 requires no member-local verification data after readiness gating).

**Success criteria:**

- [x] A valid proposal built by any member passes every honest member.
- [x] Any change to destination, amount, fee, input, change, sighash, or policy
  causes deterministic rejection before signing.
- [x] Validation does not depend on the proposer using the same BDK version or
  coin-selection algorithm.

**Required tests:**

- [x] Valid single and batched payment proposals.
- [x] Unauthorized input/output and unknown PSBT field.
- [x] Missing, duplicate, spent, reserved, reorged, and wrong-epoch UTXO.
- [x] Wrong destination, amount, change index, fee, feerate, dust, locktime,
  sequence, RBF, and sighash.
- [x] Integer overflow/underflow and maximum-bound tests.
- [x] Locally stale BDK cache versus sufficient consensus UTXO data.
- [x] Differential tests where multiple BDK coin selections are all valid.

**Milestone record:** completed on 2026-07-13. The isolated CDK transaction
policy module now validates an untrusted proposal only against the active public
FROST application, canonical chain state, and accepted intent records; no local
BDK graph, proposer-supplied digest, or mint payment side effect is consulted.
Policy version 1 fixes Bitcoin transaction version 2, zero locktime, opt-in RBF
sequences, BIP341 default key-spend sighash, input/output/weight ceilings,
250--100,000 sat/kwu feerate bounds, and a 1,000,000 sat absolute fee ceiling.
Every selected input must exactly match a `spendable` canonical UTXO, its
accepted allocation, amount, and independently derived P2TR script; observed,
confirmed, reserved, spent, reorged, quarantined, alarmed, missing, or wrong
epoch inputs fail closed. The validator reconstructs the deterministic batch
from pending intent records and its eligibility cutoff, requires one exact
non-data, non-dust recipient per intent, checks each fee contribution against
that intent's maximum and the total fee, and independently derives the internal
change script. The common proposal format now commits the eligibility cutoff,
explicit per-intent fee allocation, durable intent lifecycle, and explicit
Taproot sighash mode. After all structural and business checks, each member
recomputes the txid and every BIP341 sighash from the exact unsigned transaction
and complete ordered previous outputs, returning a deterministic policy audit
for the later acceptance milestone. All required verification data is
canonical, so this layer has no member-local unavailable-data dependency or
retryable local-cache branch; runtime readiness remains the admission layer's
responsibility. Valid single/batched transactions and two
different coin selections pass without any BDK cache; destination, amount,
change, fee, feerate, dust, version, locktime, sequence/RBF, sighash, scope,
UTXO-state, and intent-state mutations reject. No existing quote, melt, saga,
standalone BDK, or consensus apply behavior changed. All 361 `cdk-common`
federation tests and 119 CDK federation mint tests pass, the proposal fuzz target
compiles, and the full workspace compiles across all targets.

### Milestone 12: accept transactions and reserve UTXOs through AlephBFT

**Objective:** serialize the exact transaction before any signing material is
released.

**Work:**

- [x] Add transaction-proposal operation admission and deterministic apply.
- [x] Atomically transition intents to accepted batch and inputs to reserved.
- [x] Persist the exact unsigned transaction, previous outputs, PSBT commitment,
  change allocation, policy audit, and computed sighashes.
- [x] Make duplicate proposal replay return the accepted transaction.
- [x] Reject a different transaction under the same batch or intent scope.
- [x] Include accepted transactions and reservations in materialized-state
  digest and catch-up.
- [x] Add explicit abandoned/replaced transitions that cannot free inputs while
  a valid signed transaction may still exist.
- [x] Gate FROST session creation on accepted and locally applied operation
  status.

**Success criteria:**

- [x] Exactly one transaction owns each reserved UTXO and payment intent.
- [x] Every honest member stores identical transaction bytes and sighashes.
- [x] No FROST round can begin for pending, rejected, failed, or catch-up-only
  unverified state.
- [x] Restart preserves reservations and accepted transaction identity.

**Required tests:**

- [x] Concurrent valid proposals for the same batch.
- [x] Concurrent proposals sharing one UTXO but different intents.
- [x] Duplicate and conflicting operation replay.
- [x] Crash before and after atomic reservation.
- [x] Member catch-up after proposal acceptance.
- [x] Attempted signing before finalization or while readiness-gated.
- [x] State-digest tampering with transaction bytes, sighashes, or reservations.

**Milestone record:** completed on 2026-07-13. Federation operation format v4
adds a proposer-authenticated transaction proposal whose batch, replacement
lineage, intent assignments, and wallet outpoints are AlephBFT conflict scopes.
Pure admission checks proposal structure and member identity; deterministic
apply repeats the full Milestone 11 policy validation against canonical pending
intents and spendable UTXOs. An isolated, versioned transaction projection
stores the exact proposal, ordered previous outputs, PSBT commitment, change
allocation, recomputed policy audit, txid, and every Taproot sighash. One
database transaction assigns the batch and intents, records unique input
ownership, moves UTXOs from `spendable` to `reserved`, and marks the journal
operation applied. Exact replay returns the existing accepted state, while a
different transaction under the same batch, intent, lineage, or UTXO fails
closed. Abandoned and replacement transitions keep every reservation sticky.

The only existing-mint gap was the missing durable intent after an accepted
FROST on-chain melt. Existing melt logic already reserves proofs and stops
external settlement at `SetupComplete`/`Pending` without a backend payment. The
small fix writes the canonical intent in final melt apply; non-FROST,
non-on-chain, quote, proof, saga, and standalone paths are unchanged. This
prerequisite is pulled forward from Milestone 17, which still owns signed
settlement and quote completion. Transaction state participates in digest and
catch-up. The signing authorization boundary requires an active matching FROST
application, the exact locally `Applied` journal payload, accepted lifecycle,
and intact reservations. Before releasing authorization it also recomputes the
complete policy audit from the accepted transaction, intents, reserved UTXOs,
and active application, so structurally valid local audit or sighash tampering
fails closed. Concurrent same-entry apply, same-batch and shared-UTXO
competitors, rollback, replay, restart, catch-up, pre-finality/failed signing,
and transaction/sighash/reservation tampering are covered. All 368 enabled
`cdk-common` tests and 122 CDK federation mint tests pass, and the workspace
compiles across all targets. Strict clippy remains blocked by pre-existing
workspace findings outside this milestone; changed-crate clippy passes when
those existing categories are held at their baseline.

### Milestone 13: implement durable one-time FROST nonce management

**Objective:** make nonce reuse impossible across concurrency, failures,
restarts, coordinator failover, and every application sharing a root epoch.

**Work:**

- [x] Define signing session ID from federation ID, root key epoch, application
  ID, application key epoch, application derivation context, authorization
  reference, message/transcript commitment, attempt, and signer set. The CDK
  adapter additionally binds accepted operation ID, transaction ID, input index,
  and sighash type.
- [x] Add private local nonce records with generated, reserved, commitment-sent,
  share-produced, consumed, and burned terminal states.
- [x] Keep one nonce store and reservation namespace per participant key manager,
  not independent nonce stores in each payment backend or application adapter.
- [x] Generate nonce material from a cryptographically secure RNG independent of
  DKG and identity keys.
- [x] Persist nonce reservation before returning a round-one commitment.
- [x] Atomically consume or burn nonce state before or with round-two share
  release.
- [x] Burn reservations after ambiguous response, abort, timeout, signer-set
  change, invalid package, or restart recovery.
- [x] Reject duplicate round-one requests that change any session field.
- [x] Define safe idempotent response behavior for exact authenticated retries.
- [x] Exclude nonce material from snapshots, logs, metrics labels, and support
  bundles.
- [x] Add bounded cleanup for terminal nonce metadata without deleting evidence
  needed to prevent replay.

**Success criteria:**

- [x] No code path can use one nonce to produce shares for two packages.
- [x] Two applications using the same root epoch cannot collide, reuse, or claim
  each other's nonce records.
- [x] Exact retries are safe and conflicting retries fail closed.
- [x] Restart at every nonce state cannot resurrect reusable material.
- [x] Concurrent requests for the same input cannot reserve two active attempts
  without an explicit new attempt transition.

**Required tests:**

- [x] Normal reserve, commitment, share, consume lifecycle.
- [x] Duplicate exact round-one and round-two requests.
- [x] Changed application, derivation context, authorization, message, signer
  set, attempt, input, or operation under the same session.
- [x] Crash before/after commitment and before/after share persistence.
- [x] Timeout, abort, coordinator loss, and signer substitution.
- [x] Concurrent request race and database transaction failure injection.
- [x] Secret-redaction and terminal-record cleanup tests.

**Milestone record:** completed on 2026-07-13. The application-neutral
`cdk-frost` key manager now owns one nonce namespace across every registered
consumer. `SigningSessionContext` hashes the federation, root epoch, complete
application registration and key epoch, derivation binding, authoritative
authorization reference, exact message commitment, attempt, and ordered signer
set. Its attempt-independent slot prevents concurrent attempts until the prior
attempt is explicitly burned. The CDK transaction layer supplies a separately
typed authorization commitment over the accepted operation, proposal, txid,
input index, sighash type, and independently recomputed sighash.

Nonce records use a versioned validated codec and sealed secret payloads with
generated, reserved, commitment-sent, signing-locked, share-produced, consumed,
and burned states. A strict transition graph and revision CAS prevent skipped
or reversed states. Reservation and commitment state are durable before round
one leaves the member. Round two decrypts locally, durably removes reusable
secret state before cryptographic use, persists the exact package/share, then
marks consumed before responding. Exact round-one and consumed round-two
retries return cached public results; changed packages, applications, signer
sets, attempts, or authorization data fail closed. Restart recovery burns every
nonterminal record.

`RedbNonceStore` provides the production transactional local store; its path
and record debug output are redacted, and it is separate from public federation
snapshots and mint business state. `MemoryNonceStore` remains test-only.
Terminal cleanup retains permanent session/nonce tombstones while discarding
detailed retry metadata. Twenty-one `cdk-frost` tests pass, including
deterministic threshold signing, exact retries, restart at partial states,
signer/attempt conflicts, concurrent reservation, injected failure at every
persistence boundary, durable close/reopen, codec tampering, redaction, and
tombstone cleanup. All 369 enabled `cdk-common` tests and 122 CDK federation
mint tests also pass, as do the full workspace check and strict `cdk-frost`
clippy. Strict `cdk-common` clippy remains blocked by pre-existing findings; it
passes when those existing categories are held at their baseline. The `typos`
binary is not installed in this environment, so that optional check could not
run.

### Milestone 14: add authenticated, application-aware FROST round transport

**Objective:** exchange commitments and shares through existing private REST
without adding a consensus phase or coupling the reusable key manager to one
payment backend.

**Work:**

- [x] Add bounded signed private REST request/response types for commitment,
  signing package, signature share, and aggregate result notification.
- [x] Put application-neutral wire types and orchestration behind the FROST key
  manager, with registered adapters responsible for application authorization
  and transcript validation.
- [x] Reuse member identity authentication, request body hashes, sequence replay
  protection, timeouts, and response caches.
- [x] Bind all requests to root epoch, application ID and key epoch, derivation
  context, authorization reference, attempt, signer set, and canonical package
  hash.
- [x] Require the CDK adapter at each signer to load and revalidate the accepted
  transaction state locally before authorizing its request. Future adapters must
  provide their own typed, fail-closed authorization proof and validator.
- [x] Reject unregistered applications and prevent any transport caller from
  bypassing its adapter with a raw digest.
- [x] Verify coordinator-selected commitments belong to the declared signers.
- [x] Permit any member to act as coordinator while preventing two attempts from
  sharing nonce material.
- [x] Bound concurrent sessions, inputs, body sizes, retries, and per-peer work.
- [x] Keep transport outcomes out of the ordering journal unless they cause an
  explicit accepted wallet-state transition.

**Success criteria:**

- [x] A coordinator collects threshold valid commitments and shares over private
  REST and produces a valid signature.
- [x] One member cannot elicit shares for an unaccepted or altered transaction.
- [x] One application cannot elicit shares using another application's key,
  derivation context, authorization proof, or nonce commitment.
- [x] Replayed, reordered, delayed, forged, and cross-session messages fail
  without nonce reuse.
- [x] Signing works with non-coordinator members temporarily offline as long as
  threshold remains.

**Required tests:**

- [x] 2-of-3 and 3-of-5 live private REST signing.
- [x] Threshold-minus-one timeout.
- [x] Wrong sender, receiver, sequence, federation, epoch, operation, input,
  attempt, and package hash.
- [x] Wrong or unregistered application ID, application epoch, derivation
  context, and authorization proof.
- [x] Duplicate/reordered commitment and share delivery.
- [x] Invalid commitment/share and coordinator equivocation.
- [x] Request timeout before and after signer processing.
- [x] Backpressure and maximum-concurrency tests.

**Milestone record:** completed on 2026-07-13. Four bounded private REST
message kinds now carry canonical commitment, signing-package, abort, and
aggregate-result payloads inside the existing member-authenticated envelopes.
They reuse identity signatures, body hashes, per-kind sequence replay checks,
timeouts, response validation, and inbound/outbound admission limits. Every
round binds the federation, root and application epochs, typed derivation and
authorization, exact attempt and signer set, nonce owner, and canonical package
hash. These volatile messages do not enter the ordering journal.

The long-lived signer runtime opens sealed root shares and one durable redb
nonce store per root epoch. The application-neutral key manager exposes no raw
digest signing path; only registered adapters can authorize a request. The CDK
adapter reloads the accepted transaction state at commitment, share, abort, and
aggregate-result handling, reconstructs the typed input authorization and
Taproot derivation, and exact-compares them before cryptographic work. The
coordinator holds public material only, may run on any member, verifies signer
ownership and the final aggregate independently, and sends authenticated aborts
after ambiguous failures so reserved nonces fail closed.

The five live route tests pass for 2-of-3 and 3-of-5 with every non-selected
member offline, threshold-minus-one rejection, timeout plus outbound
backpressure, and response loss after signer processing followed by nonce burn
and a fresh successful attempt. The 22 `cdk-frost` tests, all 370 enabled
`cdk-common` tests (unit, integration, and doc-test accounting), and all 211 CDK
federation-filtered tests pass. Strict `cdk-frost` clippy passes; `cdk-common`
and `cdk-axum` pass with their existing repository warning categories held at
baseline. The full workspace all-targets check passes. A combined 440-test Axum
run is not a clean repository gate in this environment: 383 tests pass and 56
unrelated federation mint/melt fixture tests fail, including local-listener
tests denied by the sandbox. The isolated M14 transport suite remains clean,
and this milestone does not change mint business logic beyond startup wiring
and the existing accepted-state authorization read.

### Milestone 15: aggregate signatures, finalize PSBTs, and persist signed state

**Objective:** produce one Bitcoin-valid signed transaction and make it durable
before broadcast.

**Work:**

- [x] Aggregate and verify threshold shares independently for every Taproot
  input.
- [x] Insert aggregate key-path signatures and sighash types into the accepted
  PSBT.
- [x] Finalize the PSBT through BDK/rust-bitcoin and extract the transaction.
- [x] Verify extracted non-witness transaction data exactly matches consensus.
- [x] Verify every final signature against the accepted output key and sighash.
- [x] Persist signed transaction bytes, txid, signer-attempt metadata, fee audit,
  and input/output assignments before broadcast.
- [x] Apply the signed transaction to every member's local BDK transaction graph
  idempotently.
- [x] Include signed transaction identity and state in catch-up/digest without
  including private shares or nonces.
- [x] Define behavior when different valid signer subsets produce the same
  deterministic BIP340 signature expectation or differing valid signatures.

**Success criteria:**

- [x] The extracted transaction satisfies the supported Bitcoin Core P2TR
  key-path script rules through the permitted equivalent verifier; Core
  binaries were unavailable for a direct invocation.
- [x] Every member agrees on txid and non-witness transaction bytes.
- [x] Invalid or insufficient shares cannot advance signed state.
- [x] A crash after signing but before broadcast recovers the exact same signed
  transaction and never rebuilds with different inputs.

**Required tests:**

- [x] Single-input and multi-input signing with different threshold subsets.
- [x] Invalid share among valid shares and threshold recovery without it.
- [x] Missing input signature and wrong sighash.
- [x] PSBT mutation before and after aggregation.
- [x] Crash/restart after each input signature and after signed persistence.
- [x] Catch-up of a member that did not participate in signing.
- [x] Bitcoin Core regtest `testmempoolaccept` or equivalent script validation.

**Milestone record:** completed on 2026-07-14. The coordinator now aggregates
each accepted input independently, reconstructs the exact watch-only PSBT from
the consensus transaction and local BDK prevouts, inserts only
`SIGHASH_DEFAULT` Taproot key-path signatures, and uses the existing BDK engine
to verify and finalize the transaction. Finalization exact-compares all
non-witness bytes. The durable record includes the txid, witness-bearing bytes,
the accepted fee/sighash audit, and public session, attempt, signer-set, package,
and aggregate-signature metadata; it contains no nonce or private share.

Signed state is an authenticated `TransactionSigned` operation in the existing
AlephBFT journal. Stateful admission and atomic apply reload the accepted
proposal, require a registered threshold signer set, verify every BIP340
signature against the exact P2TR prevout and accepted sighash, and persist the
signed state with its journal application. Runtime replay and catch-up then
reconcile the exact durable transaction into each matching local BDK graph.
Exact operation/state replay is idempotent. Signed operations intentionally
reserve no pre-validation conflict key: the first cryptographically valid
record that applies wins, while an invalid record cannot block a later valid
one and any later differing valid record is deterministically rejected.

The 368 enabled `cdk-common` tests, 22 `cdk-frost` tests, 111 `cdk-bdk` tests,
17 CDK transaction-filtered tests, and five live Axum FROST transport tests pass.
They cover single and multiple inputs, different threshold subsets, an invalid
share followed by a fresh successful subset, missing/wrong signatures and
sighashes, transaction mutation, nonce-store crash recovery, signed-state
restart, and nonparticipant catch-up with byte-identical state. The all-targets
Axum federation check passes. Strict `cdk-bdk` clippy passes; broader strict
clippy remains blocked only by the repository's existing warning baseline.
Bitcoin Core binaries are unavailable here, so the permitted equivalent gate
verifies the complete supported P2TR key-path script condition directly:
canonical one-item witness, exact BIP341 digest and prevouts, tweaked output-key
match, BIP340 signature verification for every input, and successful BDK
finalization.

### Milestone 16: broadcast, rebroadcast, and ambiguous-result recovery

**Objective:** broadcast exactly the accepted signed transaction with robust
multi-member failover.

**Work:**

- [x] Allow any ready member to broadcast the persisted signed transaction.
- [x] Reuse existing accepted/already-known/rejected/transient/unknown broadcast
  classification.
- [x] Persist broadcast intent before the network request.
- [x] Treat already-known transaction as success-equivalent.
- [x] Retry transient and ambiguous failures without rebuilding or resigning.
- [x] Record deterministic rejection for operator review and replacement policy.
- [x] Reconcile mempool presence and confirmations from local chain sources.
- [x] Prevent repeated broadcaster responses from duplicating payment completion
  events.
- [x] Add proposer/coordinator/broadcaster failover timers that affect liveness,
  not transaction authority.

**Success criteria:**

- [x] Loss of the original proposer or coordinator does not prevent another
  member from broadcasting.
- [x] Broadcast timeout cannot create a second transaction or payment.
- [x] All members converge on one txid and broadcast state.
- [x] Restart automatically resumes safe rebroadcast and reconciliation.

**Required tests:**

- [x] Accepted, already-known, deterministic rejection, transient failure, and
  unknown response.
- [x] Response lost after backend accepted transaction.
- [x] Original broadcaster offline before and after request.
- [x] Multiple members broadcasting the same transaction concurrently.
- [x] Restart with signed-not-broadcast and broadcast-not-observed states.
- [x] Mempool eviction and later rebroadcast.

**Milestone record:** completed on 2026-07-14. Broadcast is now an isolated
edge loop over the accepted transaction projection and each member's existing
watch-only BDK engine. Consensus first persists a txid-bound
`TransactionBroadcastIntent`; only then may any ready member submit the exact
signed bytes through its local chain source. The implementation exposes and
reuses the standalone BDK backend's accepted, already-known, deterministic
rejection, transient, and unknown taxonomy instead of adding another backend
classifier. Raw backend detail is redacted and bounded before it can enter an
authenticated `TransactionBroadcastObservation`.

The first success-equivalent observation makes broadcast state sticky. A lost
response, transient failure, unknown result, member outage, restart, or later
mempool uncertainty schedules only an exact-byte retry; it cannot rebuild,
resign, change the txid, release inputs, or complete a Cashu payment. Already
known is success-equivalent. Deterministic rejection remains durably visible in
the ordered journal for operator review and uses a slower local retry timer.
Independently synced BDK state contributes confirmed block evidence, after
which all members stop rebroadcasting. Proposer, signing-coordinator, and
broadcaster identity are not transaction authority, and multiple members may
safely race the same bytes.

All 370 enabled `cdk-common` tests, all 111 `cdk-bdk` tests, the 16 CDK
transaction-filtered tests, and the two isolated Axum broadcast tests pass.
They cover every backend result class, accepted-response loss followed by an
already-known retry, deterministic rejection, member-local ambiguous retry,
multiple observer races, signed-not-broadcast and broadcast-not-observed
restart, wrong-txid rejection, sticky payment-intent state, post-success
rebroadcast, confirmation convergence, and the rule that locally injected
unconfirmed BDK bytes are not confirmation evidence. The relevant all-targets
check passes. Strict clippy for the touched crates remains blocked only by the
repository's existing Rust 1.95 warning baseline; ordinary no-dependency
clippy reports no warning in the new broadcast module.

### Milestone 17: integrate outgoing on-chain melts and existing sagas

**Objective:** make federated on-chain melts use the joint wallet while keeping
normal Cashu quote, error, and recovery semantics.

**Work:**

- [x] Route accepted federation on-chain melt execution into canonical payment
  intents rather than one member's local `make_payment` side effect.
- [x] Preserve existing on-chain fee-option quote and selected-fee-index behavior.
- [x] Bind payment intent IDs to melt quote and federation operation IDs.
- [x] Keep proofs pending while transaction outcome remains unresolved.
- [x] Emit normal payment observations and `MakePaymentResponse` data from
  accepted wallet transaction state.
- [x] Return txid/outpoint payment proof through existing response fields.
- [x] Reuse existing definitive and ambiguous melt saga behavior without adding
  federation-specific wallet recovery branches.
- [x] Prevent one quote from entering two transaction batches or being paid twice.
- [x] Define when a melt is considered paid: broadcast, mempool acceptance, or
  confirmation, and encode the choice in versioned policy.
- [x] Preserve standalone `CdkBdk` melts unchanged.

**Success criteria:**

- [ ] A CLI wallet can melt Cashu to a regtest address through a 3-of-5
  federation and receive the existing terminal response shape.
- [x] Exactly one Bitcoin payment occurs for one accepted melt quote.
- [x] Ambiguous broadcast or confirmation state leaves recoverable saga state.
- [x] Definitive invalid destination/amount/fee failures use normal Cashu errors.

**Required tests:**

- [x] Happy-path single and batched federated on-chain melts.
- [x] Invalid address, dust, insufficient funds, and fee above quote reserve.
- [x] Quote replay and concurrent melt submissions at different members.
- [x] Threshold unavailable before signing and after commitments.
- [x] Broadcast timeout after acceptance.
- [x] Interrupted client and mint restart recovery.
- [x] Already-paid and already-spent proof behavior.
- [x] Standalone on-chain melt regression suite.

**Implementation record:** completed on 2026-07-14. A FROST-backed on-chain
melt now stops at the existing external-settlement saga boundary and creates
one canonical intent whose v2 identity binds the quote, accepted melt operation,
destination, amount, selected fee index, maximum fee, creation time, and wallet
scope. The generic elected-member observer has one narrow joint-wallet branch:
it never calls a local payment backend and returns no observation until accepted
transaction state contains a terminal receipt. Non-FROST and standalone
`CdkBdk` payment paths are unchanged.

Broadcast intent fixes a versioned outgoing settlement policy before the first
network request. That policy is derived from the accepted application's
confirmation depth and the federation's melt-observation member threshold. A
broadcast or mempool response is not payment. Only distinct members reporting
the same block at the required depth move the transaction to `Confirmed` and
atomically mark every batched intent `Completed`. The resulting receipt binds
the quote and melt operation to one txid, recipient outpoint, exact amount, and
actual deterministic fee contribution. It is converted into the existing
`MeltQuotePayment` quorum, `MakePaymentResponse`, saga finalization, change, and
proof-state machinery; ambiguous states leave the original saga pending.

All 371 enabled `cdk-common` tests, all 34 focused federation melt tests, all
111 standalone `cdk-bdk` tests, and both Axum broadcast-classification tests
pass. Coverage includes below-depth and split-block observations, member
threshold completion, multi-intent completion under one txid, wrong settlement
policy, pre-confirmation paid-observation rejection, exact txid/outpoint response
data, actual-fee accounting, spent proofs, replay idempotency, invalid payment
terms, concurrent/restarted saga recovery, and the unchanged standalone backend.
The feature-enabled CDK/Axum library check passes. The live 3-of-5 CLI regtest
criterion above remains deliberately unchecked until the end-to-end regtest
environment is exercised in the later integration milestone.

### Milestone 18: full restart, replay, and catch-up recovery

**Objective:** prove every durable phase converges without duplicate custody
side effects.

**Work:**

- [x] Enumerate every address, observation, proposal, reservation, nonce,
  signing, signed, broadcast, confirmation, replacement, and terminal state.
- [x] Define startup recovery action or explicit no-op for every state.
- [x] Restore and validate root epochs, the registered-application registry, and
  application key epochs before enabling any application adapter or signer.
- [x] Rebuild watch-only BDK revealed indexes from accepted consensus state.
- [x] Reapply accepted and signed transactions to BDK idempotently.
- [x] Resume safe commitment/share collection only with a fresh signing attempt
  and fresh nonces where required.
- [x] Resume rebroadcast using persisted transaction bytes.
- [x] Integrate wallet records into existing journal replay and catch-up
  certificate verification.
- [x] Prevent a catching-up member from signing until state digest and readiness
  gates pass.
- [x] Add operator-visible stuck-state diagnosis without secret data.

**Success criteria:**

- [x] Killing any member at any tested state does not lose an address, reuse a
  nonce, release a reservation, rebuild a signed transaction, or duplicate a
  payment.
- [x] A member offline through multiple deposits and spends catches up to the
  same BDK scripts, UTXOs, transactions, and federation digest.
- [x] A fully restarted quorum resumes pending DKG, observation, signed-byte
  reconciliation, broadcast, and confirmation work automatically; starting a
  fresh accepted-proposal signing attempt is the coordinator-liveness work in
  Milestone 19.
- [x] Restart with concurrent CDK and test-application signing sessions preserves
  authorization isolation and the shared nonce ledger.

**Required tests:**

- [x] Failure injection before and after every durable transition.
- [x] Full federation restart during DKG, allocation, observation, proposal,
  FROST rounds, signed persistence, broadcast, and confirmation.
- [x] Restart with two registered applications and pending sessions in both.
- [x] One member offline across address-index gaps and multiple spends.
- [x] Catch-up from snapshot plus journal tail.
- [x] Tampered snapshot, transaction, descriptor, and nonce metadata rejection.
- [x] Repeated recovery invocation idempotency.

**Milestone record:** completed on 2026-07-14. Startup already validates and
materializes root/application epochs and the consumer registry before opening
the isolated signer or watch-only adapters, then replays the authenticated
journal and reconciles accepted descriptor counters and exact signed bytes into
BDK. The existing DKG driver, observer, and broadcaster restart workers resume
their idempotent durable work; an accepted signing state is deliberately
classified as `start_fresh_signing_attempt`, because participant construction
burns every nonterminal nonce and the volatile FROST rounds must never be
resumed with old material. Deterministic coordinator election and failover for
starting those attempts remains the liveness refinement in Milestone 19.

One narrow shared readiness gap was fixed outside the wallet: authenticated
FROST commitment/share routes and the observation/broadcast workers now require
the existing federation sync/readiness gate before nonce creation or consensus
submission. No mint quote, saga, proof, or settlement business path changed.
Private local status now includes a typed, bounded recovery matrix containing
only state/action counts for BDK index repair, chain evidence, proposal/signing,
sticky reservations, exact-byte reapplication/rebroadcast, confirmation,
replacement, terminal no-op, and operator-review states. It contains no
descriptor, script, nonce, share, endpoint, or backend detail.

All 373 enabled `cdk-common` tests, 23 `cdk-frost` tests, and 112 `cdk-bdk`
tests pass. The six live FROST-route tests, 23 DKG-driver recovery tests, five
CDK transaction-policy tests, the isolated scheduler-readiness test, and 17 of
18 broad Axum startup tests pass. Coverage includes DKG restart at every durable
phase, response loss and partition recovery, serialized wallet-state snapshot
plus authenticated journal tail, offline address gaps and multiple accepted
spends, repeated BDK/nonce recovery, two applications sharing one nonce ledger,
tampered digest/transaction/descriptor/nonce rejection, and catching-up signer
and side-effect-worker refusal. The one broad startup failure is an existing
terminal-rejected AlephBFT authentication fixture mismatch; the wider
payment-observation filter likewise retains its existing unrelated fixture
failures. The isolated M18 gates and all affected crate checks are clean;
ordinary no-dependency clippy passes for every touched crate, while strict
federation-wide clippy remains blocked by the documented pre-existing warning
baseline outside this milestone. The repository requests nightly rustfmt
options but only a stable toolchain is installed locally, so stable
`cargo fmt --check` also retains its known configuration/baseline mismatch;
all M18 Rust edits were formatted and `git diff --check` passes.

### Milestone 19: proposer and signing-coordinator liveness

**Objective:** remove elected-member single points of liveness failure without
weakening consensus authorization.

**Work:**

- [x] Define deterministic initial proposer selection from accepted batch ID.
- [x] Add bounded proposer lease/timeout and permission for another member to
  submit a candidate.
- [x] Ensure competing valid candidates resolve through existing consensus
  conflict handling.
- [x] Define deterministic initial signing coordinator and failover attempts.
- [x] Require fresh signer nonces on every coordinator attempt.
- [x] Prevent stale coordinators from completing an obsolete attempt after a
  later attempt becomes active.
- [x] Allow any member to aggregate already valid shares for the active attempt.
- [x] Add bounded retries, backoff, and status for unavailable signers.
- [x] Ensure idle federation behavior does not reintroduce periodic high-CPU
  consensus activity.

**Success criteria:**

- [x] Federation completes payments with any non-threshold set of members
  offline.
- [x] Offline initial proposer, coordinator, or broadcaster causes bounded delay
  but no manual intervention.
- [x] Failover cannot cause a second accepted transaction or nonce reuse.
- [x] Idle CPU and network traffic remain bounded.

**Required tests:**

- [x] Offline initial proposer before and after PSBT construction.
- [x] Coordinator failure after commitment collection and during share collection.
- [x] Signer-set substitution between attempts.
- [x] Delayed stale coordinator messages after failover.
- [x] Network partition below and above consensus/signing thresholds.
- [ ] Long idle period followed by deposit and melt.
- [ ] CPU/network benchmark for idle and active signing periods; the deterministic
  active scheduler microbenchmark and idle no-work invariant pass, but a
  multi-process network measurement remains for Milestone 26.

**Milestone record:** completed on 2026-07-14 with the two environment-backed
tests above deliberately deferred to the end-to-end milestone. The new
application-neutral liveness schedule derives proposer, signing coordinator,
and every threshold-sized signer subset from the accepted batch or proposal
ID. Absolute bounded leases survive restart, rotate across the complete member
roster, and grant the first coordinator only a short aggregation head start;
after that grace period any member may collect or replay the exact active
commitments and shares. Existing proposal conflict rules remain the only
transaction winner authority.

The FROST nonce boundary now persists a per-authorization-slot attempt
high-water mark. A later canonical attempt atomically burns any active prior
nonce, a lower or same-attempt signer substitution stays stale across reopen,
and an exact current session may replay its public commitment and cached share
without reusing secret nonce material. Private routes independently recompute
the canonical signer set, accept only the current or immediately prior lease,
and reject future attempts so an authenticated peer cannot pin the durable
high-water mark.

A thin Axum worker reads only accepted consensus projections, builds a
watch-only BDK PSBT from consensus-spendable inputs and an already allocated
change script, validates it through existing policy, and submits through the
existing operation path. It changes no quote, proof, saga, or ordinary mint
business logic. Local retry state is bounded and redacted in status. With no
pending batch or unsigned accepted transaction, each delayed round performs no
peer transport or consensus submission.

All 24 `cdk-frost` tests, 113 `cdk-bdk` tests, eight live FROST route tests, five
liveness worker tests, four enabled deterministic-schedule tests, two CDK
signed-state tests, and the federation-enabled mint daemon check pass. Coverage
includes proposer loss before/after construction, complete signer-subset
rotation, below/at-threshold partitions, coordinator loss after commitments and
shares, signer substitution, delayed stale attempts after durable reopen, and
another member aggregating cached shares. The explicit active benchmark
computed 100,000 seven-member signing schedules in 0.84 seconds. The broad
Axum run reached 392 passes; its existing quote/saga fixture failures and two
sandbox-denied listener tests remain outside this isolated milestone. Ordinary
no-dependency clippy reports only the existing repository baseline, stable
rustfmt formatted every changed Rust file despite unavailable nightly options,
and `git diff --check` passes. A real long-idle deposit/melt run and network
traffic measurement remain unchecked for Milestone 26 rather than being
claimed from component tests.

### Milestone 20: fee management, batching, and replacements

**Objective:** provide production-safe fee behavior without allowing fee policy
to bypass payment authorization.

**Work:**

- [x] Version consensus fee policy, allowed fee sources, fallback values, and
  maximum feerate/absolute fee.
- [x] Preserve immediate, standard, and economy batching semantics where they
  remain appropriate.
- [x] Make batch-intent selection deterministic while allowing proposer-local
  coin selection.
- [x] Allocate actual transaction fees to intents deterministically within each
  quoted maximum.
- [x] Define RBF eligibility, replacement lineage, minimum fee delta, recipient
  invariance, and input/output mutation rules.
- [x] Require replacement transactions to pass full consensus and use fresh
  FROST nonces/signatures.
- [x] Define CPFP support or explicitly defer it with operator guidance.
- [x] Prevent simultaneous original/replacement states from double-completing
  payment intents.
- [x] Reconcile BDK mempool eviction and replacement observations.

**Success criteria:**

- [x] Fee estimation outages cannot authorize unbounded fees.
- [x] Replacements preserve authorized recipients and amounts.
- [x] Original and replacement transaction lineage has one terminal payment
  result.
- [x] Batches never charge an intent above its accepted fee reserve.

**Required tests:**

- [x] Fee source success, timeout, disagreement, fallback, and extreme values.
- [x] Mixed-tier batching and deterministic eligibility/order.
- [x] Fee allocation caps and rounding.
- [x] Valid RBF and invalid recipient/amount/input mutations.
- [x] Original confirms after replacement proposal.
- [x] Replacement confirms while original remains in another member's mempool.
- [x] Insufficient fee reserve and no-valid-replacement behavior.

**Milestone record:** completed on 2026-07-14. Transaction-policy version 1
now permits only a proposer member's local chain source, bounds that lookup to
three seconds, falls back to 2 sat/vB, clamps the result to the consensus
minimum and 400 sat/vB maximum, and retains the 1,000,000-satoshi absolute
ceiling. Accepted intents carry their selected 1, 6, or 144-block quote target.
Those targets preserve immediate/standard/economy priority in canonical batch
selection; wall-clock delay rules are deliberately not inferred from the MVP
journal's logical operation index. Proposers still choose coins locally, while
all members derive the same bounded intent prefix and independently validate
the resulting transaction.

Proposal schema version 2 computes fee rate, weight limits, and incremental RBF
fees from the exact projected key-path-signed weight, including witness bytes.
Fees are allocated in canonical intent order without crossing any accepted
reserve. A first-release replacement must extend the current signed lineage
head, retain the exact inputs and recipient positions, scripts, and amounts,
reduce only the existing allocated change output, increase both absolute fee
and feerate by the versioned relay delta, and pass the ordinary consensus
proposal and signing path under a new proposal ID. This necessarily creates a
fresh FROST session and nonce attempt. CPFP is explicitly deferred; operators
must provision sufficient quote reserves and non-dust change for bounded RBF.

Input reservations remain owned by the lineage root. Rejected or repeatedly
unconfirmed broadcasts trigger deterministic proposer failover for a bounded
replacement, while every signed lineage transaction remains in the local BDK
graph and can still contribute a confirmation observation after replacement.
Either the original or replacement may win, but the first quorum-settled result
is terminal: all competing versions become abandoned, one receipt is emitted,
and later conflicting confirmation evidence cannot flip the winner.

All 380 `cdk-common` tests pass (379 active, one benchmark ignored), including
the two replacement race directions and terminal-winner regression. All 126
CDK federation-filtered tests pass, including full accepted-operation apply and
fresh replacement-signing authorization plus valid and mutated RBF policy
cases. All 113 `cdk-bdk` tests and six Axum liveness tests pass; the latter cover
fee success, timeout, fallback, disagreement, extreme clamping, deterministic
mixed-cap allocation, and insufficient replacement reserve. The complete
workspace all-targets check passes. Strict clippy remains blocked only by the
documented pre-existing repository warning baseline; no new warning appeared
in the changed milestone code, stable rustfmt formatted the changes despite
unavailable nightly options, and `git diff --check` passes.

### Milestone 21: lightweight and heterogeneous chain backends

**Objective:** support simple Esplora deployments while making correlated trust
and availability visible.

**Work:**

- [x] Document and test public mempool.space-compatible Esplora configuration.
- [x] Support self-hosted Esplora and Bitcoin Core RPC per member.
- [x] Keep chain-source URLs and credentials local while reporting a safe source
  class and health state.
- [x] Bound Esplora parallel requests, scan lookahead, retries, and backoff.
- [x] Avoid holding BDK/database locks during network requests.
- [x] Add privacy guidance explaining script/address query leakage to public
  services.
- [x] Add correlated-source warnings when operators intentionally declare the
  same upstream provider.
- [x] Keep consensus observation quorum based on members, while documenting that
  common upstreams reduce practical independence.
- [x] Define optional local header/confirmation verification improvements.

**Success criteria:**

- [x] A complete regtest flow works with lightweight Esplora only.
- [x] A heterogeneous federation using RPC and Esplora converges.
- [x] One backend outage or stale response does not corrupt consensus state.
- [x] Public Esplora is optional, not a federation-wide hard dependency.

**Required tests:**

- [x] Esplora sync, fee estimate, broadcast, already-known, and retry behavior.
- [x] Bitcoin Core RPC equivalent flow.
- [x] Mixed-source observation timing and convergence.
- [x] Rate limit, timeout, malformed response, stale tip, and omitted transaction.
- [x] Chain reorg and backend recovery.
- [x] Lock-duration and request-concurrency benchmark regressions.

**Milestone record:** completed on 2026-07-14. Member-local Esplora and Bitcoin
Core RPC configurations now have fail-closed request bounds, redacted debug
output, and safe source-class, opaque-source, health, and correlation status.
Esplora concurrency is limited to 16 requests, request timeouts to 120 seconds,
and retryable HTTP attempts to six retries. Continuous synchronization applies
bounded exponential backoff. The accepted-address counters remain the only
scan range, so federated wallets retain zero unallocated lookahead. The
[chain-source deployment guide](federated-bdk-chain-sources.md) covers public
mempool.space-compatible and self-hosted Esplora, member-operated RPC, privacy
leakage, shared-provider trust reduction, outage behavior, and optional local
header, proof-of-work, Merkle, and ancestry verification.

Both adapters now finish network work outside the BDK/database lock. Bitcoin
Core calls run on blocking workers with a bounded two-chunk channel before
wallet application; Esplora uses separate request-build, network, and apply
phases. Network-stall regressions require the wallet lock to remain acquirable
within 100 ms, while config tests enforce request-concurrency ceilings. A live
RPC reorg exposed one shallow-reorg gap in the pre-existing adapter: starting
the block emitter at the old checkpoint height queried a height absent from the
new best chain. The isolated adapter fix starts ancestry reconciliation at
height zero, without changing mint business logic.

All 123 active `cdk-bdk` tests pass, including mock Esplora and RPC fee,
broadcast, already-known, retry/rate-limit, timeout, malformed-response,
resource-bound, and lock-availability cases. Ignored live regtest gates were
run explicitly: an Esplora-backed wallet completed sync and broadcast while a
heterogeneous RPC/Esplora pair converged on the same descriptor graph, and an
RPC wallet survived invalidation, a competing block, and reconfirmation. The
previous five-member outage, stale/omitted observation, persistence, and
member-quorum tests remain green: all 379 active `cdk-common` federation tests
and all 126 CDK mint federation tests pass. The complete workspace all-targets
check and stable rustfmt check pass. Strict no-dependency Clippy passes for
`cdk-bdk`; mintd and Axum strict Clippy remain blocked only by their documented
pre-existing warning baseline. Broad mintd/Axum suites still contain unrelated
pre-existing federation fixture failures, while every M21-focused test passes.

### Milestone 22: root and application key rotation and federation preparation

**Objective:** rotate reusable root keys and CDK application keys safely, account
for every registered consumer, and leave a clean path for future membership
changes.

**Work:**

- [x] Add consensus proposal and activation operations for a new root key epoch
  after successful DKG and for a new CDK application key epoch derived from an
  active root.
- [x] Maintain an explicit usage registry so a root epoch cannot retire while
  any registered payment backend or FROST application still references it.
- [x] Allow an application to rotate its derived key or derivation version
  without silently rotating or mutating the reusable root package.
- [x] Keep old descriptors recognized for deposits and UTXO ownership during a
  bounded migration window.
- [x] Direct all new addresses and change to the active epoch after activation.
- [x] Build and threshold-sign sweep transactions from retiring epochs to the
  active epoch.
- [x] Prevent CDK application retirement while spendable/reserved UTXOs or
  unresolved transactions remain, and prevent root retirement until every
  application-specific retirement condition is satisfied.
- [x] Expose public active/retiring root and application epoch metadata without
  exposing private shares.
- [x] Bind signing sessions to root and application epochs and bind address
  mappings permanently to their CDK application epoch.
- [x] Document that adding members requires fresh DKG or a separately reviewed
  resharing protocol; basic FROST refresh is not silently treated as complete
  federation reconfiguration.

**Success criteria:**

- [x] New deposits use only the new epoch after activation.
- [x] Old UTXOs remain spendable by the old threshold until swept.
- [x] No transaction signs with shares or derivation data from mixed epochs.
- [x] Root retirement is blocked while a test consumer remains registered and
  succeeds after every consumer completes its retirement contract.
- [x] Rotation and sweep survive member restart and catch-up.

**Required tests:**

- [x] DKG, proposed activation, rejected activation, and successful activation.
- [x] Deposit arriving at old address before and after cutoff.
- [x] Old-to-new sweep with one member offline.
- [x] Mixed-epoch input policy and rejection.
- [x] Restart during sweep and final retirement.
- [x] Attempt to delete old shares while unresolved UTXOs remain.
- [x] Attempt to retire or delete a root share while another application still
  references it.
- [x] Independent CDK application-key rotation without root-share mutation.

**Milestone record:** completed on 2026-07-14. Consensus now owns explicit
proposed, active, retiring, and retired root/application epoch transitions,
including a registered-consumer usage contract that blocks premature
retirement and local share deletion. Application registrations are keyed by
their complete root/application/version tuple, so a CDK application can rotate
without mutating its reusable root package. Public status exposes only redacted
lifecycle metadata and commitments.

Address allocation uses only the active application epoch, while chain
observation retains bounded knowledge of active and retiring descriptors.
Epoch-sweep proposals have a distinct canonical purpose, accept inputs from
exactly one retiring epoch, pay one consensus-allocated active internal script,
and are revalidated before signing. Confirmation atomically and idempotently
marks their source UTXOs spent. The normal proposal, signing, broadcast, and
recovery state machines are reused through thin federation adapters; existing
mint quote, saga, and payment behavior was not reimplemented.

Rotation revealed one genuine pre-existing gap outside the wallet boundary:
the DKG startup path rejected every ceremony once a finalized BLS federation
configuration existed. The focused fix validates a finalized configuration as
an immutable rotation base, permits a new dual-result ceremony, reconstructs
and verifies the complete new BLS candidate, and persists the new FROST root as
`Proposed`. Bootstrap still activates both independent results together, while
rotation requires explicit consensus activation. Both ceremonies remain bound
to the same roster and threshold and fail closed on any mismatch.

The [key-rotation runbook](federated-frost-key-rotation.md) covers dual DKG,
restart/catch-up, application activation, old-descriptor migration, threshold
sweeps, guarded retirement, and the requirement for a fresh dual DKG or a
separately reviewed resharing protocol when membership changes. All 25
`cdk-frost` tests, 381 active `cdk-common` federation library tests, 5 external
FROST-DKG tests, 128 CDK mint-federation tests, 124 `cdk-bdk` tests, 9 Axum
FROST-signing tests, and 16 Axum DKG-driver tests pass. Workspace all-targets
checking and stable rustfmt pass. Strict no-dependency Clippy passes for the
new FROST and BDK boundaries; broader common/CDK strict Clippy remains blocked
only by its documented pre-existing warning baseline.

### Milestone 23: operator configuration, status, metrics, and tooling

**Objective:** make deployment and diagnosis possible without exposing custody
secrets or requiring database inspection.

**Work:**

- [x] Add validated mintd configuration for enabling the federated BDK wallet,
  Bitcoin network, local chain source, confirmation policy, fee policy, and
  bounded timeouts.
- [x] Reject local mnemonic configuration when the federated watch-only wallet
  is selected.
- [x] Extend federation status with wallet epoch, descriptor commitment, sync
  tip, revealed indexes, UTXO counts, reserved value, pending batches, signing
  attempts, and broadcast health.
- [x] Expose safe root-key status containing root epoch, DKG/ciphersuite version,
  threshold, transcript commitment, registered application IDs, and lifecycle
  state without exposing shares or derivation-sensitive secrets.
- [x] Add bounded Prometheus metrics without quote IDs, addresses, tx contents,
  key material, or unbounded labels.
- [x] Add operator commands for root DKG status, registered-application status,
  wallet status, transaction status, safe rebroadcast, and guarded root or
  application key rotation.
- [x] Require explicit guarded procedures for abandoning/replacing transactions;
  do not provide a force-spend or private-key export command.
- [x] Document mempool.space, self-hosted Esplora, and Bitcoin Core deployment
  tradeoffs.
- [x] Add startup warnings for correlated chain sources and incomplete quorum.

**Success criteria:**

- [x] Operators can identify whether a payment is waiting on consensus, chain
  observations, proposal, signatures, broadcast, or confirmation.
- [x] Status and metrics reveal no secret shares, nonce material, credentials,
  addresses, or sensitive local paths.
- [x] Invalid or mixed standalone/federated BDK configuration fails startup.

**Required tests:**

- [x] Configuration parsing, defaults, environment overrides, and invalid
  combinations.
- [x] Status for every wallet operation phase.
- [x] Metric cardinality and secret-redaction tests.
- [x] CLI smoke tests for status, rebroadcast, and rotation safety guards.
- [x] Startup fail-closed tests for missing wallet config/share/chain source.
- [x] Root/application registration, status, and retirement-guard tests.

**Milestone 23 completion record:** mintd now selects `federated-bdk`
explicitly, rejects standalone mnemonic custody and mixed wallet modes, validates
the consensus network/confirmation/fee policy against bounded member-local
Esplora or Bitcoin Core settings, and registers only a thin quote/settings
adapter. Address, payment, and settlement mutations still flow through the
accepted federation wallet operations; the adapter fails closed if called as a
standalone spender.

The authenticated private operator API provides targeted transaction status
and exact-byte rebroadcast only. The CLI combines these routes with existing DKG
status, safe public root/application status, and consensus-gated rotation.
Rebroadcast and rotation require explicit acknowledgements, and no force-spend,
force-release, share-export, nonce-export, mnemonic-export, or private-key
export command exists. The [federated BDK operations runbook](federated-bdk-operations.md)
documents configuration, independent versus correlated chain sources, status,
bounded metrics, diagnosis, rebroadcast, and guarded rotation.

Verification covers all fixed wallet phases, exact public/private root-epoch
inventory at startup, mixed/missing wallet and chain-source failures, config and
environment parsing, CLI parsing and safety guards, targeted authenticated
status, public status redaction, fixed-label metric cardinality, and guarded
root/application retirement. All 218 mintd library tests with federation test
utilities, all 9 Prometheus tests, 382 active cdk-common federation library
tests, and the focused Axum operator/startup tests pass. Stable rustfmt and
all-target checking pass. Strict no-dependency Clippy passes for Prometheus;
the common, Axum, and mintd runs contain only their documented broader warning
baseline after the new M23 warnings were removed.

A broad parallel Axum run currently reports 398 passing, 57 failing, and one
ignored test. The failures cluster in existing quote/melt shared-state tests,
one process-global metric assertion, and two listener binds denied by the
sandbox. They are not counted as M23 evidence and remain an explicit Milestone
24/full-regression blocker until the fault harness isolates shared state and the
value-flow failures are resolved.

### Milestone 24: end-to-end federation fault matrix

**Objective:** prove real Cashu minting and melting against a live FROST wallet
under realistic member and network failures.

**Work:**

- [x] Add a deterministic regtest harness for 3-of-5 and configurable larger
  federations.
- [x] Automate FROST DKG, descriptor activation, funding, deposit address
  allocation, mining, Cashu minting, on-chain melting, and balance checks.
- [x] Capture journal, state digest, BDK balance, UTXO set, transaction state,
  and Bitcoin node state at each checkpoint.
- [x] Add controlled member stop/start, REST partition, delayed messages,
  chain-source outage, database contention, and process-kill injection.
- [x] Verify no duplicate Bitcoin payments, no unauthorized outputs, no nonce
  reuse, no stuck reservations after defined recovery, and no member divergence.
- [x] Verify CLI and wallet-library behavior, not only internal APIs.
- [x] Keep tests bounded and split slow fault matrices from routine unit tests.

**Success criteria:**

- [x] A fresh 3-of-5 federation completes on-chain mint and melt with two
  members offline where thresholds permit.
- [x] All recovered members converge on identical journal and materialized-state
  digests, address mappings, UTXOs, and transactions.
- [x] Bitcoin Core reports exactly the expected deposits and outgoing payments.
- [x] Standalone mint and wallet workflows remain unchanged.

**Required tests:**

- [x] Happy-path fixed 3-of-5 flow with two deposits and two melts.
- [x] 8-of-11 smoke flow and one bounded 14-of-20 signing/DKG flow.
- [x] Offline proposer, coordinator, signer, broadcaster, and observer.
- [x] Below-threshold partition followed by healing and catch-up.
- [x] Restart at address, observation, proposal, commitment, share, signed,
  broadcast, and confirmation phases.
- [x] Conflicting transaction proposals and invalid signature share.
- [x] Esplora outage with RPC members still available and the reverse.
- [x] Reorg before minting and controlled post-liability reorg response.
- [x] Sender CLI on-chain mint and receiver CLI on-chain melt smoke tests.

**Milestone 24 completion record:** the deterministic Bitcoin Core regtest
harness now provisions dual dealer-free Cashu BLS and secp256k1 FROST DKG,
activates the Taproot descriptor, drives the public CLI through two deposits and
two melts, and checks Bitcoin transactions, balances, UTXOs, journals, state
digests, address mappings, and recovery certificates. Separate bounded recipes
cover happy, one-member-offline, process-kill, and below-threshold-heal 3-of-5
profiles. A release 8-of-11 run completed the full flow with final balances of
50,000, 90,000, 78,058, and 64,116 satoshis at its checkpoints; Bitcoin Core
reported two unique deposits and two unique outgoing transactions, and every
recovered member converged. The reusable FROST component matrix independently
completed 14-of-20 DKG and BIP340-compatible signing.

The matrix exercises role outages, every durable wallet phase, conflicting
proposals, invalid shares, heterogeneous chain-source outages, reorg behavior,
contention, catch-up, and exact-byte rebroadcast. It uncovered three small
shared-domain gaps rather than missing mint business logic: catch-up and live
finality could apply dependent operations concurrently, a completed DKG member
could not replay its exact durable activation after restart, and configured
member-request retries had no bounded delay for transient recovery. Each fix is
localized to its existing concurrency, ceremony, or connector boundary and has
a focused regression test; standalone paths remain unchanged.

Verification includes 25 `cdk-frost` tests, 124 serial `cdk-bdk` tests, all 458
active federation-enabled Axum tests (456 in the sandbox plus the two
listener-binding cases rerun with socket access), and 382 active
federation-enabled `cdk-common` tests. The slow live fault matrix is isolated
from routine unit tests, supports debug or release binaries, and validates
member process liveness and bounded staged startup.

### Milestone 25: security hardening and independent review

**Objective:** audit the cryptographic and state-machine boundaries before
mainnet use.

**Work:**

- [x] Produce a protocol specification for DKG, derivation, address allocation,
  transaction consensus, FROST sessions, nonce lifecycle, and key rotation.
- [x] Specify the canonical untweaked root package, root-to-application adapter
  boundary, compatibility requirements, authorization contract, shared
  compromise domain, and multi-application rotation rules.
- [x] Map every security requirement in this plan to code and tests.
- [x] Threat-model malicious proposer, coordinator, signer, observer,
  broadcaster, database, chain source, and wallet client.
- [x] Threat-model a malicious or compromised registered application attempting
  cross-application signing, nonce reuse, derivation confusion, root extraction,
  or premature root retirement.
- [x] Review all canonical encodings, domain separators, participant mappings,
  scalar tweaks, parity handling, and Taproot sighashes.
- [x] Fuzz DKG, config, proposal, PSBT, signing-package, signature-share,
  transaction, and catch-up decoders.
- [x] Add property tests for accounting conservation, UTXO exclusivity, intent
  uniqueness, and terminal-state monotonicity.
- [x] Review database transaction boundaries and failed-write recovery.
- [x] Review private REST authentication, replay windows, request limits, and
  denial-of-service exposure.
- [ ] Arrange independent cryptographic review of threshold derivation and
  nonce handling.
- [ ] Obtain interoperability review of the canonical root format and at least
  two independent application adapters before promising external compatibility.
- [ ] Resolve all high- and medium-severity findings before release.

**Success criteria:**

- [x] Every signature share can be traced to one root epoch, application and key
  epoch, derivation context, authorization proof, signer set, attempt, and
  durably consumed nonce; CDK shares additionally trace to one accepted
  transaction and input.
- [x] Every spendable UTXO and payment intent has one unambiguous state owner.
- [x] No identified path lets fewer than threshold members spend or lets one
  member create Cashu liability from an unconfirmed deposit.
- [ ] Independent reviewers approve the derivation and signing design or all
  requested changes are completed and retested.

**Required tests:**

- [ ] Continuous fuzz runs with retained regression corpus.
- [x] Model/property tests across randomized operation order and failure points.
- [ ] Static analysis, clippy, formatting, docs, and dependency audit.
- [x] Secret-scanning of logs, snapshots, fixtures, and generated artifacts.
- [x] Full standalone and federation regression suites.

**Milestone 25 progress record:**
[`federated-frost-protocol-and-security.md`](federated-frost-protocol-and-security.md)
now specifies the dual ceremony, root/application boundary, address and deposit
rules, exact transaction authorization, FROST session and nonce lifecycle,
broadcast/catch-up, rotation, canonical encodings, threat model, persistence and
REST boundaries, and a requirement-to-code/test matrix. It is also the explicit
independent-review handoff and states that it is not an external approval.

FROST fuzz coverage now includes both DKG rounds, every public signing wire
type, nonce records, config and DKG REST records, signing packages, catch-up,
raw transactions, and PSBTs. A dedicated sanitizer campaign composes those
targets with the existing operation and transaction-proposal fuzzers, retains
corpora, and supports continuous or bounded execution. A scheduled CI workflow
now restores and retains the evolving corpus; its first hosted run remains
required before the continuous-fuzz checkbox can close. A bounded four-target
60-second-per-target campaign completed without crashes. Deterministic 512-case
accounting/ownership and 512-step lifecycle properties preserve conservation,
uniqueness, exact rollback on rejected transitions, sticky reservations, and
terminal monotonicity.

Strict workspace Clippy, all-target checking, stable formatting, spelling, and
workflow syntax pass. Secret scans of the repository, generated test material,
the goal commit range, and the current patch pass with exact allowlist
fingerprints for reviewed deterministic fixtures. The dependency audit removed
the vulnerable `crossbeam-epoch` and `protobuf` versions. Aligning the supported
Arti runtime with its existing native-TLS Hyper connector also removed the
legacy `x509-signature` -> `ring` 0.16.20 path without changing the transport
API. The BIP353 resolver now builds bounded wire-format DNSSEC proofs over the
existing ordinary or Tor HTTPS transport and verifies the complete proof and
signature-validity interval locally before returning TXT records. This removed
Hickory and its two advisories and closed the Tor path that previously trusted
resolver JSON. One unsuppressed medium-severity RSA advisory remains through the
optional Arti graph; the security packet records its path and disposition. A
larger Arti update would require replacing the discontinued `arti-hyper`
connector and still would not remove that upstream RSA dependency. The combined
static and dependency gate therefore remains unchecked.

The standalone and federation regression matrix passes. The workspace library
suite, federation-enabled `cdk-common`, serial `cdk-bdk`, all remaining
workspace crates, and all workspace doc tests are clean. The full
federation-enabled Axum run passed 457 tests with one ignored test except for a
single catch-up fixture that had selected a non-local payment actor; after the
fixture was made to select the local actor deterministically, its focused
regression passes without changing mint behavior. The federation-enabled mint
daemon, strict workspace Clippy, and all-target workspace check also pass.

The independent cryptographic and two-adapter interoperability reviews, their
finding resolution, the first scheduled fuzz run, unresolved dependency
advisories, and continuous hosted fuzz evidence remain deliberately unchecked.
Mainnet release is still blocked on them.

### Milestone 26: performance, capacity, and release gate

**Objective:** establish bounded production behavior and publish deployment
limits.

**Work:**

- [ ] Benchmark DKG separately from steady-state address, observation,
  transaction, and signing operations.
- [ ] Measure debug and release 3-of-5, 8-of-11, and 14-of-20 profiles.
- [ ] Measure address allocation latency, BDK sync duration, proposal validation,
  consensus acceptance, commitment collection, share collection, aggregation,
  broadcast, and end-to-end melt latency.
- [ ] Measure multi-input and batched-transaction scaling.
- [ ] Measure SQLite and PostgreSQL behavior under concurrent observations and
  signing sessions.
- [ ] Measure idle CPU, private REST traffic, memory, database growth, and nonce
  cleanup.
- [ ] Establish maximum supported members, inputs, outputs, batch size, revealed
  addresses, and concurrent signing sessions.
- [ ] Keep only optimizations with repeatable improvement and no loss of
  verification, restart safety, transcript equality, or responsiveness.
- [ ] Publish operator sizing, chain-source, confirmation, fee, recovery, and
  key-rotation guidance.

**Success criteria:**

- [ ] Production limits are enforced in code and documented.
- [ ] No unbounded loop, allocation, retry, label, or request fanout exists on a
  public or private route.
- [ ] Idle federation CPU and network use remain low.
- [ ] End-to-end performance is measured and acceptable for on-chain payment
  timescales.
- [ ] Release checklist, migration procedure, and rollback limits are explicit.

**Required tests:**

- [ ] Repeatable benchmark harness with machine-independent result fields.
- [ ] Sustained deposit-observation and payment-proposal load.
- [ ] Maximum-size valid and one-over-limit operations.
- [ ] Slow/offline member and slow chain-source capacity tests.
- [ ] Long-running restart/rebroadcast/reconciliation soak test.
- [ ] Final workspace format, check, clippy, unit, integration, regtest, and
  documentation verification.

## 9. Definition of done

The federated BDK-FROST wallet is complete only when all of the following are
true:

- A fresh federation can run independent BLS Cashu DKG and application-neutral
  secp256k1 FROST root DKG ceremonies and activate both verified transcripts.
- Each member retains one canonical untweaked root share in the reusable FROST
  key manager; no BDK-, Taproot-, network-, or AlephBFT-specific processing is
  baked into the only durable representation.
- The CDK on-chain backend is a registered application that derives its own
  domain-separated key package and authorizes signing through AlephBFT.
- A second test application can reuse the activated root shares through the
  reusable key-manager boundary, derive a distinct key, and complete an authorized
  threshold signature without application-owned root-share copies or
  reconstructing a private key.
- Every member runs an identical watch-only Taproot BDK wallet without an xprv
  or complete Bitcoin signing key.
- Consensus allocates unique deposit and change addresses and preserves them
  through replay, restart, and catch-up.
- Confirmed on-chain deposits satisfy existing federated mint quotes through
  normal payment observations.
- An untrusted member can propose a BDK-built transaction, every member can
  validate it independently, and AlephBFT accepts exactly one transaction.
- A threshold signs every input through durable, nonce-safe FROST rounds.
- Any member can finalize, broadcast, rebroadcast, and reconcile the accepted
  transaction without creating a duplicate payment.
- Existing Cashu wallet, mint, melt, error, and saga behavior remains the public
  behavioral authority.
- Member outages, partitions, restarts, stale chain sources, invalid shares,
  conflicting proposals, and ambiguous broadcast responses pass the documented
  fault matrix.
- State digests, catch-up, operator status, metrics, and public configuration
  cover the wallet without exposing secrets.
- Standalone CDK mints and the mnemonic-backed `cdk-bdk` backend remain
  behaviorally unchanged.
- The derivation and FROST nonce/signing design has received focused independent
  review before mainnet funds are accepted.
- Root/application compatibility, authorization isolation, and shared rotation
  behavior have received focused independent review before the root epoch is
  advertised as reusable by external payment backends or FROST applications.
