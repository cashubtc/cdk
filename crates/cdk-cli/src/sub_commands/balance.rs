use std::collections::HashMap;

use anyhow::Result;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use cdk::Amount;

pub async fn balance(wallets: HashMap<UncheckedUrl, Wallet>) -> Result<()> {
    mint_balances(wallets).await?;
    Ok(())
}

pub async fn mint_balances(
    wallets: HashMap<UncheckedUrl, Wallet>,
) -> Result<Vec<(Wallet, Amount)>> {
    let mut wallets_vec: Vec<(Wallet, Amount)> = Vec::with_capacity(wallets.capacity());

    for (i, (mint_url, wallet)) in wallets.iter().enumerate() {
        let mint_url = mint_url.clone();
        let amount = wallet.total_balance().await?;
        println!("{i}: {mint_url} {amount}");
        wallets_vec.push((wallet.clone(), amount));
    }
    Ok(wallets_vec)
}
