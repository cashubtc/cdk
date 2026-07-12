use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletRepository;
use clap::Args;

#[derive(Args)]
pub struct MintInfoSubcommand {
    mint_url: MintUrl,
}

pub async fn mint_info(
    wallet_repository: &WalletRepository,
    sub_command_args: &MintInfoSubcommand,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();
    let info = wallet_repository.fetch_mint_info(&mint_url).await?;

    println!("{}", serde_json::to_string_pretty(&info)?);

    Ok(())
}
