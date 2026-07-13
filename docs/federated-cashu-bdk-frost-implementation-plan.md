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

> Keep this section below 500 words. Record only concise, actionable facts for
> the next implementation round: milestone, symptom, verified cause, clean fix
> or open question, and the regression test or measurement that preserves the
> lesson. Move long investigations to a dedicated issue, benchmark document, or
> commit description instead of growing this section indefinitely.

- Record implementation bugs here as they are encountered and verified.
- The existing Cashu DKG creates BLS12-381 shares. Those shares cannot be
  reinterpreted, hashed, or truncated into Bitcoin secp256k1 FROST shares. Reuse
  the ceremony infrastructure, not the cryptographic shares.
- Existing member secp256k1 identity keys authenticate federation traffic; they
  are independent keys and must not double as Bitcoin custody shares.
- The canonical DKG result must be the untweaked FROST root share and public
  package. Do not make a library-specific post-DKG Taproot package the only
  persisted representation; BIP32, BIP341, and future application tweaks belong
  to derived application packages.
- Never reconstruct the FROST group secret during normal setup, signing,
  recovery, testing, migration, or operator tooling.
- BDK's custom signer API is synchronous. Do not hide asynchronous FROST rounds
  or consensus waits inside `Wallet::sign()`.
- Never call `reveal_next_address()` independently in a public request handler.
  Consensus allocates address indexes; replay and catch-up reveal through an
  accepted target index idempotently.
- FROST nonces are one-time secrets. Persist reservation/consumption before
  exposing commitments, and never reuse a nonce after uncertainty or restart.
- A transaction proposer is untrusted. Consensus must accept the exact unsigned
  transaction before any member releases a signature share.
- Milestone 0 regtest setup timed out while mining 200 blocks in one Bitcoin
  Core RPC call. The integration-only client now mines in bounded 100-block
  calls; preserve this with the batch-splitting unit test and the on-chain
  regtest baseline.
- Milestone 1 external Taproot finalization must not treat BDK's finalizer as a
  signature verifier. The watch-only engine verifies the sighash, tweaked key,
  spent script, and Schnorr signature before finalization; preserve the missing,
  malformed, wrong-key, and wrong-sighash tests.
- Milestone 4 found pre-existing test/API drift after payment quote fields and
  HTTP transport methods changed. Fixtures, mock methods, and affected golden
  hashes were updated mechanically; no mint business flow was redesigned.
- Milestone 4 partition testing found that inbound completeness alone could let
  a FROST member advance while peers lacked its outbound package. Require a
  fully acknowledged outbound batch before each phase transition and preserve
  the partition/response-loss regression tests.
- Milestone 4 security review found confidential FROST round-two retry payloads
  in plaintext persistence. Network requests remain transient, while durable
  sender and receiver records are now sealed and context-bound; preserve the
  ciphertext-envelope and idempotent-retry tests.
- Milestone 5 found that the Milestone 3 application registration omitted its
  key epoch even though the plan required epoch-separated rotation. The small
  fix stays inside `cdk-frost`/`cdk-frost-bip32`: application derivation,
  authorization, nonce IDs, wire transcripts, chain codes, descriptors, and
  signing targets now bind the application key epoch. Preserve the wrong-epoch
  rejection and epoch-separated descriptor vectors.
- Milestone 6's federation-enabled daemon build found that the existing DKG
  operator-status label adapter had not been extended for the three isolated
  FROST phases. The small status-only fix adds those labels; it does not alter
  mint business logic or either DKG state machine. The same build exposed
  federation-only test doubles that had drifted from current HTTP transport and
  wallet-saga fields; the compatibility repair is confined to test code.

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

- [ ] Add a canonical address-allocation operation keyed by quote ID and CDK
  application key epoch.
- [ ] Store separate monotonic external and internal next-index counters in
  materialized consensus state.
- [ ] Atomically map each on-chain mint quote to one external derivation index.
- [ ] Return the existing mapping for duplicate/replayed quote requests.
- [ ] Reject attempts to remap one quote or reuse one index for another quote.
- [ ] Apply accepted indexes to BDK with `reveal_addresses_to` semantics.
- [ ] Include allocation counters and mappings in state digest and catch-up.
- [ ] Define exhaustion and maximum-index behavior.
- [ ] Ensure public handlers never advance a local index before consensus.

**Success criteria:**

- [ ] Concurrent requests at different members receive distinct addresses.
- [ ] The same quote request at every member returns one identical address.
- [ ] A member catching up across an index gap reveals every required script and
  reconstructs the same quote mapping.
- [ ] Restart cannot reuse or skip an accepted index incorrectly.

**Required tests:**

- [ ] Sequential, concurrent, duplicate, replayed, and conflicting allocation.
- [ ] Allocation with one member offline and later catch-up.
- [ ] Crash before consensus, after acceptance, before BDK persistence, and after
  BDK persistence.
- [ ] External/internal keychain separation.
- [ ] Index-boundary and exhaustion tests.
- [ ] Materialized-state digest mismatch on altered mapping or counter.

