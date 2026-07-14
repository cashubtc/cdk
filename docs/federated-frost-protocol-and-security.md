# Federated FROST protocol and security review packet

Status: Milestone 25 internal specification and review packet. This document is
not an independent cryptographic review and does not authorize mainnet funds.
The external review gates listed below remain open.

This specification covers the application-neutral secp256k1 FROST root, the
CDK Taproot BDK application, and the federation state that authorizes it. The
general Cashu federation architecture and safety requirements remain binding in
[`federated-cashu-implementation-plan.md`](federated-cashu-implementation-plan.md).
The ordered implementation and release gates remain in
[`federated-cashu-bdk-frost-implementation-plan.md`](federated-cashu-bdk-frost-implementation-plan.md).

## 1. Trust and release statement

Custody requires the configured `signature_threshold` of the fixed federation
roster. No proposer, FROST coordinator, signer, observer, broadcaster, database,
chain source, application adapter, wallet client, or public API process is
individually trusted. AlephBFT orders authorization and state transitions; each
member independently validates the exact operation before applying it or
releasing a signature share.

Federation setup is one fail-closed dual-result ceremony. The existing BLS DKG
and isolated FROST DKG use the same roster, ceremony identifier, and
`signature_threshold`, but independent cryptographic state. Both results must
verify, persist, and activate. A partial result never makes the mint or wallet
ready.

The following remain explicit release blockers:

- independent review of the untweaked-root conversion, additive derivation,
  BIP340 parity processing, BIP341 tweak order, Taproot sighashes, and nonce
  lifecycle;
- interoperability review of the canonical root package and two application
  adapters; and
- resolution and retesting of every high- or medium-severity external finding.

## 2. Protocol state machine

### 2.1 Dual BLS and FROST DKG

1. Every member validates one public federation config, ordered roster,
   threshold tuple, keyset specification, ceremony ID, and monotonically
   increasing FROST root epoch.
2. The existing BLS ceremony independently produces Cashu signing shares.
3. `cdk-frost` runs dealer-free FROST round one, confidential per-recipient
   round two, and public root confirmation. Each package binds the federation,
   ceremony, epoch, threshold, roster, sender, and receiver where applicable.
4. A member persists sealed local phase state before exposing a dependent
   outbound package. Exact replay is idempotent; conflicting replay is
   equivocation. A phase advances only after its complete outbound batch is
   acknowledged.
5. Each member converts the result to the canonical untweaked root, verifies
   its local share against its public verification share, and confirms the exact
   public bytes.
6. Activation requires every configured member's matching public FROST result
   and every required BLS result. Threshold, roster, ceremony, transcript, or
   root mismatch aborts setup without changing the active configuration.

The detailed ceremony and persistence contract is in
[`federated-cashu-frost-dkg.md`](federated-cashu-frost-dkg.md). The isolated
cryptographic implementation is `crates/cdk-frost`; federation transport and
activation records are in `crates/cdk-common/src/federation/frost_dkg.rs`; the
thin REST driver is in `crates/cdk-axum/src/federation/dkg_driver.rs`.

### 2.2 Canonical root and application derivation

The durable root is application-neutral. Its public package contains the root
epoch, DKG protocol and ciphersuite, threshold, ordered member-to-verification-
share map, full compressed group key, and transcript binding. Each member's
local package additionally contains only its own signing share and is encrypted
at rest. It contains no BDK descriptor, Bitcoin network, child index, Taproot
mode, consensus state, or complete secret key.

An application registration binds a globally derived application ID,
application key epoch, derivation version, and authorization-policy commitment.
Public additive application derivation and public non-hardened BIP32 derivation
produce the same target point and verification shares on every member. The CDK
on-chain adapter applies operations in this exact order:

1. canonical untweaked root;
2. application-domain additive tweak;
3. domain-separated public chain code;
4. non-hardened BIP32 branch and child derivation;
5. BIP340 even-Y normalization during FROST signing; and
6. BIP341 key-path-only Taproot tweak.

