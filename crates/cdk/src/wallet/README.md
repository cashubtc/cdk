# CDK wallet

`cdk::wallet` is the shared application API and protocol engine for Cashu
wallets. `Wallet` represents one mint and currency unit; `WalletManager`
coordinates multiple wallets over one seed, store, and transport policy.

The ordinary API is organized around durable workflows:

- `request_mint` returns a resumable `MintSession`;
- `plan_send` returns a reviewable `SendPlan`;
- `receive` redeems an encoded token;
- `quote_payment` returns `PaymentSession` values that prepare `PaymentPlan`s;
- `operations` discovers every locally durable session or plan after restart;
- `events` emits application-level balance, operation, and history changes;
- `synchronize` explicitly recovers and reconciles durable state;
- `history` and balance methods return application-safe views.

Proof inspection, denomination selection, raw imports, keysets,
subscriptions, authentication, Nostr backup, and similar protocol controls are
available through `wallet.advanced()` or `manager.advanced()`. Those handles
use the same wallet implementation; they are an intentional API boundary, not
a facade.

## Example

```rust,no_run
use std::sync::Arc;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::send::SendRequest;
use cdk::wallet::{Wallet, WalletIdentity, WalletOpenRequest};
use cdk::Amount;
use cdk_sqlite::wallet::memory;

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let store = Arc::new(memory::empty().await?);
let identity = WalletIdentity::new(
    "https://mint.example.com".parse()?,
    CurrencyUnit::Sat,
);
let wallet = Wallet::open(WalletOpenRequest::new(identity, store, [0; 64]))?;
wallet.synchronize(SyncPolicy::Online).await?;

let incoming = wallet
    .request_mint(MintRequest::bolt11(Amount::from(1_000)))
    .await?;
println!("Pay: {}", incoming.initial_state().payment_request);

let send = wallet.plan_send(SendRequest::new(Amount::from(100))).await?;
println!("Operation {}, maximum fee {}", send.operation_id(), send.fee());
// Call send.execute().await? or send.cancel().await?.
# Ok(())
# }
```

Persist quote and operation identifiers before execution or waiting. A
dropped plan remains durable and reserved until it is executed, canceled, or
reconciled according to recovery rules.

See [the wallet API architecture](../../../../docs/wallet-api.md) for the full
workflow map, restart behavior, concurrency guarantees, FFI parity, and
migration table.
