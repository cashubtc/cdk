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

    // Wallets are shared through the repository, so rename the mint in storage
    // directly. The CLI exits after this command and rebuilds the repository
    // from storage on the next run, so the stale in-memory entry is harmless.
    wallet_repository
        .get_wallet(old_mint_url, unit)
        .await?
        .localstore
        .update_mint_url(old_mint_url.clone(), new_mint_url.clone())
        .await?;

    println!("Mint Url changed from {old_mint_url} to {new_mint_url}");

    Ok(())
}
