use std::collections::HashMap;

use anyhow::Result;
use cdk::nuts::CurrencyUnit;
use cdk::url::UncheckedUrl;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::Amount;

pub async fn balance(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    mint_balances(multi_mint_wallet, &CurrencyUnit::Sat).await?;
    Ok(())
}

pub async fn mint_balances(
    multi_mint_wallet: &MultiMintWallet,
    unit: &CurrencyUnit,
) -> Result<Vec<(UncheckedUrl, Amount)>> {
    let wallets: HashMap<UncheckedUrl, Amount> = multi_mint_wallet.get_balances(unit).await?;

    let mut wallets_vec = Vec::with_capacity(wallets.capacity());

    for (i, (mint_url, amount)) in wallets.iter().enumerate() {
        let mint_url = mint_url.clone();
        println!("{i}: {mint_url} {amount}");
        wallets_vec.push((mint_url, *amount))
    }
    Ok(wallets_vec)
}
