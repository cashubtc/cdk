use anyhow::Result;
use cdk::wallet::WalletRepository;
use cdk::Amount;

pub async fn check_pending(wallet_repository: &WalletRepository) -> Result<()> {
    let wallets = wallet_repository.get_wallets().await;

    for (i, wallet) in wallets.iter().enumerate() {
        let mint_url = wallet.mint_url.clone();
        println!("{i}: {mint_url}");

        // Check all orphaned pending proofs (not managed by active sagas)
        // This function queries the mint and marks spent proofs accordingly
        match wallet.check_all_pending_proofs().await {
            Ok(pending_amount) => {
                if pending_amount == Amount::ZERO {
                    println!("No orphaned pending proofs found");
                } else {
                    println!(
                        "Checked pending proofs: {} {} still pending",
                        pending_amount, wallet.unit
                    );
                }
            }
            Err(e) => println!("Error checking pending proofs: {e}"),
        }
    }
    Ok(())
}
