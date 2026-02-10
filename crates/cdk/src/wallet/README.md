# CDK Wallet

The CDK [`Wallet`] is a high level Cashu wallet. The [`Wallet`] is for a single mint and single unit. Multiple [`Wallet`]s can be created to support multi mints and multi units.


## Example

### Create and Initialize [`Wallet`]

```rust
use std::sync::Arc;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seed = random::<[u8; 64]>();
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), seed, None)?;

    // Required: Recover crashed operations (swap, send, receive, melt)
    // This prevents proofs from being stuck in reserved states.
    let report = wallet.recover_incomplete_sagas().await?;
    println!("Recovered: {}, Compensated: {}, Skipped: {}, Failed: {}",
        report.recovered, report.compensated, report.skipped, report.failed);

    // Optional: Check and mint pending mint quotes (makes network calls)
    let minted = wallet.mint_unissued_quotes().await?;
    println!("Minted {} from pending quotes", minted);

    Ok(())
}
```