### Milestone 8: chain observations and consensus UTXO materialization

**Objective:** turn local BDK observations into an agreed, conservative wallet
UTXO state.

**Work:**

- [ ] Define canonical deposit observation records containing outpoint, txout,
  allocated address reference, block hash/height, and observer member.
- [ ] Validate observed scripts and amounts against accepted address allocation.
- [ ] Reuse or extend federation payment-observation quorum rules for confirmed
  on-chain deposits.
- [ ] Define agreement requirements for block identity, height, transaction, and
  confirmation depth.
- [ ] Materialize UTXO states such as observed, confirmed, spendable, reserved,
  spent, reorged, and quarantined.
- [ ] Make duplicate observations idempotent and conflicting evidence explicit.
- [ ] Define conservative reorg handling and liability alarm behavior.
- [ ] Include accepted UTXOs and observation evidence in state digest and
  catch-up snapshots.
- [ ] Keep raw local BDK observations outside consensus until submitted as
  bounded canonical evidence.

**Success criteria:**

- [ ] One member or one chain source cannot make a deposit spendable alone when
  policy requires a quorum.
- [ ] Honest members converge on identical spendable UTXOs despite observation
  timing differences.
- [ ] Conflicting block/transaction evidence cannot silently mutate a UTXO.
- [ ] Catch-up reconstructs the exact wallet UTXO set.

**Required tests:**

- [ ] Confirmed deposit with threshold matching observations.
- [ ] Unconfirmed, insufficient-confirmation, and threshold-minus-one evidence.
- [ ] Duplicate and reordered observations.
- [ ] Conflicting amount, script, outpoint, block hash, and height.
- [ ] Reorg before minting and reorg after liability issuance.
- [ ] One stale, lying, offline, or delayed observer.
- [ ] Members using a shared source versus independent sources are reported
  accurately in diagnostics.
- [ ] Snapshot/catch-up and restart convergence.

### Milestone 9: integrate incoming on-chain mint quotes

**Objective:** satisfy existing Cashu on-chain mint quotes from the federated
wallet without introducing a parallel quote or recovery model.

**Work:**

- [ ] Route federation on-chain incoming-payment creation through consensus
  address allocation.
- [ ] Preserve existing `CreateIncomingPaymentResponse`, quote ID, amount,
  expiry, and notification contracts.
- [ ] Associate confirmed accepted UTXOs with the correct mint quote.
- [ ] Submit normal federation mint payment observations only after accepted
  confirmation policy is satisfied.
- [ ] Prevent one deposit from paying multiple quotes unless the protocol
  explicitly supports and accounts for that case.
- [ ] Preserve overpayment, underpayment, duplicate payment, and multiple-UTXO
  policy explicitly.
- [ ] Reuse existing mint quote state and wallet saga behavior.
- [ ] Keep standalone on-chain mint quote behavior unchanged.

**Success criteria:**

- [ ] A federation address can receive a regtest payment and satisfy exactly one
  on-chain mint quote after required confirmation and observation quorum.
- [ ] The wallet receives ordinary Cashu mint quote states and can mint BLS
  e-cash through the existing federation connector.
- [ ] Restart and member failover do not create a second address or quote.

**Required tests:**

- [ ] Happy-path on-chain quote, deposit, confirmation, observation, and mint.
- [ ] Slow confirmation and member observation delay.
- [ ] Underpayment, overpayment, duplicate transaction, and multiple UTXOs.
- [ ] Wrong address, wrong network, and unrelated wallet transaction.
- [ ] Reorg before payment acceptance.
- [ ] Interrupted wallet/client mint recovery.
- [ ] One member offline during allocation and payment observation.
- [ ] Standalone BDK mint regressions.

### Milestone 10: define payment intents, batches, and transaction proposals

**Objective:** represent outgoing payments and candidate PSBTs as bounded,
canonical, consensus-validatable data.

**Work:**

- [ ] Define canonical on-chain payment intent with quote ID, destination script,
  amount, maximum fee, fee tier, creation time, and policy version.
- [ ] Define deterministic batch eligibility and intent ordering independent of
  proposer-local hash-map or database iteration order.
- [ ] Define a proposal operation containing batch ID, ordered intent IDs,
  selected inputs with trusted previous outputs, recipient outputs, change
  index, unsigned transaction, fee data, and PSBT commitment.
- [ ] Set strict limits for intents, inputs, outputs, transaction weight, PSBT
  bytes, and operation bytes.
- [ ] Canonicalize or exclude non-consensus PSBT metadata.
- [ ] Add conflict keys for payment intents, batches, UTXOs, and replacement
  lineage.
- [ ] Bind every proposal to root epoch, CDK application key epoch, and
  transaction policy version.
- [ ] Keep proposer identity for diagnostics without granting it authority.

**Success criteria:**

- [ ] Equivalent proposals encode identically and hash identically.
- [ ] Oversized or ambiguous proposals fail before consensus admission.
- [ ] Two proposals spending the same UTXO or intent cannot both be accepted.
- [ ] Proposal types contain enough information for every member to recompute
  all Taproot sighashes without trusting proposer-supplied digests.