The derivation contract and stable vectors are in
[`federated-cashu-frost-bip32-derivation.md`](federated-cashu-frost-bip32-derivation.md).
Public additive derivation separates namespaces but not compromise domains: a
reconstructed derived private key can expose the additive relationship to the
root. CDK never reconstructs or exports one. Applications requiring stronger
isolation need a separate DKG.

### 2.3 Address allocation and deposits

Only an accepted `AllocateAddress` operation advances an external or internal
descriptor counter. The operation binds the application epoch, keychain,
derivation index, and quote or change owner. Replay reveals through the accepted
target index and cannot allocate a second address locally.

Members observe deposits using configured chain sources and submit bounded,
authenticated evidence. The materialized UTXO state requires the configured
observation policy, confirmation depth, exact outpoint, amount, script, block,
and allocation. A reorg before liability removes spendability. A reorg after
liability is issued raises a persistent alarm and does not silently reverse
Cashu state. Normal accepted payment observations remain the only path that
marks an existing mint quote paid.

### 2.4 Transaction authorization and accounting

An untrusted proposer may use its watch-only BDK wallet to construct a candidate,
but consensus carries a bounded canonical projection rather than arbitrary PSBT
maps. The proposal binds:

- one root/application/network/policy scope and one payment batch or epoch
  sweep;
- every accepted payment intent and recipient output;
- every exact input outpoint, previous output, and address allocation;
- optional consensus-allocated change;
- the exact unsigned Bitcoin transaction, fee, fee rate, weight estimate,
  sighash mode, and PSBT-consensus commitment; and
- replacement predecessor and lineage when fee bumping.

Every member recomputes intent eligibility, destination and amount equality,
input ownership and spendability, value conservation, fee allocation and caps,
weight, Taproot sighashes, and PSBT commitment. Consensus conflict keys give one
owner to each batch, intent, UTXO, and replacement lineage. Reservations remain
sticky after signing, replacement, abandonment, ambiguous broadcast, and
restart because the original bytes may still reach Bitcoin.

### 2.5 FROST session and nonce lifecycle

For each accepted transaction input, the authorization reference commits to the
accepted operation, proposal, transaction, input index, sighash type, and exact
32-byte Taproot sighash. A signing slot additionally binds federation, root
epoch, application registration, derivation binding, authorization reference,
message commitment, deterministic signer set, and attempt.

The coordinator has no custody authority. It can only collect commitments for
the deterministic current attempt, publish the exact commitment set and
authorized package, validate shares, and aggregate a BIP340 signature. Every
signer independently reconstructs the scope, verifies accepted authorization,
and rejects a wrong root/application epoch, derivation, message, signer set,
attempt, or package hash.

Nonce state is durable and atomic:

`vacant -> reserved -> commitment published -> consumed`

Uncertain, timed-out, superseded, aborted, or restart-nonterminal records become
burned tombstones and never return to `vacant`. Exact consumed commitment/share
replay is allowed for recovery; a different replay is rejected. No API accepts
an arbitrary digest for signing, exports nonce secrets, or exports a plaintext
root share.

### 2.6 Signed record, broadcast, confirmation, and catch-up

The fully signed transaction and per-input audit are persisted before any
broadcast. A separate accepted broadcast intent authorizes only those exact
bytes. Any ready member may broadcast or rebroadcast them; observations cannot
substitute another transaction ID. Confirmation requires the configured number
of distinct authenticated observers and depth. The first valid terminal lineage
winner is sticky, while all competing transaction inputs remain reserved.

Catch-up replays finalized AlephBFT items in order and verifies a quorum of
member-signed range certificates over operation IDs, order digest, and
materialized-state digest. Catch-up application and dependent live finality are
serialized. A recovered member must converge before commitments, shares,
observations, or broadcast side effects are enabled.

### 2.7 Rotation

