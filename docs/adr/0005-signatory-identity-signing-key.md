# Signatory signing of arbitrary payloads

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-08-17
* Targeted modules: cdk-signatory
* Associated tickets/PRs: n/a

## Context and Problem Statement

The `Signatory` trait is the only seam through which the mint reaches
private-key material (ADR-0001), and every method on it is keyset-shaped:
`blind_sign`, `verify_proofs`, `keysets`, `subscribe_keysets`,
`rotate_keyset`. A mint that needs to attest to something that is not ecash,
for example a statement about itself or a payload another party must attribute
to this mint, has no way to ask for it. Producing such a signature outside the
signatory would put a private key back inside the mint process, which is
exactly what ADR-0001 removed.

The signatory already holds a non-keyset key: the master key from
`Xpriv::new_master(seed)`, whose public half it publishes as
`SignatoryKeysets::pubkey` and the mint persists into `MintInfo.pubkey`. Its
private half signs nothing today. How does the signatory sign arbitrary bytes
with it without that signature colliding with the Cashu protocol signatures the
same curve and hash already carry?

## Decision Drivers

* The signing key must stay inside the signatory, like every other key.
* The signature must be verifiable by anyone holding the mint's published
  pubkey, with no signatory round trip.
* A signature over arbitrary bytes must not be reusable as a Cashu protocol
  signature, and vice versa.
* Existing deployments advertise a `MintInfo.pubkey`; whatever signs should be
  the key they already advertise, not a second one.

## Considered Options

#### Sign the payload directly

Call `SecretKey::sign(payload)`, which SHA-256s the input and Schnorr-signs the
digest, the same path NUT-11 and NUT-20 use.

**Pros:**

* Good, because it is one line and reuses an existing helper.

**Cons:**

* Bad, because it is the identical construction NUT-11 and NUT-20 use. Anything
  that can steer the payload can obtain a signature that is also valid as one
  of those, and the mint is a signing oracle for arbitrary bytes. There is
  nothing in the signed material naming what it is for.

#### Derive a separate key for arbitrary payloads

Derive a dedicated signing key from the seed and publish its pubkey next to the
existing one, so the master key never signs attacker-chosen bytes.

**Pros:**

* Good, because a leak of the signing key reveals nothing about the master key.

**Cons:**

* Bad, because it adds a second public key to the signatory boundary, the proto
  message, and everything downstream that has to discover which key to verify
  against.
* Bad, because `MintInfo.pubkey` already exists as the mint's identity and
  nothing verifies against it yet; introducing a rival key before the first one
  has a consumer splits the identity for no gain today.

#### Sign a domain-separated HMAC-SHA256 digest with the published key

Sign with the key behind `SignatoryKeysets::pubkey`, but over
`HMAC_SHA256(key = domain_tag, msg = payload)` rather than over the payload
itself, following the HMAC-SHA256 construction NUT-12 and NUT-13 already use in
this codebase.

**Pros:**

* Good, because the domain tag is the HMAC key and is a public constant, so any
  verifier recomputes the digest with no secret.
* Good, because the digest differs from `SHA256(payload)`, so a signature here
  is not valid as a NUT-11 or NUT-20 signature over the same bytes, and one of
  those is not valid here.
* Good, because it keeps one identity key: the pubkey the mint already
  advertises is the one that verifies.

**Cons:**

* Bad, because the master key now signs caller-supplied material. Callers must
  still treat `sign` as privileged.

## Decision Outcome

Chosen option: "Sign a domain-separated HMAC-SHA256 digest with the published
key", because it is the only option that both keeps the mint's single
advertised identity and stops the signature from being interchangeable with a
Cashu protocol signature.

The construction lives in `crates/cdk-signatory/src/identity.rs`:

```
digest = HMAC_SHA256(key = b"Cashu_Signatory_Sign_v1", msg = payload)
sig    = schnorr_sign(identity_key, digest)
```

`identity::sign` and `identity::verify` are the pair; both are public so a
verifier applies the same digest rather than reimplementing it. The identity
key is `xpriv.private_key`, the private half of the already-published
`SignatoryKeysets::pubkey`.

The trait gains `sign(payload) -> Signature`. On the wire this is a new `Sign`
RPC; `CONSTANTS_SCHEMA_VERSION` moves from 2 to 3 and the version interceptor
rejects a mismatch, so a mint and a signatory across the bump refuse to
connect.

### Positive Consequences

* The mint can obtain a signature over arbitrary bytes without ever holding a
  private key.
* Verification uses the pubkey the mint already advertises, so nothing new has
  to be discovered or persisted.
* Domain separation is enforced in one place that both signer and verifier go
  through.

### Negative Consequences

* A mint and a signatory must be upgraded together: the schema version bump
  makes a mixed pair refuse to connect.
* The key at the root of the keyset derivation tree now signs caller-supplied
  material. The domain separation keeps those signatures out of the Cashu
  protocol, but `sign` is still a privileged operation and should not be
  exposed to untrusted callers.
* Nothing consumes `sign` yet. There is no `Mint::sign` pass-through; the first
  caller adds it.

## Links

* Refines [ADR-0001](0001-signatory-mint-key-segregation.md)
