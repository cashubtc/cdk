# CDK (Cashu Development Kit)

[![crates.io](https://img.shields.io/crates/v/cdk.svg)](https://crates.io/crates/cdk)
[![Documentation](https://docs.rs/cdk/badge.svg)](https://docs.rs/cdk)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

**ALPHA** This library is in early development, the API will change and should be used with caution.

The core implementation of the Cashu protocol for building wallets and mints. It builds upon the primitives defined in the `cashu` crate and provides higher-level abstractions for working with the Cashu ecosystem.

## Crate Feature Flags

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |
| `auth`      |   Yes   | Enable blind and clear auth  |

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
use cdk::nuts::{CurrencyUnit, MintQuoteState};
#[cfg(feature = "wallet")]
use cdk::wallet::Wallet;
#[cfg(feature = "wallet")]
use cdk::wallet::SendOptions;
use cdk::Amount;
use rand::random;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    #[cfg(feature = "wallet")]
    {
        let seed = random::<[u8; 64]>();

        let mint_url = "https://fake.thesimplekid.dev";
        let unit = CurrencyUnit::Sat;
        let amount = Amount::from(10);

        let localstore = memory::empty().await.unwrap();

        let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None).unwrap();

        let quote = wallet.mint_quote(amount, None).await.unwrap();

        println!("Pay request: {}", quote.request);

        loop {
            let status = wallet.mint_quote_state(&quote.id).await.unwrap();

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

The `cdk` library should always compile with any combination of features on Rust **1.75.0**.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
    cargo update -p async-compression --precise 0.4.3
    cargo update -p zstd-sys --precise 2.0.8+zstd.1.5.5
    cargo update -p flate2 --precise 1.0.35
    cargo update -p home --precise 0.5.5
    cargo update -p zerofrom --precise 0.1.5
    cargo update -p half --precise 2.4.1
    cargo update -p url --precise 2.5.2
    # For wasm32-unknown-unknown target
    cargo update -p triomphe --precise 0.1.11
```

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).
