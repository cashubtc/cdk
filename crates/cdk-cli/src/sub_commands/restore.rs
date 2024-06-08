use anyhow::Result;
use cdk::url::UncheckedUrl;
use cdk::wallet::Wallet;
use clap::Args;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    #[arg(short, long)]
    mint_url: UncheckedUrl,
}

pub async fn restore(wallet: Wallet, sub_command_args: &RestoreSubCommand) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let amount = wallet.restore(mint_url).await?;

    println!("Restored {}", amount);

    Ok(())
}