**Required tests:**

- [ ] Canonical encoding and stable hash vectors.
- [ ] Reordered intents, inputs, outputs, and PSBT maps.
- [ ] Duplicate intent/input/output and conflicting batch tests.
- [ ] Size, count, weight, and amount bounds.
- [ ] Wrong epoch, policy version, network, and malformed transaction.
- [ ] Property and fuzz tests for proposal decoding and conflict keys.

### Milestone 11: implement independent transaction-policy validation

**Objective:** let every member validate an untrusted BDK-built proposal without
requiring deterministic coin selection.

**Work:**

- [ ] Verify every input is an accepted spendable UTXO for the selected epoch.
- [ ] Verify inputs are not reserved, spent, reorged, quarantined, or assigned
  to another accepted transaction.
- [ ] Verify each payment intent is pending, included exactly once, and matches
  one recipient output exactly.
- [ ] Reject all unauthorized recipient and data-carrying outputs.
- [ ] Derive and verify the change script from the proposed internal index.
- [ ] Verify checked input/output sums, exact fee, feerate bounds, dust, weight,
  maximum fee, and per-intent fee allocation.
- [ ] Verify version, locktime, sequences, RBF policy, sighash type, and Taproot
  key-path-only policy.
- [ ] Verify PSBT previous-output data and strip/reject prohibited proprietary
  or signer-controlled fields.
- [ ] Recompute transaction ID and each input sighash locally.
- [ ] Return deterministic Cashu/federation errors for business-invalid
  proposals and retryable errors for unavailable local verification data.

**Success criteria:**

- [ ] A valid proposal built by any member passes every honest member.
- [ ] Any change to destination, amount, fee, input, change, sighash, or policy
  causes deterministic rejection before signing.
- [ ] Validation does not depend on the proposer using the same BDK version or
  coin-selection algorithm.

**Required tests:**

- [ ] Valid single and batched payment proposals.
- [ ] Unauthorized input/output and unknown PSBT field.
- [ ] Missing, duplicate, spent, reserved, reorged, and wrong-epoch UTXO.
- [ ] Wrong destination, amount, change index, fee, feerate, dust, locktime,
  sequence, RBF, and sighash.
- [ ] Integer overflow/underflow and maximum-bound tests.
- [ ] Locally stale BDK cache versus sufficient consensus UTXO data.
- [ ] Differential tests where multiple BDK coin selections are all valid.

### Milestone 12: accept transactions and reserve UTXOs through AlephBFT

**Objective:** serialize the exact transaction before any signing material is
released.

**Work:**

- [ ] Add transaction-proposal operation admission and deterministic apply.
- [ ] Atomically transition intents to accepted batch and inputs to reserved.
- [ ] Persist the exact unsigned transaction, previous outputs, PSBT commitment,
  change allocation, policy audit, and computed sighashes.
- [ ] Make duplicate proposal replay return the accepted transaction.
- [ ] Reject a different transaction under the same batch or intent scope.
- [ ] Include accepted transactions and reservations in materialized-state
  digest and catch-up.
- [ ] Add explicit abandoned/replaced transitions that cannot free inputs while
  a valid signed transaction may still exist.
- [ ] Gate FROST session creation on accepted and locally applied operation
  status.

**Success criteria:**

- [ ] Exactly one transaction owns each reserved UTXO and payment intent.
- [ ] Every honest member stores identical transaction bytes and sighashes.
- [ ] No FROST round can begin for pending, rejected, failed, or catch-up-only
  unverified state.
- [ ] Restart preserves reservations and accepted transaction identity.

**Required tests:**

- [ ] Concurrent valid proposals for the same batch.
- [ ] Concurrent proposals sharing one UTXO but different intents.
- [ ] Duplicate and conflicting operation replay.
- [ ] Crash before and after atomic reservation.
- [ ] Member catch-up after proposal acceptance.
- [ ] Attempted signing before finalization or while readiness-gated.
- [ ] State-digest tampering with transaction bytes, sighashes, or reservations.

### Milestone 13: implement durable one-time FROST nonce management

**Objective:** make nonce reuse impossible across concurrency, failures,
restarts, coordinator failover, and every application sharing a root epoch.

**Work:**

- [ ] Define signing session ID from federation ID, root key epoch, application
  ID, application key epoch, application derivation context, authorization
  reference, message/transcript commitment, attempt, and signer set. The CDK
  adapter additionally binds accepted operation ID, transaction ID, input index,
  and sighash type.
- [ ] Add private local nonce records with generated, reserved, commitment-sent,
  share-produced, consumed, and burned terminal states.
- [ ] Keep one nonce store and reservation namespace per participant key manager,
  not independent nonce stores in each payment backend or application adapter.
- [ ] Generate nonce material from a cryptographically secure RNG independent of
  DKG and identity keys.
- [ ] Persist nonce reservation before returning a round-one commitment.
- [ ] Atomically consume or burn nonce state before or with round-two share
  release.
