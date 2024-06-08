use anyhow::Result;
use cdk::url::UncheckedUrl;
use cdk::HttpClient;
use clap::Args;

#[derive(Args)]
pub struct MintInfoSubcommand {
    /// Cashu Token
    #[arg(short, long)]
    mint_url: UncheckedUrl,
}

pub async fn mint_info(sub_command_args: &MintInfoSubcommand) -> Result<()> {
    let client = HttpClient::default();

    let info = client
        .get_mint_info(sub_command_args.mint_url.clone().try_into()?)
        .await?;

    println!("{:#?}", info);

    Ok(())
}
