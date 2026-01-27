use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::wallet::WalletRepository;
use cdk::Amount;
use clap::Args;

#[derive(Args)]
pub struct BurnSubCommand {
    /// Mint Url
    mint_url: Option<MintUrl>,
}

pub async fn burn(
    wallet_repository: &WalletRepository,
    sub_command_args: &BurnSubCommand,
) -> Result<()> {
    let mut total_burnt = Amount::ZERO;

    match &sub_command_args.mint_url {
        Some(mint_url) => {
            let wallet = wallet_repository
                .get_wallet(mint_url)
                .await
                .ok_or_else(|| anyhow::anyhow!("Wallet not found for mint: {}", mint_url))?;
            total_burnt = wallet.check_all_pending_proofs().await?;
        }
        None => {
            for wallet in wallet_repository.get_wallets().await {
                let amount_burnt = wallet.check_all_pending_proofs().await?;
                total_burnt += amount_burnt;
            }
        }
    }

    println!("{total_burnt} burned");
    Ok(())
}
