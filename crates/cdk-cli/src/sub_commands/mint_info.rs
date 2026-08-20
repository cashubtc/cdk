use std::collections::HashSet;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::{Wallet, WalletRepository};
use clap::Args;

use crate::terminal::{escape_control, escape_json};

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: Option<MintUrl>,
}

pub async fn mint_info(
    wallet_repository: &WalletRepository,
    sub_command_args: &MintInfoSubcommand,
) -> Result<()> {
    if let Some(mint_url) = &sub_command_args.mint_url {
        match wallet_repository.fetch_mint_info(mint_url).await {
            Ok(info) => {
                // Mint info is entirely mint-controlled (URLs, custom currency units,
                // descriptions); escape control characters before printing.
                println!("{}", escape_json(&serde_json::to_string_pretty(&info)?));
            }
            Err(fetch_err) => {
                let wallets: Vec<Wallet> = wallet_repository.get_wallets_for_mint(mint_url).await;

                if let Some(wallet) = wallets.first() {
                    match wallet.load_mint_info().await {
                        Ok(mint_info) => {
                            println!(
                                "{}",
                                escape_json(&serde_json::to_string_pretty(&mint_info)?)
                            );
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Cannot fetch mint info {mint_url}: fetch failed {fetch_err}, cache failed {e}"));
                        }
                    }
                } else {
                    return Err(anyhow::anyhow!(
                        "Cannot fetch mint info {mint_url}: {fetch_err}"
                    ));
                }
            }
        };
    } else {
        let mut seen = HashSet::new();
        for (i, wallet) in wallet_repository
            .get_wallets()
            .await
            .iter()
            .filter(|w| seen.insert(w.mint_url.clone()))
            .enumerate()
        {
            let mint_url = wallet.mint_url.clone();
            match wallet.load_mint_info().await {
                Ok(info) => {
                    println!("{i}: {}", escape_control(&mint_url.to_string()));
                    println!("{}", escape_json(&serde_json::to_string_pretty(&info)?));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Cannot fetch mint info {mint_url}: {e}"));
                }
            };
        }
    }

    Ok(())
}
