# AGENTS.md - Cashu Development Kit (CDK)

Rust workspace (edition 2021) implementing the Cashu e-cash protocol.
23 crates in `crates/`, toolchain pinned to 1.93.0, MSRV 1.85.0.

## Build / Check / Test / Lint Commands

```bash
# Build
cargo build --workspace --all-targets          # or: just build (alias: just b)
cargo check --workspace --all-targets          # or: just check (alias: just c)

# Lint
cargo fmt --all -- --check                     # format check
cargo clippy --workspace --all-targets -- -D warnings  # or: just clippy
typos                                          # spell checker

# Test - all unit tests (excludes postgres, needs running instance)
cargo test --lib --workspace --exclude cdk-postgres    # or: just test

# Test - single crate
cargo test -p cashu
cargo test -p cdk
cargo test -p cdk-common

# Test - single test function (use substring match)
cargo test -p cashu -- test_name_substring

# Test - single integration test file
cargo test -p cdk-integration-tests --test integration_tests_pure -- --test-threads 1
cargo test -p cdk-integration-tests --test test_swap_flow -- --test-threads 1
cargo test -p cdk-integration-tests --test wallet_saga -- --test-threads 1
cargo test -p cdk-integration-tests --test mint

# Test - pure integration tests with specific DB backend
CDK_TEST_DB_TYPE=memory cargo test -p cdk-integration-tests --test integration_tests_pure -- --test-threads 1
CDK_TEST_DB_TYPE=sqlite cargo test -p cdk-integration-tests --test test_swap_flow -- --test-threads 1

# Doc tests
cargo test --doc

# WASM check
cargo check -p cdk --target wasm32-unknown-unknown
```

## Workspace Lint Rules (Cargo.toml)

These are enforced workspace-wide:
- `unsafe_code = "forbid"` -- no unsafe code anywhere
- `unwrap_used = "deny"` -- never use `.unwrap()` in non-test code; use `?`, `.expect("reason")`, or pattern match
- `missing_docs = "warn"` -- all public items should have doc comments
- `missing_debug_implementations = "warn"`
- `missing_panics_doc = "warn"`
- `use_debug = "warn"` -- avoid `{:?}` in non-debug contexts

## Code Style

### Formatting (rustfmt.toml)

- 4-space indentation
- `imports_granularity = "Module"` -- merge imports from same module
- `group_imports = "StdExternalCrate"` -- blank lines between std/external/local groups
- Run `cargo fmt --all` before committing

### Import Order

```rust
// 1. core/std
use std::collections::HashMap;
use std::sync::Arc;

// 2. External crates
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

// 3. Submodule declarations (if any)
mod x;
mod y;

// 4. Crate-internal imports
use crate::error::Error;
use super::something;
use self::x::Thing;       // always use self:: prefix for submodule imports
```

### Trait Bounds in `where` Clauses

```rust
// GOOD - bounds in where clause
fn new<N, T>(name: N, title: T) -> Self
where
    N: Into<String>,
    T: Into<String>,
{ ... }

// BAD - no inline bounds
fn new<N: Into<String>, T: Into<String>>(name: N, title: T) -> Self { ... }
```

### Use `Self` Over Type Name

In impl blocks, always use `Self` instead of repeating the type name.

### Derive Order

For public types, derive in this order: `Debug, Clone, Copy, PartialEq, Eq, Hash`.
Derive `Default` when a reasonable default exists.

### Logging / Tracing

Always use full path -- never import logging macros:
```rust
// GOOD
tracing::info!("Starting mint");
tracing::warn!("Connection lost: {}", reason);

// BAD
use tracing::warn;
warn!("Connection lost");
```

Exception: `use tracing::instrument;` is imported for the `#[instrument]` attribute.
Most public async methods should have `#[instrument(skip_all)]`.

### String Conversion

Prefer `.to_string()` / `.to_owned()` over `.into()` / `String::from()`.

### Control Flow

- Use `match` when both arms have logic; use `if let` only when one arm is empty.
- Prefer `match` over `if let ... else`.

### Modules

- Always use `mod x;` with a separate file, never inline `mod x { ... }`.
- Exception: `#[cfg(test)] mod tests { ... }` and `#[cfg(bench)] mod benches { ... }` are inline.

### fmt Imports

Import the module, not individual items:
```rust
use core::fmt;
impl fmt::Display for MyType { ... }
```

## Error Handling

- Define errors with `thiserror` (`#[derive(Debug, Error)]`), not `anyhow`.
- Use `?` operator for propagation; add context with `.map_err()` when needed.
- Per-crate errors implement `From<CrateError> for cdk_common::Error` (or the relevant domain error).
- Use structured error variants with named fields for rich context:
  ```rust
  #[error("Maximum inputs exceeded: {actual} provided, max {max}")]
  MaxInputsExceeded { actual: usize, max: usize },
  ```

## Async Patterns

- Runtime: `tokio`. Tests use `#[tokio::test]`.
- Trait definitions use `#[async_trait]` (with `#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]` for WASM-compatible traits).
- Dynamic dispatch via `Arc<dyn Trait + Send + Sync>` with `Dyn*` type aliases (e.g., `DynMintDatabase`).

