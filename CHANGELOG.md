# Changelog

<!-- All notable changes to this project will be documented in this file. -->
<!-- The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), -->
<!-- and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html). -->

## [0.13.0](https://github.com/cashubtc/cdk/releases/tag/v0.13.0)

### Summary

Version 0.13.0 marks a major milestone for mobile development with the introduction of comprehensive native mobile bindings that enable building Cashu wallets for iOS and Android using Swift and Kotlin. The release introduces cdk-ffi, a new Foreign Function Interface crate that provides UniFFI-based bindings for Swift, Kotlin, and Python, with full wallet functionality including multi-mint support, BOLT12 payments, BIP-353 address resolution, and advanced features like P2PK conditions and authentication. Mobile bindings are distributed through dedicated repositories at https://github.com/cashubtc/cdk-kotlin and https://github.com/cashubtc/cdk-swift that provide native package management for Android/JVM and iOS/macOS platforms respectively. The release also delivers significant infrastructure improvements including an event-driven payment architecture with real-time notifications, enhanced database layer with generic key-value storage, improved HTTP transport with proxy support and BIP-353 DNS resolution, and new operational features like Prometheus metrics collection and dedicated authentication database support.

### Added
- cdk-common: New `Event` enum for payment event handling with `PaymentReceived` variant ([thesimplekid]).
- cdk-common: Added `payment_method` field to `MeltQuote` struct for tracking payment method type ([thesimplekid]).
- cdk-sql-common: Database migration to add `payment_method` column to melt_quote table for SQLite and PostgreSQL ([thesimplekid]).
- cdk-common: New `MintKVStoreDatabase` trait providing generic key-value storage functionality for mint databases ([thesimplekid]).
- cdk-common: Added `KVStoreTransaction` trait for transactional key-value operations with read, write, remove, and list capabilities ([thesimplekid]).
- cdk-common: Added validation functions for KV store namespace and key parameters with ASCII character and length restrictions ([thesimplekid]).
- cdk-common: Added comprehensive test module for KV store functionality with transaction and isolation testing ([thesimplekid]).
- cdk-sql-common: Database migration to add `kv_store` table for generic key-value storage in SQLite and PostgreSQL ([thesimplekid]).
- cdk-sql-common: Implementation of `MintKVStoreDatabase` trait for SQL-based databases with namespace support ([thesimplekid]).
- cdk-common: Added `quote_id` field to `Transaction` struct for tracking associated mint or melt quote IDs ([thesimplekid]).
- cdk-sql-common: Database migration to add `quote_id` column to transactions table for SQLite and PostgreSQL ([thesimplekid]).
- cdk: Added `amount_mintable()` helper and stricter mint quote validation ([thesimplekid]).
- cdk-sql-common: Added persistent `melt_request` storage with associated blinded messages; new migrations for SQLite and PostgreSQL ([thesimplekid]).
- cdk-cln: Persist last `pay_index` in the mint KV store to avoid missed events across restarts ([thesimplekid]).
- cdk: HTTP subscriptions emit BOLT12 notifications per NUT-17 ([crodas]).
- cdk: DNS TXT resolution in HttpTransport and MintConnector for BIP‑353 lookups ([crodas]).
- cdk-ffi: Wallet FFI bindings and async constructors ([davidcaseria]).
- cdk-prometheus: New metrics crate and optional embedded Prometheus server; integrated metrics across HTTP, database, payments, and mint (feature: `prometheus`) ([asmo]).
- cdk-mintd: Optional Prometheus metrics server with configurable address/port via config and env vars (feature: `prometheus`) ([thesimplekid]).
- cdk-ldk-node: Web UI improvements (dynamic status, navigation, mobile support) ([erik]).
- cdk-postgres: Dedicated auth database support with separate schema and migrations when auth is enabled ([thesimplekid]/[asmo]).