Root and application epochs rotate through consensus states `proposed`,
`active`, `retiring`, and `retired`. New allocations use only the active
application. Retiring descriptors remain watch-only and retain signing ability
for known UTXOs. Old-to-new movement uses the ordinary proposal, reservation,
FROST, broadcast, and confirmation lifecycle. Root retirement requires every
registered consumer to retire and every old UTXO and operation to resolve. See
[`federated-frost-key-rotation.md`](federated-frost-key-rotation.md).

## 3. Canonical encoding and domain review

All public FROST binary decoders are bounded, versioned, consume the entire
input, validate their cryptographic payload, reject duplicate or noncanonical
participants, and require byte-identical re-encoding. Durable JSON structures
use explicit versions and `deny_unknown_fields`; transaction proposals enforce
an outer byte limit before decoding.

| Boundary | Version/domain marker | Security binding |
|---|---|---|
| FROST root | `CDKFRT01` | epoch, ciphersuite, threshold, roster, group key, transcript |
| DKG round 1/2 | `CDKFD101`, `CDKFD201` | ceremony context, sender, receiver, upstream package |
| Application derivation | `cdk/frost/application-derivation/v1` | root, application, policy, epoch |
| BIP32 chain code | `cdk/frost/bip32-chain-code/v1` | application package and derivation version |
| Signing target | `cdk/frost/bip32-signing-target/v1` | network, branch, index, tweak order, Taproot mode |
| Nonce commitment/share | `CDKFNC03`, `CDKFSH03` | root, application, session, derivation, participant |
| Signing context/package | `CDKFSC01`, `cdk/frost/signing-package/v1` | authorization, message, attempt, signer set, commitments |
| Transaction proposal | `cdk-federation-onchain-transaction-proposal-v3` | exact policy and unsigned transaction projection |
| PSBT projection | `cdk-federation-onchain-psbt-consensus-v1` | transaction plus ordered witness UTXOs only |
| Private REST | `cdk-federation-rest-signature-v1` | federation, sender, receiver, sequence, kind, body hash |
| Catch-up certificate | `cdk-federation-catch-up-certificate-signature-v1` | range, order digest, state digest, signer |

Scalar additions reject zero/invalid scalars and point-at-infinity results.
Hardened BIP32 children are prohibited. Full compressed points retain parity
until the ciphersuite's BIP340 normalization step; Taproot output-key tweaking
occurs only after application and child derivation. `SIGHASH_DEFAULT` is fixed
for ordinary proposals and all previous outputs are included in the Taproot
sighash computation.

## 4. Threat model

| Actor or fault | Attempt | Enforced rejection or containment |
|---|---|---|
| Malicious proposer | redirect value, hide an input, overpay fees, reuse UTXO/intent | every member recomputes transaction policy; conflict ownership is consensus material |
| Malicious coordinator | change message, signer set, attempt, commitments, or withhold output | session/package bindings reject substitution; deterministic failover burns superseded nonces |
| Malicious signer | send invalid share or equivocate | share verification and exact participant/session binding; threshold proceeds without it when possible |
| Fewer than threshold members | spend or reconstruct a key | no aggregate secret exists; FROST requires threshold valid shares |
| Malicious observer/chain source | invent deposit or confirmation, censor a reorg | quorum/source policy, exact allocation and chain binding, persistent post-liability alarm |
| Malicious broadcaster | replace bytes, duplicate payment, claim false success | exact signed bytes/txid are durable; idempotent Bitcoin broadcast and threshold confirmation |
| Malicious wallet client | race quotes, allocations, intents, or proposals | public input is only a proposal; accepted operation/conflict keys own every mutable scope |
| Compromised registered application | cross-application signing or root extraction | application registration and derivation binding reject cross-use; additive compromise remains documented shared risk |
| Database failure or rollback | lose nonce consumption, partial state, replay stale phase | nonce transition is atomic; mint apply uses database transactions; restart burns ambiguous nonce state and validates projections |
| REST replay or flood | repeat signed requests, exhaust handlers or fanout | sender/receiver signatures, message-class replay state, durable idempotency, body/concurrency/fanout limits, bounded timeouts |
| Member restart/partition | diverge state or repeat side effects | sealed DKG phases, durable nonce/transaction bytes, finalized-item replay, quorum certificates, readiness gates |

