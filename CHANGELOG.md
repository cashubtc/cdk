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

## [v0.2.0]

### Summary
This release introduces TokenV4, which uses CBOR encoding as the default token format. It also includes fee support for both wallet and mint operations.

When sending, the sender can choose to include the necessary fee to ensure that the receiver can redeem the full sent amount. If this is not done, the receiver will be responsible for the fee.

Additionally, this release introduces a Mint binary cdk-mintd that uses the cdk-axum crate as a web server to create a full Cashu mint. When paired with a Lightning backend, currently implemented as Core Lightning, it is included in this release as cdk-cln.

### Changed
cdk(wallet): `wallet:receive` will not claim `proofs` from a mint other then the wallet's mint ([thesimplekid]).
cdk(NUT00): `Token` is changed from a `struct` to `enum` of either `TokenV4` or `Tokenv3` ([thesimplekid]).
cdk(NUT00): Rename `MintProofs` to `TokenV3Token` ([thesimplekid]).


### Added
cdk: TokenV4 CBOR ([davidcaseria]/[thesimplekid]).
cdk(wallet): `wallet::receive_proof` functions to claim specific proofs instead of encoded token ([thesimplekid]).
cdk-cli: Flag on `send` to print v3 token, default is v4 ([thesimplekid]).
cdk: `MintLightning` trait ([thesimplekid]).
cdk-mintd: Mint binary ([thesimplekid]).
cdk-cln: cln backend for mint ([thesimplekid]).
cdk-axum: Mint axum server ([thesimplekid]).
cdk: NUT06 `MintInfo` and `NUTs` builder ([thesimplekid]).
cdk: NUT00 `PreMintSecret` added Keyset id ([thesimplekid])
cdk: NUT02 Support fees ([thesimplekid])

### Fixed
cdk: NUT06 deseralize `MintInfo` ([thesimplekid]).


## [v0.1.1]

### Summary

### Changed
cdk(wallet): `wallet::total_pending_balance` does not include reserved proofs ([thesimplekid]).


### Added
cdk(wallet): Added get reserved proofs [thesimplekid](https://github.com/thesimplekid).

<!-- Contributors -->
[thesimplekid]: https://github.com/thesimplekid
[davidcaseria]: https://github.com/davidcaseria
