use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletManager;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct BurnSubCommand {
    /// Mint Url
    mint_url: Option<MintUrl>,
}

pub async fn burn(wallet_manager: &WalletManager, sub_command_args: &BurnSubCommand) -> Result<()> {
    let mut total_burnt = Amount::ZERO;

    match &sub_command_args.mint_url {
        Some(mint_url) => {
            for wallet in wallet_manager.wallets().await {
                if &wallet.identity().mint_url == mint_url {
                    total_burnt += wallet.advanced().reconcile_proofs().await?;
                }
            }
        }
        None => {
            for wallet in wallet_manager.wallets().await {
                let amount_burnt = wallet.advanced().reconcile_proofs().await?;
                total_burnt += amount_burnt;
            }
        }
    }

    println!("{total_burnt} burned");
    Ok(())
}
