use std::collections::BTreeMap;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use cdk::Amount;
use cdk_common::wallet::WalletKey;

use crate::terminal::escape_control;

pub async fn balance(
    wallet_repository: &WalletRepository,
    unit: Option<&CurrencyUnit>,
) -> Result<()> {
    // Show individual mint balances
    let mint_balances = mint_balances(wallet_repository, unit).await?;

    if !mint_balances.is_empty() {
        // Aggregate totals per currency unit
        let mut unit_totals: BTreeMap<CurrencyUnit, Amount> = BTreeMap::new();
        for (_, unit, amount) in &mint_balances {
            if cfg!(feature = "experimental-unit-metadata")
                && matches!(unit, CurrencyUnit::Custom(_))
            {
                continue;
            }
            *unit_totals.entry(unit.clone()).or_insert(Amount::ZERO) += *amount;
        }

        if unit_totals.is_empty() {
            return Ok(());
        }
        println!();
        if unit_totals.len() == 1 {
            if let Some((unit, total)) = unit_totals.into_iter().next() {
                println!(
                    "Total balance across all wallets: {}",
                    format_amount(total, &unit)
                );
            }
        } else {
            println!("Total balance across all wallets:");
            for (unit, total) in &unit_totals {
                println!("  {}", format_amount(*total, unit));
            }
        }
    }

    Ok(())
}

pub async fn mint_balances(
    wallet_repository: &WalletRepository,
    target_unit: Option<&CurrencyUnit>,
) -> Result<Vec<(MintUrl, CurrencyUnit, Amount)>> {
    let wallets = wallet_repository.get_balances().await?;

    let mut wallets_vec = Vec::with_capacity(wallets.len());

    for (i, (wallet_key, amount)) in wallets
        .iter()
        .filter(|(k, a)| a > &&Amount::ZERO && target_unit.is_none_or(|u| &k.unit == u))
        .enumerate()
    {
        let WalletKey { mint_url, unit } = wallet_key.clone();
        let mut formatted = format_amount(*amount, &unit);
        if cfg!(feature = "experimental-unit-metadata") {
            let wallet = wallet_repository.get_wallet(&mint_url, &unit).await?;
            match wallet.unit_display_info().await {
                Ok(info) => {
                    formatted = escape_control(&info.format_amount(u64::from(*amount)));
                    if info.precision.is_some_and(|precision| precision > 0) {
                        formatted.push_str(&format!(" (raw amount: {amount})"));
                    }
                    if let Some(pending) = info.pending_precision {
                        formatted.push_str(&format!(" [precision changed to {pending}; showing raw amount until acknowledged]"));
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        "Display metadata unavailable: {}",
                        escape_control(&error.to_string())
                    );
                    formatted = format!("{} {}", amount, escape_control(&unit.to_string()));
                }
            }
        }
        println!("{i}: {} {formatted}", escape_control(&mint_url.to_string()));
        wallets_vec.push((mint_url, unit, *amount))
    }
    Ok(wallets_vec)
}

fn format_amount(amount: Amount, unit: &CurrencyUnit) -> String {
    if cfg!(feature = "experimental-unit-metadata") {
        escape_control(
            &cdk::nuts::UnitDisplayInfo::resolve(&unit.to_string(), None)
                .format_amount(u64::from(amount)),
        )
    } else {
        format!("{} {}", amount, escape_control(&unit.to_string()))
    }
}
