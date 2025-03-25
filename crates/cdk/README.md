
# Cashu Development Kit

**ALPHA** This library is in early development, the api will change and should be used with caution.

CDK is the core crate implementing the cashu protocol for both the Wallet and Mint.

## Crate Feature Flags

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |

## Implemented [NUTs](https://github.com/cashubtc/nuts/):

See <https://github.com/cashubtc/cdk/blob/main/README.md>

## Examples

```rust
//! Wallet example with memory store

use std::sync::Arc;
use std::time::Duration;

use cdk::amount::SplitTarget;
use cdk_sqlite::wallet::memory;
use cdk::nuts::{CurrencyUnit, MintQuoteState};
use cdk::wallet::Wallet;
use cdk::Amount;
use rand::Rng;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let seed = rand::thread_rng().gen::<[u8; 32]>();

    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let localstore = memory::empty().await.unwrap();

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None, None);

    let quote = wallet.mint_quote(amount).await.unwrap();

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

    println!("Minted {}", receive_amount);

    let token = wallet
        .send(amount, None, None, &SplitTarget::None)
        .await
        .unwrap();

    println!("{}", token);
}

```

See more examples in the [examples](./examples) folder.

## Minimum Supported Rust Version (MSRV)

The `cdk` library should always compile with any combination of features on Rust **1.63.0**.

To build and test with the MSRV you will need to pin the below dependency versions:

```shell
cargo update -p half --precise 2.2.1
cargo update -p tokio --precise 1.38.1
cargo update -p reqwest --precise 0.12.4
cargo update -p serde_with --precise 3.1.0
cargo update -p regex --precise 1.9.6
cargo update -p backtrace --precise 0.3.58
# For wasm32-unknown-unknown target
cargo update -p bumpalo --precise 3.12.0
```


## License

This project is distributed under the MIT software license - see the [LICENSE](../../LICENSE) file for details
# CDK (Cashu Development Kit)

[![crates.io](https://img.shields.io/crates/v/cdk.svg)](https://crates.io/crates/cdk)
[![Documentation](https://docs.rs/cdk/badge.svg)](https://docs.rs/cdk)
[![MIT licensed](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/cashubtc/cdk/blob/main/LICENSE)

The core implementation of the Cashu protocol for building wallets and mints.

## Overview

The `cdk` crate provides the main implementation of the Cashu protocol, offering a comprehensive toolkit for building Cashu wallets and mints. It builds upon the primitives defined in the `cashu` crate and provides higher-level abstractions for working with the Cashu ecosystem.

## Features

- **Wallet Implementation**: Complete wallet functionality for managing tokens, proofs, and transactions
- **Mint Implementation**: Server-side functionality for operating a Cashu mint
- **Database Abstractions**: Interfaces for persistent storage of wallet and mint data
- **Payment Processing**: Handling of Lightning Network payments and other payment methods
- **NUTs Implementation**: Full implementation of the Cashu NUTs (Notation, Usage, and Terminology)

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
cdk = "0.8.1"
```

### Wallet Example

```rust
use cdk::wallet::{Wallet, WalletBuilder};
use cdk_common::mint_url::MintUrl;
use std::str::FromStr;

async fn create_wallet() {
    // Create a wallet with an in-memory database
    let wallet = WalletBuilder::default()
        .build()
        .await
        .expect("Failed to create wallet");
    
    // Add a mint to the wallet
    let mint_url = MintUrl::from_str("https://example.mint").expect("Valid mint URL");
    wallet.add_mint(mint_url).await.expect("Failed to add mint");
    
    // Now you can receive, send tokens, etc.
}
```

### Mint Example

```rust
use cdk::mint::{Mint, MintBuilder};
use cdk::mint::config::MintConfig;

async fn create_mint() {
    // Create a mint configuration
    let config = MintConfig::default();
    
    // Build a mint with the configuration
    let mint = MintBuilder::new(config)
        .build()
        .await
        .expect("Failed to create mint");
    
    // The mint can now process requests, issue tokens, etc.
}
```

## Components

The crate includes several key modules:

- **wallet**: Implementation of the Cashu wallet
- **mint**: Implementation of the Cashu mint
- **database**: Database abstractions for persistent storage
- **payment**: Payment processing functionality
- **nuts**: Implementation of the Cashu NUTs

## License

This project is licensed under the [MIT License](https://github.com/cashubtc/cdk/blob/main/LICENSE).
