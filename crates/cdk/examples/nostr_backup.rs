//! # Nostr Mint Backup Example (NUT-XX)
//!
//! This example demonstrates how to backup and restore your mint list
//! to/from Nostr relays using the MultiMintWallet.
//!
//! ## Features
//!
//! - Backup your mint list to multiple Nostr relays
//! - Restore your mints on any device with the same seed
//! - Keep your mint configuration synchronized across wallets
//!
//! ## Security
//!
//! - Backup keys are derived deterministically from your seed
//! - Mint list is encrypted using NIP-44 (self-encryption)
//! - Only someone with your seed can decrypt the backup
//! - Events use addressable format (kind 30078) for easy updates
//!
//! ## Usage
//!
//! ```bash
//! cargo run --example nostr_backup --features="wallet nostr"
//! ```

use std::sync::Arc;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::{BackupOptions, RestoreOptions};
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing for debug output
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .init();

    println!("NUT-XX Nostr Mint Backup Example");
    println!("=================================\n");

    // Generate a random seed for the wallet
    // In production, this would be derived from a BIP-39 mnemonic
    let seed: [u8; 64] = random();

    // Currency unit for the wallet
    let unit = CurrencyUnit::Sat;

    // Initialize the memory store for the first wallet
    let localstore = Arc::new(memory::empty().await?);

    // Create a new WalletRepository
    let wallet: WalletRepository = WalletRepository::new(localstore.clone(), seed).await?;

    // ============================================================================
    // Step 1: Add test mints to the wallet
    // ============================================================================

    println!("Step 1: Adding mints to the wallet");
    println!("-----------------------------------");

    let mints = vec![
        "https://fake.thesimplekid.dev",
        "https://testnut.cashu.space",
    ];

    for mint_url in &mints {
        println!("  Adding mint: {}", mint_url);
        match wallet.add_mint(mint_url.parse()?).await {
            Ok(()) => println!("    + Added successfully"),
            Err(e) => println!("    x Failed to add: {}", e),
        }
    }

    // Verify mints were added
    let wallets: Vec<cdk::Wallet> = wallet.get_wallets().await;
    println!("\n  Wallet now contains {} mint(s):", wallets.len());
    for w in &wallets {
        println!("    - {}", w.mint_url);
    }

    println!();

    // ============================================================================
    // Step 2: Derive backup keys
    // ============================================================================

    println!("Step 2: Deriving backup keys from seed");
    println!("---------------------------------------");

    let backup_keys = wallet.backup_keys()?;
    println!("  Public key: {}", backup_keys.public_key().to_hex());
    println!("  This key is deterministically derived from your wallet seed.");
    println!("  Anyone with the same seed will derive the same keys.\n");

    // ============================================================================
    // Step 3: Backup mints to Nostr relays
    // ============================================================================

    println!("Step 3: Backing up mint list to Nostr relays");
    println!("---------------------------------------------");

    let relays = vec!["wss://relay.damus.io", "wss://nos.lol"];

    println!("  Relays: {:?}", relays);
    println!("  Publishing backup event...");

    let backup_result = wallet
        .backup_mints(
            relays.clone(),
            BackupOptions::new().client("nostr-backup-example"),
        )
        .await?;

    println!("  + Backup published!");
    println!("    Event ID: {}", backup_result.event_id);
    println!("    Public Key: {}", backup_result.public_key.to_hex());
    println!("    Mints backed up: {}", backup_result.mint_count);

    println!();

    // ============================================================================
    // Step 4: Simulate restore on a "new device"
    // ============================================================================

    println!("Step 4: Simulating restore on a new device");
    println!("-------------------------------------------");

    // Create a fresh wallet with the same seed (simulating a new device)
    let new_localstore = Arc::new(memory::empty().await?);
    let new_wallet = WalletRepository::new(new_localstore, seed).await?;

    // Verify the new wallet is empty
    let new_wallets: Vec<cdk::Wallet> = new_wallet.get_wallets().await;
    println!("  New wallet starts with {} mint(s)", new_wallets.len());

    // Derive keys on the new wallet - should be the same!
    let new_backup_keys = new_wallet.backup_keys()?;
    println!(
        "  New wallet public key: {}",
        new_backup_keys.public_key().to_hex()
    );
    println!(
        "  Keys match: {}",
        backup_keys.public_key() == new_backup_keys.public_key()
    );

    println!();

    // ============================================================================
    // Step 5: Restore mints from Nostr relays
    // ============================================================================

    println!("Step 5: Restoring mint list from Nostr relays");
    println!("----------------------------------------------");

    println!("  Fetching backup from relays...");

    let restore_result = new_wallet
        .restore_mints(relays.clone(), true, RestoreOptions::default())
        .await?;

    println!("  + Restore complete!");
    println!("    Mints found in backup: {}", restore_result.mint_count);
    println!("    Mints newly added: {}", restore_result.mints_added);
    println!("    Backup timestamp: {}", restore_result.backup.timestamp);

    println!("\n  Mints in backup:");
    for mint in &restore_result.backup.mints {
        println!("    - {}", mint);
    }

    // Verify the mints were restored
    let restored_wallets: Vec<cdk::Wallet> = new_wallet.get_wallets().await;
    println!(
        "\n  New wallet now contains {} mint(s):",
        restored_wallets.len()
    );
    for w in &restored_wallets {
        println!("    - {}", w.mint_url);
    }

    println!();

    Ok(())
}
