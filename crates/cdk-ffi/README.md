# CDK FFI Bindings

UniFFI bindings for the CDK (Cashu Development Kit), providing foreign function interface access to wallet functionality for multiple programming languages.

## Supported Languages

- **🎯 Dart** - Flutter and Dart VM
- **🐹 Go** - With cgo
- **🐍 Python** - With REPL integration for development
- **🍎 Swift** - iOS and macOS development
- **🎯 Kotlin** - Android and JVM development

The recipes below cover this crate and the Python bindings. The released
bindings are built from the wrapper crates under `bindings/<lang>/rust/`; see
`bindings/<lang>/README.md` for how to build each one.

## Development Tasks

### Build & Check
```bash
just ffi-build        # Build FFI library (release)
just ffi-build --debug # Build debug version
just ffi-check         # Check compilation
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

## Live Tests

The live Python test in `tests/test_live_async_onchain_melt.py` covers
`PreparedMelt.confirm_prefer_async()`, immediate and pending melt outcomes,
`PendingMelt.wait()`, and `Wallet.finalize_pending_melts()` against
`https://testnut.cashudevkit.org`.

## Language Packages

For production use, see language-specific repositories:

- [cdk-dart](https://github.com/cashubtc/cdk-dart) - Dart/Flutter packages
- [cdk-swift](https://github.com/cashubtc/cdk-swift) - iOS/macOS packages
- [cdk-kotlin](https://github.com/cashubtc/cdk-kotlin) - Android/JVM packages
- [cdk-go](https://github.com/cashubtc/cdk-go) - Golang packages

## Creating receive requests (NUT-18)

Call `WalletRepository.createRequest(params)` in Swift/Kotlin to construct a
request using CDK. Encode `result.paymentRequest` with `toStringEncoded()`
(NUT-18) or `toBech32String()` (NUT-26). `PaymentRequest.fromString(...)`
decodes either format.

To use an existing Nostr receiving identity, set `transport = "nostr"`,
`nostrPubkey` to its public key (hex or npub), and `nostrRelays` to the app's
relay URLs. Set `paymentId` to preserve the app's request ID, or omit it to
leave the ID unset. CDK builds the nprofile without needing a private key.
The app still handles receiving payments through its existing signer/inbox;
`nostrWaitInfo` is absent for an explicit public destination.

Omitting `nostrPubkey` with Nostr transport generates a managed ephemeral identity
and returns `nostrWaitInfo`. Pass that handle to
`WalletRepository.waitForNostrPayment(info, options)` to connect, decrypt NIP-17
messages, validate the payment against the original request, and redeem it into
the wallet. Use `nil`/`null` options for ordinary payments; provide receive options
with signing keys or preimages for P2PK/HTLC payments. Nostr transport keys remain
inside CDK and are independent of spending keys.

Call `nostrWaitInfo.cancel()` to stop listening. Only one wait may run per handle.
Cancellation reports `PaymentRequestReceiveCancelled`; an already-started redemption
finishes and reports its result. Once redemption starts, the handle is consumed,
even if it returns an error. Consult wallet transactions and receive recovery before
asking the payer to resend. Dropping an async wait also stops listening, but if
redemption had already started its outcome may require wallet recovery.

The handle is memory-only: keep it until payment completes. Pending requests do not
survive an app restart. Apps needing durable inbox identities should use the explicit
public-key path and their existing signer/inbox storage.

`CreateRequestParams` also supports amountless requests, units, descriptions,
mint policies, HTTP or no transport, supported methods/fees, and P2PK/multisig
or HTLC conditions. Core validation errors propagate through FFI. See the
[migration notes](../../docs/migrations/v0.19.md) for Rust API changes and the
[test guide](tests/README.md#payment-request-tests) for binding coverage.