### Changed
- cdk-common: Refactored `MintPayment` trait method `wait_any_incoming_payment` to `wait_payment_event` with event-driven architecture ([thesimplekid]).
- cdk-common: Updated `wait_payment_event` return type to stream `Event` enum instead of `WaitPaymentResponse` directly ([thesimplekid]).
- cdk: Updated mint payment handling to process payment events through new `Event` enum pattern ([thesimplekid]).
- cashu: Updated BOLT12 payment method specification from NUT-24 to NUT-25 ([thesimplekid]).
- cdk: Updated BOLT12 import references from nut24 to nut25 module ([thesimplekid]).
- cdk: Do not fallback from WebSocket to HTTP on first error in subscriptions; retry only when appropriate ([crodas]).
- cdk: Abstracted HTTP Transport; centralized `pay_request` logic into the cdk library ([lollerfirst]).
- cdk: MultiMintWallet refactor for clearer APIs and behavior when managing multiple mints ([davidcaseria]/[thesimplekid]).
- cdk: Treat `None` proxy host matcher as "apply to all hosts" for HTTP transport ([lollerfirst]).
- cdk: QuoteId handling unified as string in APIs and storage ([thesimplekid]).
- cdk-axum: Close WebSocket connections sooner on shutdown/errors to avoid dangling clients ([crodas]).
- cdk-sql-common: Consistent ordering for SQL migrations and build uses `OUT_DIR` for embedded migration files ([vnprc]).
- cdk-signatory: Updated protobuf to latest spec and added signatory-related DB migrations ([crodas]).
- cdk-redb: Bumped dependency version ([thesimplekid]).

### Fixed
- cdk: Wallet melt track and use payment method from quote for BOLT11/BOLT12 routing ([thesimplekid]).
- cdk: Improve error response details and mapping across HTTP/WS paths ([thesimplekid]).
- cdk: Fix config being overwritten on startup in certain scenarios ([thesimplekid]).
- cdk: WASM compatibility fixes for HTTP subscriptions and time handling (use `instant`) ([gudnuf]).
- cdk-postgres: Fix reconnection in connection pool and migration prefixes.
- cdk: Check keyset max order when generating/using keysets ([thesimplekid]).
- cdk: Correct error code returned for duplicate signature conditions ([lollerfirst]).
- cdk: Ensure all mint quotes are returned in listings ([thesimplekid]).
- cdk-axum: Improve error response detail structure ([thesimplekid]).


## [0.12.1](https://github.com/cashubtc/cdk/releases/tag/v0.12.1)

### Fixed
- cdk-postgres: TLS support for PostgreSQL connections ([asmogo]).
- cdk: patch sha-512 derivation -> sha-256 derivation ([lollerfirst]).

## [0.12.0](https://github.com/cashubtc/cdk/releases/tag/v0.12.0)

### Summary

Version 0.12.0 delivers end-to-end BOLT12 offers and payments, adds BIP‑353 address resolution for BOLT12 payments, and introduces cdk-ldk-node, an integrated Lightning backend that lets a single binary run both a Cashu mint and a Lightning node with full BOLT11 and BOLT12 support. It also adds a local, admin-focused web UI for cdk-ldk-node with dashboards for channels, invoices and offers, payments, and on-chain activity. On the data layer, the release expands storage with PostgreSQL via the new cdk-postgres crate and accelerates the shared SQL stack (cdk-sql-common) with statement caching and structured, namespaced/global migrations. Operationally, the mint now exposes explicit start and stop lifecycle methods, enabling graceful startup and shutdown of background services. Wallet keyset management has been clarified with renamed APIs that separate local storage from network fetches—making load_mint_keysets the primary entry point for token operations—and the MSRV is updated to 1.85.0.

