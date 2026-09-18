# The mint identity key and the signed mint info

* Status: accepted
* Authors: Cesar Rodas
* Date: 2026-08-26
* Targeted modules: cashu, cdk, cdk-axum, cdk-signatory, cdk-mintd
* Associated tickets/PRs: https://github.com/cashubtc/nuts/pull/416

## Context and Problem Statement

Everything `GET /v1/info` returns, the mint's URLs, its message of the day, its
contact details, its terms of service, and the table of NUTs it claims to
support, is whatever arrives over the wire. A wallet has no way to tell the
mint's answer from a rewritten one.

Fixing that means the mint signing a statement about itself, and it had no way
to produce one. The `Signatory` trait is the only seam through which the mint
reaches private-key material (ADR-0001), and every method on it is
keyset-shaped: `blind_sign`, `verify_proofs`, `keysets`, `subscribe_keysets`,
`rotate_keyset`. Signing outside the signatory would put a private key back
inside the mint process, which is exactly what ADR-0001 removed.

NUT-06 (cashubtc/nuts#416) settles both halves. The mint has an identity key
derived from its seed, and it signs with BIP-340 over the SHA-256 of a payload:

```
identity_key = HMAC_SHA256(key = seed, msg = b"Cashu_Mint_Identity_v1" || ctr)
sig          = schnorr_sign(identity_key, SHA256(payload))
```

`ctr` starts at zero and increments while the candidate falls outside
`1..n-1`. The mint publishes the public half as `GetInfoResponse.pubkey`.

The payload for the mint info signature is the response describing itself:

1. Take the complete `GetInfoResponse` JSON object.
2. Remove the top-level `signature` and `time` members.
3. Canonicalize with JCS (RFC 8785), encode UTF-8.
4. SHA-256, sign with BIP-340, hex into `signature`.

CDK already published a pubkey, but a different one: the BIP-32 master key from
`Xpriv::new_master(seed)`, whose private half signed nothing. So the question is
not only how the signatory signs, but which key it signs with and what happens
to the key deployments already advertise.

## Decision Drivers

* The signing key must stay inside the signatory, like every other key.
* A wallet holding the mint's published pubkey must verify with nothing but a
  NUT-06 implementation, and with no signatory round trip.
* Verification must not depend on CDK having implemented every NUT the mint
  advertises.
* `/v1/info` is unauthenticated and hit on every wallet handshake.
* The spec's "wallets MUST reject" cannot be obeyed literally today without
  disconnecting from every mint in existence.

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
callers, which is why there is no public `Mint::sign` pass-through. `Mint` is
what every embedder holds, and a public `sign(Vec<u8>)` on it would be exactly
the oracle the Cons above warn about. The mint info signature needs no such API,
so signing stays private.

### The signing payload

Step 3 needs JCS, and nothing in the workspace could produce it. The choice was
a crate (`serde_jcs`, `json-canon`) against roughly eighty lines in `cashu`.
The crate's advantage is ECMAScript float formatting, the part of RFC 8785 that
is easy to get subtly wrong. Nothing in the `MintInfo` tree is a float.

Hand-rolled in `crates/cashu/src/util/jcs.rs`, rejecting floats rather than
formatting them. It keeps a new low-traffic dependency out of the crate every
CDK consumer pulls in, and the rejected cases are ones no Cashu type can
produce. Integers past 2^53 are rejected for the same reason: they have no
exact double, so a conformant implementation would emit the nearest one, and
emitting the literal digits would be bytes another implementation disagrees
with.

Keys are sorted by UTF-16 code units rather than the UTF-8 byte order
`serde_json`'s `BTreeMap` gives for free. Today every key in the tree is a fixed
ASCII field name and the two orders coincide, so this changes no output. It is
written that way so the first map-keyed field added does not silently produce
non-canonical bytes.

### What the wallet verifies against

Rebuilding the payload from a deserialized `MintInfo` is the obvious approach
and it is wrong. `serde` drops members the struct does not model, and `Nuts`
models a fixed set of NUT identifiers. A mint advertising a NUT that CDK has
not implemented signed those members too, so the rebuilt payload is missing
bytes and verification fails against a mint that did nothing wrong.

The wallet verifies the response as JSON, before deserializing. `HttpClient` is
the only place holding it in that form. `MintInfo::verify_signature` still
exists for producers and carries the caveat in its doc comment.

### How strict the wallet is

The spec says a wallet MUST reject a response with no signature. No mint
deployed today signs one, so obeying that immediately would leave the CDK
wallet unable to talk to any of them.

So: verify what is there, warn about what is not. A signature that is present
is always verified and a bad or malformed one is always an error. A missing one
is a warning by default and an error under `set_require_signed_mint_info`, so a
wallet that wants the spec's behaviour today can have it. The default flips
once mints sign.

### Where the mint signs

`Mint::mint_info()` is not only the `/v1/info` source. Quote issuance and
melting call it on every request, as do the mint-rpc handlers. Signing inside it
would put canonicalization on every quote request and would make a signatory
outage break minting and melting, which need the settings rather than an
attestation. Signing lives in a separate `Mint::signed_mint_info()`, and only
the HTTP handler calls it.

The signature is cached against the payload it covers. `/v1/info` is
unauthenticated and the signatory may be a remote gRPC service, so a round trip
per request would be an amplification vector. Any change to what is served, a
configuration update, a keyset rotation, the auth overlay, changes the payload,
so the cache needs no explicit invalidation, and the zeroed auxiliary
randomness makes a cached signature byte-identical to a fresh one.

`time` is stamped by the HTTP layer after the mint signs. NUT-06 excludes it
from the payload precisely so that stays valid, which is also what keeps the
cache useful: without the exclusion every response would need its own
signature.

Neither the wallet nor the mint persists a signature. The wallet databases
already drop `max_array_length`, so a stored signature could never re-verify.
On the mint it is derived from everything else, so `set_mint_info` and the
constructor clear it.

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

* A wallet holding the mint's pubkey can tell the info it received is the info
  the mint published.
* A NUT-06 wallet can verify the mint with no CDK-specific code, and the mint
  obtains its signature without ever holding a private key.
* Verification interoperates with mints advertising NUTs CDK has not
  implemented.
* The identity key is no longer the root of the keyset derivation tree.
* The canonicalizer is general RFC 8785 and usable by anything else that needs
  to sign JSON.

### Negative Consequences

* The advertised pubkey changes on upgrade, and operators who pinned it with a
  remote signatory must edit their configuration.
* A mint and a signatory must be upgraded together: the schema version bump
  makes a mixed pair refuse to connect.
* A signature is a plain BIP-340 signature over `SHA256(payload)`, with no tag
  in the digest saying what it is for.
* `/v1/info` now depends on the signatory. A cold start during a signatory
  outage serves 500s where it used to serve a database read.
* A mint emitting a float anywhere in `/v1/info` cannot be verified, even
  leniently, because the failure lands in the signature-present branch. If that
  turns up in the wild the fix is a real ECMAScript number serializer, not a
  silent skip.
* Duplicate JSON keys are last-wins in `serde_json` and an error in RFC 8785. A
  mint sending them produces a payload the wallet cannot reconstruct.
* Multi-instance mints (ADR-0004) sharing a database produce identical
  signatures only when their input and output limits match, since
  `max_array_length` is derived from running limits and sits inside the payload.
  Each response is self-consistent either way.

## Links

* Refines [ADR-0001](0001-signatory-mint-key-segregation.md)
