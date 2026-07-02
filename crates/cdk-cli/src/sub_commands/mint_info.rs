use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::{MintConnector, WalletRepository};
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: Option<MintUrl>,
}

pub async fn mint_info(
    wallet_repository: &WalletRepository,
    proxy: Option<Url>,
    sub_command_args: &MintInfoSubcommand,
) -> Result<()> {
    if let Some(mint_url) = &sub_command_args.mint_url {
        let client = match proxy {
            Some(proxy) => HttpClient::with_proxy(mint_url.clone(), proxy, None, true)?,
            None => HttpClient::new(mint_url.clone(), None),
        };

        match client.get_mint_info().await {
            Ok(info) => {
                println!("{}", serde_json::to_string_pretty(&info)?);
            }
            Err(_) => {
                let wallets = wallet_repository.get_wallets_for_mint(mint_url).await;

                for (i, wallet) in wallets.iter().enumerate() {
                    match wallet.load_mint_info().await {
                        Ok(mint_info) => {
                            println!("{i}: {mint_url}");
                            println!("{}", serde_json::to_string_pretty(&mint_info)?);
                        }
                        Err(e) => {
                            println!("Cannot fetch mint info {mint_url}: {e}")
                        }
                    }
                }
            }
        };
    } else {
        let wallets = wallet_repository.get_wallets().await;
        for (i, wallet) in wallets.iter().enumerate() {
            let mint_url = wallet.mint_url.clone();
            match wallet.load_mint_info().await {
                Ok(info) => {
                    println!("{i}: {mint_url}");
                    println!("{}", serde_json::to_string_pretty(&info)?);
                }
                Err(e) => {
                    println!("Cannot fetch mint info {mint_url}: {e}");
                }
            };
        }
    }

    Ok(())
}