- [ ] Burn reservations after ambiguous response, abort, timeout, signer-set
  change, invalid package, or restart recovery.
- [ ] Reject duplicate round-one requests that change any session field.
- [ ] Define safe idempotent response behavior for exact authenticated retries.
- [ ] Exclude nonce material from snapshots, logs, metrics labels, and support
  bundles.
- [ ] Add bounded cleanup for terminal nonce metadata without deleting evidence
  needed to prevent replay.

**Success criteria:**

- [ ] No code path can use one nonce to produce shares for two packages.
- [ ] Two applications using the same root epoch cannot collide, reuse, or claim
  each other's nonce records.
- [ ] Exact retries are safe and conflicting retries fail closed.
- [ ] Restart at every nonce state cannot resurrect reusable material.
- [ ] Concurrent requests for the same input cannot reserve two active attempts
  without an explicit new attempt transition.

**Required tests:**

- [ ] Normal reserve, commitment, share, consume lifecycle.
- [ ] Duplicate exact round-one and round-two requests.
- [ ] Changed application, derivation context, authorization, message, signer
  set, attempt, input, or operation under the same session.
- [ ] Crash before/after commitment and before/after share persistence.
- [ ] Timeout, abort, coordinator loss, and signer substitution.
- [ ] Concurrent request race and database transaction failure injection.
- [ ] Secret-redaction and terminal-record cleanup tests.

### Milestone 14: add authenticated, application-aware FROST round transport

**Objective:** exchange commitments and shares through existing private REST
without adding a consensus phase or coupling the reusable key manager to one
payment backend.

**Work:**

- [ ] Add bounded signed private REST request/response types for commitment,
  signing package, signature share, and aggregate result notification.
- [ ] Put application-neutral wire types and orchestration behind the FROST key
  manager, with registered adapters responsible for application authorization
  and transcript validation.
- [ ] Reuse member identity authentication, request body hashes, sequence replay
  protection, timeouts, and response caches.
- [ ] Bind all requests to root epoch, application ID and key epoch, derivation
  context, authorization reference, attempt, signer set, and canonical package
  hash.
- [ ] Require the CDK adapter at each signer to load and revalidate the accepted
  transaction state locally before authorizing its request. Future adapters must
  provide their own typed, fail-closed authorization proof and validator.
- [ ] Reject unregistered applications and prevent any transport caller from
  bypassing its adapter with a raw digest.
- [ ] Verify coordinator-selected commitments belong to the declared signers.
- [ ] Permit any member to act as coordinator while preventing two attempts from
  sharing nonce material.
- [ ] Bound concurrent sessions, inputs, body sizes, retries, and per-peer work.
- [ ] Keep transport outcomes out of the ordering journal unless they cause an
  explicit accepted wallet-state transition.

**Success criteria:**

- [ ] A coordinator collects threshold valid commitments and shares over private
  REST and produces a valid signature.
- [ ] One member cannot elicit shares for an unaccepted or altered transaction.
- [ ] One application cannot elicit shares using another application's key,
  derivation context, authorization proof, or nonce commitment.
- [ ] Replayed, reordered, delayed, forged, and cross-session messages fail
  without nonce reuse.
- [ ] Signing works with non-coordinator members temporarily offline as long as
  threshold remains.

**Required tests:**

- [ ] 2-of-3 and 3-of-5 live private REST signing.
- [ ] Threshold-minus-one timeout.
- [ ] Wrong sender, receiver, sequence, federation, epoch, operation, input,
  attempt, and package hash.
- [ ] Wrong or unregistered application ID, application epoch, derivation
  context, and authorization proof.
- [ ] Duplicate/reordered commitment and share delivery.
- [ ] Invalid commitment/share and coordinator equivocation.
- [ ] Request timeout before and after signer processing.
- [ ] Backpressure and maximum-concurrency tests.

### Milestone 15: aggregate signatures, finalize PSBTs, and persist signed state

**Objective:** produce one Bitcoin-valid signed transaction and make it durable
before broadcast.

**Work:**

- [ ] Aggregate and verify threshold shares independently for every Taproot
  input.
- [ ] Insert aggregate key-path signatures and sighash types into the accepted
  PSBT.
- [ ] Finalize the PSBT through BDK/rust-bitcoin and extract the transaction.
- [ ] Verify extracted non-witness transaction data exactly matches consensus.
- [ ] Verify every final signature against the accepted output key and sighash.
- [ ] Persist signed transaction bytes, txid, signer-attempt metadata, fee audit,
  and input/output assignments before broadcast.
- [ ] Apply the signed transaction to every member's local BDK transaction graph
  idempotently.
- [ ] Include signed transaction identity and state in catch-up/digest without
  including private shares or nonces.
- [ ] Define behavior when different valid signer subsets produce the same
  deterministic BIP340 signature expectation or differing valid signatures.

**Success criteria:**

- [ ] The extracted transaction is accepted by Bitcoin Core script verification.
- [ ] Every member agrees on txid and non-witness transaction bytes.
- [ ] Invalid or insufficient shares cannot advance signed state.
- [ ] A crash after signing but before broadcast recovers the exact same signed
  transaction and never rebuilds with different inputs.

