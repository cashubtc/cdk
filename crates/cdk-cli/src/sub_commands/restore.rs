use std::collections::HashMap;

use anyhow::{anyhow, Result};
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: UncheckedUrl,
}

pub async fn restore(
    wallets: HashMap<UncheckedUrl, Wallet>,
    sub_command_args: &RestoreSubCommand,
) -> Result<()> {
    let wallet = wallets
        .get(&sub_command_args.mint_url)
        .ok_or(anyhow!("Unknown mint url"))?;

    let amount = wallet.restore().await?;

    println!("Restored {}", amount);

    Ok(())
}
