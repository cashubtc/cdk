# CDK – Cashu Development Kit for Swift

Swift bindings for [CDK](https://github.com/cashubtc/cdk), a Cashu protocol implementation.

## Installation

### Swift Package Manager

Add to your `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/cashubtc/cdk-swift", from: "0.16.0"),
]
```

Then add `"Cdk"` as a dependency of your target:

```swift
.target(name: "MyApp", dependencies: [
    .product(name: "Cdk", package: "cdk-swift"),
]),
```

### Xcode

1. Open your project in Xcode
2. Go to **File > Add Package Dependencies...**
3. Enter `https://github.com/cashubtc/cdk-swift`
4. Select the version rule (e.g. "Up to Next Major Version" from `0.16.0`)
5. Click **Add Package**
6. Select the `Cdk` library and add it to your target

## Requirements

- iOS 14+ / macOS 13+
- Swift 5.9+

## Quick Start

```swift
import Cdk

// 1. Create a wallet
let wallet = try Wallet(
    mintUrl: "https://mint.example.com",
    unit: .sat,
    mnemonic: try generateMnemonic(),
    store: .sqlite(path: "wallet.sqlite"),
    config: WalletConfig(targetProofCount: nil)
)

// 2. Request a mint quote
let quote = try await wallet.mintQuote(
    paymentMethod: .bolt11,
    amount: Amount(value: 1000),
    description: nil,
    extra: nil
)
print("Pay this invoice: \(quote.request)")

// 3. After paying the invoice, mint ecash
let proofs = try await wallet.mint(
    quoteId: quote.id,
    amountSplitTarget: .none,
    spendingConditions: nil
)

// 4. Check balance
let balance = try await wallet.totalBalance()
print("Balance: \(balance.value) sats")
```

## Pre-built binaries

The Swift package uses a pre-built `CashuDevKitFFI.xcframework.zip`, committed at the root of the cdk-swift repository and resolved by SPM as a local binary target. The same archive is also attached to each [GitHub release](https://github.com/cashubtc/cdk-swift/releases).

Supported platforms:

| Platform | Architecture |
|----------|-------------|
| iOS | arm64 |
| iOS Simulator | arm64, x86_64 |
| macOS | arm64, x86_64 |

## Building from source

The Rust library and the Swift sources come from the
[CDK monorepo](https://github.com/cashubtc/cdk). This repository carries release
artifacts only, so there is no Rust crate here to build. The shipped
`CashuDevKitFFI.xcframework.zip` is assembled by the release workflow; a local
build produces a dylib and a throwaway SPM package for testing.

```bash
git clone https://github.com/cashubtc/cdk
cd cdk
just binding-swift
```

Unlike the other bindings, this one needs no nix: only a Rust toolchain
(`rustup`, which picks up the pinned version from `rust-toolchain.toml`), Xcode
and [just](https://github.com/casey/just). It builds
`target/release/libcdk_ffi.dylib` and writes a local SPM package, `Sources/` and
`Package.swift`, at the monorepo root. Both are gitignored.

The local recipe generates from the `cdk-ffi` crate and links the dylib directly,
while the release builds `cdk-ffi-swift` and ships an XCFramework binary target,
so a local build does not exercise the packaging the released package uses.

## Testing

From the monorepo root, after `just binding-swift`:

```bash
just test-swift
```

## CI/CD — Publishing Workflow

The `swift-publish.yml` workflow (in the CDK monorepo) builds the XCFramework,
generates Swift sources, syncs everything to `cdk-swift`, and creates a tagged
release. Every Apple slice is compiled from the monorepo checkout against its
`Cargo.lock`, so the framework never depends on a crates.io release of
`cdk-ffi`. The following secrets and variables must be configured in the **CDK
monorepo** repository settings (Settings > Secrets and variables > Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos. Used to clone, push, and create releases. Shared across all FFI publish workflows. |

#### How to create the PAT

1. Go to **GitHub > Settings > Developer settings > Personal access tokens > Fine-grained tokens**.
2. Create a token scoped to the FFI target repositories with **Contents** (read/write) and **Metadata** (read) permissions.
3. Add it as a repository secret named `FFI_DEPLOY_KEY` in the monorepo.

### Variables

| Name | Purpose | Example |
|---|---|---|
| `CDK_SWIFT_REPO` | Owner/repo of the target Swift package repository. | `cashubtc/cdk-swift` |

Set this under **Settings > Secrets and variables > Actions > Variables**.

## License

[MIT](https://github.com/cashubtc/cdk/blob/main/LICENSE)
