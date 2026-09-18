# CDK Python

Python language bindings for the [Cashu Development Kit (CDK)](https://github.com/cashubtc/cdk).

## About

CDK Python provides UniFFI-generated Python bindings for the Cashu Development
Kit, enabling developers to build Cashu ecash applications in Python with access
to CDK's wallet functionality.

The native library is bundled inside the wheel and loaded with `ctypes`, so
there is no compiler and no build step at install time.

## Features

- **Complete Wallet Operations**: Create, configure, and manage Cashu wallets
- **Mint & Melt**: Request quotes and perform minting and melting operations
- **Token Management**: Send and receive Cashu tokens
- **Proof Handling**: Track proof states and manage transactions
- **BIP39 Support**: Mnemonic generation and management
- **Rate Limiting**: Client-side pacing of mint traffic
- **Subscriptions**: Real-time updates via NUT-17

## Installation

```bash
pip install cdk-python
```

Wheels are also attached to each
[cdk-python release](https://github.com/cashubtc/cdk-python/releases) if you
would rather download one directly:

```bash
pip install cdk_python-<version>-py3-none-<platform>.whl
```

Each release carries a `checksums.sha256` asset to verify the download against.

### Requirements

- Python 3.10 or higher
- Supported platforms:
  - Linux (x86_64, ARM64) — `manylinux_2_28`, so glibc 2.28 or newer
  - macOS (Apple Silicon, Intel)
  - Windows (x86_64)

One wheel covers every supported Python version: the native library is loaded
through `ctypes` rather than the CPython ABI, so every wheel is tagged
`py3-none-<platform>`.

## Quick Start

```python
import asyncio

import cdk


async def main():
    wallet = cdk.Wallet(
        mint_url="https://testnut.cashudevkit.org",
        unit=cdk.CurrencyUnit.SAT(),
        mnemonic=cdk.generate_mnemonic(),
        store=cdk.sqlite_wallet_store(":memory:"),
        config=cdk.WalletConfig(target_proof_count=3),
    )

    mint_info = await wallet.fetch_mint_info()
    print(f"Mint: {mint_info.name if mint_info else 'unknown'}")

    quote = await wallet.mint_quote(
        cdk.PaymentMethod.BOLT11(),
        cdk.Amount(value=100),
        "Test deposit",
        None,
    )
    print(f"Pay this invoice: {quote.request}")

    balance = await wallet.total_balance()
    print(f"Balance: {balance.value} sats")


asyncio.run(main())
```

## Examples

Runnable versions of these live in [`examples/`](examples/).

### Creating a Wallet with SQLite

```python
import cdk

wallet = cdk.Wallet(
    mint_url="https://testnut.cashudevkit.org",
    unit=cdk.CurrencyUnit.SAT(),
    mnemonic=cdk.generate_mnemonic(),
    store=cdk.sqlite_wallet_store("/path/to/wallet.db"),
    config=cdk.WalletConfig(target_proof_count=3),
)
```

Pass `":memory:"` as the store path for an ephemeral wallet. A wallet database
can also be opened on its own:

```python
db = cdk.create_wallet_db(cdk.WalletDbBackend.SQLITE(path="/path/to/wallet.db"))
```

### Minting

```python
quote = await wallet.mint_quote(
    cdk.PaymentMethod.BOLT11(),
    cdk.Amount(value=100),
    "Deposit",
    None,
)

# Pay quote.request, then issue the proofs.
proofs = await wallet.mint(quote.id, cdk.SplitTarget.NONE(), None)
print(f"Minted {len(proofs)} proofs")
```

### Sending and Receiving Tokens

Sending is two-phase: prepare, inspect the fee, then confirm. Note that
`SendOptions` and `ReceiveOptions` have no default values, so every field must
be given.

```python
prepared = await wallet.prepare_send(
    cdk.Amount(value=50),
    cdk.SendOptions(
        memo=cdk.SendMemo(memo="Payment for coffee", include_memo=True),
        conditions=None,
        amount_split_target=cdk.SplitTarget.NONE(),
        send_kind=cdk.SendKind.ONLINE_EXACT(),
        include_fee=False,
        use_p2bk=False,
        max_proofs=None,
        metadata={},
        p2pk_signing_keys=[],
        p2pk_locked_proof_send_mode=cdk.P2pkLockedProofSendMode.SWAP,
    ),
)
print(f"Sending {prepared.amount().value} sats, fee {prepared.fee().value} sats")

token = await prepared.confirm("Payment for coffee")
print(f"Token: {token.encode()}")
```

```python
received = await wallet.receive(
    cdk.Token.decode(encoded_token),
    cdk.ReceiveOptions(
        amount_split_target=cdk.SplitTarget.NONE(),
        p2pk_signing_keys=[],
        preimages=[],
        metadata={},
    ),
)
print(f"Received: {received.value} sats")
```

A prepared send that is not confirmed should be released with
`await prepared.cancel()`.

### Inspecting a Token

All `Token` methods are synchronous.

```python
token = cdk.Token.decode(encoded_token)

print(token.mint_url().url)
print(token.value().value)
print(token.memo())
print(token.unit())

for proof in token.proofs_simple():
    print(proof.amount.value, proof.keyset_id)
```

### Melt Quote (Lightning Payment)

Melting is two-phase in the same way as sending.

```python
melt_quote = await wallet.melt_quote(
    cdk.PaymentMethod.BOLT11(),
    "lnbc...",
    None,
    None,
)

prepared = await wallet.prepare_melt(melt_quote.id)
print(f"Amount {prepared.amount().value}, fee reserve {prepared.fee_reserve().value}")

finalized = await prepared.confirm()
print(f"Payment preimage: {finalized.preimage}")
print(f"Fee paid: {finalized.fee_paid.value} sats")
```

### Generating Mnemonics

```python
mnemonic = cdk.generate_mnemonic()
print(f"Mnemonic: {mnemonic}")

entropy = cdk.mnemonic_to_entropy(mnemonic)
```

A wallet is restored by passing an existing mnemonic to `cdk.Wallet(...)` and
then calling `await wallet.restore()`.

### Transaction History

```python
transactions = await wallet.list_transactions(None)
for tx in transactions:
    print(f"Amount: {tx.amount.value}, Timestamp: {tx.timestamp}")

incoming = await wallet.list_transactions(cdk.TransactionDirection.INCOMING)
outgoing = await wallet.list_transactions(cdk.TransactionDirection.OUTGOING)

tx = await wallet.get_transaction(transaction_id)
```

### Proof State Management

```python
unspent = await wallet.get_proofs_by_states([cdk.ProofState.UNSPENT])
pending = await wallet.get_proofs_by_states([cdk.ProofState.PENDING])
spent = await wallet.get_proofs_by_states([cdk.ProofState.SPENT])

print(f"Balance: {(await wallet.total_balance()).value} sats")
print(f"Pending: {(await wallet.total_pending_balance()).value} sats")
print(f"Reserved: {(await wallet.total_reserved_balance()).value} sats")
```

### Rate Limiting

Wallets pace their mint traffic by default.

```python
wallet = cdk.Wallet(
    mint_url="https://testnut.cashudevkit.org",
    unit=cdk.CurrencyUnit.SAT(),
    mnemonic=cdk.generate_mnemonic(),
    store=cdk.sqlite_wallet_store(":memory:"),
    config=cdk.WalletConfig(
        target_proof_count=None,
        rate_limit=cdk.RateLimit.CUSTOM(capacity=5, refill_per_minute=30),
    ),
)

wallet.set_rate_limit(cdk.RateLimit.DISABLED())
print(wallet.is_rate_limited())
```

## Development

These bindings are developed in the
[cdk monorepo](https://github.com/cashubtc/cdk) under `bindings/python/`, which
is the source of truth; this repository receives them on each release.

### Building from Source

Requirements:

- Rust 1.85.0 or higher
- Python 3.10 or higher
- just (command runner)

```bash
git clone https://github.com/cashubtc/cdk.git
cd cdk

# Build the wheel for your platform
just binding-python

# Install it and run the test suite
just test-python
```

`just binding-python` builds the `cdk-ffi-python` crate in release mode, runs
`uniffi-bindgen` to generate `src/cdk/cdk_ffi.py`, copies the native library in
beside it, and produces a wheel in `dist/`. The generated module and the library
are build artifacts and are not checked in.

This repository can also build on its own, which is what the release workflow
does: `rust/` is a standalone crate depending on a published `cdk-ffi`, so
`cargo build --release` inside it needs no monorepo checkout.

Note that `uniffi` names the library it loads after the `cdk-ffi` namespace
rather than the wrapper crate, so the built `libcdk_ffi_python.*` is renamed to
`libcdk_ffi.*` when it is copied into the package.

### Running Tests

```bash
# Run all tests
pytest

# Run a specific test file
pytest tests/test_wallet.py

# Run with verbose output
pytest -v
```

The mint-backed tests are skipped unless a mint is configured:

```bash
CDK_PYTHON_TEST_MINT_URL=https://testnut.cashudevkit.org pytest
```

`CDK_PYTHON_MINT_SETTLEMENT_DELAY_SECONDS` controls how long those tests wait
for the mint to settle a quote (default 3).

### Running Examples

```bash
python examples/wallet_setup.py

CDK_PYTHON_TEST_MINT_URL=https://testnut.cashudevkit.org \
    python examples/mint_and_send.py
```

See [`examples/README.md`](examples/README.md) for what each one covers.

## Project Structure

```
cdk-python/
├── rust/                   # cdk-ffi-python, the crate this repo compiles
├── src/
│   └── cdk/                # Python package (generated bindings + native lib)
├── wheels/                 # Prebuilt wheels for every supported platform
├── tests/                  # pytest suite
├── examples/               # Runnable examples
├── build.sh                # Build a wheel for the host platform
├── pyproject.toml          # Package configuration
├── setup.py                # Platform wheel configuration
├── pytest.ini              # Test configuration
├── requirements-dev.txt    # Development dependencies
├── LICENSE.md              # Dual Apache-2.0 / MIT notice
├── LICENSE-APACHE
└── LICENSE-MIT
```

This tree is generated from `bindings/python/` in the cdk monorepo on every
release and replaces the repository contents wholesale, so edits made here are
overwritten. Report issues and send patches against the monorepo.

`rust/` depends on a published `cdk-ffi` rather than on a path inside the
monorepo, so this repository builds on its own:

```bash
./build.sh
```

That compiles the crate, regenerates `src/cdk/cdk_ffi.py`, and writes a wheel to
`dist/`. Point `rust/Cargo.toml` at a different `cdk-ffi` version first if you
want to build against one; the bindings are regenerated to match.

## Documentation

- [CDK Documentation](https://docs.cashu.space)
- [Cashu Protocol](https://github.com/cashubtc/nuts)
- [API Reference](https://docs.rs/cdk)

## Related Projects

- [CDK](https://github.com/cashubtc/cdk) - Rust core library
- [CDK Swift](https://github.com/cashubtc/cdk-swift) - Swift bindings
- [CDK Kotlin](https://github.com/cashubtc/cdk-kotlin) - Kotlin bindings
- [CDK Go](https://github.com/cashubtc/cdk-go) - Go bindings
- [CDK Dart](https://github.com/cashubtc/cdk-dart) - Dart bindings

## Contributing

Contributions are welcome. Because these bindings are generated from
`crates/cdk-ffi` in the cdk monorepo, changes to the API belong there rather
than in this repository.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## License

Licensed under either of Apache License, Version 2.0 or MIT license at your
option.

## Support

- GitHub Issues: https://github.com/cashubtc/cdk/issues
- Discord: https://discord.gg/cashu
- Telegram: https://t.me/CashuBTC

## Acknowledgments

Built with [UniFFI](https://mozilla.github.io/uniffi-rs/) by Mozilla.
