# Wallet API architecture

CDK has two deliberate wallet layers:

| Layer | Crate | Use it for |
|---|---|---|
| Application facade | `cdk-ffi` | Product code, common wallet workflows, and generated Swift/Kotlin/Python/Dart/Go bindings |
| Protocol engine | `cdk` | Proof selection, keysets, NUT-specific controls, custom recovery, and other expert integrations |

The application facade is implemented once in Rust and exported from the same
types with UniFFI. Rust and foreign-language apps therefore use the same
`Wallet`, `CashuWallet`, request records, sessions, plans, outcomes, and error
categories. The engine does not have a second manually mirrored wallet trait.

## Object model

`CashuWallet` is the optional multi-mint root. It owns the seed, storage,
transport configuration, and configured wallets. `Wallet` always represents
exactly one mint and one currency unit.

```text
CashuWallet
  ├── Wallet(mint A, sat)
  ├── Wallet(mint A, usd)
  └── Wallet(mint B, sat)
```

Open `Wallet` directly for a single-mint application. Open `CashuWallet` when
the application needs portfolio-wide balances, history, synchronization, or a
cross-mint transfer.

## Common workflows

### Open and synchronize

```rust,no_run
use cdk_ffi::{
    generate_mnemonic, CurrencyUnit, SyncPolicy, Wallet, WalletConfig,
    WalletOpenRequest, WalletStore,
};

# async fn example() -> Result<(), cdk_ffi::FfiError> {
let wallet = Wallet::open(WalletOpenRequest {
    mint_url: "https://mint.example.com".to_string(),
    unit: CurrencyUnit::Sat,
    mnemonic: generate_mnemonic()?,
    store: WalletStore::Sqlite {
        path: "wallet.sqlite".to_string(),
    },
    config: Some(WalletConfig::default()),
})?;

let local = wallet.balance().await?;
let report = wallet.synchronize(SyncPolicy::Online).await?;
println!("{} available", report.balance.available.value);
# let _ = local;
# Ok(())
# }
```

Construction is local. Network reconciliation happens only when an operation
requires it or when the application explicitly selects `SyncPolicy::Online`.

### Receive an incoming payment

```rust,no_run
# use cdk_ffi::{Amount, MintRequest, PaymentMethod, Wallet};
# async fn example(wallet: &Wallet) -> Result<(), cdk_ffi::FfiError> {
let session = wallet
    .request_minting(MintRequest {
        method: PaymentMethod::Bolt11,
        amount: Some(Amount::new(1_000)),
        description: Some("Coffee".to_string()),
        extra: None,
    })
    .await?;

display_to_payer(session.initial_state().payment_request);
let current = session.refresh().await?;
if matches!(current.state, cdk_ffi::QuoteState::Paid) {
    session.claim().await?;
}
# Ok(())
# }
# fn display_to_payer(_: String) {}
```

The quote ID returned by `MintSession::id` resumes the session with
`Wallet::minting_session` after a restart.

### Send ecash

`Wallet::plan_send(SendRequest)` reserves funds and returns a `SendPlan`.
Review `amount()` and `fee()`, then call exactly one of `confirm()` or
`cancel()`. Persist `operation_id()` before confirmation. Recreate the handle
after a restart with `Wallet::send_plan(operation_id)`.

### Pay Lightning or on-chain

Use `Wallet::quote_payment(PaymentQuoteRequest)` with a typed `PaymentTarget`:

- `Bolt11` for an invoice;
- `Bolt12` for an offer;
- `Onchain` for a Bitcoin address and amount;
- `Custom` for an extension payment rail.

The method returns one or more `PaymentSession` objects; on-chain targets can
have several fee options. Review `quote()`, call `prepare()`, persist the
resulting `PaymentPlan::operation_id`, and then confirm or cancel it.

`confirm_prefer_async()` returns `PaymentConfirmation::Completed` or
`PaymentConfirmation::Pending`. A pending result contains a `PendingPayment`
that can be reconstructed with `Wallet::pending_payment(operation_id)` and
awaited with `wait()`.

## Durable operations

Prepared handles never own the authoritative mutable operation state. Each
handle owns a wallet reference, a stable operation ID, and immutable preview
values. The saga persisted in `WalletDatabase` is authoritative. Confirmation,
cancellation, recovery, and reconstructed handles reload that state.

This gives every fund-reserving workflow the same lifecycle:

```text
request → persisted plan → confirm ──→ completed receipt
                         └ cancel  ──→ funds released
                         └ restart ──→ load by operation ID
```

Dropping a plan does not cancel it. Applications should persist operation IDs
and call `synchronize(Online)` on startup or resume.

## Synchronization

`synchronize(LocalOnly)` reads local balances without contacting a mint.
`synchronize(Online)` recovers interrupted sagas, claims paid mint quotes,
reconciles pending proofs, and finalizes pending payments. `SyncReport` reports
the wallet identity, resulting balances, recovered/compensated/pending/failed
operation counts, and recovered amounts.

`CashuWallet::synchronize` applies the same policy to every configured wallet.

## Cross-mint transfers

`CashuWallet::plan_cross_mint_transfer` creates a persisted maximum-balance
transfer from a source wallet to a destination wallet over Lightning. The plan
is resumed by source operation ID. Confirmation returns either:

- `Completed`, when the source payment and destination issuance both finish;
- `ClaimPending`, when the source payment succeeded but destination issuance
  must be retried. `synchronize(Online)` retries paid destination quotes.

The second outcome is not a failed source payment and must not be retried by
creating another transfer.

## Errors and secrets

`FfiError::Cdk` exposes a stable `WalletErrorKind`, numeric code,
human-readable message, and `retryable` flag. Mint failures retain their Cashu
protocol codes; local request validation uses code `40000`. Applications should
branch on the category and retryability, not parse messages.

Mnemonic, seed, payment proof, token secret, proof secret, and authentication
material are redacted from `Debug` output. Public request records deliberately
do not expose engine signing keys, proofs, or keyset internals.

## Evolving the facade

Add a workflow to `crates/cdk-ffi/src/portable.rs` only when it belongs in the
common application path. Keep protocol controls in `cdk`. Run
`just ffi-api-check` after changing facade objects. The checked-in
`crates/cdk-ffi/wallet-api.manifest` and generated-binding check lock method
signatures, prevent accidental object growth, and prevent legacy engine objects
from leaking back into UniFFI.
