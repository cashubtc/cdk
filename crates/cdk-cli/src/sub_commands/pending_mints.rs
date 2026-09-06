use anyhow::Result;
use cdk::wallet::WalletManager;
use cdk::Amount;

pub async fn mint_pending(wallet_manager: &WalletManager) -> Result<()> {
    let wallets = wallet_manager.wallets().await;
    let mut total_amount = Amount::ZERO;

    for wallet in wallets {
        let amount = wallet.advanced().reconcile_proofs().await?;
        total_amount += amount;
    }

    println!("Amount: {total_amount}");

    Ok(())
}
