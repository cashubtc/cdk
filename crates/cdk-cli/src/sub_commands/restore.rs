use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
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
    let _unit = CurrencyUnit::from_str(&sub_command_args.unit)?;
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = match multi_mint_wallet.get_wallet(&mint_url).await {
        Some(wallet) => wallet.clone(),
        None => {
            multi_mint_wallet.add_mint(mint_url.clone(), None).await?;
            multi_mint_wallet
                .get_wallet(&mint_url)
                .await
                .expect("Wallet should exist after adding mint")
                .clone()
        }
    };

    let amount = wallet.restore().await?;

    println!("Restored {amount}");

    Ok(())
}
