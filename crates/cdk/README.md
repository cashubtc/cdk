
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
use cdk::cdk_database::WalletMemoryDatabase;
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

    let localstore = WalletMemoryDatabase::default();

    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed);

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


## License

This project is distributed under the MIT software license - see the [LICENSE](../../LICENSE) file for details
