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

That extends to the released artifacts. The four wrapper crates are workspace
members, so every native library and every generated binding is compiled from
this checkout with `cargo --locked` against the root `Cargo.lock` and the
`release-ffi` profile. There is no dependency pin anywhere in that build: the
release tag decides which tree is compiled, and it is chosen before the build
starts.

The binding repositories carry build artifacts, not sources. They receive
generated bindings, the platform project files and the compiled libraries, and
they carry the GitHub release. They contain no Rust crate and nothing in them is
buildable, so no binding release goes through crates.io.

Every repository commits its compiled libraries, so whatever a tag points at
contains everything that tag delivers. That matters because not every language
has a second channel: Kotlin's release uploads no assets and nightlies skip
Maven Central, so a nightly would otherwise deliver nothing at all.

| Language | Committed at | Also published as |
|---|---|---|
| Swift | `CashuDevKitFFI.xcframework.zip` at the root, a local `binaryTarget(path:)` | a release asset |
| Dart | `prebuilt/<target-triple>/` | per-platform release archives |
| Kotlin | `cdk-android/src/main/jniLibs/<abi>/` | the Maven Central AAR |
| Go | `bindings/cdkffi/native/<goos>_<goarch>/` | a release tarball |

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

Every binding builds from a clone of this monorepo; the publishing repositories
carry artifacts only. Each language has one recipe that generates the bindings
and builds the native library, and one that tests them.

```bash
# Dart
just binding-dart    && just test-dart

# Go
just binding-go      && just test-go

# Kotlin
just binding-kotlin  && just test-kotlin

# Swift (macOS only)
just binding-swift   && just test-swift
```

Prerequisites: `just`, plus [nix](https://nixos.org/download) with flakes enabled
for Dart, Go and Kotlin, whose `binding-*` recipes wrap
`nix build .#<lang>-bindings` and get their Rust toolchain from nix. Swift is the
exception: `binding-swift` is plain cargo and needs a rustup toolchain and Xcode
instead. The test recipes additionally need that language's SDK on PATH, which
`nix develop .#bindings` provides for Dart, Go and Kotlin.

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
Rust crates, but the two are independent. Binding builds compile `cdk-ffi` from
this repository at the release commit, so `ffi-release-all` needs only the tag
and green CI on it. It can run before, after, or without the crates.io publish,
and it can be re-run on its own if a language fails.

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

Nightly tags include the UTC date and short source commit, for example
`v0.18.0-nightly.20260801.g1a2b3c4`. The release notes link the full CDK source
commit. Stable and nightly builds follow the same path, so a nightly is a real
rehearsal of a release.

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
- The tag must contain the dispatchable FFI workflows and their reusable workflows
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