**Required tests:**

- [ ] Single-input and multi-input signing with different threshold subsets.
- [ ] Invalid share among valid shares and threshold recovery without it.
- [ ] Missing input signature and wrong sighash.
- [ ] PSBT mutation before and after aggregation.
- [ ] Crash/restart after each input signature and after signed persistence.
- [ ] Catch-up of a member that did not participate in signing.
- [ ] Bitcoin Core regtest `testmempoolaccept` or equivalent script validation.

### Milestone 16: broadcast, rebroadcast, and ambiguous-result recovery

**Objective:** broadcast exactly the accepted signed transaction with robust
multi-member failover.

**Work:**

- [ ] Allow any ready member to broadcast the persisted signed transaction.
- [ ] Reuse existing accepted/already-known/rejected/transient/unknown broadcast
  classification.
- [ ] Persist broadcast intent before the network request.
- [ ] Treat already-known transaction as success-equivalent.
- [ ] Retry transient and ambiguous failures without rebuilding or resigning.
- [ ] Record deterministic rejection for operator review and replacement policy.
- [ ] Reconcile mempool presence and confirmations from local chain sources.
- [ ] Prevent repeated broadcaster responses from duplicating payment completion
  events.
- [ ] Add proposer/coordinator/broadcaster failover timers that affect liveness,
  not transaction authority.

**Success criteria:**

- [ ] Loss of the original proposer or coordinator does not prevent another
  member from broadcasting.
- [ ] Broadcast timeout cannot create a second transaction or payment.
- [ ] All members converge on one txid and broadcast state.
- [ ] Restart automatically resumes safe rebroadcast and reconciliation.

**Required tests:**

- [ ] Accepted, already-known, deterministic rejection, transient failure, and
  unknown response.
- [ ] Response lost after backend accepted transaction.
- [ ] Original broadcaster offline before and after request.
- [ ] Multiple members broadcasting the same transaction concurrently.
- [ ] Restart with signed-not-broadcast and broadcast-not-observed states.
- [ ] Mempool eviction and later rebroadcast.

### Milestone 17: integrate outgoing on-chain melts and existing sagas

**Objective:** make federated on-chain melts use the joint wallet while keeping
normal Cashu quote, error, and recovery semantics.

**Work:**

- [ ] Route accepted federation on-chain melt execution into canonical payment
  intents rather than one member's local `make_payment` side effect.
- [ ] Preserve existing on-chain fee-option quote and selected-fee-index behavior.
- [ ] Bind payment intent IDs to melt quote and federation operation IDs.
- [ ] Keep proofs pending while transaction outcome remains unresolved.
- [ ] Emit normal payment observations and `MakePaymentResponse` data from
  accepted wallet transaction state.
- [ ] Return txid/outpoint payment proof through existing response fields.
- [ ] Reuse existing definitive and ambiguous melt saga behavior without adding
  federation-specific wallet recovery branches.
- [ ] Prevent one quote from entering two transaction batches or being paid twice.
- [ ] Define when a melt is considered paid: broadcast, mempool acceptance, or
  confirmation, and encode the choice in versioned policy.
- [ ] Preserve standalone `CdkBdk` melts unchanged.

**Success criteria:**

- [ ] A CLI wallet can melt Cashu to a regtest address through a 3-of-5
  federation and receive the existing terminal response shape.
- [ ] Exactly one Bitcoin payment occurs for one accepted melt quote.
- [ ] Ambiguous broadcast or confirmation state leaves recoverable saga state.
- [ ] Definitive invalid destination/amount/fee failures use normal Cashu errors.

**Required tests:**

- [ ] Happy-path single and batched federated on-chain melts.
- [ ] Invalid address, dust, insufficient funds, and fee above quote reserve.
- [ ] Quote replay and concurrent melt submissions at different members.
- [ ] Threshold unavailable before signing and after commitments.
- [ ] Broadcast timeout after acceptance.
- [ ] Interrupted client and mint restart recovery.
- [ ] Already-paid and already-spent proof behavior.
- [ ] Standalone on-chain melt regression suite.

### Milestone 18: full restart, replay, and catch-up recovery

**Objective:** prove every durable phase converges without duplicate custody
side effects.

**Work:**

- [ ] Enumerate every address, observation, proposal, reservation, nonce,
  signing, signed, broadcast, confirmation, replacement, and terminal state.
- [ ] Define startup recovery action or explicit no-op for every state.
- [ ] Restore and validate root epochs, the registered-application registry, and
  application key epochs before enabling any application adapter or signer.
- [ ] Rebuild watch-only BDK revealed indexes from accepted consensus state.
- [ ] Reapply accepted and signed transactions to BDK idempotently.
- [ ] Resume safe commitment/share collection only with a fresh signing attempt
  and fresh nonces where required.
- [ ] Resume rebroadcast using persisted transaction bytes.
- [ ] Integrate wallet records into existing journal replay and catch-up
  certificate verification.
