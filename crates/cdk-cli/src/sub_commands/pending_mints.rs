use anyhow::Result;
use cdk::wallet::WalletRepository;
use cdk::Amount;

pub async fn mint_pending(wallet_repository: &WalletRepository) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;
    let mut total_amount = Amount::ZERO;

    for wallet in wallets {
        let amount = wallet.check_all_mint_quotes().await?;
        total_amount += amount;
    }

    println!("Amount: {total_amount}");

    Ok(())
}
