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
| **Swift** | `bindings/swift/` | Active | CI workflow | `just test-swift` |
| **Kotlin** | `bindings/kotlin/` | Active | `just binding-kotlin` | `just test-kotlin` |
| **Go** | `bindings/go/` | Active | `just binding-go` | `just test-go` |
| **Python** | `bindings/python/` | Active | `just binding-python` | `just test-python` |

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

### Python

- **Distribution name:** `cdk-python` (import package: `cdk`)
- **Rust crate:** `cdk-ffi-python`
- **Binding generator:** uniffi-bindgen's in-tree Python backend
- Follows the cdk-dart model: the monorepo syncs `bindings/python/` to a release
  branch on cdk-python, then **clones that branch and builds `rust/` from it**,
  so CI compiles exactly what a downstream user compiles rather than the
  workspace copy
- The built artifacts are then **committed into cdk-python** and attached to its
  release: the generated `src/cdk/cdk_ffi.py` and prebuilt wheels under
  `wheels/`, the way cdk-go commits its bindings and native libraries
- `rust/` is synced with its `cdk-ffi` dependency rewritten to a crates.io
  version, so anyone can rebuild the wheel with `./build.sh` without cloning
  this monorepo. Because CI builds that same pinned crate, the published wheel
  and a local rebuild come from the same source
- Wheels go to PyPI and are attached to the cdk-python GitHub release, for
  Linux (`manylinux_2_28`, x86_64 and aarch64), macOS (arm64 and x86_64) and
  Windows (x86_64)
- Every wheel is tagged `py3-none-<platform>`: the library is loaded with
  `ctypes`, not linked against the CPython ABI
- uniffi names the loaded library after the `cdk-ffi` namespace, so the built
  `libcdk_ffi_python.*` is renamed to `libcdk_ffi.*` inside the package
- Tests are pytest-based under `bindings/python/tests/`, porting the same
  scenarios the Dart, Go, Kotlin and Swift suites run; the mint-backed ones are
  skipped unless `CDK_PYTHON_TEST_MINT_URL` is set
- Runnable examples live in `bindings/python/examples/`; `just examples-python`
  runs the offline ones

## Planned targets

| Language | Status | Notes |
|----------|--------|-------|
| **React Native** | Planned | — |

Adding a new language binding involves creating a `bindings/<lang>/` directory
with a thin wrapper crate and the appropriate build tooling.

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

# Python
just binding-python  # Build the wheel
just test-python     # Install it into a clean venv and run tests
just examples-python # Run the offline examples
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

All `ffi-release-*` recipes dispatch with `--ref v<VERSION>`, so GitHub loads
the workflow definitions from the release tag. The unified workflow's relative
calls load the language workflows from that same commit. This keeps backport
releases, such as those from `0.17.x`, on their matching build workflows.
The `release_tag` and `cdk_ref` inputs control the source checkout; they do not
select the workflow definition.

For an existing release tag, you can dispatch directly:

```bash
gh workflow run ffi-publish-all.yml --repo cashubtc/cdk \
  --ref v0.17.0 --field release_tag=v0.17.0
```

If a workflow fix was added to `0.17.x` after the tag was created, use
`--ref 0.17.x` with the same `release_tag` to run the corrected branch workflow
against the tagged sources. In the Actions UI, select `0.17.x` in **Use workflow
from** when dispatching manually. Backport workflow fixes before tagging future
releases so the tag contains the matching workflows.

### Nightly bindings

The **FFI - Nightly Bindings** workflow (`.github/workflows/ffi-nightly.yml`)
runs daily at 02:17 UTC and can also be started manually. It builds the exact
current `main` commit and creates an immutable GitHub prerelease in each binding
repository:

- `cashubtc/cdk-dart`
- `cashubtc/cdk-go`
- `cashubtc/cdk-kotlin`
- `cashubtc/cdk-swift`
- `cashubtc/cdk-python`

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

# Python
just ffi-release-python 0.17.0
```

### Prerequisites

- The version tag (e.g. `v0.17.0`) must exist on the remote
- The tag must contain the dispatchable FFI workflows and their reusable workflows
- Dart, Go, Kotlin, Python, and Swift stable release workflows check out
  `refs/tags/<release_tag>` and reject `cdk_ref` values that differ from
  `release_tag`
- The `FFI_DEPLOY_KEY` GitHub secret must have write access to `cdk-dart`,
  `cdk-go`, `cdk-kotlin`, `cdk-python`, and `cdk-swift` repos
- Kotlin publishing requires the `SONATYPE_USERNAME`, `SONATYPE_PASSWORD`,
  `SIGNING_KEY`, and `SIGNING_PASSWORD` GitHub secrets
- The `CDK_DART_REPO`, `CDK_GO_REPO`, `CDK_KOTLIN_REPO`, `CDK_PYTHON_REPO`, and
  `CDK_SWIFT_REPO` GitHub Actions variables must point to the target binding
  repositories
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