- [ ] Prevent a catching-up member from signing until state digest and readiness
  gates pass.
- [ ] Add operator-visible stuck-state diagnosis without secret data.

**Success criteria:**

- [ ] Killing any member at any tested state does not lose an address, reuse a
  nonce, release a reservation, rebuild a signed transaction, or duplicate a
  payment.
- [ ] A member offline through multiple deposits and spends catches up to the
  same BDK scripts, UTXOs, transactions, and federation digest.
- [ ] A fully restarted quorum resumes pending work automatically.
- [ ] Restart with concurrent CDK and test-application signing sessions preserves
  authorization isolation and the shared nonce ledger.

**Required tests:**

- [ ] Failure injection before and after every durable transition.
- [ ] Full federation restart during DKG, allocation, observation, proposal,
  FROST rounds, signed persistence, broadcast, and confirmation.
- [ ] Restart with two registered applications and pending sessions in both.
- [ ] One member offline across address-index gaps and multiple spends.
- [ ] Catch-up from snapshot plus journal tail.
- [ ] Tampered snapshot, transaction, descriptor, and nonce metadata rejection.
- [ ] Repeated recovery invocation idempotency.

### Milestone 19: proposer and signing-coordinator liveness

**Objective:** remove elected-member single points of liveness failure without
weakening consensus authorization.

**Work:**

- [ ] Define deterministic initial proposer selection from accepted batch ID.
- [ ] Add bounded proposer lease/timeout and permission for another member to
  submit a candidate.
- [ ] Ensure competing valid candidates resolve through existing consensus
  conflict handling.
- [ ] Define deterministic initial signing coordinator and failover attempts.
- [ ] Require fresh signer nonces on every coordinator attempt.
- [ ] Prevent stale coordinators from completing an obsolete attempt after a
  later attempt becomes active.
- [ ] Allow any member to aggregate already valid shares for the active attempt.
- [ ] Add bounded retries, backoff, and status for unavailable signers.
- [ ] Ensure idle federation behavior does not reintroduce periodic high-CPU
  consensus activity.

**Success criteria:**

- [ ] Federation completes payments with any non-threshold set of members
  offline.
- [ ] Offline initial proposer, coordinator, or broadcaster causes bounded delay
  but no manual intervention.
- [ ] Failover cannot cause a second accepted transaction or nonce reuse.
- [ ] Idle CPU and network traffic remain bounded.

**Required tests:**

- [ ] Offline initial proposer before and after PSBT construction.
- [ ] Coordinator failure after commitment collection and during share collection.
- [ ] Signer-set substitution between attempts.
- [ ] Delayed stale coordinator messages after failover.
- [ ] Network partition below and above consensus/signing thresholds.
- [ ] Long idle period followed by deposit and melt.
- [ ] CPU/network benchmark for idle and active signing periods.

### Milestone 20: fee management, batching, and replacements

**Objective:** provide production-safe fee behavior without allowing fee policy
to bypass payment authorization.

**Work:**

- [ ] Version consensus fee policy, allowed fee sources, fallback values, and
  maximum feerate/absolute fee.
- [ ] Preserve immediate, standard, and economy batching semantics where they
  remain appropriate.
- [ ] Make batch-intent selection deterministic while allowing proposer-local
  coin selection.
- [ ] Allocate actual transaction fees to intents deterministically within each
  quoted maximum.
- [ ] Define RBF eligibility, replacement lineage, minimum fee delta, recipient
  invariance, and input/output mutation rules.
- [ ] Require replacement transactions to pass full consensus and use fresh
  FROST nonces/signatures.
- [ ] Define CPFP support or explicitly defer it with operator guidance.
- [ ] Prevent simultaneous original/replacement states from double-completing
  payment intents.
- [ ] Reconcile BDK mempool eviction and replacement observations.

**Success criteria:**

- [ ] Fee estimation outages cannot authorize unbounded fees.
- [ ] Replacements preserve authorized recipients and amounts.
- [ ] Original and replacement transaction lineage has one terminal payment
  result.
- [ ] Batches never charge an intent above its accepted fee reserve.

**Required tests:**

- [ ] Fee source success, timeout, disagreement, fallback, and extreme values.
- [ ] Mixed-tier batching and deterministic eligibility/order.
- [ ] Fee allocation caps and rounding.
- [ ] Valid RBF and invalid recipient/amount/input mutations.
- [ ] Original confirms after replacement proposal.
- [ ] Replacement confirms while original remains in another member's mempool.
- [ ] Insufficient fee reserve and no-valid-replacement behavior.

### Milestone 21: lightweight and heterogeneous chain backends

**Objective:** support simple Esplora deployments while making correlated trust
and availability visible.

**Work:**

- [ ] Document and test public mempool.space-compatible Esplora configuration.
- [ ] Support self-hosted Esplora and Bitcoin Core RPC per member.
- [ ] Keep chain-source URLs and credentials local while reporting a safe source
  class and health state.
- [ ] Bound Esplora parallel requests, scan lookahead, retries, and backoff.
- [ ] Avoid holding BDK/database locks during network requests.
- [ ] Add privacy guidance explaining script/address query leakage to public
  services.
