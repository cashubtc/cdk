# CDK portable wallet API

`cdk-ffi` is CDK's application-facing wallet facade. The Rust implementation
is the single source for Rust applications and generated Python, Swift, Kotlin,
Dart, and Go bindings.

The facade offers a compact workflow API:

- `Wallet` for one mint and currency unit;
- `CashuWallet` as a multi-mint root;
- request records and typed `PaymentTarget` values;
- mint/payment sessions;
- durable send, payment, pending-payment, and cross-mint plans;
- explicit synchronization and structured errors.

Proof, keyset, swap, and other protocol-level controls live in the `cdk` wallet
engine. The architecture and lifecycle contract are documented in
[`docs/wallet-api.md`](../../docs/wallet-api.md).

## Python example

```python
import cdk_ffi

wallet = cdk_ffi.Wallet.open(
    cdk_ffi.WalletOpenRequest(
        mint_url="https://mint.example.com",
        unit=cdk_ffi.CurrencyUnit.SAT(),
        mnemonic=cdk_ffi.generate_mnemonic(),
        store=cdk_ffi.WalletStore.SQLITE(path="wallet.sqlite"),
    )
)

balance = await wallet.balance()
session = await wallet.request_minting(
    cdk_ffi.MintRequest(
        method=cdk_ffi.PaymentMethod.BOLT11(),
        amount=cdk_ffi.Amount(value=1_000),
    )
)
print(session.initial_state().payment_request)
```

## Development

```bash
just ffi-check                 # Rust compile check
just ffi-api-check             # generated API manifest check
just ffi-generate python       # one language
just ffi-generate-all          # Python, Swift, and Kotlin
just ffi-test                  # deterministic Python tests
just ffi-test-live-python      # live testnut payment test
```

The live Python test covers mint sessions, typed on-chain quoting,
`PaymentPlan.confirm_prefer_async()`, immediate and pending outcomes, durable
pending-handle reconstruction, and startup-style synchronization.

Production packages are published in the `cashubtc/cdk-swift`,
`cashubtc/cdk-kotlin`, `cashubtc/cdk-go`, `cashubtc/cdk-dart`, and
`cashubtc/cdk-python` repositories.
