# AGENTS.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CDK (Cashu Development Kit) is a collection of Rust crates implementing the [Cashu](https://github.com/cashubtc) e-cash protocol. It provides:
- **Libraries** for building Cashu-compatible wallets and mints
- **Binaries** including a mint server (`cdk-mintd`) and wallet CLI (`cdk-cli`)
- **Foreign Function Interface** (FFI) for binding to other languages (Python, Swift, Kotlin)

**Cashu Protocol**: Implements NUTs (Nostr Upgrade Tokens) 00-25, including payment methods (BOLT11, BOLT12), signing conditions, HTLCs, and deterministic secrets.

## Architecture Overview

### Crate Dependency Layers

The project follows a clean layered architecture:

1. **Protocol Foundation** - `cashu` crate
   - Cashu protocol types (NUT00-25), serialization
   - Cryptographic primitives (DHKE, blind signatures)
   - Invoice handling (BOLT11, BOLT12)
   - No CDK dependencies; portable and foundational

2. **Abstraction Layer** - `cdk-common` crate
   - Database trait definitions (`KeysDatabase`, `QuotesDatabase`, `ProofsDatabase`, `SignaturesDatabase`, `SagaDatabase`, `KVStoreDatabase`)
   - Payment processor trait (`MintPayment`) - the key abstraction for Lightning backends
   - Shared error types, event streaming abstractions
   - Comprehensive shared types for mint/wallet operations

3. **Core Logic** - `cdk` crate
   - `Mint` struct: keysets, melt/swap/mint operations, saga pattern for crash-safe transactions
   - `Wallet` struct: payment requests, token handling
   - Event subscriptions and WebSocket support
   - Optional OIDC authentication

### Storage Backend Crates

All implement the database traits from `cdk-common`:

- **`cdk-sqlite`**: SQLite (single-file, good for wallets and development)
- **`cdk-postgres`**: PostgreSQL (production mints with connection pooling and scalability)
- **`cdk-redb`**: Embedded Rust key-value store (wasm-compatible, zero-copy reads)
- **`cdk-sql-common`**: Shared SQL utilities for SQL-based backends

Key architectural insight: Backends are swappable through trait objects at runtime. The `Mint` struct holds multiple payment processors in a HashMap keyed by currency unit.

### Lightning Network Backend Crates

All implement the `MintPayment` trait from `cdk-common`:

- **`cdk-fake-wallet`**: Testing/demo backend with auto-fulfillment (never use in production)
- **`cdk-lnbits`**: Remote LNbits instance via HTTP API
- **`cdk-cln`**: Native CLN (Core Lightning) integration via gRPC
- **`cdk-lnd`**: Native LND integration via gRPC with TLS support
- **`cdk-ldk-node`**: Embedded LDK Lightning node with built-in HTTP server

Payment processors are async-first and stream events via `futures::Stream` for WebSocket subscriptions.

### Supporting Crates

- **`cdk-common`**: Trait definitions and shared types (foundational)
- **`cdk-signatory`**: Cryptographic operations and key management (embeddable or remote gRPC)
- **`cdk-prometheus`**: Prometheus metrics integration
- **`cdk-axum`**: HTTP framework utilities (WebSocket, Redis caching, OpenAPI)
- **`cdk-ffi`**: UniFFI language bindings (Python, Swift, Kotlin)
- **`cdk-mint-rpc`**: Mint administration gRPC interface
- **`cdk-payment-processor`**: Payment processing utilities
- **`cdk-integration-tests`**: Integration test suite

### Application Binaries

- **`cdk-mintd`**: Full-featured mint server (configurable storage and Lightning backend)
- **`cdk-cli`**: Wallet CLI tool

## Development Setup

### Quick Start

```bash
# Install Nix (optional but recommended for consistent environment)
# See DEVELOPMENT.md for detailed instructions

# With Nix
nix develop -c $SHELL

# Without Nix, install:
# - Rust 1.85.0+ (via rustup)
# - protobuf compiler (protoc)
# - typos CLI tool
```

### Common Commands

Use the Justfile for common development tasks:

