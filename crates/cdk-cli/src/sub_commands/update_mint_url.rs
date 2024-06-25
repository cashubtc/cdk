use std::collections::HashMap;

use anyhow::{anyhow, Result};
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use clap::Args;

#[derive(Args)]
pub struct UpdateMintUrlSubCommand {
    /// Old Mint Url
    old_mint_url: UncheckedUrl,
    /// New Mint Url
    new_mint_url: UncheckedUrl,
}

pub async fn update_mint_url(
    wallets: HashMap<UncheckedUrl, Wallet>,
    sub_command_args: &UpdateMintUrlSubCommand,
) -> Result<()> {
    let UpdateMintUrlSubCommand {
        old_mint_url,
        new_mint_url,
    } = sub_command_args;

    let mut wallet = wallets
        .get(old_mint_url)
        .ok_or(anyhow!("Unknown mint url"))?
        .clone();

    wallet.update_mint_url(new_mint_url.clone()).await?;

    println!("Mint Url changed from {} to {}", old_mint_url, new_mint_url);

    Ok(())
}
