use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{RecoveryOptions, RecoveryStrategy, WalletRepository};
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
    /// Use legacy linear scan recovery
    #[arg(long, default_value_t = false)]
    legacy_scan: bool,
}

pub async fn restore(
    wallet_repository: &WalletRepository,
    sub_command_args: &RestoreSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    let strategy = if sub_command_args.legacy_scan {
        RecoveryStrategy::LinearScan
    } else {
        RecoveryStrategy::Fast
    };

    let restored = wallet
        .restore_with_options(RecoveryOptions { strategy })
        .await?;

    println!("Restored: {}", restored.unspent);
    println!("Spent: {}", restored.spent);
    println!("Pending: {}", restored.pending);

    Ok(())
}
