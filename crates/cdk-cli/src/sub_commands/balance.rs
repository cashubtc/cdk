use std::collections::HashMap;

use anyhow::Result;
use cdk::nuts::CurrencyUnit;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use cdk::Amount;

pub async fn balance(wallet: Wallet) -> Result<()> {
    let _ = mint_balances(&wallet).await;
    Ok(())
}

pub async fn mint_balances(
    wallet: &Wallet,
) -> Result<Vec<(UncheckedUrl, HashMap<CurrencyUnit, Amount>)>> {
    let mints_amounts: Vec<(UncheckedUrl, HashMap<_, _>)> =
        wallet.mint_balances().await?.into_iter().collect();

    for (i, (mint, balance)) in mints_amounts.iter().enumerate() {
        println!("{i}: {mint}:");
        for (unit, amount) in balance {
            println!("- {amount} {unit}");
        }
        println!("---------");
    }

    Ok(mints_amounts)
}
