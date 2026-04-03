use std::str::FromStr;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct GetPublicKeysSubCommand {
    /// Show the latest public key
    #[arg(long)]
    pub latest: bool,
    /// Mint URL to select wallet context
    #[arg(long)]
    mint_url: Option<String>,
}

pub async fn get_public_keys(
    wallet_repository: &WalletRepository,
    sub_command_args: &GetPublicKeysSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = match &sub_command_args.mint_url {
        Some(url) => MintUrl::from_str(url)?,
        None => {
            let wallets = wallet_repository.get_wallets().await;
            wallets
                .iter()
                .find(|wallet| &wallet.unit == unit)
                .map(|wallet| wallet.mint_url.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("No wallet found for unit {}. Use --mint-url.", unit)
                })?
        }
    };

    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    if sub_command_args.latest {
        let latest_public_key = wallet.get_latest_public_key().await?;

        match latest_public_key {
            Some(key) => {
                println!("\npublic key found!\n");

                println!("public key: {}", key.pubkey.to_hex());
                println!("derivation path: {}", key.derivation_path);
            }
            None => {
                println!("\npublic key not found!\n");
            }
        }

        return Ok(());
    }

    let list_public_keys = wallet.get_public_keys().await?;
    if list_public_keys.is_empty() {
        println!("\n public not found! \n");
        return Ok(());
    }
    println!("\npublic keys found:\n");
    for public_key in list_public_keys {
        println!("public key: {}", public_key.pubkey.to_hex());
        println!("derivation path: {}", public_key.derivation_path);
    }
    Ok(())
}
