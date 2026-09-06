# CDK Language Bindings

Language bindings for the [Cashu Development Kit][cdk], exposing the shared
`cdk::wallet` workflow API to non-Rust languages through UniFFI.

This approach is heavily inspired by [Bark FFI Bindings][bark], particularly its
model for exporting a Rust codebase through [UniFFI][uniffi] and making it
accessible from other languages.

## Monorepo approach

All binding development happens in this directory. Each language binding may have
its own repository for publishing releases and platform-specific packaging, but
**this monorepo is the single source of truth** for the FFI layer and generated
bindings. This keeps bindings as first-class citizens alongside the Rust core:
they evolve together, are tested together, and breakage is caught before it
reaches downstream consumers.

## Architecture

The bindings use a thin packaging layout around the same wallet implementation
used by Rust applications:

```
crates/cdk/              Wallet workflows and advanced protocol operations.

crates/cdk-ffi/          Binding-safe conversion and object-lifetime bridge.

bindings/<lang>/rust/    Language-specific wrapper crate — thin layer that
                         re-exports cdk-ffi and adds per-language UniFFI
                         configuration (module names, package names, etc.).

bindings/<lang>/         Language project — generated sources, tests, and
                         build tooling for the target language.
```

The public wallet contract is centered on `Wallet`, `WalletManager`, request
records, typed payment targets, sessions, durable operation plans, explicit
synchronization, operation discovery, application events, and structured
errors. Deliberate proof/keyset/auth and other protocol controls are excluded
from the default generated API. Build a wrapper with `--features
advanced-wallet` to add `advanced_wallet` and `advanced_manager`; wallet
business logic is not reimplemented here.

`WalletDatabase` remains a callback interface for applications that provide a
custom store, while `WalletStore` selects built-in SQLite/Postgres or custom
storage.

Each language wrapper crate is a single `pub use cdk_ffi::*;` re-export with its
own `uniffi.toml` controlling language-specific code generation.

## Current targets

| Language | Directory | Status | Build | Test |
|----------|-----------|--------|-------|------|
| **Dart** | `bindings/dart/` | Active | `just binding-dart` | `just test-dart` |
| **Swift** | `bindings/swift/` | Active | CI workflow | `just test-swift` |
| **Kotlin** | `bindings/kotlin/` | Active | `just binding-kotlin` | `just test-kotlin` |
| **Go** | `bindings/go/` | Active | `just binding-go` | `just test-go` |

### Dart

- **Package name:** `cdk`
- **Rust crate:** `cdk-ffi-dart`
- **Binding generator:** [uniffi-dart][uniffi-dart] v0.1.0+v0.30.0
- Dart sources are generated into `bindings/dart/lib/src/generated/`
- Post-generation patches are applied by `bindings/dart/rust/uniffi-bindgen.rs`
  to work around uniffi-dart codegen bugs (see doc-comments in that file)

### Swift

- **Module name:** `Cdk` (FFI module: `CashuDevKitFFI`)
- **Rust crate:** `cdk-ffi-swift`
- **Binding generator:** uniffi-bindgen-swift (local, in `bindings/swift/rust/`)
- Builds an XCFramework for iOS (device + simulator) and macOS (arm64 + x86_64)
- `Package.swift` is generated during the CI publish workflow
- Swift sources are generated into `bindings/swift/Sources/Cdk/`

## Planned targets

| Language | Status | Notes |
|----------|--------|-------|
| **Python** | Configured | UniFFI config exists in `crates/cdk-ffi/uniffi.toml` |
| **React Native** | Planned | — |

Python already has UniFFI configuration in the core FFI crate. Adding a new
language binding involves creating a `bindings/<lang>/` directory with a thin
wrapper crate and the appropriate build tooling.

## Building and testing

Prerequisites: Rust toolchain, and the target language SDK.

