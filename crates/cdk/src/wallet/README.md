# CDK Wallet

The CDK [`Wallet`] is a high level Cashu wallet. The [`Wallet`] is for a single mint and single unit. Multiple [`Wallet`]s can be created to support multi mints and multi units.


## Example

### Create [`Wallet`]
```rust
  use std::sync::Arc;
  use cdk::nuts::CurrencyUnit;
  use cdk::wallet::Wallet;
  use cdk_sqlite::wallet::memory;
  use rand::Rng;

  #[tokio::main]
  async fn main() -> anyhow::Result<()> {
    let seed = rand::thread_rng().gen::<[u8; 32]>();
    let mint_url = "https://testnut.cashu.space";
    let unit = CurrencyUnit::Sat;

    let localstore = memory::empty().await?;
    let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None);
    Ok(())
  }
```
