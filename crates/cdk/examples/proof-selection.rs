//! Wallet example with memory store

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::{
    select_proofs, MintMetadataRequest, ProofQuery, ProofSelectionFeePolicy, ProofSelectionRequest,
};
use cdk::wallet::mint::MintRequest;
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed for the wallet
    let seed = random::<[u8; 64]>();

    // Mint URL and currency unit
    let mint_url = "https://testnut.cashudevkit.org";
    let unit = CurrencyUnit::Sat;

    // Initialize the memory store
    let localstore = Arc::new(memory::empty().await?);

    // Create a new wallet
    let wallet = Wallet::open(cdk::wallet::WalletOpenRequest::new(
        cdk::wallet::WalletIdentity::new(mint_url.parse()?, unit),
        localstore,
        seed,
    ))?;

    // Amount to mint
    for amount in [64] {
        let amount = Amount::from(amount);

        let session = wallet.request_mint(MintRequest::bolt11(amount)).await?;
        let receipt = session.wait(Duration::from_secs(10)).await?;

        println!("Minted {}", receipt.amount);
    }

    // Get unspent proofs
    let proofs = wallet
        .advanced()
        .proofs(ProofQuery::default())
        .await?
        .into_iter()
        .map(|proof| proof.proof)
        .collect();

    // Select proofs to send
    let amount = Amount::from(64);
    let active_keyset_ids = wallet
        .advanced()
        .mint_metadata(MintMetadataRequest::default())
        .await?
        .keysets
        .into_iter()
        .filter(|k| k.active.unwrap_or(false))
        .map(|keyset| keyset.id)
        .collect();
    let selected = select_proofs(ProofSelectionRequest {
        amount,
        proofs,
        active_keyset_ids,
        keyset_fees: HashMap::new(),
        fee_policy: ProofSelectionFeePolicy::Exclude,
    })?;
    for (i, proof) in selected.iter().enumerate() {
        println!("{}: {}", i, proof.amount);
    }

    Ok(())
}