## Naming Conventions

- Structs/Enums: `PascalCase` (`Mint`, `SwapSaga`, `ProofsFeeBreakdown`)
- Functions: `snake_case`, verb-leading (`process_swap_request`, `verify_inputs`)
- Builder methods: `with_*` prefix (`with_name()`, `with_auth()`)
- Constants: `SCREAMING_SNAKE_CASE`
- Type aliases for dynamic dispatch: `Dyn*` prefix (`DynMintDatabase`)

## Project Structure

23 workspace crates in `crates/`, grouped by role:

**Foundation**
- `cashu` -- core Cashu protocol types, crypto, NUT specs (`nuts/nut00`–`nut27`)
- `cdk-common` -- shared traits (`MintDatabase`, `WalletDatabase`, `MintPayment`), error types, pub/sub

**Core SDK**
- `cdk` -- main library: mint logic (`src/mint/`), wallet logic (`src/wallet/`), feature-gated (`mint`, `wallet`)
- `cdk-http-client` -- HTTP/WebSocket client for wallet-to-mint communication

**Storage backends**
- `cdk-sql-common` -- shared SQL query logic (used by SQLite and Postgres)
- `cdk-sqlite` -- SQLite storage (includes in-memory mode for testing)
- `cdk-postgres` -- PostgreSQL storage (requires running instance)
- `cdk-redb` -- Redb embedded storage (wallet only)

**Lightning backends**
- `cdk-cln` -- Core Lightning (CLN)
- `cdk-lnd` -- LND
- `cdk-lnbits` -- LNBits
- `cdk-ldk-node` -- LDK Node (embedded Lightning, includes web management UI)
- `cdk-fake-wallet` -- always-succeeding fake backend for testing

**Services / RPC**
- `cdk-signatory` -- blind signature creation (embedded or remote gRPC)
- `cdk-mint-rpc` -- mint management gRPC service + CLI
- `cdk-payment-processor` -- payment processing gRPC service

**HTTP server**
- `cdk-axum` -- Axum-based HTTP server for the mint (NUT endpoints, WebSocket, auth, caching)

**Binaries**
- `cdk-mintd` -- mint daemon (wires all crates together)
- `cdk-cli` -- CLI wallet with subcommands (mint, melt, send, receive, etc.)

**Other**
- `cdk-ffi` -- UniFFI bindings for cross-language use
- `cdk-npubcash` -- npub.cash integration
- `cdk-prometheus` -- Prometheus metrics exporter
- `cdk-integration-tests` -- full-stack integration tests

**Non-workspace**
- `fuzz/` -- fuzzing targets (20 fuzz harnesses, excluded from workspace)
- `misc/` -- helper scripts, Docker configs, Keycloak setup, Grafana dashboards

### Dependency Flow

```
cashu  (protocol types, crypto, NUT specs)
  └─ cdk-common  (traits, errors, pub/sub)
       ├─ cdk  (mint + wallet business logic, sagas)
       │    ├─ cdk-axum  (HTTP server)
       │    ├─ cdk-signatory  (signing)
       │    ├─ cdk-payment-processor
       │    ├─ cdk-mint-rpc
       │    └─ cdk-http-client  (wallet-side)
       ├─ Storage: cdk-sql-common → cdk-sqlite, cdk-postgres
       │           cdk-redb
       └─ Lightning: cdk-cln, cdk-lnd, cdk-lnbits, cdk-ldk-node, cdk-fake-wallet
```

### Key Files for Common Tasks

| Task | Where to look |
|---|---|
| Protocol types / NUT specs | `crates/cashu/src/nuts/` |
| Database traits | `crates/cdk-common/src/database/` |
| Payment backend trait | `crates/cdk-common/src/payment.rs` |
| Error definitions | `crates/cdk-common/src/error.rs` |
| Mint business logic | `crates/cdk/src/mint/` |
| Wallet business logic | `crates/cdk/src/wallet/` |
| HTTP API handlers | `crates/cdk-axum/src/router_handlers.rs` |
| Mint daemon config/setup | `crates/cdk-mintd/src/config.rs`, `setup.rs` |
| Integration test setup | `crates/cdk-integration-tests/src/init_*.rs` |
| Workspace deps & lint rules | Root `Cargo.toml` |
| Build/test recipes | `justfile` |

## Commit Style

Conventional commits: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`.

## Docs & References

| Document | Path |
|---|---|
| Developer setup & workflow | `DEVELOPMENT.md` |
| Code style guide | `CODE_STYLE.md` |
| Regtest testing guide | `REGTEST_GUIDE.md` |
| Security policy | `SECURITY.md` |
| v0.15 migration guide | `docs/migrations/v0.15.md` |
| Wallet architecture | `crates/cdk/src/wallet/README.md` |
| Mint daemon example config | `crates/cdk-mintd/example.config.toml` |
| LDK Node networking | `crates/cdk-ldk-node/NETWORK_GUIDE.md` |
| TLS/certificate setup | `crates/cdk-mint-rpc/CERTIFICATES.md` |
| Wallet SDK examples (21) | `crates/cdk/examples/` |
