# Federated FROST cryptographic boundary review

Status: milestone 2 implementation record, 2026-07-13.

This note records the dependency and boundary decision behind `cdk-frost`. It
does not replace the focused derivation review required by milestone 3 or the
independent security review and mainnet gates later in the implementation
tracker.

## Selected implementation

- Crate: `frost-secp256k1-tr`, pinned to `3.0.0`.
- Upstream: `ZcashFoundation/frost`.
- License: MIT or Apache-2.0.
- Declared MSRV: Rust 1.81; CDK requires Rust 1.85.
- The release supplies trusted-dealer generation, a three-part DKG, refresh and
  repair primitives, two-round FROST signing, BIP340 serialization, even-Y
  normalization, and BIP341 tweak helpers.
- CDK enables only the upstream serialization feature and wraps every public
  wire value in a bounded, versioned, canonical CDK encoding.

The upstream project describes the crates as stable and SemVer-covered. NCC
audited the generic core and several ciphersuites at version 0.6.0, including
key generation, DKG, and signing. The upstream audit statement explicitly says
that `frost-secp256k1-tr` was **not** in scope. This is a material residual risk:
the CDK Taproot adapter must receive focused cryptographic review before it is
connected to wallet value, and production/mainnet activation remains blocked
on the tracker security gates.

Primary references:

- <https://github.com/ZcashFoundation/frost>
- <https://docs.rs/frost-secp256k1-tr/3.0.0/frost_secp256k1_tr/>
- <https://crates.io/crates/frost-secp256k1-tr/3.0.0>

## Canonical root versus upstream processing

CDK's canonical root is the untweaked output of the root ceremony: epoch, DKG
protocol version, ciphersuite, threshold, roster mapping, one local signing
share, every public verification share, the compressed group key, and a
transcript binding. It contains no application, BDK, network, child key, or
Taproot policy.

The selected ciphersuite performs BIP340 even-Y normalization transiently in
its signing and aggregation preprocessing. Its DKG `post_dkg` hook additionally
applies the BIP341 key-path-only Taproot tweak. That processed DKG output is not
the CDK root format and must never be persisted as if it were. Milestone 4 must
extract and validate the untweaked DKG result before the upstream `post_dkg`
application step, or use reviewed equivalent conversion code. Application,
child, and Taproot processing happen later and in the order frozen by milestone
3.

Trusted-dealer generation does not invoke the Taproot `post_dkg` hook, so it is
used only as the milestone 2 test harness for canonical untweaked packages. No
trusted dealer is introduced into the production ceremony.

## CDK-owned boundary

The new `cdk-frost` crate has no dependency on BDK, Bitcoin networks, Cashu
mint business logic, AlephBFT, database implementations, or transport. It owns:

- stable member, root epoch, application, derivation-version, session, and nonce
  identifiers;
- an opaque zeroizing canonical local root package with redacted debug output;
- public additive application derivation bound to root epoch, transcript, group
  key, application ID, derivation version, and authorization-policy commitment;
- strict reconstruction of upstream key packages from canonical components,
  without an aggregate secret;
- a participant key manager with reviewed application registration, public
  derivation, single-use nonce commitments, authorized signature shares, and
  verified aggregation; and
- exact public encodings that reject duplicate participants, unknown versions
  and ciphersuites, truncation, trailing bytes, oversized inputs, malformed
  commitments, and malformed shares.

There is no production root reconstruction function, plaintext root-share
export, complete derived private-key export, or direct sign-digest method. An
application adapter is explicitly part of the custody boundary: it must validate
its typed authoritative request and returns the exact 32-byte BIP340 message only
after authorization succeeds. A future root-share migration format must be
versioned and encrypted and is intentionally not present in this milestone.

Public additive derivation provides namespace separation, not compromise
isolation. Exposure of a complete derived private key can reveal the additive
tweak relationship and compromise the root and other applications. Adapters
that require stronger isolation or incompatible algebra need a separate DKG.

## Domain labels

The version-1 boundary reserves these exact ASCII labels; future changes require
a derivation/protocol version change and new test vectors:

| Purpose | Label |
|---|---|
| Root DKG context | `cdk/frost/root-dkg/v1` |
| Root transcript | `cdk/frost/root-transcript/v1` |
| Application identifier | `cdk/frost/application-id/v1` |
| Application derivation | `cdk/frost/application-derivation/v1` |
| Nonce identity | `cdk/frost/nonce-id/v1` |
| Signing authorization transcript | `cdk/frost/signing-authorization/v1` |

The root ceremony will bind federation ID, roster, threshold, member mapping,
ceremony ID, root epoch, DKG protocol version, and ciphersuite under the first
two labels in milestone 4. Application derivation already binds root epoch,
root transcript, root group key, application ID, derivation version, and policy
commitment. Nonce and signing messages additionally bind the application
registration and signing session; the registered adapter must include the same
scope when deriving its authorized 32-byte message.
