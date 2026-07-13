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
- Rust toolchain (the native library is compiled from source via [native_toolchain_rust](https://pub.dev/packages/native_toolchain_rust))

## Usage

```dart
import 'package:cdk/cdk.dart';
```

## Building

The Rust native library is built automatically when you run `dart pub get` or `dart run`. No manual compilation step is needed.

If you're in a Nix environment, OpenSSL paths are detected automatically from `NIX_CFLAGS_COMPILE` and `NIX_LDFLAGS`.

## Pre-built binaries

Pre-built native libraries for all supported platforms are available as [GitHub release assets](https://github.com/cashubtc/cdk-dart/releases).

Supported targets:

| Platform | Architecture |
|----------|-------------|
| Linux | x86_64, aarch64 |
| macOS | aarch64 |
| Windows | x86_64 |
| Android | aarch64, armv7, x86_64 |
| iOS | aarch64 |

## CI/CD — Publishing Workflow

The `dart-publish.yml` workflow (in the CDK monorepo) builds native binaries,
syncs sources to `cdk-dart`, and creates a tagged release. The following secrets
and variables must be configured in the **CDK monorepo** repository settings
(Settings → Secrets and variables → Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos (`cdk-dart`, `cdk-kotlin`, `cdk-swift`). Used to clone, push, and create releases. Shared across all FFI publish workflows. |

#### How to create the PAT

1. Go to **GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens**.
2. Create a token scoped to the `cdk-dart`, `cdk-kotlin`, and `cdk-swift` repositories with **Contents** (read/write) and **Metadata** (read) permissions.
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

