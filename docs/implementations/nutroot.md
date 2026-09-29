# Nutroot draft implementation

This work targets cashubtc/nuts PR #443 at revision
`9fdc29b104fd703402e98aab130303a685ef9b41`. The protocol remains a draft;
this implementation targets that revision rather than following later changes
automatically.

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
as on the BLS branch before this change. This includes the `auth` unit:
version-02 BATs authorize the exact HTTP method, path/query, and body bytes.
Previously issued version-00/01 BATs remain redeemable as bearer tokens.

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

- Durable NUT-07/NUT-17 spend commitments, stored atomically with input
  reservation. Only spent disclosure leaves expose the exact accepted witness
  and input digest; private leaves, key paths, and pending inputs reveal no opening.
- NUT-18/NUT-26 Nutroot policies, fresh per-output blinding, wallet send/receive
  enforcement, and FFI policy types. Both request encodings preserve leaf bytes.
- `nutspA` signing packages with transaction reconstruction, partial-signature
  verification, receiver-slot fallback, merging, and atomic witness application.
- `nutrcA` receipts with canonical transcript parsing and independent mint-state
  verification. Wallets journal signed attempts before sending them and retain
  the journal after the spent proofs are removed.
- NUT-22 request-bound BATs through wallet storage, HTTP transports, and Axum.
  JSON is serialized once before signing; Axum binds the original request URI
  and exact body under the configured body limit. Non-HTTP mint adapters must
  set trusted request context with `BlindAuthToken::set_request_context` before
  verifying a version-02 BAT. Context and private signing keys are never serialized.

## Wallet interfaces

Set `SendOptions.nutroot` to request freshly locked outputs. It cannot be combined
with legacy `conditions`, and requires online version-02 issuance. Payment
requests carrying both encodings select `nutroot` for version 02 and `nut10`
for legacy issuance. A Nutroot-only request cannot be paid using legacy outputs.
A legacy-only locking request cannot silently fall back to bearer v3 outputs.

On receipt, set `ReceiveOptions.nutroot` to the advertised policy and supply
receiver keys through the wallet keyring or `p2pk_signing_keys`. This verifies
blinding, NUMS offsets, ephemeral-key presence, and the complete leaf set before
signing. Merely being able to spend one leaf does not verify the payment policy.

Use `SigningPackage::new`, `sign`, `merge`, and `apply` for multi-party script
spends. `Wallet::sign_nutroot_package` signs a caller-approved package using its
keyring; inspect the package's inputs, outputs, and trusted melt quote amount
before approving it. The package carries no trusted digest or private transfer
keys. Signatures from another transaction cannot be merged into it.

`Wallet::nutroot_receipt_ids` lists signed attempts. `export_nutroot_receipt`
exports an entry only after checking mint signatures and the mint's spent
commitments. `verify_nutroot_receipt` performs the same checks for an imported
receipt. Receipts expose the transaction and exercised witness, so disclosure
is the payer's choice. These operations are also exposed through FFI.

## Validation scope

Tests cover the pinned crypto vectors, transaction and receipt tampering,
partial multisignature merging, payment-policy construction, stored exact witness
bytes, spend-state disclosure, live and snapshot subscriptions, issuance-version
migration, request-bound BAT verification/replay, and raw HTTP authorization.
The `nutroot` integration test exercises payments between two CDK wallets,
policy rejection, receipt export, and mixed legacy/version-02 redemption.

Run the integration tests without external services:

```sh
CDK_TEST_DB_TYPE=memory cargo test -p cdk-integration-tests --test nutroot
```

These are implementation tests; independent interoperability with another wallet
or mint implementation has not been certified. Deploy the pinned draft only with
compatible clients and retain database backups for migration and recovery.

Strict workspace Clippy still reports pre-existing BLS warnings about redundant
conversions and missing panic documentation in `nut01`, `nut12`, `nut28`, and
DHKE helpers. The nightly toolchain also emits existing future-recursion warnings.
These lint results are separate from the passing compilation and runtime checks.