- [ ] Add correlated-source warnings when operators intentionally declare the
  same upstream provider.
- [ ] Keep consensus observation quorum based on members, while documenting that
  common upstreams reduce practical independence.
- [ ] Define optional local header/confirmation verification improvements.

**Success criteria:**

- [ ] A complete regtest flow works with lightweight Esplora only.
- [ ] A heterogeneous federation using RPC and Esplora converges.
- [ ] One backend outage or stale response does not corrupt consensus state.
- [ ] Public Esplora is optional, not a federation-wide hard dependency.

**Required tests:**

- [ ] Esplora sync, fee estimate, broadcast, already-known, and retry behavior.
- [ ] Bitcoin Core RPC equivalent flow.
- [ ] Mixed-source observation timing and convergence.
- [ ] Rate limit, timeout, malformed response, stale tip, and omitted transaction.
- [ ] Chain reorg and backend recovery.
- [ ] Lock-duration and request-concurrency benchmarks.

### Milestone 22: root and application key rotation and federation preparation

**Objective:** rotate reusable root keys and CDK application keys safely, account
for every registered consumer, and leave a clean path for future membership
changes.

**Work:**

- [ ] Add consensus proposal and activation operations for a new root key epoch
  after successful DKG and for a new CDK application key epoch derived from an
  active root.
- [ ] Maintain an explicit usage registry so a root epoch cannot retire while
  any registered payment backend or FROST application still references it.
- [ ] Allow an application to rotate its derived key or derivation version
  without silently rotating or mutating the reusable root package.
- [ ] Keep old descriptors recognized for deposits and UTXO ownership during a
  bounded migration window.
- [ ] Direct all new addresses and change to the active epoch after activation.
- [ ] Build and threshold-sign sweep transactions from retiring epochs to the
  active epoch.
- [ ] Prevent CDK application retirement while spendable/reserved UTXOs or
  unresolved transactions remain, and prevent root retirement until every
  application-specific retirement condition is satisfied.
- [ ] Expose public active/retiring root and application epoch metadata without
  exposing private shares.
- [ ] Bind signing sessions to root and application epochs and bind address
  mappings permanently to their CDK application epoch.
- [ ] Document that adding members requires fresh DKG or a separately reviewed
  resharing protocol; basic FROST refresh is not silently treated as complete
  federation reconfiguration.

**Success criteria:**

- [ ] New deposits use only the new epoch after activation.
- [ ] Old UTXOs remain spendable by the old threshold until swept.
- [ ] No transaction signs with shares or derivation data from mixed epochs.
- [ ] Root retirement is blocked while a test consumer remains registered and
  succeeds after every consumer completes its retirement contract.
- [ ] Rotation and sweep survive member restart and catch-up.

**Required tests:**

- [ ] DKG, proposed activation, rejected activation, and successful activation.
- [ ] Deposit arriving at old address before and after cutoff.
- [ ] Old-to-new sweep with one member offline.
- [ ] Mixed-epoch input policy and rejection.
- [ ] Restart during sweep and final retirement.
- [ ] Attempt to delete old shares while unresolved UTXOs remain.
- [ ] Attempt to retire or delete a root share while another application still
  references it.
- [ ] Independent CDK application-key rotation without root-share mutation.

### Milestone 23: operator configuration, status, metrics, and tooling

**Objective:** make deployment and diagnosis possible without exposing custody
secrets or requiring database inspection.

**Work:**

- [ ] Add validated mintd configuration for enabling the federated BDK wallet,
  Bitcoin network, local chain source, confirmation policy, fee policy, and
  bounded timeouts.
- [ ] Reject local mnemonic configuration when the federated watch-only wallet
  is selected.
- [ ] Extend federation status with wallet epoch, descriptor commitment, sync
  tip, revealed indexes, UTXO counts, reserved value, pending batches, signing
  attempts, and broadcast health.
- [ ] Expose safe root-key status containing root epoch, DKG/ciphersuite version,
  threshold, transcript commitment, registered application IDs, and lifecycle
  state without exposing shares or derivation-sensitive secrets.
- [ ] Add bounded Prometheus metrics without quote IDs, addresses, tx contents,
  key material, or unbounded labels.
- [ ] Add operator commands for root DKG status, registered-application status,
  wallet status, transaction status, safe rebroadcast, and guarded root or
  application key rotation.
- [ ] Require explicit guarded procedures for abandoning/replacing transactions;
  do not provide a force-spend or private-key export command.
- [ ] Document mempool.space, self-hosted Esplora, and Bitcoin Core deployment
  tradeoffs.
- [ ] Add startup warnings for correlated chain sources and incomplete quorum.

**Success criteria:**

- [ ] Operators can identify whether a payment is waiting on consensus, chain
  observations, proposal, signatures, broadcast, or confirmation.
- [ ] Status and metrics reveal no secret shares, nonce material, credentials,
  addresses, or sensitive local paths.
- [ ] Invalid or mixed standalone/federated BDK configuration fails startup.

