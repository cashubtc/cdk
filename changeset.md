# Build the FFI bindings from the workspace, not the binding repos

Supersedes #2126.

## Problem

The Dart, Kotlin, Swift and Go release binaries were compiled inside a clone of
their own publishing repository. No monorepo sits beside that clone, so the
copied wrapper crate could not use a path dependency on `cdk-ffi`. Every release
rewrote the manifest into something standalone, pulling `cdk-ffi` from crates.io
with no lockfile at all.

Two consequences follow from that, and neither is fixable inside the old shape:

- **The release cannot inherit the release's `Cargo.lock`.** Cargo resolves the
  whole dependency graph fresh at build time, so every release shipped a
  dependency resolution that nothing had ever tested. The binaries users install
  were not built from the tree CI is green on.
- **The release cannot start until the crates.io publish has landed**, because
  the manifest points at a version that has to exist first. Which source a
  release is built from ends up decided by a version string in a generated
  manifest rather than by the tag.

## Approach

Build all four wrapper crates here, in the workspace, with `--locked` against the
root `Cargo.lock` and a shared `release-ffi` profile:

```bash
cargo build --locked --profile release-ffi -p cdk-ffi-<lang> --target <triple>
```

The release tag selects the source tree before the build starts. There is no
version pin, no revision pin, and no crates.io dependency anywhere in the release
path. What ships is built from the same lockfile CI tested.

The publishing repositories stay. Go module paths and Swift Package Manager both
resolve by repository URL, so `cashubtc/cdk-{dart,go,kotlin,swift}` remain the
distribution channel. They just stop carrying a Rust crate: the workflows sync
sources and compiled artifacts into them, and delete the crate copy and its
toolchain pin. Nothing in those repos is buildable any more, which is the point.
That is the best of both worlds: reproducible builds from one lockfile, and
per-ecosystem delivery from the repositories each toolchain expects.

## Delivery per ecosystem

Each language keeps the packaging its own toolchain wants, and the compiled
output lives in the tree the tag points at.

| Binding | What the release ships |
|---|---|
| Dart | Native libraries committed under `prebuilt/<target-triple>/`; the build hook copies the one matching the target and fails naming it when absent |
| Swift | `CashuDevKitFFI.xcframework.zip` committed at the repo root, resolved by SPM as a local `binaryTarget(path:)` |
| Kotlin | Android libraries committed under `cdk-android/src/main/jniLibs/` as well as published to Maven Central, so a nightly tag that skips the Maven publish still carries them |
| Go | Native libraries and generated bindings committed, with per-platform cgo link files |

Go also moves to the `cdk-ffi-go` crate, so the release ships the crate CI
actually exercises.

## Guardrails added

- `cargo metadata --locked` runs in pre-commit checks. The release builds use
  `--locked`, so a lock that drifted from `Cargo.toml` would otherwise first
  surface on release day.
- The Windows leg of the Dart and Go matrices now runs in bash. It was failing in
  PowerShell before cargo even started, because the build step declared no shell
  and PowerShell does not accept backslash line continuations. Stripping is
  skipped on `windows-msvc`, where debug info lives in a separate `.pdb` and the
  `release-ffi` profile already strips.
- `release-ffi` deliberately omits `panic = "abort"`: UniFFI turns a Rust panic
  into a foreign exception through `catch_unwind`, and aborting would take the
  host app down instead.

## Documentation

- ADR 0005, `docs/adr/0005-binding-repos-carry-artifacts-only.md`, records the
  decision and the options weighed against it.
- Every bindings README gains a build-from-source section that starts from the
  monorepo clone and states what that binding actually needs. These files are
  rsync'd into the publishing repos and become their READMEs, where no justfile
  exists, so the monorepo clone has to be explicit.
- New `just binding-swift` recipe, so every language shares one
  `just binding-<lang>` entry point.

## Tradeoffs

Committing compiled artifacts grows the binding repositories: roughly 157 MB for
Dart, 40 MB for Swift zipped and 32 MB for Kotlin per release. Existing history
is untouched. The alternative, release assets only, left Kotlin with nothing to
deliver on a nightly, since its release uploads no assets and nightlies skip
Maven Central.

Two user-visible changes need release notes: the Go library rename from
`libcdk_ffi` to `libcdk_ffi_go`, and the repository growth above.

## Before merge

- [ ] Re-enable the green-CI gate in `release-ci.yml`, currently commented out
      for dispatch testing.
- [ ] Dispatch a nightly per language and confirm each target repo's tag carries
      the compiled artifacts. Kotlin is the one to verify specifically.