```bash
# Dart
just binding-dart    # Generate bindings
just test-dart       # Run tests

# Swift (macOS only — build runs in CI via swift-publish workflow)
just test-swift      # Run tests

# Kotlin
just binding-kotlin  # Generate bindings
just test-kotlin     # Run tests

# Go
just binding-go      # Generate bindings
just test-go         # Run tests

# Shared Rust/UniFFI surface
cargo check -p cdk-ffi --all-targets
cargo check -p cdk-ffi --all-targets --features advanced-wallet
```

## Releasing

### All bindings at once

The recommended way to release all FFI bindings is through the unified workflow,
which triggers Dart, Go, Kotlin, and Swift builds in parallel:

```bash
just ffi-release-all 0.17.0
```

This runs the **FFI - Publish All Bindings** GitHub Actions workflow
(`.github/workflows/ffi-publish-all.yml`), which:
- Calls all four language publish workflows as reusable workflows
- Creates the corresponding releases in the separate binding repositories

The `release` just recipe calls `ffi-release-all` automatically after publishing
Rust crates.

### Nightly bindings

The **FFI - Nightly Bindings** workflow (`.github/workflows/ffi-nightly.yml`)
runs daily at 02:17 UTC and can also be started manually. It builds the exact
current `main` commit and creates an immutable GitHub prerelease in each binding
repository:

- `cashubtc/cdk-dart`
- `cashubtc/cdk-go`
- `cashubtc/cdk-kotlin`
- `cashubtc/cdk-swift`

Nightly tags include the UTC date and short source commit, for example
`v0.18.0-nightly.20260801.g1a2b3c4`. The release notes link the full CDK source
commit, and the generated Rust wrapper pins `cdk-ffi` to that exact commit.

The workflow checks each binding repository independently and skips a language
when that CDK commit already has a nightly release. This also allows a later run
to retry only languages missing after a partial failure. Nightlies do not merge
into the binding repositories' default branches and do not publish to Maven
Central or other package registries.

### Individual bindings

Each binding can also be released independently:

```bash
# Dart
just ffi-release-dart 0.17.0

# Kotlin
just ffi-release-kotlin 0.17.0

# Swift
just ffi-release-swift 0.17.0

# Go (separate workflow)
just ffi-release-go 0.17.0
```

### Prerequisites

- The version tag (e.g. `v0.17.0`) must exist on the remote
- Dart, Go, Kotlin, and Swift stable release workflows check out `refs/tags/<release_tag>`
  and reject `cdk_ref` values that differ from `release_tag`
- The `FFI_DEPLOY_KEY` GitHub secret must have write access to `cdk-dart`,
  `cdk-go`, `cdk-kotlin`, and `cdk-swift` repos
- Kotlin publishing requires the `SONATYPE_USERNAME`, `SONATYPE_PASSWORD`,
  `SIGNING_KEY`, and `SIGNING_PASSWORD` GitHub secrets
- The `CDK_DART_REPO`, `CDK_GO_REPO`, `CDK_KOTLIN_REPO`, and `CDK_SWIFT_REPO` GitHub
  Actions variables must point to the target binding repositories
- `CACHIX_AUTH_TOKEN` is optional; when present, Kotlin release builds can use
  the authenticated Cachix cache
- `gh` CLI must be authenticated for just commands

## Credits

- [Bark FFI Bindings][bark] — the architectural model for this binding layer
- [UniFFI][uniffi] — Mozilla's framework for generating cross-language bindings
  from Rust
- [uniffi-dart][uniffi-dart] — community Dart backend for UniFFI
- uniffi-bindgen-swift — local Swift binding generator (in `bindings/swift/rust/`)

[cdk]: https://github.com/cashubtc/cdk
[bark]: https://gitlab.com/ark-bitcoin/bark-ffi-bindings
[uniffi]: https://github.com/mozilla/uniffi-rs
[uniffi-dart]: https://github.com/Uniffi-Dart/uniffi-dart