**Required tests:**

- [ ] Configuration parsing, defaults, environment overrides, and invalid
  combinations.
- [ ] Status for every wallet operation phase.
- [ ] Metric cardinality and secret-redaction tests.
- [ ] CLI smoke tests for status, rebroadcast, and rotation safety guards.
- [ ] Startup fail-closed tests for missing wallet config/share/chain source.
- [ ] Root/application registration, status, and retirement-guard tests.

### Milestone 24: end-to-end federation fault matrix

**Objective:** prove real Cashu minting and melting against a live FROST wallet
under realistic member and network failures.

**Work:**

- [ ] Add a deterministic regtest harness for 3-of-5 and configurable larger
  federations.
- [ ] Automate FROST DKG, descriptor activation, funding, deposit address
  allocation, mining, Cashu minting, on-chain melting, and balance checks.
- [ ] Capture journal, state digest, BDK balance, UTXO set, transaction state,
  and Bitcoin node state at each checkpoint.
- [ ] Add controlled member stop/start, REST partition, delayed messages,
  chain-source outage, database contention, and process-kill injection.
- [ ] Verify no duplicate Bitcoin payments, no unauthorized outputs, no nonce
  reuse, no stuck reservations after defined recovery, and no member divergence.
- [ ] Verify CLI and wallet-library behavior, not only internal APIs.
- [ ] Keep tests bounded and split slow fault matrices from routine unit tests.

**Success criteria:**

- [ ] A fresh 3-of-5 federation completes on-chain mint and melt with two
  members offline where thresholds permit.
- [ ] All recovered members converge on identical journal and materialized-state
  digests, address mappings, UTXOs, and transactions.
- [ ] Bitcoin Core reports exactly the expected deposits and outgoing payments.
- [ ] Standalone mint and wallet workflows remain unchanged.

**Required tests:**

- [ ] Happy-path fixed 3-of-5 flow with two deposits and two melts.
- [ ] 8-of-11 smoke flow and one bounded 14-of-20 signing/DKG flow.
- [ ] Offline proposer, coordinator, signer, broadcaster, and observer.
- [ ] Below-threshold partition followed by healing and catch-up.
- [ ] Restart at address, observation, proposal, commitment, share, signed,
  broadcast, and confirmation phases.
- [ ] Conflicting transaction proposals and invalid signature share.
- [ ] Esplora outage with RPC members still available and the reverse.
- [ ] Reorg before minting and controlled post-liability reorg response.
- [ ] Sender CLI on-chain mint and receiver CLI on-chain melt smoke tests.

### Milestone 25: security hardening and independent review

**Objective:** audit the cryptographic and state-machine boundaries before
mainnet use.

**Work:**

- [ ] Produce a protocol specification for DKG, derivation, address allocation,
  transaction consensus, FROST sessions, nonce lifecycle, and key rotation.
- [ ] Specify the canonical untweaked root package, root-to-application adapter
  boundary, compatibility requirements, authorization contract, shared
  compromise domain, and multi-application rotation rules.
- [ ] Map every security requirement in this plan to code and tests.
- [ ] Threat-model malicious proposer, coordinator, signer, observer,
  broadcaster, database, chain source, and wallet client.
- [ ] Threat-model a malicious or compromised registered application attempting
  cross-application signing, nonce reuse, derivation confusion, root extraction,
  or premature root retirement.
- [ ] Review all canonical encodings, domain separators, participant mappings,
  scalar tweaks, parity handling, and Taproot sighashes.
- [ ] Fuzz DKG, config, proposal, PSBT, signing-package, signature-share,
  transaction, and catch-up decoders.
- [ ] Add property tests for accounting conservation, UTXO exclusivity, intent
  uniqueness, and terminal-state monotonicity.
- [ ] Review database transaction boundaries and failed-write recovery.
- [ ] Review private REST authentication, replay windows, request limits, and
  denial-of-service exposure.
- [ ] Arrange independent cryptographic review of threshold derivation and
  nonce handling.
- [ ] Obtain interoperability review of the canonical root format and at least
  two independent application adapters before promising external compatibility.
- [ ] Resolve all high- and medium-severity findings before release.

**Success criteria:**

- [ ] Every signature share can be traced to one root epoch, application and key
  epoch, derivation context, authorization proof, signer set, attempt, and
  durably consumed nonce; CDK shares additionally trace to one accepted
  transaction and input.
- [ ] Every spendable UTXO and payment intent has one unambiguous state owner.
- [ ] No identified path lets fewer than threshold members spend or lets one
  member create Cashu liability from an unconfirmed deposit.
- [ ] Independent reviewers approve the derivation and signing design or all
  requested changes are completed and retested.

**Required tests:**

- [ ] Continuous fuzz runs with retained regression corpus.
- [ ] Model/property tests across randomized operation order and failure points.
- [ ] Static analysis, clippy, formatting, docs, and dependency audit.
- [ ] Secret-scanning of logs, snapshots, fixtures, and generated artifacts.
- [ ] Full standalone and federation regression suites.

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
