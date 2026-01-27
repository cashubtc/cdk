use std::collections::BTreeMap;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletRepository;
use cdk::Amount;

pub async fn balance(wallet_repository: &WalletRepository) -> Result<()> {
    // Show individual mint balances
    let mint_balances = mint_balances(wallet_repository).await?;

    // Show total balance using the new unified interface
    let total = wallet_repository.total_balance().await?;
    if !mint_balances.is_empty() {
        println!();
        println!("Total balance across all wallets: {}", total);
    }

    Ok(())
}

pub async fn mint_balances(
    wallet_repository: &WalletRepository,
) -> Result<Vec<(MintUrl, Amount)>> {
    let wallets: BTreeMap<MintUrl, Amount> = wallet_repository.get_balances().await?;

    let mut wallets_vec = Vec::with_capacity(wallets.len());

    for (i, (mint_url, amount)) in wallets
        .iter()
        .filter(|(_, a)| a > &&Amount::ZERO)
        .enumerate()
    {
        let mint_url = mint_url.clone();
        // Get the wallet to show its unit
        if let Some(wallet) = wallet_repository.get_wallet(&mint_url).await {
            println!("{i}: {mint_url} {amount} {}", wallet.unit);
        } else {
            println!("{i}: {mint_url} {amount}");
        }
        wallets_vec.push((mint_url, *amount))
    }
    Ok(wallets_vec)
}
