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

let wallet = try Wallet.open(request: WalletOpenRequest(
    mintUrl: "https://mint.example.com",
    unit: .sat,
    mnemonic: try generateMnemonic(),
    store: .sqlite(path: "wallet.sqlite")
))

let session = try await wallet.requestMinting(request: MintRequest(
    method: .bolt11,
    amount: Amount(value: 1_000)
))
print("Pay this invoice: \(session.initialState().paymentRequest)")

// After payment settles:
_ = try await session.refresh()
let claimed = try await session.claim()
let balance = try await wallet.balance()
print("Claimed \(claimed.value); available \(balance.available.value) sats")
```

## Pre-built binaries

The Swift package uses a pre-built `CashuDevKitFFI.xcframework` downloaded automatically via SPM from [GitHub releases](https://github.com/cashubtc/cdk-swift/releases).

Supported platforms:

| Platform | Architecture |
|----------|-------------|
| iOS | arm64 |
| iOS Simulator | arm64, x86_64 |
| macOS | arm64, x86_64 |

## Testing

```bash
just test-swift
```

## CI/CD — Publishing Workflow

The `swift-publish.yml` workflow (in the CDK monorepo) builds the XCFramework,
generates Swift sources, syncs everything to `cdk-swift`, and creates a tagged
release. The following secrets and variables must be configured in the **CDK
monorepo** repository settings (Settings > Secrets and variables > Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos (`cdk-dart`, `cdk-kotlin`, `cdk-swift`). Used to clone, push, and create releases. Shared across all FFI publish workflows. |

#### How to create the PAT

1. Go to **GitHub > Settings > Developer settings > Personal access tokens > Fine-grained tokens**.
2. Create a token scoped to the `cdk-dart`, `cdk-kotlin`, and `cdk-swift` repositories with **Contents** (read/write) and **Metadata** (read) permissions.
3. Add it as a repository secret named `FFI_DEPLOY_KEY` in the monorepo.

### Variables

| Name | Purpose | Example |
|---|---|---|
| `CDK_SWIFT_REPO` | Owner/repo of the target Swift package repository. | `cashubtc/cdk-swift` |

Set this under **Settings > Secrets and variables > Actions > Variables**.

## License

[MIT](https://github.com/cashubtc/cdk/blob/main/LICENSE)
