use std::collections::BTreeMap;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use cdk::Amount;
use cdk_common::wallet::WalletKey;

/// Print balances for each mint for the given [`CurrencyUnit`] (e.g. sat from `cdk balance`, or
/// `--unit usd` for USD-only wallets).
pub async fn balance(
    wallet_repository: &WalletRepository,
    filter_unit: &CurrencyUnit,
) -> Result<()> {
    let mint_balances = mint_balances(wallet_repository, filter_unit).await?;

    if !mint_balances.is_empty() {
        // Aggregate totals per currency unit
        let mut unit_totals: BTreeMap<CurrencyUnit, Amount> = BTreeMap::new();
        for (_, unit, amount) in &mint_balances {
            *unit_totals.entry(unit.clone()).or_insert(Amount::ZERO) += *amount;
        }

        println!();
        if unit_totals.len() == 1 {
            if let Some((unit, total)) = unit_totals.into_iter().next() {
                println!("Total balance across all wallets: {} {}", total, unit);
            }
        } else {
            println!("Total balance across all wallets:");
            for (unit, total) in &unit_totals {
                println!("  {} {}", total, unit);
            }
        }
    } else {
        println!("No balance for unit {filter_unit}.");
    }

    Ok(())
}

pub async fn mint_balances(
    wallet_repository: &WalletRepository,
    filter_unit: &CurrencyUnit,
) -> Result<Vec<(MintUrl, CurrencyUnit, Amount)>> {
    let wallets = wallet_repository.get_balances().await?;

    let mut wallets_vec = Vec::with_capacity(wallets.len());

    for (i, (wallet_key, amount)) in wallets
        .iter()
        .filter(|(_, a)| a > &&Amount::ZERO)
        .filter(|(wk, _)| wk.unit == *filter_unit)
        .enumerate()
    {
        let WalletKey { mint_url, unit } = wallet_key.clone();
        println!("{i}: {mint_url} {amount} {unit}");
        wallets_vec.push((mint_url, unit, *amount))
    }
    Ok(wallets_vec)
}
