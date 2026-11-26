# Federation custody: FROST, on-chain wallet and Bark

The federation holds Bitcoin jointly. No proposer, coordinator, signer,
observer, broadcaster, database, chain source or single member can move funds;
spending needs an operation accepted by consensus and `signature_threshold`
valid FROST signature shares. Payment evidence rules are in the
[payment trust model](payment-trust-model.md).

Two custody backends share one FROST root:

- **Joint on-chain wallet** (`federation-bdk`): a watch-only BDK wallet over
  Taproot descriptors derived from the FROST root, used for on-chain mint and
  melt quotes.
- **Bark** (`federation-bark`): Ark VTXOs held under keys derived from the same
  root, used for Lightning (BOLT11) mint and melt quotes.

Both are feature-gated and independent; a member enables either, both or
neither.

## Threshold model

Let `n` be the member count, `t` the `signature_threshold` and `q` the
consensus threshold. The FROST signing threshold is exactly `t`, the BLS
threshold; there is no separate custody threshold, and startup fails if FROST
material uses another threshold or participant set. Signing tolerates `n - t`
unavailable signers, but ordered spending needs both `q` consensus members and
`t` signers, so the tighter of the two governs availability. Fewer than `t`
members cannot spend, and no set of signers can spend without an accepted
operation.

## FROST root and applications

Setup runs a dealer-free FROST DKG (`cdk-frost`, `frost-secp256k1-tr`) beside
the BLS DKG, with the same roster, ceremony and threshold but independent
cryptographic state (see [setup ceremony](setup-ceremony.md)). Its result is a
canonical, untweaked, application-neutral secp256k1 root: a public package
(epoch, protocol, ciphersuite, threshold, roster-to-verification-share map,
group key, transcript binding) and each member's own signing share, sealed at
rest. It contains no descriptor, network, chain code or tweak.

Applications are registered under the root with an application ID, key epoch,
derivation version and authorization-policy commitment: the CDK on-chain
application and, optionally, the Bark application. The key manager exposes
derived public packages, nonce commitments, authorized signature shares and
aggregate verification. It never exposes a plaintext share, a derived private
key, root reconstruction or arbitrary-digest signing. Public additive
derivation separates namespaces but not compromise domains: a reconstructed
derived private key would expose the root, which is why none is ever
reconstructed. An application that needs isolation, another curve or hardened
derivation needs its own DKG.

Custody keys are static: in-place rotation of the root or an application key,
descriptor retirement and automatic sweeps are not supported. Replacing keys
means a new federation wallet and an ordinary threshold-authorized migration.

### On-chain derivation

`cdk-frost-bip32` is the only Bitcoin-specific adapter (application label
`org.cashubtc.cdk.federated-onchain`, derivation version 1). For root point
`P`, application tweak `a` and public BIP32 tweaks `b_0`, `b_j`, the child key
is `P + aG + b_0G + b_jG` and each signing share is `s_i + a + b_0 + b_j`; adding
a public scalar to every Shamir share shifts the secret by the same scalar, so
no secret is reconstructed. The order is fixed:

1. canonical untweaked root;
2. application-domain additive tweak;
3. domain-separated public chain code (SHA-256 over domain, root epoch,
   application and application group key);
4. non-hardened BIP32 branch (`0` external, `1` internal);
5. non-hardened BIP32 index;
6. BIP340 even-Y normalization inside FROST signing;
7. BIP341 key-path-only `TapTweak` (`tr(xpub/branch/*)`, no script tree).

Hardened derivation is rejected. If a child index is invalid, the next index is
used, which consensus must accept before use. Stable descriptor, address and
signature vectors are in the crate tests.

### Nonces and signing sessions

For every accepted transaction input, the authorization reference commits to
the accepted operation, proposal, transaction, input index, sighash type and
exact Taproot sighash. A signing slot also binds the federation, root epoch,
application, derivation, signer set and attempt. The coordinator has no
authority: it collects commitments, publishes the exact commitment set and
package, verifies shares and aggregates a BIP340 signature. Every signer
reconstructs the scope, re-derives the message from accepted state and rejects
any substitution. `crates/cdk-federation/src/signer_fan_out.rs` sends one
request per selected signer (local signer in process, others over authenticated
REST), bounded by the outbound concurrency limit, and waits for every request
before returning, so an abort never races an in-flight request.

