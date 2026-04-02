# CDK Language Bindings

Language bindings for the [Cashu Development Kit][cdk], exposing the CDK Wallet
and its associated traits to non-Rust languages through FFI.

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

The bindings follow a two-tier architecture:

```
crates/cdk-ffi/          Core FFI crate — defines all exported types,
                         traits, and functions using UniFFI proc-macros.

bindings/<lang>/rust/    Language-specific wrapper crate — thin layer that
                         re-exports cdk-ffi and adds per-language UniFFI
                         configuration (module names, package names, etc.).

bindings/<lang>/         Language project — generated sources, tests, and
                         build tooling for the target language.
```

The core `cdk-ffi` crate (`crates/cdk-ffi/`) contains:
- FFI-compatible wrappers for wallet operations, database traits, token handling,
  and type conversions
- `#[uniffi::export]` annotations that produce cross-language metadata
- A `WalletDatabase` callback interface so foreign languages can provide their
  own storage backend
- `WalletStore` enum with factory functions (`sqliteWalletStore`,
  `postgresWalletStore`, `customWalletStore`) for easy database setup

Each language wrapper crate is a single `pub use cdk_ffi::*;` re-export with its
own `uniffi.toml` controlling language-specific code generation.

## Current targets

| Language | Directory | Status | Build | Test |
|----------|-----------|--------|-------|------|
| **Dart** | `bindings/dart/` | Active | `just binding-dart` | `just test-dart` |
| **Swift** | `bindings/swift/` | Active | `just binding-swift` | `just test-swift` |
| **React Native** | `bindings/react-native/` | Active | `just binding-react-native` | `just test-react-native` |

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
- `Package.swift` is generated at the repo root by `generate-bindings.sh`
- Swift sources are generated into `bindings/swift/Sources/Cdk/`

### React Native

- **Package name:** `@cashudevkit/cdk-react-native`
- **Rust crate:** `cdk-ffi-react-native`
- **Binding generator:** [uniffi-bindgen-react-native][uniffi-rn] v0.30.0-1
- Uses `ubrn` CLI to build iOS/Android native libraries and generate TypeScript bindings
- Generated TypeScript sources go into `bindings/react-native/src/generated/`
- React Native Turbo Module architecture (New Architecture)

## Planned targets

| Language | Status | Notes |
|----------|--------|-------|
| **Kotlin** | Configured | UniFFI config exists in `crates/cdk-ffi/uniffi.toml` (package: `org.cashudevkit`) |
| **Python** | Configured | UniFFI config exists in `crates/cdk-ffi/uniffi.toml` |

Kotlin and Python already have UniFFI configuration in the core FFI crate. Adding
a new language binding involves creating a `bindings/<lang>/` directory with a
thin wrapper crate and the appropriate build tooling.

## Building and testing

Prerequisites: Rust toolchain, and the target language SDK.

```bash
# Dart
just binding-dart    # Generate bindings
just test-dart       # Run tests

# Swift (macOS only)
just binding-swift   # Build XCFramework + generate bindings
just test-swift      # Run tests

# React Native
just binding-react-native  # Build native libs + generate TS bindings
just test-react-native     # Run Jest tests
```

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
[uniffi-rn]: https://github.com/jhugman/uniffi-bindgen-react-native
