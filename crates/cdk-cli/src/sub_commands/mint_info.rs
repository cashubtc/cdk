use anyhow::Result;
use cdk::url::UncheckedUrl;
use cdk::HttpClient;
use clap::Args;
use url::Url;

#[derive(Args)]
pub struct MintInfoSubcommand {
    /// Cashu Token
    mint_url: UncheckedUrl,
}

pub async fn mint_info(proxy: Option<Url>, sub_command_args: &MintInfoSubcommand) -> Result<()> {
    let client = match proxy {
        Some(proxy) => HttpClient::with_nws_proxy(proxy)?,
        None => HttpClient::new(),
    };

    let info = client
        .get_mint_info(sub_command_args.mint_url.clone().try_into()?)
        .await?;

    println!("{:#?}", info);

    Ok(())
}