```bash
# Building and checking
just build              # Build all crates
just check             # Check all crates (faster than build)
just b                 # Alias for build
just c                 # Alias for check

# Code quality
just format            # Format code (stable rustfmt)
just clippy            # Run clippy lints
just clippy-fix        # Auto-fix clippy issues
just typos             # Check for typos
just typos-fix         # Fix typos

# Testing
just test              # Run library and unit tests (excludes postgres)
just test-pure [db]    # Run pure integration tests (memory|redb|sqlite default: memory)
just test-all [db]     # Comprehensive test suite with integration tests
just mutants           # Run mutation testing on a specific crate
just mutants-cashu     # Mutation tests on cashu crate
just mutants-quick     # Quick mutations on changed files since HEAD

# Testing with Nutshell
just test-nutshell     # Test against Nutshell reference implementation

# Regtest environment (local Bitcoin + Lightning nodes + CDK mints)
just regtest [db]      # Start interactive regtest with mprocs TUI
just regtest-status    # Show regtest environment status
just regtest-logs      # Show regtest environment logs

# Final pre-commit check
just final-check       # Format + clippy + typos + tests

# Documentation
just check-docs        # Check doc generation
just docs-strict       # Build docs with strict warnings

# FFI (Foreign Function Interface)
just ffi-generate python [--release]  # Generate Python bindings
just ffi-generate swift [--release]   # Generate Swift bindings
just ffi-generate-all                 # Generate all language bindings
```

### Key Testing Patterns

The codebase uses environment variables to customize test behavior:

```bash
# Run tests against different database backends
CDK_TEST_DB_TYPE=memory cargo test      # In-memory (default)
CDK_TEST_DB_TYPE=sqlite cargo test      # SQLite
CDK_TEST_DB_TYPE=redb cargo test        # Redb

# Run specific test with output
cargo test -p cdk-integration-tests --test integration_tests_pure -- --nocapture

# Single-threaded test (some integration tests require this)
cargo test --test test_name -- --test-threads=1
```

## Code Organization

### Workspace Structure

```
crates/
├── cashu/                      # Protocol types and crypto (no CDK deps)
├── cdk-common/                 # Trait definitions and shared types
├── cdk/                        # Core wallet and mint logic
├── cdk-sqlite/                 # SQLite storage backend
├── cdk-postgres/               # PostgreSQL storage backend
├── cdk-redb/                   # Redb storage backend
├── cdk-sql-common/             # Shared SQL utilities and migrations
├── cdk-cln/                    # Core Lightning Network backend
├── cdk-lnd/                    # LND Lightning backend
├── cdk-lnbits/                 # LNbits HTTP backend
├── cdk-ldk-node/               # Embedded LDK Lightning backend
├── cdk-fake-wallet/            # Testing backend
├── cdk-signatory/              # Crypto and key management
├── cdk-prometheus/             # Metrics integration
├── cdk-axum/                   # HTTP server utilities
├── cdk-ffi/                    # Foreign Function Interface
├── cdk-mint-rpc/               # Mint administration gRPC
├── cdk-payment-processor/      # Payment processing utilities
├── cdk-integration-tests/      # Integration test suite
├── cdk-cli/                    # Wallet CLI (binary)
└── cdk-mintd/                  # Mint server (binary)
```

### Key Module Patterns

**Mint Flow**:
- Core logic in `cdk/src/mint/`: keyset management, token minting, melting, swapping
- Saga pattern for crash-safe multi-step operations
- Storage abstraction via `cdk-common` database traits

**Wallet Flow**:
- Core logic in `cdk/src/wallet/`: token selection, payments, subscriptions
- Payment request handling with event streaming
- Subscription support via `futures::Stream` and WebSocket

**Database Transactions**:
- All database operations use trait bounds from `cdk-common::database`
- Multiple implementations: SQLite, PostgreSQL, Redb
- SQL migrations in `cdk-sql-common/src/{mint,wallet}/migrations/`

**Lightning Integration**:
- Payment processors implement `MintPayment` trait
- Async event streaming for quote status updates
- Each backend handles quote fulfillment and verification

### Feature Flags

Key workspace features:

