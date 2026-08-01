# CDK (Cashu Development Kit)

[![crates.io](https://img.shields.io/crates/v/cdk.svg)](https://crates.io/crates/cdk)
[![Documentation](https://docs.rs/cdk/badge.svg)](https://docs.rs/cdk)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

The core implementation of the Cashu protocol for building wallets and mints. It builds upon the primitives defined in the `cashu` crate and provides higher-level abstractions for working with the Cashu ecosystem.

## Crate Feature Flags

The following crate feature flags are available:

| Feature      | Default | Description                                      |
|--------------|:-------:|--------------------------------------------------|
| `wallet`     |   Yes   | Enable Cashu wallet features                     |
| `mint`       |   Yes   | Enable Cashu mint features                       |
| `nostr`      |   Yes   | Enable Nostr wallet integration                  |
| `bip353`     |   Yes   | Enable BIP-353 DNS resolution                    |
| `npubcash`   |    No   | Enable npub.cash integration                     |
| `nwc`        |    No   | Enable Nostr Wallet Connect                      |
| `tor`        |    No   | Enable the explicit native Tor wallet client     |
| `iroh`       |    No   | Enable native hybrid HTTP(S)/Iroh wallet routing |
| `prometheus` |    No   | Enable Prometheus metrics                        |
| `federation` |    No   | Enable federation support                        |

## Implemented [NUTs](https://github.com/cashubtc/nuts/):

See <https://github.com/cashubtc/cdk/blob/main/README.md>

## Components

The crate includes several key modules:

- **wallet**: Implementation of the Cashu wallet
- **mint**: Implementation of the Cashu mint
- **database**: Database abstractions for persistent storage
- **payment**: Payment processing functionality
- **nuts**: Implementation of the Cashu NUTs

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk = "*"
```

## Example

```rust,no_run
//! Wallet example with memory store
//! Note: This example requires the "wallet" feature to be enabled (enabled by default)

use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "wallet")]
use cdk::amount::SplitTarget;
use cdk_sqlite::wallet::memory;
use cdk::nuts::{CurrencyUnit, MintQuoteState, PaymentMethod};
#[cfg(feature = "wallet")]
use cdk::wallet::{RecoveryReport, SendOptions, Wallet};
use cdk::Amount;
use rand::random;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    #[cfg(feature = "wallet")]
    {
        let seed = random::<[u8; 64]>();

        let mint_url = "https://testnut.cashudevkit.org";
        let unit = CurrencyUnit::Sat;
        let amount = Amount::from(10);

        let localstore = memory::empty().await.unwrap();

        let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();

        // Required: Recover from interrupted operations (swap, send, receive, melt)
        let recovery: RecoveryReport = wallet.recover_incomplete_sagas().await.unwrap();
        println!("Recovered {} operations", recovery.recovered);

        // Optional: Check and mint pending mint quotes (requires network)
        let _minted = wallet.mint_unissued_quotes().await.unwrap();

        let quote = wallet.mint_quote(PaymentMethod::BOLT11, Some(amount), None, None).await.unwrap();

        println!("Pay request: {}", quote.request);

        loop {
            let status = wallet.check_mint_quote_status(&quote.id).await.unwrap();

            if status.state == MintQuoteState::Paid {
                break;
            }

            println!("Quote state: {}", status.state);

            sleep(Duration::from_secs(5)).await;
        }

        let receive_amount = wallet
            .mint(&quote.id, SplitTarget::default(), None)
            .await
            .unwrap();

        println!("Minted {:?}", receive_amount);

        // Send the token
        let prepared_send = wallet.prepare_send(Amount::ONE, SendOptions::default()).await.unwrap();
        let token = prepared_send.confirm(None).await.unwrap();

        println!("{}", token);
    }
}
```

See more examples in the [examples](./examples) folder.

## Minimum Supported Rust Version (MSRV)

The workspace MSRV is Rust **1.91.0**. The optional native `iroh` feature uses
the current stable Iroh 1.0 API and is not available on browser/WASM targets.

`Cargo.lock` and `Cargo.lock.msrv` are kept byte-identical. Verify the checked-in
dependency graph with:

```shell
cargo check --workspace --all-targets --locked
```

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).
