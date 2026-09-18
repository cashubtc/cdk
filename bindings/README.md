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

That extends to the released artifacts. The five wrapper crates are workspace
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
| Python | `wheels/` and the generated `src/cdk/cdk_ffi.py` | the PyPI wheels, also release assets |

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
- The native libraries and the wheels are built here, like every other binding:
  `cargo build --locked --profile release-ffi -p cdk-ffi-python` per target,
  then one wheel assembled per platform from `bindings/python/`
- The built artifacts are **committed into cdk-python** and attached to its
  release: the generated `src/cdk/cdk_ffi.py` and prebuilt wheels under
  `wheels/`, the way cdk-go commits its bindings and native libraries. cdk-python
  carries no `rust/`, so nothing there is compiled
- Wheels go to PyPI and are attached to the cdk-python GitHub release, for
  Linux (`manylinux_2_28`, x86_64 and aarch64), macOS (arm64 on 11, x86_64 on
  10.12) and Windows (x86_64)
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

# Python
just binding-python  && just test-python

# Swift (macOS only)
just binding-swift   && just test-swift
```

Prerequisites: `just`, plus [nix](https://nixos.org/download) with flakes enabled
for Dart, Go and Kotlin, whose `binding-*` recipes wrap
`nix build .#<lang>-bindings` and get their Rust toolchain from nix. Python and
Swift are the exceptions: their recipes are plain cargo, so they need a rustup
toolchain rather than nix, plus Python 3.10 or higher and Xcode respectively. The
test recipes additionally need that language's SDK on PATH, which
`nix develop .#bindings` provides for Dart, Go and Kotlin and `nix develop .#ffi`
provides for Python.

## Releasing

### All bindings at once

The recommended way to release all FFI bindings is through the unified workflow,
which triggers Dart, Go, Kotlin, Python and Swift builds in parallel:

```bash
just ffi-release-all 0.17.0
```

This runs the **FFI - Publish All Bindings** GitHub Actions workflow
(`.github/workflows/ffi-publish-all.yml`), which:
- Calls all five language publish workflows as reusable workflows
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
- `cashubtc/cdk-python`

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

# Python
just ffi-release-python 0.17.0
```

These recipes leave the Python workflow's `publish_target` input at `none`, so
they build and attach wheels without touching PyPI. Only `just ffi-release-all`
sets it, to `pypi` for a stable tag and `none` for a pre-release. To reach a
registry from a single-language run, dispatch the workflow directly with the
input set:

```bash
gh workflow run python-publish.yml --repo cashubtc/cdk \
  --ref v0.17.0 --field release_tag=v0.17.0 --field publish_target=test-pypi
```

### Prerequisites

- The version tag (e.g. `v0.17.0`) must exist on the remote
- The tag must contain the dispatchable FFI workflows and their reusable workflows
- Dart, Go, Kotlin, Python, and Swift stable release workflows check out
  `refs/tags/<release_tag>` and reject `cdk_ref` values that differ from
  `release_tag`
- The `FFI_DEPLOY_KEY` GitHub secret must have write access to `cdk-dart`,
  `cdk-go`, `cdk-kotlin`, `cdk-python`, and `cdk-swift` repos. The workflows use
  it both as a git credential and as `GH_TOKEN`, so it needs enough scope to
  push the `release/<tag>` branch, squash-merge it into the default branch and
  delete it, and to run `gh release create`, which creates the tag and uploads
  the release assets
- Kotlin publishing requires the `SONATYPE_USERNAME`, `SONATYPE_PASSWORD`,
  `SIGNING_KEY`, and `SIGNING_PASSWORD` GitHub secrets
- Python publishing to PyPI requires the `PYPI_TOKEN` GitHub secret: a PyPI API
  token authorized to publish the `cdk-python` project. It is used when
  `publish_target` is `pypi`
- Publishing to TestPyPI requires a separate `TEST_PYPI_TOKEN` secret, since
  TestPyPI is a separate account with its own tokens. It is used when
  `publish_target` is `test-pypi`
- Both PyPI secrets are optional. When the one the selected target needs is
  missing, the workflow logs a warning and skips the upload; the wheels are
  still committed to `cdk-python` and attached to its GitHub release
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
