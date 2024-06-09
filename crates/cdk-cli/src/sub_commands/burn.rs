use anyhow::Result;
use cdk::wallet::Wallet;
use clap::Args;

#[derive(Args)]
pub struct BurnSubCommand {
    /// Token Memo
    #[arg(short, long)]
    mint_url: Option<String>,
}

pub async fn burn(wallet: Wallet, sub_command_args: &BurnSubCommand) -> Result<()> {
    let amount_burnt = wallet
        .check_all_pending_proofs(sub_command_args.mint_url.clone().map(|u| u.into()))
        .await?;

    println!("{amount_burnt} burned");
    Ok(())
}