```toml
# In Cargo.toml
cdk:
  mint         # Enable mint functionality
  wallet       # Enable wallet functionality
  auth         # Enable OIDC authentication
  nostr        # Nostr integration (requires wallet)
  bip353       # BIP353 DNS name support
  tor          # Tor support
  prometheus   # Prometheus metrics
  http_subscription  # HTTP-based subscriptions
  swagger      # OpenAPI documentation

cashu:
  mint         # Mint-specific types
  wallet       # Wallet-specific types
  auth         # Authentication support (OIDC)
  swagger      # OpenAPI support
```

## Important Architecture Decisions

### Trait-Based Plugin System

All major components use trait-based abstractions:
- Database backends implement `KeysDatabase`, `QuotesDatabase`, etc.
- Payment processors implement `MintPayment`
- Swappable at runtime via trait objects

This design enables:
- Mixing database backends (e.g., PostgreSQL for quotes, SQLite for proofs)
- Testing with fake wallet before production Lightning
- Easy addition of new backends

### Saga Pattern for Crash Safety

Minting and melting operations use the saga pattern via `SagaDatabase`:
- State is persisted at each step
- Recovers from crashes mid-operation
- Critical for mint reliability

### Keyset Caching

Keyset updates use `Arc<ArcSwap<>>` for lock-free concurrent reads:
- Allows mints to rotate keys without blocking operations
- Pattern: observe current keyset, use it for operation

### Async-First Design

- All I/O is non-blocking with Tokio
- Database operations are async traits
- Payment processors stream events asynchronously
- Compatible with WebSocket subscriptions and polling

## Testing Strategy

### Test Layers

1. **Unit Tests**: In-crate tests for isolated functions (`#[test]` and `#[tokio::test]`)
2. **Integration Tests**: Multi-crate flows in `cdk-integration-tests/`
3. **Pure Integration Tests**: No external services required
4. **Lightning Integration Tests**: Against real/fake Lightning backends

### Test Database Support

Most tests use in-memory storage for speed. Multi-database testing uses:

```bash
# Run same tests against all backends
just test-all memory   # In-memory
just test-all sqlite   # SQLite file
just test-all redb     # Redb
```

### Mutation Testing

Validate test quality by introducing small code changes:

```bash
just mutants cashu     # Test cashu crate thoroughly
just mutants-diff      # Only test changed code since HEAD
```

## Important Notes

### Database Migrations

When modifying database schemas:

1. Create migration files using: `just new-migration {mint|wallet} <name>`
2. Add SQL to the generated migration file
3. Migrations in `cdk-sql-common/src/{mint,wallet}/migrations/` are auto-applied

### Code Formatting

The project accepts **both stable and nightly rustfmt** formatting:
- CI checks with stable rustfmt
- Nightly formatting is automated via GitHub Actions nightly job
- Use `just format` to format with stable

### MSRV (Minimum Supported Rust Version)

Currently **1.85.0**. Check `Cargo.toml`:
```toml
[workspace.package]
rust-version = "1.85.0"
```

Keep this in mind when using newer Rust features.

### Workspace Lints

The workspace enforces strict linting:
- `unsafe_code = forbid`
- Clippy pedantic and nursery
- Missing docs warnings
- See `Cargo.toml` `[workspace.lints]` for full list

## Running the Full Stack

### Interactive Regtest Environment

```bash
just regtest sqlite        # Start local Bitcoin + 4 LN nodes + 2 CDK mints

# In another terminal, explore:
just ln-cln1 info         # CLN node 1 info
just ln-lnd2 getinfo      # LND node 2 info
just btc getblockcount    # Bitcoin regtest
just mint-info            # Mint information
just mint-test            # Run integration tests against regtest
```

See `REGTEST_GUIDE.md` for comprehensive regtest documentation.

## References

- **Development Guide**: `DEVELOPMENT.md` - Detailed setup and workflow
- **Regtest Guide**: `REGTEST_GUIDE.md` - Full regtest environment documentation
- **Security Policy**: `SECURITY.md`
- **Changelog**: `CHANGELOG.md`
- **Cashu Protocol**: https://github.com/cashubtc/nuts (NUT specifications)
- **Cashu Specs**: https://cashu.space
