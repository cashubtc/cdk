//! Wallet example with memory store

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_common::nut02::KeySetInfosMethods;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::new(mint_url, unit, localstore, seed, None)?;

    // Amount to mint
    for amount in [64] {
        let amount = Amount::from(amount);

        let quote = wallet.mint_quote(amount, None).await?;
        let proofs = wallet
            .wait_and_mint_quote(
                quote,
                Default::default(),
                Default::default(),
                Duration::from_secs(10),
            )
            .await?;

        // Mint the received amount
        let receive_amount = proofs.total_amount()?;
        println!("Minted {}", receive_amount);
    }

    // Get unspent proofs
    let proofs = wallet.get_unspent_proofs().await?;

    // Select proofs to send
    let amount = Amount::from(64);
    let active_keyset_ids = wallet
        .refresh_keysets()
        .await?
        .active()
        .map(|keyset| keyset.id)
        .collect();
    let selected =
        Wallet::select_proofs(amount, proofs, &active_keyset_ids, &HashMap::new(), false)?;
    for (i, proof) in selected.iter().enumerate() {
        println!("{}: {}", i, proof.amount);
    }

    Ok(())
}
