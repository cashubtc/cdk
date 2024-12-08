use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::client::MintConnector;
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: MintUrl,
}

pub async fn mint_info(proxy: Option<Url>, sub_command_args: &MintInfoSubcommand) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let client = match proxy {
        Some(proxy) => HttpClient::with_proxy(mint_url, proxy, None, true)?,
        None => HttpClient::new(mint_url),
    };

    let info = client.get_mint_info().await?;

    println!("{:#?}", info);

    Ok(())
}