### Added
- dev: Goose recipes for changelog and commit message generation with Just commands ([thesimplekid]).
- cashu: `KeySetInfos` type alias and `KeySetInfosMethods` trait for filtering keysets ([thesimplekid]).
- cdk: Mint lifecycle management with `start()` and `stop()` methods for graceful background service control ([thesimplekid]).
- cdk: Background task management for invoice payment monitoring with proper shutdown handling ([thesimplekid]).
- cashu: NUT-19 support in the wallet ([crodas]).
- cdk: SIG_ALL support for swap and melt operations ([thesimplekid]).
- cdk-sql-common: Add cache to SQL statements for better performance ([crodas]).
- cdk-integration-tests: New binary `start_fake_auth_mint` for testing fake mint with authentication ([thesimplekid]).
- cdk-integration-tests: New binary `start_fake_mint` for testing fake mint instances ([thesimplekid]).
- cdk-integration-tests: New binary `start_regtest_mints` for testing regtest mints ([thesimplekid]).
- cdk-integration-tests: Shared utilities module for common integration test functionality ([thesimplekid]).
- cdk-redb: Database migration to increment keyset counters by 1 for existing keysets with counter > 0 ([thesimplekid]).
- cdk-sql-common: Database migration to increment keyset counters by 1 for existing keysets with counter > 0 ([thesimplekid]).
- cdk-ldk-node: New Lightning backend implementation using LDK Node for improved Lightning Network functionality ([thesimplekid]).
- cdk-ldk-node: Local web management UI (dashboard, channels, invoices/offers, payments, on‑chain). Intended for localhost/admin use only; do not expose publicly ([thesimplekid]/[erik]).
- cdk-common: Added `start()` and `stop()` methods to `MintPayment` trait for payment processor lifecycle management ([thesimplekid]).
- cdk-mintd: Added LDK Node backend support with comprehensive configuration options ([thesimplekid]).
- cdk-postgres: Postgres Database for mint and wallet ([crodas]).
- cdk: BOLT12 mint quote WebSocket subscriptions (NUT-17) ([crodas]).
- cdk: Future streams for payments and minting proofs ([crodas]).
- cdk: Log-to-file support ([thesimplekid]).
- cdk(wallet): BIP-353 support ([thesimplekid]).
- security: Zeroize secrets on drop ([vnprc]).

### Changed
- cdk-common: Modified `Database::get_keyset_counter` trait method to return `u32` instead of `Option<u32>` for simpler keyset counter handling ([thesimplekid]).
- cdk: Refactored wallet keyset management methods for better clarity and separation of concerns ([thesimplekid]).
- cdk: Renamed `get_keyset_keys` to `fetch_keyset_keys` to indicate network operation ([thesimplekid]).
- cdk: Renamed `get_active_mint_keyset` to `fetch_active_keyset` for consistency ([thesimplekid]).
- cdk: Updated `get_active_mint_keysets` to `refresh_keysets` with improved keyset refresh logic ([thesimplekid]).
- cdk: Improved `load_mint_keysets` method to be the primary method for getting keysets for token operations ([thesimplekid]).
- cdk: Enhanced keyset management with better offline/online operation separation ([thesimplekid]).
- cdk: Updated method documentation to clarify storage vs network operations ([thesimplekid]).
- cdk: Refactored invoice payment monitoring to use centralized lifecycle management instead of manual task spawning ([thesimplekid]).
- cdk: Enhanced mint startup to initialize payment processors before starting background services ([thesimplekid]).
- cdk: Improved mint shutdown to gracefully stop payment processors alongside background services ([thesimplekid]).
- cdk-mintd: Updated to use new mint lifecycle methods for improved service management ([thesimplekid]).
- cdk-integration-tests: Updated test utilities to use new mint lifecycle management ([thesimplekid]).
- cdk: HTTP retry only on transport errors ([crodas]).
- cdk-lnbits: Migrate to LNBits v1 websocket API and remove pre-v1 code paths ([thesimplekid]).
- cdk-cln: Use millisatoshis (msats) for amounts ([thesimplekid]).
- cdk: NUT-20 support toggle in mint builder configuration ([thesimplekid]).
- cashu/cdk: New secret derivation per updated spec ([lollerfirst]).
- cdk-sqlite: Introduce `cdk-sql-common` crate for shared SQL storage codebase ([crodas]).
- cdk-sqlite: Rename `still_active` to `stale` for better clarity ([crodas]).
- cdk-integration-tests: Refactored regtest setup to use Rust binaries instead of shell scripts ([thesimplekid]).
- cdk-integration-tests: Improved environment variable handling for test configurations ([thesimplekid]).
- cdk-integration-tests: Enhanced CLN client connection with retry logic ([thesimplekid]).
- cdk-integration-tests: Updated integration tests to use proper temp directory management ([thesimplekid]).
- cdk-integration-tests: Simplified regtest shell scripts to use new binaries ([thesimplekid]).
- crates/cdk-mintd: Moved mintd library functions to separate module for better organization and testability ([thesimplekid]).
- dev/docker: Switch base image to Debian Trixie ([thesimplekid]).
- Updated MSRV to 1.85.0 ([thesimplekid]).
- dev: Simplified Nix flake configuration by removing specific dependency version constraints from MSRV shell hook ([thesimplekid]).

