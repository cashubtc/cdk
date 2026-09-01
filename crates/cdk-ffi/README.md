# CDK FFI Bindings

UniFFI bindings for the CDK (Cashu Development Kit), providing foreign function interface access to wallet functionality for multiple programming languages.

## Supported Languages

- **🐍 Python** - With REPL integration for development
- **🍎 Swift** - iOS and macOS development
- **🎯 Kotlin** - Android and JVM development

## Development Tasks

### Build & Check
```bash
just ffi-build        # Build FFI library (release)
just ffi-build --debug # Build debug version
just ffi-check         # Check compilation
just ffi-clean         # Clean build artifacts
```

### Generate Bindings
```bash
# Generate for specific languages
just ffi-generate python
just ffi-generate swift
just ffi-generate kotlin

# Generate all languages
just ffi-generate-all

# Use --debug for faster development builds
just ffi-generate python --debug
```

### Development & Testing
```bash
# Python development with REPL
just ffi-dev-python    # Generates bindings and opens Python REPL with cdk_ffi loaded

# Test bindings
just ffi-test-python   # Test Python bindings import
just ffi-test-live-python # Run live Python test against testnut.cashudevkit.org
```

## Quick Start

```bash
# Start development
just ffi-dev-python

# In the Python REPL:
>>> dir(cdk_ffi)  # Explore available functions
>>> help(cdk_ffi.generate_mnemonic)  # Get help
```

## Mobile Nostr identity

`NostrSigner` is the recommended Swift/Kotlin entry point for the wallet's
active Nostr identity. It derives NIP-06 path `m/44'/1237'/0'/0/0` from a
BIP-39 mnemonic entirely in Rust, or can import secret-key hex/`nsec` and
generate a secure random identity. The same object provides canonical Nostr
event signing (including NIP-98), NIP-44 v2, a strict restartable NIP-17 inbox,
the compressed Cashu P2PK public key, and npub.cash authentication.
Use the existing `generateMnemonic()`/`generate_mnemonic()` binding when
creating a new mnemonic-backed wallet; mobile code never needs to generate or
pass BIP-39 seed bytes.

Swift:

```swift
let signer = try NostrSigner.fromMnemonic(mnemonic: words, passphrase: nil)
let event = try signer.signEvent(event: NostrUnsignedEvent(
    createdAt: UInt64(Date().timeIntervalSince1970),
    kind: 27_235,
    tags: [["u", url], ["method", "POST"]],
    content: ""
))
let npubCash = NpubCashClient.withSigner(baseUrl: "https://npub.cash", signer: signer)
let inbox = try NostrInbox.withSigner(signer: signer, relays: relays, since: lookback)
```

Kotlin:

```kotlin
val signer = NostrSigner.fromMnemonic(words, null)
val event = signer.signEvent(
    NostrUnsignedEvent(now, 27_235U.toUShort(), tags, ""),
)
val npubCash = NpubCashClient.withSigner("https://npub.cash", signer)
val inbox = NostrInbox.withSigner(signer, relays, lookback)
```

When a `WalletRepository` already exists, use
`NostrSigner.fromWalletRepository(repository)` so the mnemonic is parsed once
and its BIP-39 seed never crosses the FFI boundary. `NostrInbox.stop()` is
async and guarantees that the stopped run cannot invoke another callback after
it returns.

NWC (`m/44'/1237'/1'/0/0`), NUT-27 backups, Cashu proof derivation, and CDK's
other deterministic P2PK keyring paths remain separate purpose-specific key
domains.

## Live Tests

The live Python test in `tests/test_live_async_onchain_melt.py` covers
`PreparedMelt.confirm_prefer_async()`, immediate and pending melt outcomes,
`PendingMelt.wait()`, and `Wallet.finalize_pending_melts()` against
`https://testnut.cashudevkit.org`.

## Language Packages

For production use, see language-specific repositories:

- [cdk-swift](https://github.com/cashubtc/cdk-swift) - iOS/macOS packages
- [cdk-kotlin](https://github.com/cashubtc/cdk-kotlin) - Android/JVM packages  
- [cdk-go](https://github.com/cashubtc/cdk-go) - Golang packages
- [cdk-python](https://github.com/cashubtc/cdk-python) - PyPI packages
