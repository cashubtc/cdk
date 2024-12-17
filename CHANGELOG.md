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

#[0.6.0]

### Changed
cdk: Enforce `quote_id` to uuid type in mint ([tdelabro]).
cdk: Refactor wallet mint connector ([ok300]).

### Added
cdk: `NUT19` Settings in `NUT06` info ([thesimplekid]).
cdk: `NUT17` Websocket support for wallet ([crodas]).
cdk-axum: Redis cache backend ([crodas]).
cdk-mints: Get mint settings from env vars ([thesimplekid]).
cdk-axum: HTTP compression support ([ok300]).

### Fixed
cdk-sqlite: keyset counter was overwritten when keyset was fetched from mint ([thesimplekid]).
cdk-cli: on `mint` use `unit` from cli args ([thesimplekid]).
cdk-cli: on `restore` create `wallet` if it does not exist ([thesimplekid]).
cdk: Signaling support for optional nuts ([thesimpekid]).
cdk-phd: Check payment has valid uuis ([thesimplekid]).

#[0.5.0]
### Changed
- cdk: Bump `bitcoin` to `0.32.2` ([prusnak]).
- cdk: Bump `lightning-invoice` to `0.32.2` ([prusnak]).
- cdk: Bump `lightning` to `0.0.124` ([prusnak]).
- cdk: `PaymentMethod` as a `non_exhaustive` enum ([thesimplekid]).
- cdk: `CurrencyUnit` as a `non_exhaustive` enum ([thesimplekid]).
- cdk: Enforce token is single mint ([thesimplekid]).
- cdk: Mint will return change for over paid melt even over fee reserve ([davidcaseria]).
- cdk: Refactor ln_backeds to be on the `cdk::Mint` and not with axum ([thesimplekid]).
- cdk: Change is returned in the check quote response ([thesimplekid]).
- cdk: Move unit conversion util fn to amount module ([davidcaseria]).
- cdk: Remove spent proofs from db when check state is called ([mubarak23]).
- cdk: Use `MintUrl` directly in wallet client ([ok300]).
- cdk-cli: Change cdk-cli pay command to melt ([mubarak23]).
- cdk: Rename `Wallet::get_proofs` to `Wallet::get_unspent_proofs` ([ok300]).
- cdk: `Id` to `u32` changed from `TryFrom` to `From` ([vnrpc]). 


### Added
- cdk: Added description to `MintQuoteBolt11Request` ([lollerfirst]).
- cdk(wallet): Added description to `mint_quote` ([lollerfirst]).
- cdk: Add `amount` and `fee_paid` to `Melted` ([davidcaseria]).
- cdk: Add `from_proofs` on `Melted` ([davidcaseria]). 
- cdk: Add unit on `PaymentResponse` ([thesimplekid]).
- cdk: Add description for mint quote ([lollerfirst]).
- cdk-axum: Add cache to some endpoints ([lollerfirst]).
- cdk: Add Proofs trait ([ok300]).
- cdk: Wallet verifies keyset id when first fetching keys ([thesimplekid]).
- cdk-mind: Add swagger docs ([ok300]).
- cdk: NUT18 payment request support ([thesimplekid]).
- cdk: Add `Wallet::get_proofs_with` ([ok300]).
- cdk: Mint NUT-17 Websocket support ([crodas]).

### Removed
- cdk: Remove `MintMeltSettings` since it is no longer used ([lollerfirst]).
- cdk: `PaymentMethod::Custom` ([thesimplekid]).
- cdk: Remove deprecated `MeltBolt11Response` ([thesimplekid]).

### Fixed
- cdk: Check of inputs to include fee ([thesimplekid]).
- cdk: Make unit mandatory in tokenv4 ([ok300]).




#[0.4.0]
### Summary

### Changed
- cdk: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-axum: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-strike: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-lnbits: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-phoenixd: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-fake-wallet: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-cln: Reduce MSRV to 1.63.0 ([thesimplekid]).
- cdk-sqlite: Reduce MSRV to 1.66.0 ([thesimplekid]).
- cdk-redb: Reduce MSRV to 1.66.0 ([thesimplekid]).
- cdk: Format url base lowercase ([callebtc]).
- cdk: Use CDK error type instead of mint and wallet specific ([thesimplekid]).
- cdk-cli: Tokenv4 error print diagnostic notation ([ok300]).
- cdk-redb: Remove use of mutex ([thesimplekid]).

### Added
- cdk: Multiple error types ([thesimplekid]).


### Fixed
- cdk(mint): use checked addition on amount to ensure there is no overflow ([thesimplekid]).

### Removed
- cdk(wallet): Removed CDK wallet error ([thesimplekid]).
- cdk(mint): Removed CDK mint error ([thesimplekid]).


## [0.3.0]

### Summary

