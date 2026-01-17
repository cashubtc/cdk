//! Example of configuring the wallet with custom settings, including metadata cache TTL.
//!
//! This example demonstrates:
//! 1. Creating a Wallet with a custom metadata cache TTL
//! 2. Creating a MultiMintWallet and adding a mint with a custom configuration
//! 3. Updating the configuration of an active wallet

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::multi_mint_wallet::WalletConfig;
use cdk::wallet::{MultiMintWallet, WalletBuilder};
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate a random seed
    let seed = random::<[u8; 64]>();
    let unit = CurrencyUnit::Sat;
    let localstore = Arc::new(memory::empty().await?);

    // ==========================================
    // 1. Configure a single Wallet
    // ==========================================
    println!("\n=== Single Wallet Configuration ===");
    let mint_url = MintUrl::from_str("https://fake.thesimplekid.dev")?;

    // Create a wallet with a custom 10-minute TTL (default is 1 hour)
    let wallet = WalletBuilder::new()
        .mint_url(mint_url.clone())
        .unit(unit.clone())
        .localstore(localstore.clone())
        .seed(seed)
        .set_metadata_cache_ttl(Some(Duration::from_secs(600))) // 10 minutes
        .build()?;

    println!("Created wallet with 10 minute metadata cache TTL");

    // You can also update the TTL on an existing wallet
    wallet.set_metadata_cache_ttl(Some(Duration::from_secs(300))); // Change to 5 minutes
    println!("Updated wallet TTL to 5 minutes");

    // ==========================================
    // 2. Configure MultiMintWallet
    // ==========================================
    println!("\n=== MultiMintWallet Configuration ===");

    // Create the MultiMintWallet
    let multi_wallet = MultiMintWallet::new(localstore.clone(), seed, unit.clone()).await?;

    // Define configuration for a new mint
    // This config uses a very short 1-minute TTL
    let config = WalletConfig::new().with_metadata_cache_ttl(Some(Duration::from_secs(60)));

    let mint_url_2 = MintUrl::from_str("https://testnut.cashu.space")?;

    // Add the mint with the custom configuration
    multi_wallet
        .add_mint_with_config(mint_url_2.clone(), config.clone())
        .await?;
    println!("Added mint {} with 1 minute TTL", mint_url_2);

    // Update configuration for an existing mint
    // Let's disable auto-refresh (set to None) for the first mint
    let no_refresh_config = WalletConfig::new().with_metadata_cache_ttl(None); // Never expire

    multi_wallet.add_mint(mint_url.clone()).await?; // Add first mint with default settings
    multi_wallet
        .set_mint_config(mint_url.clone(), no_refresh_config)
        .await?;
    println!("Updated mint {} to never expire metadata cache", mint_url);

    Ok(())
}
