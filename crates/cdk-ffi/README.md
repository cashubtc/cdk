# CDK language bindings

`cdk-ffi` is a thin UniFFI exposure of the workflow API implemented by
`cdk::wallet`. Generated Swift, Kotlin, Python, Dart, and Go bindings use the
same object lifecycle as Rust:

- `Wallet` for one mint and currency unit;
- `WalletManager` for a multi-mint application;
- request records and typed payment targets;
- resumable mint sessions;
- durable send, payment, request-payment, and cross-mint plans;
- durable operation discovery and high-level application events;
- explicit synchronization, receipts, history, and structured errors.

Wallet selection, proof reservation, payment execution, recovery, and other
business rules remain in `cdk`. This crate only converts binding-safe values
and bridges object lifetimes.

## Python example

```python
import cdk_ffi

wallet = cdk_ffi.Wallet.open(
    cdk_ffi.WalletOpenRequest(
        mint_url="https://mint.example.com",
        unit=cdk_ffi.CurrencyUnit.SAT(),
        mnemonic=cdk_ffi.generate_mnemonic(),
        store=cdk_ffi.WalletStore.SQLITE(path="wallet.sqlite"),
        config=None,
    )
)

local_balance = await wallet.balance()
await wallet.synchronize(cdk_ffi.SyncPolicy.ONLINE)

session = await wallet.request_mint(
    cdk_ffi.MintRequest(
        method=cdk_ffi.PaymentMethod.BOLT11(),
        amount=cdk_ffi.Amount(value=1_000),
        description="Coffee",
        extra=None,
    )
)
print(session.initial_state().payment_request)
```

Persist session quote IDs and plan operation IDs before any external side
effect. Rebuild state with `operations()`, then follow each typed resume
instruction; do not infer success from a dropped binding object.

## Advanced bindings

The default generated API intentionally omits proof, keyset, authentication,
raw import, explicit funding, and low-level subscription controls. Build with
`--features advanced-wallet` to add `advanced_wallet`, `advanced_manager`, and
the related expert records. This changes only the exported surface: the
advanced objects still delegate to the same core wallet workflows.

## Development

```bash
just ffi-check
just ffi-generate python
just ffi-generate-all
just ffi-test
just ffi-test-live-python
```

`cargo check -p cdk-ffi --all-targets` validates the normal Rust/UniFFI
surface. `cargo check -p cdk-ffi --all-targets --features advanced-wallet`
validates the opt-in expert surface. Binding smoke tests should exercise the
same request → session/plan → receipt flow as Rust integration tests.

The complete architecture, durability contract, and previous-to-current API
mapping are in [the wallet API guide](../../docs/wallet-api.md).

Production packages are published in the `cashubtc/cdk-swift`,
`cashubtc/cdk-kotlin`, `cashubtc/cdk-go`, `cashubtc/cdk-dart`, and
`cashubtc/cdk-python` repositories.
