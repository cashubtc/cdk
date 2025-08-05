//! Wallet example with memory store

use std::collections::HashMap;
use std::sync::Arc;

use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MintQuoteState, NotificationPayload};
use cdk::wallet::{Wallet, WalletSubscription};
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

        // Request a mint quote from the wallet
        let quote = wallet.mint_quote(amount, None).await?;
        println!("Pay request: {}", quote.request);

        // Subscribe to the wallet for updates on the mint quote state
        let mut subscription = wallet
            .subscribe(WalletSubscription::Bolt11MintQuoteState(vec![quote
                .id
                .clone()]))
            .await;

        // Wait for the mint quote to be paid
        while let Some(msg) = subscription.recv().await {
            if let NotificationPayload::MintQuoteBolt11Response(response) = msg {
                if response.state == MintQuoteState::Paid {
                    break;
                }
            }
        }

        // Mint the received amount
        let proofs = wallet.mint(&quote.id, SplitTarget::default(), None).await?;
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
