# Federated BDK-FROST architecture decision

Status: accepted for implementation milestone 0 on 2026-07-13.

This decision is subordinate to
`docs/federated-cashu-implementation-plan.md` and records the frozen boundaries
needed to execute
`docs/federated-cashu-bdk-frost-implementation-plan.md`. Changes to these
boundaries require an explicit update to both this decision and the tracker
before production behavior changes.

## Decision

The federation uses three separate authorities:

1. AlephBFT orders and authorizes wallet state transitions.
2. BDK is a watch-only descriptor, chain-index, transaction-builder, PSBT, and
   broadcast engine. It does not own federated signing keys and its local chain
   graph is not consensus state.
3. A reusable participant-local FROST key manager owns one canonical
   application-neutral secp256k1 root share and all nonce state. The CDK
   on-chain adapter may request shares only for a transaction already accepted
   by AlephBFT.

FROST signing is an asynchronous two-round workflow outside BDK's synchronous
signer traits. No network request, consensus wait, or nonce transition is hidden
inside `bdk_wallet::Wallet::sign`.

Existing mint quote, proof, saga, payment-observation, journal, replay, catch-up,
and error behavior remains authoritative. The federated BDK adapter enters
through existing payment and federation interfaces. A missing shared-domain
operation discovered during implementation is documented in the tracker and
fixed narrowly in its owning layer with standalone and federation regression
coverage; it is not reimplemented inside the wallet.

## Threshold and fault model

Let `n` be `ThresholdParams::member_count`, `t` be
`ThresholdParams::signature_threshold`, and `q` be
`ThresholdParams::consensus_threshold`.

- The Bitcoin FROST signing threshold is exactly `t`, the Cashu BLS signature
  threshold. There is no separately configurable Bitcoin custody threshold.
- Startup and activation fail closed if FROST public or private material uses a
  threshold other than `t` or a participant set other than the federation
  roster.
- The existing BFT default is `f = floor((n - 1) / 3)` and `q = n - f`.
  Existing validation requires `q >= t`.
- Threshold signing remains possible with at most `n - t` unavailable signers.
  End-to-end ordered spending remains possible only while both `q` consensus
  members and `t` valid FROST signers are available, so the tighter live quorum
  governs availability.
- Fewer than `t` FROST members cannot spend, and no set of signers may spend
  without an accepted transaction. Equivocation, invalid shares, conflicting
  observations, roster mismatch, or transcript mismatch fail closed.

Federation setup runs independent BLS and secp256k1 FROST DKGs through shared
ceremony infrastructure. The federation becomes wallet-ready only after both
results have independently completed, verified, persisted, and activated. The
BLS shares, FROST shares, and member identity keys are never converted into or
used as one another.

## Root and application boundary

The canonical DKG result is an untweaked secp256k1 FROST root epoch containing
one local share plus a public package bound to the federation, roster,
threshold, protocol/ciphersuite version, ceremony, epoch, participant mapping,
verification shares, group key, and transcript. It contains no BDK descriptor,
Bitcoin network, chain code, BIP32 child, BIP341 tweak, transaction policy, or
AlephBFT operation.

Every application defines a stable application identifier, derivation version,
and authorization adapter. Public additive derivation gives
namespace separation but not compromise isolation. The key manager exposes
derived public packages, durable nonce commitments, authorized signature
shares, and aggregate verification. It does not expose a plaintext share,
complete derived private key, root-secret reconstruction, or arbitrary-digest
signing operation.

Root reuse is permitted only when an adapter uses compatible secp256k1
Schnorr/FROST algebra and satisfies the shared authorization, domain separation,
nonce, persistence, recovery, and static-key rules. An application that needs another
curve or scheme, a complete private key, hardened private derivation, or an
incompatible lifecycle requires a separate DKG.

## State ownership

| State or transition | Authoritative owner | Supporting role |
|---|---|---|
| Deposit and change index allocation | Federation wallet state machine after AlephBFT acceptance | BDK reveals through the accepted target index |
| Local chain scan and transaction graph | Each member's watch-only BDK database | Produces bounded observation candidates only |
| Confirmed deposit and spendable UTXO | Federation wallet state machine | Member observations require configured quorum evidence |
| Payment intent and batch eligibility | Existing mint/payment saga plus federation wallet state | BDK estimates fees and proposes a candidate |
| Coin selection and candidate PSBT | Untrusted proposer using BDK | Every member independently validates the complete proposal |
| Transaction acceptance and UTXO reservation | AlephBFT journal and deterministic apply | BDK state alone cannot reserve or authorize |
| Root share and nonce lifecycle | Participant-local FROST key manager | Application adapters supply typed authorization proofs |
| CDK signing authorization | CDK on-chain FROST adapter | Reloads the accepted transaction and derives each sighash locally |
| Signature collection and aggregation | Replaceable FROST coordinator | Signers verify the complete session; coordinator has no authority |
| Signed transaction bytes and txid | Federation wallet state after deterministic acceptance | Every BDK graph applies the accepted transaction idempotently |
| Broadcast and rebroadcast | Replaceable ready member | Uses only persisted accepted signed bytes |
| Confirmation, replacement, or terminal outcome | Federation wallet state from accepted observations | Existing melt saga remains the public behavioral authority |

