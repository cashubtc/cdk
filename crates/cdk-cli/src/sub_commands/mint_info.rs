use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::client::HttpClientMethods;
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: MintUrl,
}

pub async fn mint_info(proxy: Option<Url>, sub_command_args: &MintInfoSubcommand) -> Result<()> {
    let client = match proxy {
        Some(proxy) => HttpClient::with_proxy(proxy, None, true)?,
        None => HttpClient::new(),
    };

    let info = client
        .get_mint_info(sub_command_args.mint_url.clone())
        .await?;

    println!("{:#?}", info);

    Ok(())
}
