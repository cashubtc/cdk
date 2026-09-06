use std::collections::HashSet;

use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::advanced::{MetadataSource, MintMetadataRequest};
use cdk::wallet::WalletManager;
use clap::Args;

use crate::terminal::{escape_control, escape_json};

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: Option<MintUrl>,
}

pub async fn mint_info(
    wallet_manager: &WalletManager,
    sub_command_args: &MintInfoSubcommand,
) -> Result<()> {
    if let Some(mint_url) = &sub_command_args.mint_url {
        match wallet_manager.mint_info(mint_url).await {
            Ok(info) => {
                // Mint info is entirely mint-controlled (URLs, custom currency units,
                // descriptions); escape control characters before printing.
                println!("{}", escape_json(&serde_json::to_string_pretty(&info)?));
            }
            Err(fetch_err) => {
                let wallets = wallet_manager
                    .wallets()
                    .await
                    .into_iter()
                    .filter(|wallet| &wallet.identity().mint_url == mint_url)
                    .collect::<Vec<_>>();

                if let Some(wallet) = wallets.first() {
                    match wallet
                        .advanced()
                        .mint_metadata(MintMetadataRequest {
                            source: MetadataSource::CacheOnly,
                        })
                        .await
                    {
                        Ok(metadata) => {
                            println!(
                                "{}",
                                escape_json(&serde_json::to_string_pretty(&metadata.info)?)
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
        for (i, wallet) in wallet_manager.wallets().await.into_iter().enumerate() {
            let mint_url = wallet.identity().mint_url;
            if !seen.insert(mint_url.clone()) {
                continue;
            }
            match wallet
                .advanced()
                .mint_metadata(MintMetadataRequest::default())
                .await
            {
                Ok(metadata) => {
                    println!("{i}: {}", escape_control(&mint_url.to_string()));
                    println!(
                        "{}",
                        escape_json(&serde_json::to_string_pretty(&metadata.info)?)
                    );
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Cannot fetch mint info {mint_url}: {e}"));
                }
            };
        }
    }

    Ok(())
}
