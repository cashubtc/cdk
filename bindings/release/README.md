# Reproducible binding candidates

The CDK monorepo owns the source, generators, build recipes, dependency locks,
verification, and publication commands. One exact source commit identifies all
of them. The language repositories remain distribution destinations.

This is an opt-in release path while platform qualification is completed. The
existing publication and nightly workflows remain available; they do **not**
acquire reproducibility guarantees from this tooling. See
[qualification.md](qualification.md) before switching stable releases.

## Operator commands

Run these commands from the monorepo root. `just` dispatches the release tools
through Nix; no separate Rust or language-toolchain installation is needed.

| Step | Command |
| --- | --- |
| Run the tooling tests | `just ffi-release-tools-test` |
| List groups supported on this host | `just ffi-candidate groups` |
| Build a group locally | `just ffi-candidate build --help` |
| Start the GitHub candidate build | `just ffi-candidate-dispatch YOUR_RECIPE_REF` |
| Download a candidate | `just ffi-candidate download --help` |
| Independently rebuild and compare | `just ffi-candidate verify --help` |
| Collect all verification receipts | `just ffi-candidate collect --help` |
| Publish verified artifacts locally | `just ffi-candidate publish --help` |

The sections below contain complete commands for each step. Use
`just ffi-candidate --help` for the full CLI. The dispatch shortcut requires a
pushed branch/tag and GitHub authentication with permission to run the workflow.
An optional second argument selects another repository:
`just ffi-candidate-dispatch YOUR_RECIPE_REF OWNER/REPO`.
Dispatch starts the build asynchronously; follow its progress in GitHub Actions.
It does not verify the candidate or publish a stable release.

## Requirements and inputs

Install `just` and Nix with flakes and `nix-command` enabled. Every entrypoint
obtains its tools from the monorepo's `flake.lock`, including Rust from
`rust-toolchain.toml`. No rustup, system Cargo, or system Gradle is used.

`inputs.json` selects external platform inputs, deployment targets, and
publication destinations. Cargo dependencies are vendored by Nix from the root
`Cargo.lock`, including the pinned Dart generator Git revision. The Go generator
and its dependency lock are already defined in `flake.nix`. Android uses the
pinned NDK. Windows builds use Nix's LLVM and cargo-xwin on Linux; the extracted
Microsoft SDK and CRT are a hash-checked Nix fixed-output dependency.

The release vendor derivation applies one checked patch to the pinned Dart
generator: its converter registry uses a sorted map so independent processes
emit declarations in the same order. The patch and updated Cargo file checksum
are part of the committed recipe (`scripts/bindings/vendor.py`).

Linux distributions are compiled inside the digest-pinned manylinux images,
with Nix providing Rust and vendored dependencies. This avoids shipping a
library that depends on a developer's Nix store. Docker is required, or select
local Podman with `CONTAINER_ENGINE=podman`. The container has no network during
compilation. It mounts the Nix store read-only and disposable build directories;
SELinux label separation is disabled for these mounts without relabeling them.

Apple builds require an Apple Silicon Mac and the exact Xcode build and SDKs in
`inputs.json`. Select Xcode before invoking the build or set `DEVELOPER_DIR`.
Nix supplies Rust and shared tools; the locally licensed Apple SDK is an
explicit external input. Version mismatches fail before compilation.

Gradle dependency locks and `gradle/verification-metadata.xml` pin resolved
libraries/plugins and their bytes. Release builds use strict verification and
never update these files. Dart consumer tests use `dart-deps.lock`. To
intentionally update language dependencies:

```bash
scripts/bindings/lock-dependencies
```

Review the changed versions and hashes. Dependency downloads may be cached;
project compilation and packaging always start in fresh directories. Cargo
builds are offline and do not use sccache. On the configured workstation,
scratch lives under `/data/rust/tmp` and isolated Cargo outputs under
`/data/rust/targets`; the shared target directory is never cleaned. Elsewhere,
scratch uses `TMPDIR` and Cargo outputs remain within the disposable workspace.

## Candidate build

The `FFI - Build reproducible candidate` workflow is manually dispatched on the
exact recipe branch/tag. It first requires the existing full CDK CI and Nutshell
checks on that commit. It never receives registry credentials or signing keys.
Only its final upload job has repository write permission, using the temporary
GitHub token to create a public prerelease.

```bash
just ffi-candidate-dispatch YOUR_RECIPE_REF
```

Candidate tags include the version, source commit, run ID, and attempt. Tags and
assets are never overwritten. Public release assets retain the candidate after
temporary Actions artifacts expire. The release notes and workflow summary
report the SHA-256 of `candidate.json`.

Local builds run the same commands as CI:

```bash
just ffi-candidate groups
just ffi-candidate build --group generated --output "$TMPDIR/cdk-generated"
just ffi-candidate build --group linux-x86_64 \
  --candidate "$TMPDIR/cdk-generated" --output "$TMPDIR/cdk-linux"
```

Use new output directories. Git recipes must be clean and committed. JJ recipes
are snapshotted at `@` and exported through their Git object store without
changing bookmarks or checking out another revision. Preserve that exact commit
on an official branch before asking others to verify it. A later JJ edit creates
a different source identity.