Availability remains threshold-dependent. Correlated chain sources, common
hosting, shared operator credentials, or a compromised threshold are outside
the protocol's fault assumption and must be treated as common-mode failures.

## 5. Persistence and REST review

The security-critical transaction boundaries are:

- sealed DKG phase records precede dependent outbound messages;
- the canonical root share is encrypted and verified before activation;
- nonce reservation, publication, consumption/burn, cached exact replay, and
  revision update occur in one nonce-store transaction;
- accepted mint operations and BLS share provenance are applied in the existing
  mint database transaction;
- transaction state validates all secondary ownership indexes after each
  transition and on decode; and
- signed bytes precede broadcast intent, which precedes external broadcast and
  observations.

Failed writes return errors before the next side effect. Restart code treats
nonterminal nonce uncertainty as burned, reconstructs state from durable bytes,
and never creates a competing transaction or nonce for an existing attempt.

Private REST requests authenticate the exact body hash, message kind, sender,
receiver, federation, and sequence. DKG effectful replays additionally use
durable ceremony identity/equivocation checks; status, catch-up, and liveness
traffic use bounded volatile replay windows. DKG and FROST routes have dedicated
body limits, all routes inherit the daemon body limit, private handlers use an
inbound semaphore, peer fanout uses an outbound semaphore and per-request
timeouts, catch-up and consensus payload counts are bounded, and operator
details are redacted before persistence or response.

## 6. Requirement-to-code-and-test trace

| Requirement | Primary code boundary | Preserving evidence |
|---|---|---|
| Dual BLS/FROST same-threshold activation | `frost_dkg.rs`, Axum DKG driver, mintd setup | dual convergence, mismatch, restart, partition, exact activation replay tests |
| Isolated untweaked root and no reconstruction/export | `cdk-frost` canonical/key manager | root immutability, redacted debug, cross-application and threshold tests |
| BIP32/BIP340/BIP341 correctness | `cdk-frost-bip32` | stable descriptors/addresses/signatures and 256-index derivation property |
| Consensus-only address allocation | `address_allocation.rs`, BDK wallet engine | uniqueness, contiguous counters, replay/restart/catch-up tests |
| Confirmed deposits only create liability | `chain_observation.rs`, mint federation adapter | source quorum, depth, reorg, post-liability alarm, live mint smoke |
| Exact transaction and accounting authorization | `transaction_proposal.rs`, mint transaction policy | structural mutation, sighash, fee, accounting, PSBT, conflict and randomized property tests |
| One owner per intent, UTXO, and lineage | `FederationTransactionState` | competing proposal, sticky reservation, replacement, randomized ownership tests |
| Durable one-time nonces and traceable shares | `nonce.rs`, `nonce_redb.rs`, key manager, Axum FROST signing | restart at every phase, failover, stale/future attempt, exact replay and audit tests |
| Exact signed bytes before broadcast | transaction state, broadcaster | ambiguous result, rebroadcast, duplicate-payment and convergence tests |
| Authenticated bounded private REST | `transport.rs`, Axum federation router/peer client | replay, sender/receiver, body, concurrency, timeout and redaction tests |
| Ordered certified recovery | journal, consensus catch-up, sync loop | digest/certificate tamper, chunk, partition and catch-up/live serialization tests |
| Safe root/application rotation | `frost_rotation.rs`, wallet liveness | epoch separation, sweep, retirement guards and restart tests |

