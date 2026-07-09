use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::MintConnector;
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: MintUrl,
}

pub async fn mint_info(
    proxy: Option<Url>,
    danger_accept_invalid_certs: bool,
    sub_command_args: &MintInfoSubcommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    #[cfg(feature = "federation")]
    if let Some(federation_wallet) = crate::utils::federation_wallet_for_mint_url(&mint_url)? {
        let info = federation_wallet.mint_connector()?.get_mint_info().await?;
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    let client = match proxy {
        Some(proxy) => HttpClient::with_proxy(mint_url, proxy, None, danger_accept_invalid_certs)?,
        None => HttpClient::new(mint_url, None),
    };

    let info = client.get_mint_info().await?;

    println!("{}", serde_json::to_string_pretty(&info)?);

    Ok(())
}
