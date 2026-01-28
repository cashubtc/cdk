use anyhow::Result;
use cdk::nuts::nut00::ProofsMethods;
use cdk::wallet::WalletRepository;

pub async fn check_pending(wallet_repository: &WalletRepository) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;

    for (i, wallet) in wallets.iter().enumerate() {
        let mint_url = wallet.mint_url.clone();
        println!("{i}: {mint_url}");

        // Get all pending proofs
        let pending_proofs = wallet.get_pending_proofs().await?;
        if pending_proofs.is_empty() {
            println!("No pending proofs found");
            continue;
        }

        println!(
            "Found {} pending proofs with {} {}",
            pending_proofs.len(),
            pending_proofs.total_amount()?,
            wallet.unit
        );

        // Try to reclaim any proofs that are no longer pending
        match wallet.reclaim_unspent(pending_proofs).await {
            Ok(()) => println!("Successfully reclaimed pending proofs"),
            Err(e) => println!("Error reclaimed pending proofs: {e}"),
        }
    }
    Ok(())
}
