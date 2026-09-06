use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletManager;
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct GeneratePublicKeySubCommand {
    /// Mint URL to select wallet context
    #[arg(long)]
    mint_url: Option<String>,
}

pub async fn generate_public_key(
    wallet_manager: &WalletManager,
    sub_command_args: &GeneratePublicKeySubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = match &sub_command_args.mint_url {
        Some(url) => MintUrl::from_str(url)?,
        None => {
            let wallets = wallet_manager.wallets().await;
            wallets
                .iter()
                .map(|wallet| wallet.identity())
                .find(|identity| &identity.unit == unit)
                .map(|identity| identity.mint_url)
                .ok_or_else(|| {
                    anyhow::anyhow!("No wallet found for unit {}. Use --mint-url.", unit)
                })?
        }
    };

    let wallet = get_or_create_wallet(wallet_manager, &mint_url, unit).await?;
    let public_key = wallet.advanced().create_signing_key().await?;

    println!("\npublic key generated!\n");
    println!("public key: {}", public_key.to_hex());

    Ok(())
}
