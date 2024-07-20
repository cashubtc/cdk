# Changelog

<!-- All notable changes to this project will be documented in this file. -->

<!-- The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), -->
<!-- and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). -->

<!-- Template

## [Unreleased]

### Summary

### Changed

### Added

### Fixed

### Removed

-->


## [Unreleased]

### Summary

### Changed
- cdk(wallet): `fn send` returns `Token` so the user can use the struct of convert it to a v3 or v4 string ([thesimplekid]).
- cdk(wallet): Publicly export `MultiMintWallet` ([thesimplekid]).
- cdk(cdk-database/mint): Get `pending` and `spent` `proofs` by `ys` or `secrets` instead of a single proofs ([thesimplekid]).
- cdk(cdk-database/mint): Change `add_blind_signature` to `add_blind_signatures` ([thesimplekid]).
- cdk(cdk-database/mint): Rename `add_active_keyset` to `set_active_keyset` ([thesimplekid]).
- cdk(cdk-database/wallet): Change `get_proofs` to return `Vec<ProofInfo>` instead of `Option<Vec<ProofInfo>>` ([thesimplekid]).

### Added
- cdk(NUT-11): Add `Copy` on `SigFlag` ([thesimplekid]).
- cdk(wallet): Add `fn send_proofs` that marks proofs as `reserved` and creates token ([thesimplekid]).
- cdk(wallet): Add `fn melt_proofs` that uses specific proofs for `melt` instead of selecting ([thesimplekid]).
- cdk-cli(receive): Add support for signing keys to be nostr nsec encoded ([thesimplekid]).
- cdk-fake-wallet: Add Fake wallet for testing ([thesimplekid]).
- cdk(cdk-database/mint): Add `add_proofs`, `get_proofs_by_ys`, `get_proofs_states`, and `update_proofs_states` ([thesimplekid]).
- cdk(cdk-database/mint): Add `get_blinded_signatures_for_keyset` to get all blind signatures for a `keyset_id` ([thesimplekid]).

### Fixed
- cdk(mint): `SIG_ALL` is not allowed in `melt` ([thesimplekid]).
- cdk(mint): On `swap` verify correct number of sigs on outputs when `SigAll` ([thesimplekid]).

### Removed
- cdk(wallet): Remove unused argument `SplitTarget` on `melt` ([thesimplekid]).
- cdk(cdk-database/mint): Remove `get_spent_proofs`, `get_spent_proofs_by_ys`,`get_pending_proofs`, `get_pending_proofs_by_ys`, and `remove_pending_proofs` ([thesimplekid]).

## [v0.2.0]

### Summary
This release introduces TokenV4, which uses CBOR encoding as the default token format. It also includes fee support for both wallet and mint operations.

When sending, the sender can choose to include the necessary fee to ensure that the receiver can redeem the full sent amount. If this is not done, the receiver will be responsible for the fee.

Additionally, this release introduces a Mint binary cdk-mintd that uses the cdk-axum crate as a web server to create a full Cashu mint. When paired with a Lightning backend, currently implemented as Core Lightning, it is included in this release as cdk-cln.

### Changed
- cdk(wallet): `wallet:receive` will not claim `proofs` from a mint other then the wallet's mint ([thesimplekid]).
- cdk(NUT00): `Token` is changed from a `struct` to `enum` of either `TokenV4` or `Tokenv3` ([thesimplekid]).
- cdk(NUT00): Rename `MintProofs` to `TokenV3Token` ([thesimplekid]).
- cdk(wallet): Additional arguments in `send` `send_kind` and `include_fees` for control of how to handle fees in a send ([thesimplekid]).
- cdk(wallet): Additional arguments in `create_swap` `include_fees` for control of if fees to redeam the send proofs are included in send amount ([thesimplekid]).

### Added
- cdk: TokenV4 CBOR ([davidcaseria]/[thesimplekid]).
- cdk(wallet): `wallet::receive_proof` functions to claim specific proofs instead of encoded token ([thesimplekid]).
- cdk-cli: Flag on `send` to print v3 token, default is v4 ([thesimplekid]).
- cdk: `MintLightning` trait ([thesimplekid]).
- cdk-mintd: Mint binary ([thesimplekid]).
- cdk-cln: cln backend for mint ([thesimplekid]).
- cdk-axum: Mint axum server ([thesimplekid]).
- cdk: NUT06 `MintInfo` and `NUTs` builder ([thesimplekid]).
- cdk: NUT00 `PreMintSecret` added Keyset id ([thesimplekid]).
- cdk: NUT02 Support fees ([thesimplekid]).

### Fixed
- cdk: NUT06 deseralize `MintInfo` ([thesimplekid]).


## [v0.1.1]

### Summary

### Changed
- cdk(wallet): `wallet::total_pending_balance` does not include reserved proofs ([thesimplekid]).


### Added
- cdk(wallet): Added get reserved proofs [thesimplekid](https://github.com/thesimplekid).

<!-- Contributors -->
[thesimplekid]: https://github.com/thesimplekid
[davidcaseria]: https://github.com/davidcaseria
