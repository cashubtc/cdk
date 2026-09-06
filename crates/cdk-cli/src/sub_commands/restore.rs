use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{RestoreRequest, WalletManager};
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
}

pub async fn restore(
    wallet_manager: &WalletManager,
    sub_command_args: &RestoreSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = get_or_create_wallet(wallet_manager, &mint_url, unit).await?;

    let restored = wallet.restore_from_seed(RestoreRequest::default()).await?;

    println!("Restored: {}", restored.unspent);
    println!("Spent: {}", restored.spent);
    println!("Pending: {}", restored.pending);

    Ok(())
}
