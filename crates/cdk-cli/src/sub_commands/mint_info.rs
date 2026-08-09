use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletRepository;
use clap::Args;

use crate::terminal::escape_control;

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

    // Mint info is entirely mint-controlled (URLs, custom currency units,
    // descriptions); escape control characters before printing.
    println!("{}", escape_control(&serde_json::to_string_pretty(&info)?));

    Ok(())
}
