use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, Proof};
use cdk::wallet::WalletRepository;

pub async fn proofs(wallet_repository: &WalletRepository) -> Result<()> {
    list_proofs(wallet_repository).await?;
    Ok(())
}

async fn list_proofs(
    wallet_repository: &WalletRepository,
) -> Result<Vec<(MintUrl, (Vec<Proof>, CurrencyUnit))>> {
    let mut proofs_vec = Vec::new();

    let wallets = wallet_repository.get_wallets().await;

    for (i, wallet) in wallets.iter().enumerate() {
        let mint_url = wallet.mint_url.clone();
        println!("{i}: {mint_url}");
        println!("|   Amount | Unit | State    | Secret                                                           | DLEQ proof included");
        println!("|----------|------|----------|------------------------------------------------------------------|--------------------");

        // Unspent proofs
        let unspent_proofs = wallet.get_unspent_proofs().await?;
        for proof in unspent_proofs.iter() {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                wallet.unit,
                "unspent",
                proof.secret,
                proof.dleq.is_some()
            );
        }

        // Pending proofs
        let pending_proofs = wallet.get_pending_proofs().await?;
        for proof in pending_proofs {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                wallet.unit,
                "pending",
                proof.secret,
                proof.dleq.is_some()
            );
        }

        // Reserved proofs
        let reserved_proofs = wallet.get_reserved_proofs().await?;
        for proof in reserved_proofs {
            println!(
                "| {:8} | {:4} | {:8} | {:64} | {}",
                proof.amount,
                wallet.unit,
                "reserved",
                proof.secret,
                proof.dleq.is_some()
            );
        }

        println!();
        proofs_vec.push((mint_url, (unspent_proofs, wallet.unit.clone())));
    }
    Ok(proofs_vec)
}
