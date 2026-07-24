use anyhow::Result;
use cdk::wallet::WalletRepository;

#[derive(clap::Args, Clone)]
pub struct FinalizeReceivesSubCommand {
    /// Specify the mint url
    #[arg(short, long)]
    pub mint_url: Option<String>,
}

pub async fn finalize_receives(
    wallet_repository: &WalletRepository,
    sub_command_args: &FinalizeReceivesSubCommand,
) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;

    let mut found_mint = false;
    for wallet in wallets.iter() {
        if let Some(mint_url) = &sub_command_args.mint_url {
            if wallet.mint_url.to_string() != *mint_url {
                continue;
            }
        }
        found_mint = true;

        match wallet.finalize_pending_receives().await {
            Ok(amount) => {
                println!(
                    "Finalized pending receives for {}: {} {}",
                    wallet.mint_url, amount, wallet.unit
                );
            }
            Err(e) => {
                println!("Error finalizing receives for {}: {}", wallet.mint_url, e);
            }
        }
    }

    if !found_mint {
        if let Some(mint_url) = &sub_command_args.mint_url {
            println!("No wallet found for mint: {}", mint_url);
        } else {
            println!("No wallets found.");
        }
    }

    Ok(())
}
