# CDK Wallet

The CDK [`Wallet`] is a high level Cashu wallet. The [`Wallet`] is for a single mint and single unit. Multiple [`Wallet`]s can be created to support multi mints and multi units.


## Example

### Create [`Wallet`]
```rust
  use std::str::FromStr;
  use std::sync::Arc;
  use cdk::mint_url::MintUrl;
  use cdk::nuts::CurrencyUnit;
  use cdk::wallet::Wallet;
  use rand::Rng;

  let seed = rand::thread_rng().gen::<[u8; 32]>();
  let mint_url = MintUrl::from_str("https://testnut.cashu.space").unwrap();
  let unit = CurrencyUnit::Sat;

  let wallet = Wallet::builder(seed.to_vec()).build(mint_url, unit).unwrap();
```
