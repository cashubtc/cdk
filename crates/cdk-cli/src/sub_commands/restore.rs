use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::types::WalletKey;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
    /// Currency unit e.g. sat
    #[arg(default_value = "sat")]
    unit: String,
}

pub async fn restore(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &RestoreSubCommand,
) -> Result<()> {
    let unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = match multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
    {
        Some(wallet) => wallet.clone(),
        None => {
            multi_mint_wallet
                .create_and_add_wallet(&mint_url.to_string(), unit, None)
                .await?
        }
    };

    let amount = wallet.restore().await?;

    println!("Restored {amount}");

    Ok(())
}
