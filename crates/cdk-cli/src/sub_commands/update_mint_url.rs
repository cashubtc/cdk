use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{WalletIdentity, WalletManager};
use clap::Args;

#[derive(Args)]
pub struct UpdateMintUrlSubCommand {
    /// Old Mint Url
    old_mint_url: MintUrl,
    /// New Mint Url
    new_mint_url: MintUrl,
}

pub async fn update_mint_url(
    wallet_manager: &WalletManager,
    sub_command_args: &UpdateMintUrlSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let UpdateMintUrlSubCommand {
        old_mint_url,
        new_mint_url,
    } = sub_command_args;

    let mut wallet = wallet_manager
        .wallet(WalletIdentity::new(
            sub_command_args.old_mint_url.clone(),
            unit.clone(),
        ))
        .await?;

    wallet
        .advanced_mut()
        .relocate_mint(new_mint_url.clone())
        .await?;

    println!("Mint Url changed from {old_mint_url} to {new_mint_url}");

    Ok(())
}
