use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
}

pub async fn restore(
    wallet_repository: &WalletRepository,
    sub_command_args: &RestoreSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = wallet_repository
        .get_or_create_wallet(&mint_url, unit.clone())
        .await?;

    let restored = wallet.restore().await?;

    println!("Restored: {}", restored.unspent);
    println!("Spent: {}", restored.spent);
    println!("Pending: {}", restored.pending);

    Ok(())
}
