# CDK – Cashu Development Kit for Swift

Swift bindings for [CDK](https://github.com/cashubtc/cdk), a Cashu protocol implementation.

## Installation

### Swift Package Manager

Add to your `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/{{CDK_SWIFT_REPO}}", from: "0.16.0"),
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
3. Enter `https://github.com/{{CDK_SWIFT_REPO}}`
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

The Swift package uses a pre-built `CashuDevKitFFI.xcframework` downloaded automatically via SPM from [GitHub releases](https://github.com/{{CDK_SWIFT_REPO}}/releases).

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
