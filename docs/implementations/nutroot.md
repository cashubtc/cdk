# Nutroot draft implementation

This work targets cashubtc/nuts PR #443 at revision
`9fdc29b104fd703402e98aab130303a685ef9b41`. It is experimental and does not yet
implement the entire proposal.

## Issuance and migration

The mint keeps one active issuance keyset per unit. An operator can set:

```toml
[info]
keyset_version = "02"
```

The accepted values are `"00"`, `"01"`, and `"02"`. Remove the older
`use_keyset_v2` option when setting this; conflicting selections fail startup.
The Rust builder exposes `with_keyset_version(KeySetVersion::Version02)`.

On restart, an explicit selection rotates mismatched active keysets. The old
keysets become inactive for issuance but remain available to verify and redeem
outstanding proofs, subject to their existing expiry rules. Wallets can swap
old proofs into the newly active keyset. Restarting with the same version does
not rotate again merely because the option is present. Without an explicit
selection, existing active keysets are preserved; new keysets use version `02`,
as on the BLS branch before this change. The `auth` unit stays on version `01` when `02` is selected,
until request-bound v3 BAT authorization is integrated.

Deploy wallets that understand Nutroot before changing issuance. A legacy
wallet can no longer obtain legacy outputs once the mint issues version `02`.
Complete outstanding unlocked mint quotes before switching: version-02 issuance
requires a quote locking key, which cannot be added to an existing unlocked
quote by the wallet.
Back up the database and seed before migrating. Preserve the wallet database,
including saga records: random locking material cannot be restored from the
seed alone. Coordinate restarts of instances sharing a mint database so an old
process cannot continue issuing from its cached active keyset.

Earlier experimental BLS version-02 proofs used different secret hashing and
key derivation. They are not compatible with these Nutroot rules. The
00/01 redemption path does not migrate those experimental proofs.

## Implemented

- Canonical point secrets and decoded-byte BLS hashing; framed NUT-13 key
  derivation and mint-identity-scoped quote signing keys.
- Nutroot leaves, trees, tweaks, key-path and script-path witnesses,
  transaction transcripts, and upstream cryptographic vectors.
- Mint, batch-mint, swap, and melt authorization using transaction-bound
  signatures. Mint-side quote amounts come from stored quote state.
- Token spend information, wallet database and FFI conversion, and stripping
  transaction witnesses from transferable v3 tokens.
- Bearer transfer keys, receiver blinding, NUMS script-only locking, supported
  P2PK/HTLC policy translation, and exact Nutroot policy verification helpers.
  Keyless hashlocks/refunds and legacy SIG_ALL policies are rejected for v3.
- Saving output secrets and transfer metadata before requests, with recovery
  through persisted saga data and ordered seed recovery for older records.

## Remaining before full PR support

- NUT-07/NUT-17 spend commitment storage and disclosure openings. Version-02
  witnesses are currently suppressed in check-state responses to protect
  private leaves; even disclosure leaves have no opening response yet.
- NUT-18/NUT-26 request serialization, wallet payment/receive-policy integration,
  and FFI exposure of the Nutroot request option. Core locking helpers exist.
- Signing-package (`nutspA`) and spend-receipt (`nutrcA`) transport workflows.
- NUT-22 authorization bound to exact HTTP request bytes. APIs without request
  bytes reject version-02 BATs; the builder retains legacy auth keysets until
  this is integrated.
- Full cross-wallet interoperability and end-to-end regression coverage,
  including disclosure, multi-party signing, and locked-quote recovery.

Passing core cryptographic and wallet tests does not constitute conformance
with all of PR #443. Do not advertise complete support while these remain.
