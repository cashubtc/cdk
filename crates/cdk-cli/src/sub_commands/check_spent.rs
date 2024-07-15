use anyhow::Result;
use cdk::wallet::MultiMintWallet;

pub async fn check_spent(multi_mint_wallet: &MultiMintWallet) -> Result<()> {
    for wallet in multi_mint_wallet.get_wallets().await {
        let amount = wallet.check_all_pending_proofs().await?;

        println!("Amount marked as spent: {}", amount);
    }

    Ok(())
}