### Fixed
- cashu: Fixed CurrencyUnit custom units preserving original case instead of being converted to uppercase ([thesimplekid]).
- cdk: Fix P2PK spending-condition validation and requirements ([thesimplekid]).
- cdk: Fixed BOLT12 missing payments notifications ([crodas]).
- cdk-axum/mint: Fix BOLT12 WebSocket behavior on mint ([thesimplekid]).
- cdk-lnbits: Fix payment check and unit handling ([thesimplekid]).
- cdk-sqlite: Fix `get_mint_quote_by_request_lookup_id` function synchronization ([crodas]).
- cdk-sqlite: Reduce mmap_size to 5 MiB to avoid resource issues ([thesimplekid]).
- cdk: Remove unwrap in startup checks ([thesimplekid]).
- cdk: Allow paid and issued BOLT12 quotes to settle internally ([gudnuf]).
- cdk: Include change in melt quote state updates ([thesimplekid]).
- cdk-mintd/axum: Pass auth config from mintd through to axum correctly ([thesimplekid]).

### Migration
- cdk-sql-common: Improve migrations with namespaced and global migrations support ([crodas]).


## [0.11.0](https://github.com/cashubtc/cdk/releases/tag/v0.11.0)

### Summary

Version 0.11.0 brings significant architectural changes to enhance database reliability and performance. The major changes include:

1. **Database Engine Change**: Replaced `sqlx` with `rusqlite` as the SQLite database driver and removed support for `redb`. This change provides better performance and reliability for database operations.

2. **Transaction Management**: Introduced robust database transaction support that encapsulates all database changes. The new Transaction trait implements a rollback operation on Drop unless explicitly committed, ensuring data integrity.

3. **Race Condition Prevention**: Added READ-and-lock operations to securely read and lock records from the database for exclusive access, preventing race conditions in concurrent operations.

### ⚠️ Important Migration Note for redb Users
If you are currently running a mint with redb, you must migrate to SQLite before upgrading to v0.11. Follow these steps:

1. Stop your current mint
2. Back up your database
3. Use the migration script available at: https://github.com/cashubtc/cdk/blob/main/misc/convert_redb_to_sqlite.sh
4. Update your config file to target the SQLite database engine
5. Start your mint with v0.11


