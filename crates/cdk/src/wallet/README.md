# CDK Wallet

The CDK [`Wallet`] is a high level Cashu wallet. The [`Wallet`] is for a single mint and single unit. Multiple [`Wallet`]s can be created to support multi mints and multi units.


## Example

### Create [`Wallet`]
```rust
  use std::sync::Arc;
  use cdk::cdk_database::WalletMemoryDatabase;
  use cdk::nuts::CurrencyUnit;
  use cdk::wallet::Wallet;
  use rand::Rng;

  let seed = rand::thread_rng().gen::<[u8; 32]>();
  let mint_url = "https://testnut.cashu.space";
  let unit = CurrencyUnit::Sat;

  let localstore = WalletMemoryDatabase::default();
  let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None);
```
