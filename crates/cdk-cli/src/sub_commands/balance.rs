use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct BalanceSubCommand {
    /// Currency unit e.g. sat
    #[arg(long, default_value = "sat")]
    pub unit: String,
}

pub async fn balance(multi_mint_wallet: &MultiMintWallet, unit: &CurrencyUnit) -> Result<()> {
    mint_balances(multi_mint_wallet, &unit).await?;
    Ok(())
}

pub async fn balance_command(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &BalanceSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    balance(multi_mint_wallet, &unit).await
}

pub async fn mint_balances(
    multi_mint_wallet: &MultiMintWallet,
    unit: &CurrencyUnit,
) -> Result<Vec<(MintUrl, Amount)>> {
    let wallets: BTreeMap<MintUrl, Amount> = multi_mint_wallet.get_balances(unit).await?;

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