### Added
- cdk-lnbits: Support lnbits v1 and pre-v1 [PR](https://github.com/cashubtc/cdk/pull/802) ([thesimplekid]).
- Support for Keyset v2 [PR](https://github.com/cashubtc/cdk/pull/702) ([lollerfirst]).
- Add option to limit the token size of a send [PR](https://github.com/cashubtc/cdk/pull/855) ([davidcaseria]).
- Database transaction support [PR](https://github.com/cashubtc/cdk/pull/826) ([crodas]).
- Support for multsig refund [PR](https://github.com/cashubtc/cdk/pull/860) ([thesimplekid]).
- Convert unit helper fn [PR](https://github.com/cashubtc/cdk/pull/856) ([davidcaseria]).

### Changed
- cdk-sqlite: remove sqlx in favor of rusqlite ([crodas]).
- cdk-lnd: use custom tonic gRPC instead of fedimint-tonic-grpc [PR](https://github.com/cashubtc/cdk/pull/831) ([thesimplekid]).
- cdk-cln: remove the us of mutex on cln client [PR](https://github.com/cashubtc/cdk/pull/832) ([thesimplekid]).

### Fixed
- mint start up check was not checking unpaid quotes [PR](https://github.com/cashubtc/cdk/pull/844) ([gudnuf]).
- Naming of blinded_message column on blind_signatures was y [PR](https://github.com/cashubtc/cdk/pull/845) ([thesimplekid]).
- cdk-cli: Create wallets for non sat units if supported [PR](https://github.com/cashubtc/cdk/pull/841) ([thesimplekid]).

### Removed
- cdk-redb support for the mint [PR](https://github.com/cashubtc/cdk/pull/787) ([thesimplekid]).
- cdk-sqlite remove unused melt_request table [PR](https://github.com/cashubtc/cdk/pull/819) ([crodas])


## [0.10.1](https://github.com/cashubtc/cdk/releases/tag/v0.10.1)
### Fix
- Set mint version when mint rpc is enabled [PR](https://github.com/cashubtc/cdk/pull/803) ([thesimplekid]).
- `cdk-signatory` is optional for wallet [PR](https://github.com/cashubtc/cdk/pull/815) ([thesimplekid]).

## [0.10.0](https://github.com/cashubtc/cdk/releases/tag/v0.10.0)
### Added
- SignatoryManager service [PR](https://github.com/cashubtc/cdk/pull/509) ([crodas]).
- Mint URL flag option [PR](https://github.com/cashubtc/cdk/pull/765) ([thesimplekid]).
- Export NUT-06 supported settings field [PR](https://github.com/cashubtc/cdk/pull/764) ([davidcaseria]).
- Docker build workflow for arm64 images [PR](https://github.com/cashubtc/cdk/pull/770) ([asmo]).

### Changed
- cdk-redb: Removed mint storage functionality to be wallet-only ([thesimplekid]).
- Updated Nix flake to 25.05 and removed Nix cache [PR](https://github.com/cashubtc/cdk/pull/769) ([thesimplekid]).
- Updated dependencies [PR](https://github.com/cashubtc/cdk/pull/761) ([thesimplekid]).
- Refactored NUT-04 and NUT-05 [PR](https://github.com/cashubtc/cdk/pull/749) ([thesimplekid]).
- Updated Nix flake to 25.05 and removed Nix cache [PR](https://github.com/cashubtc/cdk/pull/769) ([thesimplekid]).

## [0.9.3](https://github.com/cashubtc/cdk/releases/tag/v0.9.3)
### Changed
- Melt will perform swap before attempting to melt if exact amount is not available [PR](https://github.com/cashubtc/cdk/pull/793) ([crodas]).

### Fixed
- Handle old nut15 format to keep compatibility with older nutshell version [PR](https://github.com/cashubtc/cdk/pull/794) ([thesimplekid]).

## [0.9.2](https://github.com/cashubtc/cdk/releases/tag/v0.9.2)
### Added
- HTLC from hash support [PR](https://github.com/cashubtc/cdk/pull/753) ([thesimplekid]).
- Optional transport and NUT-10 secret on payment request [PR](https://github.com/cashubtc/cdk/pull/744) ([thesimplekid]).
- Multi-part payments support in cdk-cli [PR](https://github.com/cashubtc/cdk/pull/743) ([thesimplekid]).

### Changed
- Refactored Lightning module to use common types [PR](https://github.com/cashubtc/cdk/pull/751) ([thesimplekid]).
- Updated LND to support mission control and improved requery behavior [PR](https://github.com/cashubtc/cdk/pull/746) ([lollerfirst]).

### Fixed
- NUT-18 payment request encoding/decoding [PR](https://github.com/cashubtc/cdk/pull/758) ([thesimplekid]).
- Mint URL trailing slash handling [PR](https://github.com/cashubtc/cdk/pull/757) ([thesimplekid]).
- Get spendable to return witness [PR](https://github.com/cashubtc/cdk/pull/756) ([thesimplekid]).
- Melt start up check [PR](https://github.com/cashubtc/cdk/pull/745) ([thesimplekid]).
- Race conditions with proof state updates ([crodas]).

## [0.9.1](https://github.com/cashubtc/cdk/releases/tag/v0.9.1)
### Fixed
- Remove URLs in gRPC management interface ([thesimplekid]).
- Only count signatures from unique pubkeys ([thesimplekid]).
- Race conditions with proof state updates ([crodas]).
- Debug print of Info struct ([thesimplekid]).
- Correct mnemonic hashing in Debug implementation ([thesimplekid]).

### Changed
- Updated lnbits-rs to 0.5.0 ([Darrell]).
- Update stable Rust to 1.86.0 ([thesimplekid]).
- Added CORS headers in responses [PR](https://github.com/cashubtc/cdk/pull/719) ([lollerfirst]).
- Mint should not enforce expiry ([thesimplekid]).
- Ensure unique proofs when calculating token value ([thesimplekid]).

## [0.9.0](https://github.com/cashubtc/cdk/releases/tag/v0.9.0)
### Added
- Amountless invoices [NUT](https://github.com/cashubtc/nuts/pull/173) [PR](https://github.com/cashubtc/cdk/pull/497) ([thesimplekid]).
- `create_time`, `paid_time` to mint and melt quotes [PR](https://github.com/cashubtc/cdk/pull/708) ([thesimplekid]).
- cdk-mint-rpc: Added get mint and melt quotes ttl [PR](https://github.com/cashubtc/cdk/pull/716) ([thesimplekid]).

### Changed
- cashu: Move wallet mod to cdk-common ([thesimplekid]).
- Export Mint DB Traits [PR](https://github.com/cashubtc/cdk/pull/710) ([davidcaseria]).
- Move Mint and Melt quote to `cdk` commit from `cashu` [PR](https://github.com/cashubtc/cdk/pull/665) ([thesimplekid]).

### Fixed
- Creation of memory sqlite db [PR](https://github.com/cashubtc/cdk/pull/707) ([crodas]).
- cdk-cli: Ensure auth wallet is created before attempting to mint pending [PR](https://github.com/cashubtc/cdk/pull/704) ([thesimplekid]).
- cdk-mint-rpc: Adding mint urls was not updating correctly [PR](https://github.com/cashubtc/cdk/pull/716) ([thesimplekid]).
- cdk-mint-rpc: Fixed setting long description [PR](https://github.com/cashubtc/cdk/pull/716) ([thesimplekid]).


## [v0.8.1](https://github.com/cashubtc/cdk/releases/tag/v0.8.1)
### Fixed
- cashu: Handle url with paths [PR](https://github.com/cashubtc/cdk/pull/678) ([benthecarman]).

### Changed
- cdk: Export `MintKeySetInfo` [PR](https://github.com/cashubtc/cdk/pull/673) ([davidcaseria]).

## [v0.8.0](https://github.com/cashubtc/cdk/releases/tag/v0.8.0)
### Fixed
- cdk: Proof matches conditions was not matching payment conditions correctly ([thesimplekid]).
- cdk: Updating mint_url would remove proofs when we want to keep them ([ok300]).
- Wallet: Fix ability to receive cashu tokens that include DLEQ proofs ([ok300]).
- cdk-sqlite: Wallet was not storing dleq proofs ([thesimplekid]).

### Changed
- Updated MSRV to 1.75.0 ([thesimplekid]).
- cdk-sqlite: Do not use `UPDATE OR REPLACE` ([crodas]).
- cdk: Refactor keyset init ([lollerfirst]).
- Feature-gated lightning backends (CLN, LND, LNbits, FakeWallet) for selective compilation ([thesimplekid]).
- cdk-sqlite: Update sqlx to 0.7.4 ([benthecarman]).
- Unifies and optimizes the proof selection algorithm to use Wallet::select_proofs ([davidcaseria]).
- Wallet::send now requires a PreparedSend ([davidcaseria]).
- WalletDatabase proof state update functions have been consolidated into update_proofs_state ([davidcaseria]).
- Moved `MintQuote` and `MeltQuote` from `cashu` to `cdk-common` ([thesimplekid]).

### Added
- Added redb feature to mintd in order to meet MSRV target ([thesimplekid]).
- cdk-sqlite: In memory sqlite database ([crodas]).
- Add `tos_url` to `MintInfo` ([nodlAndHodl]).
- cdk: Add tos_url setter to `MintBuilder` ([thesimplekid]).
- Added optional "request" and "unit" fields to MeltQuoteBolt11Response [NUT Change](https://github.com/cashubtc/nuts/pull/235) ([thesimplekid]).
- Added optional "amount" and "unit" fields to MintQuoteBolt11Response [NUT Change](https://github.com/cashubtc/nuts/pull/235) ([thesimplekid]).
- Compile-time error when no lightning backend features are enabled ([thesimplekid]).
- Add support for sqlcipher ([benthecarman]).
- Payment processor ([thesimplekid]).
- Payment request builder ([thesimplekid]).
- Sends should be initiated by calling Wallet::prepare_send ([davidcaseria]).
- A SendOptions struct controls optional functionality for sends ([davidcaseria]).
- Allow Amount splitting to target a fee rate amount ([davidcaseria]).
- Utility functions for Proofs ([davidcaseria]).
- Utility functions for SendKind ([davidcaseria]).
- Completed checked arithmetic operations for Amount (i.e., checked_mul and checked_div) ([davidcaseria]).

### Removed
- Remove support for Memory Database in cdk ([crodas]).
- Remove `AmountStr` ([crodas]).
- Remove `get_nostr_last_checked` from `WalletDatabase` ([thesimplekid]).
- Remove `add_nostr_last_checked` from `WalletDatabase` ([thesimplekid]).

## [cdk-mintd:v0.7.4](https://github.com/cashubtc/cdk/releases/tag/cdk-mintd-v0.7.4)
### Changed
- cdk-mintd: Update to cdk v0.7.2 ([thesimplekid]).

## [cdk:v0.7.2](https://github.com/cashubtc/cdk/releases/tag/cdk-v0.7.2)
### Fixed
- cdk: Ordering of swap verification checks ([thesimplekid]).

## [cdk-mintd-v0.7.2](https://github.com/cashubtc/cdk/releases/tag/cdk-mintd-v0.7.2)
### Fixed
- cdk-mintd: Fixed mint and melt error on mint initialized with RPC interface disabled ([ok300]).


## [v0.7.1](https://github.com/cashubtc/cdk/releases/tag/v0.7.1)
### Changed
- cdk: Debug print of `Id` is hex ([thesimplekid]).
- cdk: Debug print of mint secret is the hash ([thesimplekid]).
- cdk: Use check_incoming payment on attempted mint or check mint quote ([thesimplekid]).
- cdk-cln: Use `call_typed` for cln rpc calls ([daywalker90]).

### Added
- cdk: Mint builder add ability to set custom derivation paths ([thesimplekid]).

### Fixed
- cdk-cln: Return error on stream error ([thesimplekid]).


## [v0.7.0](https://github.com/cashubtc/cdk/releases/tag/v0.7.0)
### Changed
- Moved db traits to `cdk-common` ([crodas]).
- Moved other common types to `cdk-common` ([crodas]).
- `Wallet::mint` returns the minted `Proofs` and not just the amount ([davidcaseria]).

### Added
- `Token::to_raw_bytes` serializes generic token to raw bytes ([lollerfirst]).
- `Token::try_from` for `Vec<u8>` constructs a generic token from raw bytes ([lollerfirst]).
- `TokenV4::to_raw_bytes()` serializes a TokenV4 to raw bytes following the spec ([lollerfirst]).
- `Wallet::receive_raw` which receives raw binary tokens ([lollerfirst]).
- cdk-mint-rpc: Mint management gRPC client and server ([thesimplekid]).
- cdk-common: cdk specific types and traits ([crodas]).
- cashu: Core types and functions defined in NUTs ([crodas]).

### Fixed
- Multimint unit check when wallet receiving token ([thesimplekid]).
- Mint start up with most recent keyset after a rotation ([thesimplekid]).


## [cdk-v0.6.1, cdk-mintd-v0.6.2](https://github.com/cashubtc/cdk/releases/tag/cdk-mintd-v0.6.1)
### Fixed
- cdk: Missing check on mint that outputs equals the quote amount ([thesimplekid]).
- cdk: Reset mint quote status if in state that cannot continue ([thesimplekid]).

## [v0.6.1](https://github.com/cashubtc/cdk/releases/tag/cdk-v0.6.1)
### Added
- cdk-mintd: Get work-dir from env var ([thesimplekid]).

## [v0.6.0](https://github.com/cashubtc/cdk/releases/tag/v0.6.0)
### Changed
- cdk: Enforce `quote_id` to uuid type in mint ([tdelabro]).
- cdk: Refactor wallet mint connector ([ok300]).

### Added
- cdk: `NUT19` Settings in `NUT06` info ([thesimplekid]).
- cdk: `NUT17` Websocket support for wallet ([crodas]).
- cdk-axum: Redis cache backend ([crodas]).
- cdk-mints: Get mint settings from env vars ([thesimplekid]).
- cdk-axum: HTTP compression support ([ok300]).

### Fixed
- cdk-sqlite: Keyset counter was overwritten when keyset was fetched from mint ([thesimplekid]).
- cdk-cli: On `mint` use `unit` from cli args ([thesimplekid]).
- cdk-cli: On `restore` create `wallet` if it does not exist ([thesimplekid]).
- cdk: Signaling support for optional nuts ([thesimplekid]).
- cdk-phd: Check payment has valid uuid ([thesimplekid]).

## [v0.5.0](https://github.com/cashubtc/cdk/releases/tag/v0.5.0)
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
- cdk: `Id` to `u32` changed from `TryFrom` to `From` ([vnprc]).


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
- cdk-mintd: Add swagger docs ([ok300]).
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




## [v0.4.0](https://github.com/cashubtc/cdk/releases/tag/v0.4.0)
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
- cdk(mint): Use checked addition on amount to ensure there is no overflow ([thesimplekid]).

### Removed
- cdk(wallet): Removed CDK wallet error ([thesimplekid]).
- cdk(mint): Removed CDK mint error ([thesimplekid]).


## [v0.3.0](https://github.com/cashubtc/cdk/releases/tag/v0.3.0)
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
- cdk(mint): Typo rename `total_redeame` to `total_redeemed` ([vnprc]).
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
- cdk: Add `MintUrl` that sanitizes mint url by removing trailing `/` ([cjbeery24]).
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

## [v0.2.0](https://github.com/cashubtc/cdk/releases/tag/v0.2.0)
### Summary
This release introduces TokenV4, which uses CBOR encoding as the default token format. It also includes fee support for both wallet and mint operations.

When sending, the sender can choose to include the necessary fee to ensure that the receiver can redeem the full sent amount. If this is not done, the receiver will be responsible for the fee.

Additionally, this release introduces a Mint binary cdk-mintd that uses the cdk-axum crate as a web server to create a full Cashu mint. When paired with a Lightning backend, currently implemented as Core Lightning, it is included in this release as cdk-cln.

### Changed
- cdk(wallet): `wallet:receive` will not claim `proofs` from a mint other than the wallet's mint ([thesimplekid]).
- cdk(NUT00): `Token` is changed from a `struct` to `enum` of either `TokenV4` or `Tokenv3` ([thesimplekid]).
- cdk(NUT00): Rename `MintProofs` to `TokenV3Token` ([thesimplekid]).
- cdk(wallet): Additional arguments in `send` `send_kind` and `include_fees` for control of how to handle fees in a send ([thesimplekid]).
- cdk(wallet): Additional arguments in `create_swap` `include_fees` for control of if fees to redeem the send proofs are included in send amount ([thesimplekid]).

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
- cdk: NUT06 deserialize `MintInfo` ([thesimplekid]).


## [v0.1.1](https://github.com/cashubtc/cdk/releases/tag/v0.1.1)
### Changed
- cdk(wallet): `wallet::total_pending_balance` does not include reserved proofs ([thesimplekid]).

### Added
- cdk(wallet): Added get reserved proofs ([thesimplekid]).

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
[crodas]: https://github.com/crodas
[tdelabro]: https://github.com/tdelabro
[daywalker90]: https://github.com/daywalker90
[nodlAndHodl]: https://github.com/nodlAndHodl
[benthecarman]: https://github.com/benthecarman
[Darrell]: https://github.com/Darrellbor
[asmo]: https://github.com/asmogo
[gudnuf]: https://github.com/gudnuf
[codingpeanut157]: https://github.com/codingpeanut157
[erik]: https://github.com/swedishfrenchpress
