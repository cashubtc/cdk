
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
use rand::random;
use tokio::time::sleep;

#[tokio::main]
async fn main() {
    let seed = Arc::new(random::<[u8; 32]>());

    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(10);

    let localstore = memory::empty().await.unwrap();

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed);

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