Nonce state is durable and atomic:
`vacant -> reserved -> commitment published -> consumed`. Uncertain, timed
out, superseded, aborted or restart-nonterminal records become burned
tombstones and never return to `vacant`. Exact replay of a consumed share is
allowed for recovery; a different replay is rejected. Nonces live in redb files
apart from the SQL database and must never be restored from a backup (see
[operations](operations.md#frost-nonce-files-never-restore)).

### Encodings

Binary FROST decoders are bounded, versioned, consume their whole input,
reject duplicate or noncanonical participants and require byte-identical
re-encoding. Durable JSON uses explicit versions and `deny_unknown_fields`.

| Boundary | Marker |
|---|---|
| FROST root package | `CDKFRT01` |
| DKG rounds 1 and 2 | `CDKFD101`, `CDKFD201` |
| Application derivation | `cdk/frost/application-derivation/v1` |
| BIP32 chain code, signing target | `cdk/frost/bip32-chain-code/v1`, `cdk/frost/bip32-signing-target/v1` |
| Nonce commitment, signature share | `CDKFNC03`, `CDKFSH03` |
| Signing context, package | `CDKFSC01`, `cdk/frost/signing-package/v1` |
| Transaction proposal, PSBT projection | `cdk-federation-onchain-transaction-proposal-v2`, `cdk-federation-onchain-psbt-consensus-v1` |

## Joint on-chain wallet

Three authorities are kept apart:

1. AlephBFT orders and authorizes wallet state transitions.
2. BDK is a watch-only descriptor, chain-index, transaction-builder, PSBT and
   broadcast engine. It holds no keys, and its local chain graph is not
   consensus state. `BdkWalletEngine` (`cdk-bitcoin-wallet`) serves standalone
   and federated wallets; federated mode never calls `Wallet::sign` and never
   has a mnemonic.
3. The member's FROST key manager holds the root share and nonce state and
   signs only for an accepted transaction. Signing is an asynchronous two-round
   workflow outside BDK's signer traits.

| State | Authority | Supporting role |
|---|---|---|
| Deposit and change index allocation | Accepted `AddressAllocation` | BDK reveals up to the accepted index |
| Chain scan and transaction graph | Each member's watch-only BDK database | Produces bounded observation candidates |
| Confirmed deposit, spendable UTXO | Accepted `ChainObservation` quorum | |
| Payment intent and batch eligibility | Melt saga plus federation wallet state | BDK estimates fees |
| Coin selection and candidate PSBT | Untrusted proposer | Every member validates the whole proposal |
| Transaction acceptance and UTXO reservation | Accepted `TransactionProposal` | |
| Signing authorization | CDK on-chain FROST adapter | Reloads the accepted transaction and derives each sighash |
| Signed bytes and txid | Accepted `TransactionSigned` | |
| Broadcast | Any ready member, using only the persisted signed bytes | Accepted `TransactionBroadcastIntent` |
| Confirmation, replacement, terminal outcome | Accepted `TransactionBroadcastObservation` quorum | Melt saga remains the public outcome |

The network, confirmation depth and fee-policy version are consensus
configuration, fixed in the setup proposal and taken from the active CDK
on-chain application; the chain source, credentials, timeouts and storage are
member-local and never restated in the member config.

### Transaction authorization

A proposal carries a bounded canonical projection, not arbitrary PSBT maps. It
binds the root/application/network/policy scope and payment batch, every
payment intent and recipient output, every input outpoint, previous output and
allocation, optional consensus-allocated change, the unsigned transaction, fee,
fee rate, weight estimate, sighash mode and PSBT commitment, and the
predecessor and lineage when fee bumping. Every member recomputes intent
eligibility, destinations and amounts, input ownership and spendability, value
conservation, fee caps, weight, Taproot sighashes and the PSBT commitment
(`crates/cdk/src/mint/federation/transaction_policy.rs`). Conflict keys give
each batch, intent, UTXO and lineage one owner. `SIGHASH_DEFAULT` is fixed and
all previous outputs enter the sighash.

The signed transaction is persisted before any broadcast; a separate accepted
broadcast intent authorizes only those bytes. Reservations stay sticky after
signing, replacement, abandonment, ambiguous broadcast and restart, because the
original bytes may still reach Bitcoin. Automatic RBF compares the accepted
fee with a fresh bounded estimate; an unavailable estimate never triggers a
fallback bump. A replacement keeps the original eligible for rebroadcast until
a descendant has accepted broadcast evidence, and confirmation polling covers
every signed transaction in the lineage, so either settles the payment exactly
once. There is no force-spend, force-release, share export, nonce export or
private-key export.

### Chain observation and reorgs

Each member queries only scripts already revealed by accepted allocations
(zero unallocated lookahead). A timeout, malformed reply, stale tip or omitted
transaction degrades only that member's wallet, which fails closed for
consensus-dependent actions until a sync succeeds.

One accepted reorg observation moves the UTXO to `reorged`. Before Cashu
liability is issued for it, a false or transient reorg recovers only by the
observation quorum reconfirming the exact original block in a new observation
epoch. After liability is issued, a reorg is sticky and raises a persistent
liability alarm; re-inclusion in another block is not recovered automatically.
A spent output is frozen: later deposit or reorg evidence for it is not
recorded.

## Bark

Bark's wallet and action state machines run on each member as a local
execution engine for plans the federation has accepted. The federation's Bark
projection is the authority; each member's Bark SQLite database is a
rebuildable cache.

| Concern | Authority | Not authoritative |
|---|---|---|
| Bark application identity, Ark server, network, policy | Accepted Bark application in the federation config | Member config |
| VTXO key indexes | Accepted Bark records | Bark's local key index |
| Spendable, reserved and spent VTXOs | Bark projection | Bark SQLite state |
| Send, claim, revocation, refresh and exit plans | Accepted `ActionPlan` | A member's candidate plan |
| Payment result | Certified evidence and accepted transitions | One Ark response or mailbox event |
| Threshold signature authorization | Accepted plan and exact signing transcript | Coordinator request |
| Nonces | Member-local durable `cdk-frost` state | In-memory maps |
| Worker actor | Deterministic journal-derived selection | Process locks |

Persistence has four layers: federation SQL (accepted Bark operations and the
projection, included in the state digest and in state transfer); the
member-local Bark SQLite cache; member-local FROST protocol state; and the
member-local protected receive-secret store (encrypted, replicated among
members, backed up with the member's private material, never in the public
snapshot).

`cdk-bark` implements Bark's `VtxoCustody` over the CDK root share
(`crates/cdk-bark/src/custody.rs`). It uses three namespaced protocols under
the Bark application: BIP340 FROST for direct Schnorr signatures, virtual
MuSig2 for the federation's contribution to Bark's outer MuSig2 with the Ark
server, and threshold Diffie-Hellman with DLEQ proofs for blinded mailbox
identifiers. Each protocol context binds the federation and application, epochs,
accepted operation and journal index, action and phase, object ordinal,
locator, message commitment, signer set, attempt, Ark server and network. There
is no local keypair.

Bark action kinds are `LightningSend`, `IncomingClaim`, `FailedSendRevocation`,
`Refresh` and `EmergencyExit`. The Bark drivers (`crates/cdk-bark/src/node/`)
reach the node only through the `FederatedBarkNode` port. Refresh rounds run
automatically at the accepted margin; an overdue refresh can escalate into an
emergency exit to the joint on-chain wallet when
`federation.bark.automatic_emergency_exit` is enabled. Recovery inputs are
reserved in the `bark_recovery` projection and spent through a member-local
watch-only recovery wallet.

The Bark dependency is pinned to an exact reviewed Git revision in
`crates/cdk-bark/Cargo.toml`.

## Release status

The custody design has an internal specification and review, not an
independent cryptographic audit. Before mainnet value, it still needs
independent review of the untweaked-root conversion, additive derivation,
BIP340 parity handling, BIP341 tweak order, Taproot sighashes and the nonce
lifecycle, interoperability review of the root package and both application
adapters, and resolution of any high or medium finding.