### Changed
- cdk(wallet): `fn send` returns `Token` so the user can use the struct of convert it to a v3 or v4 string ([thesimplekid]).
- cdk(wallet): Publicly export `MultiMintWallet` ([thesimplekid]).
- cdk(cdk-database/mint): Get `pending` and `spent` `proofs` by `ys` or `secrets` instead of a single proofs ([thesimplekid]).
- cdk(cdk-database/mint): Change `add_blind_signature` to `add_blind_signatures` ([thesimplekid]).
- cdk(cdk-database/mint): Rename `add_active_keyset` to `set_active_keyset` ([thesimplekid]).
- cdk(cdk-database/wallet): Change `get_proofs` to return `Vec<ProofInfo>` instead of `Option<Vec<ProofInfo>>` ([thesimplekid]).
- cdk-cli: Receive will add wallet when receiving if mint is unknown ([thesimplekid]).
- cdk(cdk-database/mint): Rename `get_blinded_signatures` to `get_blind_signatures` ([thesimplekid]).
- cdk(cdk-database/mint): Rename `get_blinded_signatures_for_keyset` to `get_blind_signatures_for_keyset` ([thesimplekid]).
- cdk(mint): typo rename `total_redeame` to `total_redeemed` ([vnprc])
- cdk(mint): Refactored `MintKeySet::generate_from_xpriv` and `MintKeySet::generate_from_seed` methods to accept max_order, currency_unit, and derivation_path parameters directly ([vnprc]).
- cdk(wallet): Return WalletKey for UnknownWallet error ([davidcaseria]).
- cdk(cdk-lightning): `CreateInvoiceResponse` added expiry time to better support backends where it cannot be set ([thesimplekid]).
- cdk(cdk-lightning): Use `Amount` type instead of `u64` ([thesimplekid]).
- cdk(cdk-lightning): `CreateInvoice` requires unit argument ([thesimplekid]).
- cdk(cdk/multi_mint_wallet): `get_balances` returns a `BTreeMap` instead of `HashMap` ([thesimplekid]).

### Added
- cdk(NUT-11): Add `Copy` on `SigFlag` ([thesimplekid]).
- cdk(wallet): Add `fn send_proofs` that marks proofs as `reserved` and creates token ([thesimplekid]).
- cdk(wallet): Add `fn melt_proofs` that uses specific proofs for `melt` instead of selecting ([thesimplekid]).
- cdk-cli(receive): Add support for signing keys to be nostr nsec encoded ([thesimplekid]).
- cdk-fake-wallet: Add Fake wallet for testing ([thesimplekid]).
- cdk(cdk-database/mint): Add `add_proofs`, `get_proofs_by_ys`, `get_proofs_states`, and `update_proofs_states` ([thesimplekid]).
- cdk(cdk-database/mint): Add `get_blinded_signatures_for_keyset` to get all blind signatures for a `keyset_id` ([thesimplekid]).
- cdk(mint): Add `total_issued` and `total_redeamed` ([thesimplekid]).
- cdk(cdk-database/mint) Add `get_proofs_by_keyset_id` ([thesimplekid]).
- cdk(wallet/mint): Add `mint_icon_url` ([cjbeery24]).
- cdk: Add `MintUrl` that sanatizes mint url by removing trailing `/` ([cjbeery24]).
- cdk(cdk-database/mint): Add `update_proofs` that both adds new `ProofInfo`s to the db and deletes ([davidcaseria]).
- cdk(cdk-database/mint): Add `set_pending_proofs`, `reserve_proofs`, and `set_unspent_proofs` ([davidcaseria]).


### Fixed
- cdk(mint): `SIG_ALL` is not allowed in `melt` ([thesimplekid]).
- cdk(mint): On `swap` verify correct number of sigs on outputs when `SigAll` ([thesimplekid]).
- cdk(mint): Use amount in payment_quote response from ln backend ([thesimplekid]).
- cdk(mint): Create new keysets for added supported units ([thesimplekid]).
- cdk(mint): If there is an error in swap proofs should be reset to unspent ([thesimplekid]).

### Removed
- cdk(wallet): Remove unused argument `SplitTarget` on `melt` ([thesimplekid]).
- cdk(cdk-database/mint): Remove `get_spent_proofs`, `get_spent_proofs_by_ys`,`get_pending_proofs`, `get_pending_proofs_by_ys`, and `remove_pending_proofs` ([thesimplekid]).
- cdk: Remove `UncheckedUrl` in favor of `MintUrl` ([cjbeery24]).
- cdk(cdk-database/mint): Remove `set_proof_state`, `remove_proofs` and `add_proofs` ([davidcaseria]).

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
[vnprc]: https://github.com/vnprc
[cjbeery24]: https://github.com/cjbeery24
[callebtc]: https://github.com/callebtc
[ok300]: https://github.com/ok300
[lollerfirst]: https://github.com/lollerfirst
[prusnak]: https://github.com/prusnak
[mubarak23]: https://github.com/mubarak23
[vnprc]: https://github.com/vnprc
[crodas]: https://github.com/crodas
[tdelabro]: https://github.com/tdelabro

