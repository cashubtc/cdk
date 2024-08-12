use std::str::FromStr;

use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::WalletKey;
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
    let wallet = multi_mint_wallet
        .get_wallet(&WalletKey::new(sub_command_args.mint_url.clone(), unit))
        .await
        .ok_or(anyhow!("Unknown mint url"))?;

    let amount = wallet.restore().await?;

    println!("Restored {}", amount);

    Ok(())
}
