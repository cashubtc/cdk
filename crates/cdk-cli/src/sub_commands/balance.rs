use std::collections::BTreeMap;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::MultiMintWallet;
use cdk::Amount;

pub async fn balance(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    // Show individual mint balances
    let mint_balances = mint_balances(multi_mint_wallet, multi_mint_wallet.unit()).await?;

    // Show total balance using the new unified interface
    let total = multi_mint_wallet.total_balance().await?;
    if !mint_balances.is_empty() {
        println!();
        println!(
            "Total balance across all wallets: {} {}",
            total,
            multi_mint_wallet.unit()
        );
    }

    Ok(())
}

pub async fn mint_balances(
    multi_mint_wallet: &MultiMintWallet,
    unit: &CurrencyUnit,
) -> Result<Vec<(MintUrl, Amount)>> {
    let wallets: BTreeMap<MintUrl, Amount> = multi_mint_wallet.get_balances().await?;

    let mut wallets_vec = Vec::with_capacity(wallets.len());

    for (i, (mint_url, amount)) in wallets
        .iter()
        .filter(|(_, a)| a > &&Amount::ZERO)
        .enumerate()
    {
        let mint_url = mint_url.clone();
        println!("{i}: {mint_url} {amount} {unit}");
        wallets_vec.push((mint_url, *amount))
    }
    Ok(wallets_vec)
}
