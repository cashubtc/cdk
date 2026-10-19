# Binding repositories carry build artifacts, not sources

* Status: accepted
* Authors: CDK Developers
* Date: 2026-09-24
* Targeted modules: `bindings/*`, `crates/cdk-ffi`, the FFI publish workflows
* Associated tickets/PRs: n/a

## Context and Problem Statement

The four language bindings are workspace members of this repository, and their
release builds compile here against the root `Cargo.lock`. Each language also has
a publishing repository (`cashubtc/cdk-{dart,go,kotlin,swift}`) that the release
workflows sync sources into, because Go module paths and Swift Package Manager
both resolve by repository URL.

Those repositories were also receiving a copy of the language's wrapper crate.
That copy cannot use a path dependency on `cdk-ffi`, since no monorepo sits
beside it, so every release had to rewrite the manifest into something
standalone. How should the copied crate reach the CDK source?

## Decision Drivers

* The release build must resolve dependencies exactly as CI does, from the root
  `Cargo.lock`, and must not depend on a crates.io publish having landed first.
* Which source tree a release is built from should be decided by the tag, before
  the build starts, not by a version or revision embedded in a manifest.
* Whatever is shipped has to survive the packaging format of four different
  ecosystems.

## Considered Options

#### Rewrite the copied manifest to a crates.io version

What the workflows originally did: `cdk-ffi = { version = "=X.Y.Z" }`.

**Pros:**

* Standalone and self-describing.

**Cons:**

* Bad, because the binding release could not start until `cargo publish -p
  cdk-ffi` had landed, coupling two unrelated pipelines.
* Bad, because the crates.io tarball is not the tagged tree; excluded files and a
  republished crate both make them diverge.
* Bad, because no lockfile travelled with it, so the release binaries were built
  from a dependency resolution nothing had ever tested.

#### Include the monorepo as a git submodule

`.gitmodules` pinned per release, with the wrapper crate using a path dependency
into the submodule.

**Pros:**

* Good, because the pinned commit is explicit in the repository itself.
* Good, because no dependency resolution happens at build time.

**Cons:**

* Bad, because `dart pub` clones git dependencies with `git clone --mirror` and
  has no submodule support (dart-lang/pub#2807). Dart is the only language whose
  consumers ever compile the copied crate, so this breaks exactly the audience it
  would need to serve.
* Bad, because Go module zips exclude nested submodules by design and `go get`
  never initialises them (golang/go#26716), so Go consumers get an empty
  directory.
* Bad, because it works only for Swift and for Kotlin clones, neither of which
  ever builds the crate: Swift consumes a prebuilt xcframework and Kotlin a Maven
  AAR.
* Bad, because every binding repository would grow a full monorepo checkout to
  serve a build path that almost nobody takes.

#### Ship no Rust crate at all

The binding repositories carry generated sources, platform project files and
prebuilt libraries. Nothing in them is buildable.

**Pros:**

* Good, because with nothing to build there is nothing to pin, and the question
  of how the copy reaches CDK source disappears.
* Good, because it matches reality: no consumer of Go, Swift or Kotlin ever
  compiled the copied crate. It was inert payload.
* Good, because it removes the manifest rewriting, the lockfile seeding and the
  release-profile mirroring the other options all require.
* Good, because the tag alone determines what was built.

**Cons:**

* Bad, because Dart loses its build-from-source fallback, so prebuilt coverage
  becomes mandatory and a missing target is a hard error.
* Bad, because the compiled libraries have to travel some other way, and the
  repositories carry them instead.

## Decision Outcome

Chosen option: "Ship no Rust crate at all", because it is the only option that
removes the coupling rather than relocating it, and because the crate it deletes
was already unused by three of the four ecosystems.

The pin lives in the checkout. `<lang>-build-native` checks out the release tag
and runs `cargo build --locked --profile release-ffi -p cdk-ffi-<lang>` against
the workspace, resolving `cdk-ffi` through the ordinary path dependency.

Every repository commits its compiled libraries. Dart keeps `prebuilt/<triple>/`,
Kotlin keeps `cdk-android/src/main/jniLibs/`, Go keeps
`bindings/cdkffi/native/`, and Swift gains `CashuDevKitFFI.xcframework.zip` at
its root, resolved by a local `binaryTarget(path:)` rather than a URL and
checksum.

The alternative, distributing them purely as release assets, was tried and
rejected. It reads better on repository size but it puts the artifacts outside
the tagged tree, and one gap proved the point: the Kotlin release uploads no
assets and skips the Maven publish for nightlies, so gitignoring its libraries
made a nightly produce nothing at all. Committing keeps the invariant simple:
whatever a tag points at contains everything that tag delivers.

### Positive Consequences

* A binding release needs only the tag and green CI on it. It can run before,
  after, or without the crates.io publish.
* Release binaries and the generated bindings that call them always come from
  one tree.
* Every tag in a binding repository is self-contained. Checking one out gives
  the libraries, so a nightly works even though it skips Maven Central, and a
  release cannot half-exist.

### Negative Consequences

* Every binding repository grows by a full set of binaries per release: roughly
  157 MB for Dart, 136 MB for Swift before compression, 32 MB for Kotlin. This is
  the accepted cost of the self-containment above.
* `dart pub` clones git dependencies with `git clone --mirror`, so a Dart
  consumer downloads that history in full.
* Dart's prebuilt matrix is load-bearing. A target or link mode with no committed
  library now fails the build, because the Rust crate it used to fall back to is
  no longer synced.

## Links

* `bindings/README.md` describes the resulting release flow and where each
  language's libraries live.
* `DEVELOPMENT.md` documents the `release-ffi` profile.
