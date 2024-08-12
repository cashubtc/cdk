use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    /// Cashu Token
    mint_url: MintUrl,
}

pub async fn mint_info(proxy: Option<Url>, sub_command_args: &MintInfoSubcommand) -> Result<()> {
    let client = match proxy {
        Some(proxy) => HttpClient::with_proxy(proxy, None, true)?,
        None => HttpClient::new(),
    };

    let info = client
        .get_mint_info(sub_command_args.mint_url.clone().try_into()?)
        .await?;

    println!("{:#?}", info);

    Ok(())
}
