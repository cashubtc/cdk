use std::collections::HashSet;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletRepository;
use clap::Args;

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
                println!("{}", serde_json::to_string_pretty(&info)?);
            }
            Err(_) => {
                let wallets = wallet_repository.get_wallets_for_mint(mint_url).await;

                if wallets.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Cannot fetch mint info {mint_url}: No wallet found for this mint"
                    ));
                }

                for (i, wallet) in wallets.iter().enumerate() {
                    match wallet.load_mint_info().await {
                        Ok(mint_info) => {
                            println!("{i}: {mint_url}");
                            println!("{}", serde_json::to_string_pretty(&mint_info)?);
                        }
                        Err(e) => {
                            return Err(anyhow::anyhow!("Cannot fetch mint info {mint_url}: {e}"));
                        }
                    }
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
                    println!("{i}: {mint_url}");
                    println!("{}", serde_json::to_string_pretty(&info)?);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Cannot fetch mint info {mint_url}: {e}"));
                }
            };
        }
    }

    Ok(())
}
