# CDK – Cashu Development Kit for Dart

Dart bindings for [CDK](https://github.com/cashubtc/cdk), a Cashu protocol implementation.

## Installation

Add to your `pubspec.yaml`:

```yaml
dependencies:
  cdk:
    git:
      url: https://github.com/cashubtc/cdk-dart
      ref: vX.Y.Z  # replace with desired version
```

## Requirements

- Dart SDK `^3.10.0`

## Usage

```dart
import 'package:cdk/cdk.dart';
```

## Native library

Consuming this package involves no compilation step, no Rust toolchain and no
network access. The native libraries are committed in this package under
`prebuilt/<target-triple>/`, and the build hook copies the one matching your
target. They are built from the CDK monorepo at the commit this version was
tagged from, so the binary and the Dart bindings always come from one tree.

Supported targets:

| Platform | Architecture |
|----------|-------------|
| Linux | x86_64, aarch64 |
| macOS | aarch64, x86_64 |
| Windows | x86_64 |
| Android | aarch64, armv7, x86_64 |
| iOS | aarch64 (device), aarch64 and x86_64 (simulator) |

Every target ships a dynamic library, because that is the link mode Dart and
Flutter ask for on all of them, iOS included. A target or link mode with no
committed library fails the build naming what it wanted, because this package
ships no Rust sources to fall back to.

## Building from source

The native library and the generated Dart sources are built in the
[CDK monorepo](https://github.com/cashubtc/cdk). This package carries release
artifacts only, so there is no Rust crate here to build.

```bash
git clone https://github.com/cashubtc/cdk
cd cdk
just binding-dart
```

`just binding-dart` runs `nix build .#dart-bindings` and copies the generated
sources and the native library into `bindings/dart/lib/src/generated/`. It needs
[nix](https://nixos.org/download) with flakes enabled and
[just](https://github.com/casey/just). Nix supplies the Rust toolchain pinned in
`rust-toolchain.toml`, so a separate rustup install is not required.

Run the tests from the monorepo root with `just test-dart`. The recipe needs the
Dart SDK on PATH; `nix develop .#bindings` provides it.

Each target ships the flavour Dart asks for on that platform: dynamic
everywhere except iOS, which is statically linked. A target or link mode with no
committed library fails the build naming what it wanted, because this package
ships no Rust sources to fall back to.

## Building from source

The native library and the generated Dart sources are built in the
[CDK monorepo](https://github.com/cashubtc/cdk). This package carries release
artifacts only, so there is no Rust crate here to build.

```bash
git clone https://github.com/cashubtc/cdk
cd cdk
just binding-dart
```

`just binding-dart` runs `nix build .#dart-bindings` and copies the generated
sources and the native library into `bindings/dart/lib/src/generated/`. It needs
[nix](https://nixos.org/download) with flakes enabled and
[just](https://github.com/casey/just). Nix supplies the Rust toolchain pinned in
`rust-toolchain.toml`, so a separate rustup install is not required.

Run the tests from the monorepo root with `just test-dart`. The recipe needs the
Dart SDK on PATH; `nix develop .#bindings` provides it.

## CI/CD — Publishing Workflow

The `dart-publish.yml` workflow (in the CDK monorepo) builds native binaries,
syncs sources to `cdk-dart`, and creates a tagged release. The following secrets
and variables must be configured in the **CDK monorepo** repository settings
(Settings → Secrets and variables → Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos. Used to clone, push, and create releases. Shared across all FFI publish workflows. |

#### How to create the PAT

1. Go to **GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens**.
2. Create a token scoped to the FFI target repositories with **Contents** (read/write) and **Metadata** (read) permissions.
3. Add it as a repository secret named `FFI_DEPLOY_KEY` in the monorepo.

### Variables

| Name | Purpose | Example |
|---|---|---|
| `CDK_DART_REPO` | Owner/repo of the target Dart package repository. | `cashubtc/cdk-dart` |

Set this under **Settings → Secrets and variables → Actions → Variables**.

## Testing

By default, running tests will skip live mint integration tests to allow offline/local testing:

```bash
dart test
```

To run the live mint integration tests, provide the `CDK_DART_TEST_MINT_URL` environment variable:

```bash
CDK_DART_TEST_MINT_URL=https://testnut.cashudevkit.org dart test
```

If the mint has a slower auto-payment settlement, you can optionally configure the settlement delay (in seconds):

```bash
CDK_DART_TEST_MINT_URL=https://testnut.cashudevkit.org CDK_DART_MINT_SETTLEMENT_DELAY_SECONDS=5 dart test
```

## License

[MIT](https://github.com/cashubtc/cdk/blob/main/LICENSE)