Every FROST input audit traces root epoch and application/key epoch through the
accepted transaction scope; derivation and authorization through the signing
reference; signer set, attempt, session and package through the input audit; and
nonce consumption through the participant nonce record. BLS share provenance is
separately specified in
[`federated-cashu-signature-share-provenance.md`](federated-cashu-signature-share-provenance.md).

## 7. Fuzzing, properties, and audit commands

The retained security corpus covers FROST DKG packages, root packages, signing
contexts, derivations, authorization requests, nonce commitments/sets/records,
signature shares, federation config, DKG REST payloads, signing packages,
catch-up payloads, Bitcoin transactions, PSBTs, federation operations, and
transaction proposals.

Run a bounded campaign with:

```bash
just fuzz-federation-security 60 1
```

Use duration `0` for a continuous local campaign. New crashes must be minimized
and committed as regression corpus entries before fixing. Deterministic property
tests cover canonical derivation, decoder truncation/trailing bytes, accounting
conservation, UTXO and intent uniqueness, randomized operation order/failure,
and terminal monotonicity.

The local release review must also run formatting, workspace checks and Clippy,
unit and integration tests, documentation tests, dependency audit, and a scan of
logs, fixtures, snapshots, generated configs, and staged diffs for secrets.
Tool output and generated runtime directories are not release artifacts.

## 8. Dependency and secret audit status

The 2026-07-14 lockfile audit removed vulnerable `crossbeam-epoch` and
`protobuf` versions by updating `crossbeam-epoch` and moving Prometheus to its
non-Protobuf 0.14 implementation. The fuzz-workspace lockfile has no reported
vulnerabilities. The root lockfile still reports four unresolved advisories, so
the dependency release gate remains failed rather than ignored:

| Advisory | Dependency path | Disposition |
|---|---|---|
| `RUSTSEC-2026-0119` | optional BIP353 resolver -> `hickory-proto` 0.25.2 | fixed upstream in 0.26.1, but no update compatible with the workspace MSRV was identified |
| `RUSTSEC-2026-0118` | optional BIP353 resolver -> `hickory-proto` 0.25.2 | no fixed upstream release is available |
| `RUSTSEC-2025-0009` | optional Tor transport -> Arti -> `ring` 0.16.20 | the compatible Arti upgrade range retains the dependency; do not suppress |
| `RUSTSEC-2023-0071` | optional Tor transport -> Arti -> `rsa` 0.9.10 | medium severity and no fixed upstream release is available; do not suppress |

The Tor dependencies are not part of the default workspace feature graph, but
they remain supported code and therefore remain in the release audit. A trial
upgrade was rejected because it did not remove these advisories and replacing
the upstream Hyper connector locally would add unreviewed network security
surface.

Repository and goal-commit secret scans pass with exact fingerprints for known
deterministic Cashu vectors, fuzz proofs, local demo credentials, and CI
fixtures. No path-wide or rule-wide suppression is allowed; new or moved
findings must be reviewed explicitly.

## 9. Independent reviewer handoff

Reviewers should start with this packet, then inspect:

1. [`federated-cashu-frost-crypto-review.md`](federated-cashu-frost-crypto-review.md)
   and the exact `cdk-frost` conversion from upstream DKG output;
2. [`federated-cashu-frost-bip32-derivation.md`](federated-cashu-frost-bip32-derivation.md)
   and stable descriptor/signature vectors;
3. [`federated-cashu-frost-dkg.md`](federated-cashu-frost-dkg.md), sealed
   persistence, and dual activation;
4. nonce storage, attempt failover, session/package hashes, and every path that
   calls the participant key manager;
5. the CDK on-chain authorization adapter and the independent test application
   adapter proving root reuse and domain separation; and
6. rotation retirement guards and shared-compromise documentation.

Requested reviewer deliverables are a written severity-ranked finding list,
explicit approval or rejection of the derivation/tweak order and nonce model,
canonical-root interoperability feedback for both adapters, and confirmation
that all requested changes were retested. Until those deliverables exist, the
Milestone 25 independent-review checkboxes and mainnet release gate remain open.