## Existing `CdkBdk` responsibility map

| Current responsibility | Standalone path | Federated path |
|---|---|---|
| Mnemonic and BIP84 private descriptors | Retained unchanged in a thin adapter | Prohibited |
| Persisted BDK wallet creation/loading | Reused | Reused with public external/internal Taproot descriptors in a separate namespace |
| Address derivation | Local `reveal_next_address` | Apply a consensus-allocated target index idempotently |
| Chain sync and transaction graph | Reused | Reused as an observation cache |
| Receive-address/quote tracking | Retained | Mapping is authoritative federation state; existing quote contracts are reused |
| Incoming confirmation scanning | Retained | Produces observations; quorum and consensus materialize UTXOs |
| Fee estimation | Retained | Candidate input to versioned bounded consensus policy |
| Send intents and batching | Retained | Existing melt saga creates canonical federation payment intents |
| Coin selection and PSBT construction | Reused after extraction | Performed by an untrusted proposer |
| `Wallet::sign` with local xprv | Retained only for standalone | Never called |
| Transaction finalization | Reused after external signatures are inserted | Reused only after threshold signatures verify |
| Broadcast classification and rebroadcast | Reused | Reused for the one accepted signed transaction |
| Receive/send recovery and events | Retained | Integrated with journal replay, catch-up, and existing saga outcomes |
| Backend lifecycle and settings | Retained | Thin federated adapter reports consensus policy and readiness |

## Network and confirmation policy

The first release supports the same configured rust-bitcoin networks as the
current backend: Bitcoin mainnet, testnet, signet, and regtest. Regtest is the
required development and end-to-end test network. Mainnet activation remains
blocked on the tracker security and release gates.

Network, minimum confirmation depth, observation quorum, reorg policy, and fee
policy version are consensus configuration. Chain-source type, URL, credentials,
timeouts, and request limits remain local. The default confirmation depth is
six and must be at least one; regtest tests may explicitly configure one. A
member-local mempool or block observation never creates Cashu liability by
itself.

## Feature and configuration boundary

The reserved Rust feature names are:

- `federation-frost` for the reusable FROST root DKG, key manager, persistence,
  and authenticated round transport; it depends on the existing `federation`
  feature.
- `federation-bdk` for the CDK on-chain application adapter and watch-only BDK
  runtime; it depends on `federation-frost` and the existing BDK features.

The reserved configuration sections are `federation_frost` for root ceremony,
epoch, and key-manager settings and `federation_bdk` for network, descriptors,
chain source, confirmation, fee, and wallet policy. Enabling a section without
its compiled feature, activated root/application config, local share, or chain
source is an error. Incomplete support is never selected automatically and
never falls back to a mnemonic or standalone mint.

## Fedimint lessons intentionally reused

Review of the local Fedimint wallet modules supports four small design lessons:

- persist unsigned and pending transactions as explicit states rather than
  treating a PSBT or node mempool as authority;
- validate withdrawal policy and fees before collecting signature material;
- make signature collection and broadcast resumable from durable transaction
  identity; and
- keep fee and chain observations explicit consensus inputs with bounded
  fallbacks.

CDK does not copy Fedimint's module architecture, descriptor scheme, database
layout, consensus items, or signature mechanism. CDK keeps AlephBFT as its
ordering layer, its existing mint/payment sagas as business authority, BDK as a
watch-only engine, and FROST behind a CDK-owned reusable key-manager boundary.

## Consequences

Standalone `CdkBdk` behavior and its mnemonic remain unchanged. Federated mode
adds a separate watch-only database and asynchronous signing workflow. This
costs additional persisted state and round trips, but it prevents a proposer,
coordinator, chain source, broadcaster, or single member from obtaining custody
authority. It also leaves one reviewed root-key boundary for future compatible
payment backends without coupling those backends to BDK or Cashu mint logic.
