# Federated FROST BIP32 and Taproot derivation review

Status: Milestone 3 focused implementation review, 2026-07-13. This is not an
independent external audit. The later independent cryptography and adversarial
review gates remain required before mainnet value.

## Frozen derivation contract

The generic `cdk-frost` crate owns the application-neutral root, application
registration, additive signing plans, nonce lifecycle, FROST signing, and
strict wire bindings. The `cdk-frost-bip32` crate is the only Bitcoin-specific
adapter. It owns the stable application label
`org.cashubtc.cdk.federated-onchain`, derivation version `1`, chain code,
BIP32 branches/indexes, descriptors, and BIP341 mode. Neither crate depends on
mint business logic, consensus, storage, or transport.

For root public point `P`, root shares `s_i`, application tweak `a`, and public
BIP32 child tweaks `b_0` and `b_j`, the internal child is:

`P_j = P + aG + b_0G + b_jG`

and every signing share is:

`s_i,j = s_i + a + b_0 + b_j (mod n)`.

Adding the same public scalar to every Shamir share adds that scalar to the
polynomial's constant term, so threshold interpolation yields the matching
child secret without reconstructing it. Verification shares receive the same
point additions. All intermediate full compressed points retain parity.

The exact order is:

1. Canonical, untweaked FROST root package and full compressed root point.
2. CDK application-domain additive tweak.
3. Domain-separated application chain code.
4. Public non-hardened BIP32 CKD for branch `0` (external) or `1` (internal).
5. Public non-hardened BIP32 CKD for index `0..=2^31-1`.
6. BIP340 even-Y normalization inside `frost-secp256k1-tr` signing.
7. BIP341 `TapTweak` with no script Merkle root for `tr(xpub/branch/*)`.

This follows [BIP32 public child derivation](https://github.com/bitcoin/bips/blob/master/bip-0032.mediawiki),
[BIP340 x-only signing](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki),
and [BIP341 Taproot output-key tweaking](https://github.com/bitcoin/bips/blob/master/bip-0341.mediawiki).
The upstream Taproot ciphersuite's `post_dkg` output is not persisted or reused
as the canonical root. Its even-Y and tweak operations occur only on ephemeral,
fully derived signing packages.

## Chain code, descriptors, and invalid children

The application chain code is SHA-256 over the versioned domain, root epoch,
application ID/version/policy, and full application group key. It is public and
deterministic. The synthetic application Xpub has depth zero and uses the full
application point. BDK receives `tr(xpub/0/*)` and `tr(xpub/1/*)`.

Hardened derivation is rejected. If BIP32 rejects a non-hardened child because
`IL` is invalid or the child point is infinity, the adapter returns the exact
next normal index. Consensus must accept that retry before use; the final
normal index has no retry and fails closed. Network is included in the signing
derivation binding, while the underlying public child remains network-neutral.

Nonce commitments and signature shares bind root epoch, application metadata,
session, and the complete derivation hash. Branch, index, network, tweak list,
or Taproot-mode substitution between rounds is therefore rejected.

## Review evidence and residual risk

Focused tests prove BDK descriptor/address equality for external and internal
branches at indexes `0`, `1`, `17`, `1_000_003`, and `2^31-1`, plus 256
deterministic property indexes. They cover a threshold Taproot signature under
the derived output key, stable xpub/address/key/binding/signature vectors,
wrong-target aggregation, network, root epoch, application ID/version, and
hardened-index rejection. `cargo clippy` passes for both FROST crates with
warnings denied.

Public additive derivation is operationally useful but does not isolate a
complete derived-private-key compromise: because tweaks are public, exposure
of a complete derived private key can reveal the application/root key and
therefore endanger every application sharing that root. CDK never reconstructs
or exports such a key, but independent cryptographic review, durable nonce
storage, DKG review, and mainnet adversarial review remain release gates.
