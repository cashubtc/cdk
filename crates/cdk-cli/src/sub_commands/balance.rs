use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use cdk::Amount;
use cdk_common::wallet::WalletKey;

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
) -> Result<Vec<(MintUrl, CurrencyUnit, Amount)>> {
    let wallets = wallet_repository.get_balances().await?;

    let mut wallets_vec = Vec::with_capacity(wallets.len());

    for (i, (wallet_key, amount)) in wallets
        .iter()
        .filter(|(_, a)| a > &&Amount::ZERO)
        .enumerate()
    {
        let WalletKey { mint_url, unit } = wallet_key.clone();
        println!("{i}: {mint_url} {amount} {unit}");
        wallets_vec.push((mint_url, unit, *amount))
    }
    Ok(wallets_vec)
}
