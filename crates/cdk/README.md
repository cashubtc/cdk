# CDK (Cashu Development Kit)

[![crates.io](https://img.shields.io/crates/v/cdk.svg)](https://crates.io/crates/cdk)
[![Documentation](https://docs.rs/cdk/badge.svg)](https://docs.rs/cdk)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA:** This library is in early development. Its API may change and should
be used with care.

`cdk` implements the Cashu protocol for building wallets and mints. It builds
on the protocol types in `cashu` and provides durable, higher-level workflows
for issuance, ecash transfer, external payments, recovery, and mint operation.

## Crate feature flags

| Feature | Default | Description |
|---|:---:|---|
| `wallet` | Yes | Cashu wallet workflows and protocol controls |
| `mint` | Yes | Cashu mint implementation |
| `auth` | Yes | Clear and blind authentication |

See the repository [README](https://github.com/cashubtc/cdk/blob/main/README.md)
for the implemented NUTs and workspace-wide documentation.

## Wallet model

`Wallet` represents one mint and currency unit. `WalletManager` coordinates
multiple wallets that share a seed, store, and transport policy. The normal API
uses domain modules, typed requests, resumable sessions, durable
execute-or-cancel plans, operation discovery, application events, and receipts.
Protocol-level proof, keyset, subscription, authentication, and raw import
controls are grouped behind `advanced()`.

## Example

```rust,no_run
use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::mint::MintRequest;
use cdk::wallet::operation::SyncPolicy;
use cdk::wallet::send::SendRequest;
use cdk::wallet::{Wallet, WalletIdentity, WalletOpenRequest};
use cdk::Amount;
use cdk_sqlite::wallet::memory;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(memory::empty().await?);
    let identity = WalletIdentity::new(
        "https://testnut.cashudevkit.org".parse()?,
        CurrencyUnit::Sat,
    );
    let wallet = Wallet::open(WalletOpenRequest::new(identity, store, [0; 64]))?;

    // Startup side effects are explicit.
    wallet.synchronize(SyncPolicy::Online).await?;

    let incoming = wallet
        .request_mint(MintRequest::bolt11(Amount::from(10)))
        .await?;
    println!("Pay request: {}", incoming.initial_state().payment_request);
    let minted = incoming.wait(Duration::from_secs(300)).await?;
    println!("Minted {}", minted.amount);

    // Planning reserves funds. Persist the operation ID, then execute or cancel.
    let plan = wallet.plan_send(SendRequest::new(Amount::ONE)).await?;
    println!("Send operation {}, maximum fee {}", plan.operation_id(), plan.fee());
    let sent = plan.execute().await?;
    println!("{}", sent.token);

    Ok(())
}
```

See the [wallet API guide](../../docs/wallet-api.md) for payment workflows,
restart behavior, multi-mint transfers, FFI parity, and migration guidance.
More runnable examples are in the [examples](./examples) directory.

## Minimum supported Rust version

The workspace MSRV is Rust **1.85.0**. `rust-toolchain.toml` identifies the
toolchain used by the repository.

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).
