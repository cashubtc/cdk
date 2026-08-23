# Signatory signing with the NUT-06 mint identity key

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-08-22
* Targeted modules: cdk-signatory, cdk-mintd, cdk
* Associated tickets/PRs: https://github.com/cashubtc/nuts/pull/416

## Context and Problem Statement

The `Signatory` trait is the only seam through which the mint reaches
private-key material (ADR-0001), and every method on it is keyset-shaped:
`blind_sign`, `verify_proofs`, `keysets`, `subscribe_keysets`, `rotate_keyset`.
A mint that needs to attest to something that is not ecash, starting with a
statement about itself, has no way to ask for it. Producing such a signature
outside the signatory would put a private key back inside the mint process,
which is exactly what ADR-0001 removed.

NUT-06 (cashubtc/nuts#416) settles what that signature has to be. It specifies
both halves: the mint has an identity key derived from its seed, and it signs
with BIP-340 over the SHA-256 of the payload.

```
identity_key = HMAC_SHA256(key = seed, msg = b"Cashu_Mint_Identity_v1" || ctr)
sig          = schnorr_sign(identity_key, SHA256(payload))
```

`ctr` starts at zero and increments while the candidate falls outside
`1..n-1`. The mint publishes the public half as `GetInfoResponse.pubkey`.

CDK already published a pubkey there, but a different one: the BIP-32 master
key from `Xpriv::new_master(seed)`, whose private half signed nothing. So the
question is not only how the signatory signs, but which key it signs with and
what happens to the key deployments already advertise.

## Decision Drivers

* The signing key must stay inside the signatory, like every other key.
* A wallet holding the mint's published pubkey must verify with nothing but a
  NUT-06 implementation, and with no signatory round trip.
* Whatever is built here should carry the mint info signature later without
  changing again.

## Considered Options

#### Sign in the mint with a key the mint holds

**Pros:**

* Good, because it needs no change to the signatory boundary at all.

**Cons:**

* Bad, because it puts private-key material back in the mint process. ADR-0001
  exists to prevent exactly this.

#### Sign with the published BIP-32 master key over a domain-separated digest

Keep advertising the master pubkey and sign
`HMAC_SHA256(key = b"Cashu_Signatory_Sign_v1", msg = payload)`, following the
HMAC-SHA256 construction NUT-12 and NUT-13 already use in this codebase.

**Pros:**

* Good, because no deployment sees its advertised pubkey change.
* Good, because the digest differs from `SHA256(payload)`, so a signature here
  is not also valid as a NUT-11 or NUT-20 signature over the same bytes.

**Cons:**

* Bad, because no NUT-06 wallet could verify it. Neither the key nor the digest
  is the one the spec names, and the reason to sign at all is that someone else
  checks the signature.
* Bad, because the master key is the ancestor of every keyset key, so a `sign`
  oracle over it is strictly worse than one over a sibling key.

#### Derive the identity key per NUT-06 and sign the payload

**Pros:**

* Good, because the key is derived outside the BIP-32 tree, so no keyset
  descends from it.
* Good, because verification is `PublicKey::verify(payload, sig)`, which
  already existed and already does SHA-256 then `verify_schnorr` against the
  x-only key. A verifier needs nothing from `cdk-signatory`.
* Good, because signing the canonical `GetInfoResponse` bytes becomes the only
  remaining work for NUT-06 mint info signatures.

**Cons:**

* Bad, because the mint's advertised `pubkey` changes on upgrade.
* Bad, because the digest carries no domain tag, so a `sign` caller who
  controls the payload gets a signature over bytes of their choosing under the
  mint's identity.

## Decision Outcome

Chosen option: "Derive the identity key per NUT-06 and sign the payload".
Compliance is the whole point of the feature, and the separate derived key is
also the better key to be signing with.

The construction lives in `crates/cdk-signatory/src/identity.rs`. `DbSignatory`
derives the key from the same seed it hands to `Xpriv::new_master`, but outside
that tree, and publishes its public half as `SignatoryKeysets::pubkey`. Keyset
derivation is untouched.

The trait gains `sign(payload) -> Signature`. On the wire this is a new `Sign`
RPC; `CONSTANTS_SCHEMA_VERSION` moves from 2 to 3 and the version interceptor
rejects a mismatch, so a mint and a signatory across the bump refuse to
connect.

Signing zeroes the BIP-340 auxiliary randomness. That is not required by the
spec, but it makes a re-served `/v1/info` byte-identical and lets the spec's
example vector be a fixture.

On the domain separation that a tagged digest would have bought: the mint
identity key is not a keyset key and not a wallet key, so it signs in no other
Cashu context, and the only payload NUT-06 defines is a self-describing JSON
object. `sign` is a privileged operation and must not be reachable by untrusted
callers.

### Migration

Every existing mint's advertised pubkey changes, because CDK previously
published the BIP-32 master key. Three places had to learn about it:

* `cdk-mintd` derives its expected identity the same way, so its startup guard
  compares like with like.
* A configuration that pins `mint_info.pubkey` to the old master key starts
  with a warning instead of `SigningIdentityChange`, on the local-seed path
  where the old value can be recomputed. With a remote signatory there is no
  seed to check against and the operator must update the configuration.
* `Mint` rewrites a persisted `MintInfo.pubkey` that disagrees with the
  signatory. Previously it only filled the field when absent, which would have
  left an upgraded mint advertising one key while signing with another.

### Positive Consequences

* The mint can obtain a signature over arbitrary bytes without ever holding a
  private key.
* A NUT-06 wallet can verify the mint with no CDK-specific code.
* The identity key is no longer the root of the keyset derivation tree.

### Negative Consequences

* The advertised pubkey changes on upgrade, and operators who pinned it with a
  remote signatory must edit their configuration.
* A mint and a signatory must be upgraded together: the schema version bump
  makes a mixed pair refuse to connect.
* A signature is a plain BIP-340 signature over `SHA256(payload)`, with no tag
  in the digest saying what it is for.
* Nothing consumes `sign` yet. There is no `Mint::sign` pass-through; the mint
  info signature is the first caller.

## Links

* Refines [ADR-0001](0001-signatory-mint-key-segregation.md)