| Group | Build/verification host | Outputs |
| --- | --- | --- |
| `generated` | Linux x86_64 | Generated Dart, Go, Kotlin, Swift and FFI sources |
| `linux-x86_64` | Linux x86_64 + Docker/Podman | Dart Linux archive, Go native library |
| `linux-aarch64` | Linux aarch64 + Docker/Podman | Dart Linux archive, Go native library |
| `android` | Linux x86_64 | Three Dart archives, four Kotlin ABIs, unsigned Maven packages, Kotlin distribution |
| `darwin` | Apple Silicon + pinned Xcode | Five Dart archives, two Go libraries, Swift XCFramework ZIP and distribution |
| `windows` | Linux x86_64 | Cross-compiled MSVC Dart archive and Go DLL |
| `packages` | Linux x86_64 or aarch64 | Complete Dart and Go distributions assembled from native groups |

All groups except `generated` consume `generated.tar`. `packages` also consumes
the native groups' Dart archives and Go libraries. Place imported files and their
`group-*.json` inventories in the `--candidate` directory. Every import must have
the same source identity. Combine all group outputs into one directory and seal
it with:

```bash
just ffi-candidate candidate --directory "$TMPDIR/cdk-candidate"
```

The manifest records the source commit, commit epoch, version, input hashes,
complete output inventories, and imported artifact hashes. Unexpected or missing
files fail validation. Archives have stable ordering, permissions, ownership,
and timestamps. Normalization changes container metadata, never native content.

## Independent verification

Get the candidate ID and expected **manifest SHA-256 from a trusted announcement**.
A checksum downloaded alongside the candidate only checks consistency. Review
the selected source/recipe before executing it: verification runs build code.

```bash
just ffi-candidate download \
  --tag CANDIDATE_ID --sha256 MANIFEST_SHA256 \
  --output "$TMPDIR/cdk-candidate"

just ffi-candidate verify \
  --candidate "$TMPDIR/cdk-candidate" --sha256 MANIFEST_SHA256 \
  --output "$TMPDIR/cdk-verification"
```

Downloads and verification need no GitHub credentials. The verifier checks the
manifest, clones the source repository into scratch, checks out its exact commit,
and requires that it is reachable from a source-repository branch. It executes
that commit's build scripts, not the verifier checkout's current recipes.
`--source-repo` permits a test repository; the default is `cashubtc/cdk`.

By default, verification rebuilds every group supported by the host. Repeat
`--group` to select specific groups. Each invocation uses fresh Cargo and Gradle
outputs. Nix may cache tools/dependencies but never substitutes the native CDK
libraries or generated wrappers being compared. The verifier compares complete
artifact hashes and imported input inventories and writes a receipt only after
equality. It continues after individual group failures, retaining diagnostics,
and returns failure if any selected group failed.

An assembly receipt proves assembly of its recorded inputs. It does not prove
the imported Apple/Windows/Linux binaries were independently compiled. Collection
requires their separate native receipts as well. Running the verifier on another
GitHub runner exercises the workflow but is weaker evidence than an independent
machine/operator outside GitHub.

## Collect and publish

Copy **trusted** `receipt-*.json` files from the verification hosts into one
directory. Receipts are operator assertions, not cryptographic attestations of
honest compilation. The collector checks consistency and complete coverage; it
cannot establish the operator's honesty. Do not accept arbitrary receipts from
an untrusted download.

```bash
just ffi-candidate collect \
  --candidate "$TMPDIR/cdk-candidate" --sha256 MANIFEST_SHA256 \
  --receipts "$TMPDIR/cdk-receipts" --output "$TMPDIR/cdk-collected.json"
```

Every group is required. There is deliberately no unverified-publication bypass.
Return to a clean checkout of the candidate's exact source commit. Publish each
destination explicitly, using a separate persistent state directory:

```bash
just ffi-candidate publish \
  --candidate "$TMPDIR/cdk-candidate" --sha256 MANIFEST_SHA256 \
  --collected "$TMPDIR/cdk-collected.json" \
  --destination swift --confirm MANIFEST_SHA256 \
  --output "$HOME/cdk-publication-state/CANDIDATE_ID/swift"
```

Destinations are `dart`, `go`, `kotlin`, `swift`, and `maven`. Publication refuses
CI, revalidates every artifact and receipt, and never compiles CDK or rebuilds a
language package. GitHub publication uses the configured local Git and `gh`
credentials. It commits the candidate distribution to the destination repository
and pushes an immutable version tag, without merging into its default branch.
Existing tags must contain identical candidate content; existing release assets
must match before a retry can succeed. Missing assets from an interrupted upload
can be resumed without replacing files.

Maven publication uses local `SONATYPE_USERNAME`, `SONATYPE_PASSWORD`, and
`CDK_SIGNING_KEY` (a key available to GPG). It signs the five existing unsigned
Maven files, creates a transport bundle, uploads one user-managed deployment,
waits for validation, and publishes it. Signatures and the outer transport
bundle are outside the unsigned package comparison. Credentials are never
written to publication state. Keep the state directory to resume a partial
operation. If the upload response was lost, retrieve its deployment ID from the
Portal and resume with `--deployment-id ID`; the tool refuses a blind re-upload.

## Correctness and qualification

Reproducibility does not establish correctness. The candidate workflow requires
the existing full project CI. Build recipes also check Linux native loading,
Android ELF alignment and Maven compilation, Dart wallet calls on native Linux
and macOS, Go wallet calls through the assembled distribution, and Swift package
compilation. Android device/minification behavior, Apple minimum-OS execution,
and Windows-native consumer execution need separate qualification.

Run release-tool contract tests with `just ffi-release-tools-test`. Full
qualification requires fresh builds, independent byte comparisons for every
group, target-language consumer checks, and hosted candidate/retry tests. Never
label an entire release reproducible while one of its shipped groups remains
unverified.
