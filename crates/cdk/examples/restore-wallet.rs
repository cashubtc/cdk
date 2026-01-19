use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::wallet::Wallet;
use cdk::Amount;
use cdk_sqlite::wallet::memory;
use rand::random;

/// This example demonstrates wallet restoration from a seed.
///
/// It shows:
/// - Creating a wallet with a known seed
/// - Minting proofs
/// - Creating a new wallet with the same seed but fresh storage
/// - Restoring proofs from the mint using the seed
///
/// This is useful for recovering a wallet on a new device or after data loss,
/// as long as you have the original seed.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configuration
    let mint_url = "https://fake.thesimplekid.dev";
    let unit = CurrencyUnit::Sat;
    let amount = Amount::from(50);

    // Generate a seed - in production, use a mnemonic and store it securely!
    // For this example, we use random bytes
    let seed: [u8; 64] = random();
    println!("Seed generated (first 32 bytes shown as hex)");
    println!("(In production, use a BIP39 mnemonic instead)\n");

    // ========================================
    // Step 1: Create original wallet and mint proofs
    // ========================================
    println!("--- ORIGINAL WALLET ---");

    let original_store = Arc::new(memory::empty().await?);
    let original_wallet = Wallet::new(mint_url, unit.clone(), original_store, seed, None)?;

    // Mint some proofs
    println!("Minting {} sats...", amount);
    let quote = original_wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;

    let proofs = original_wallet
        .wait_and_mint_quote(
            quote,
            Default::default(),
            Default::default(),
            Duration::from_secs(30),
        )
        .await?;

    let original_amount = proofs.total_amount()?;
    let original_balance = original_wallet.total_balance().await?;
    println!("Minted {} sats", original_amount);
    println!("Original wallet balance: {} sats", original_balance);

    // ========================================
    // Step 2: Simulate wallet loss - create new wallet with same seed
    // ========================================
    println!("\n--- RESTORED WALLET ---");
    println!("Simulating wallet recovery with same seed...\n");

    // Create a fresh storage (simulating a new device or data loss)
    let restored_store = Arc::new(memory::empty().await?);

    // Create wallet with the SAME seed but EMPTY storage
    let restored_wallet = Wallet::new(mint_url, unit, restored_store, seed, None)?;

    // Check balance before restore - should be 0
    let balance_before = restored_wallet.total_balance().await?;
    println!("Balance before restore: {} sats", balance_before);

    // ========================================
    // Step 3: Restore proofs from the mint
    // ========================================
    println!("Restoring proofs from mint...");

    // The restore method:
    // 1. Generates the same blinded messages from the seed
    // 2. Queries the mint for signatures on those messages
    // 3. Reconstructs and stores unspent proofs
    let restored_amount = restored_wallet.restore().await?;
    println!("Restored {} sats from mint", restored_amount);

    // Verify final balance
    let final_balance = restored_wallet.total_balance().await?;
    println!("\nFinal restored balance: {} sats", final_balance);
    println!("Original balance was:   {} sats", original_balance);

    if final_balance == original_balance {
        println!("\nSuccess! Wallet fully restored.");
    } else {
        println!("\nNote: Balance may differ if some proofs were spent.");
    }

    Ok(())
}
