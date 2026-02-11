use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use clap::Args;

#[derive(Args)]
pub struct UpdateMintUrlSubCommand {
    /// Old Mint Url
    old_mint_url: MintUrl,
    /// New Mint Url
    new_mint_url: MintUrl,
}

pub async fn update_mint_url(
    wallet_repository: &WalletRepository,
    sub_command_args: &UpdateMintUrlSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let UpdateMintUrlSubCommand {
        old_mint_url,
        new_mint_url,
    } = sub_command_args;

    let mut wallet = wallet_repository
        .get_wallet(&sub_command_args.old_mint_url, unit)
        .await?
        .clone();

    wallet.update_mint_url(new_mint_url.clone()).await?;

    println!("Mint Url changed from {old_mint_url} to {new_mint_url}");

    Ok(())
}
