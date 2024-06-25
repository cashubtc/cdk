use std::collections::HashMap;

use anyhow::Result;
use cdk::wallet::Wallet;
use cdk::{Amount, UncheckedUrl};
use clap::Args;

#[derive(Args)]
pub struct BurnSubCommand {
    /// Mint Url
    mint_url: Option<UncheckedUrl>,
}

pub async fn burn(
    wallets: HashMap<UncheckedUrl, Wallet>,
    sub_command_args: &BurnSubCommand,
) -> Result<()> {
    let mut total_burnt = Amount::ZERO;
    match &sub_command_args.mint_url {
        Some(mint_url) => {
            let wallet = wallets.get(mint_url).unwrap();
            total_burnt = wallet.check_all_pending_proofs().await?;
        }
        None => {
            for wallet in wallets.values() {
                let amount_burnt = wallet.check_all_pending_proofs().await?;
                total_burnt += amount_burnt;
            }
        }
    }

    println!("{total_burnt} burned");
    Ok(())
}
