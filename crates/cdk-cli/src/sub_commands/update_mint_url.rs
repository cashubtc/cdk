use anyhow::{anyhow, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::MultiMintWallet;
use clap::Args;

#[derive(Args)]
pub struct UpdateMintUrlSubCommand {
    /// Old Mint Url
    old_mint_url: MintUrl,
    /// New Mint Url
    new_mint_url: MintUrl,
}

pub async fn update_mint_url(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &UpdateMintUrlSubCommand,
) -> Result<()> {
    let UpdateMintUrlSubCommand {
        old_mint_url,
        new_mint_url,
    } = sub_command_args;

    let mut wallet = multi_mint_wallet
        .get_wallet(&WalletKey::new(
            sub_command_args.old_mint_url.clone(),
            CurrencyUnit::Sat,
        ))
        .await
        .ok_or(anyhow!("Unknown mint url"))?
        .clone();

    wallet.update_mint_url(new_mint_url.clone()).await?;

    println!("Mint Url changed from {} to {}", old_mint_url, new_mint_url);

    Ok(())
}
