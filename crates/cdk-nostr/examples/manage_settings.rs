//! Settings management example for the `NpubCash` SDK
//!
//! This example demonstrates:
//! - Setting the mint URL
//! - Reading the current account settings
//! - Toggling NUT-20 quote locking
//! - Handling API responses

use std::sync::Arc;

use cdk_nostr::nostr_sdk::Keys;
use cdk_nostr::npubcash::{JwtAuthProvider, NpubCashClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let base_url =
        std::env::var("NPUBCASH_URL").unwrap_or_else(|_| "https://npubx.cash".to_string());

    let keys = if let Ok(nsec) = std::env::var("NOSTR_NSEC") {
        Keys::parse(&nsec)?
    } else {
        println!("No NOSTR_NSEC found, generating new keys");
        Keys::generate()
    };

    println!("Public key: {}", keys.public_key());

    let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));

    let client = NpubCashClient::new(base_url, auth_provider);

    println!("\n=== Setting Mint URL ===");
    let mint_url = "https://testnut.cashu.space";
    match client.set_mint_url(mint_url).await {
        Ok(response) => {
            let user = response.data.user();
            println!("✓ Successfully set mint URL");
            println!(
                "  Current mint URL: {}",
                user.mint_url.as_deref().unwrap_or("None")
            );
            println!("  Lock quotes: {}", user.lock_quote);
        }
        Err(e) => {
            eprintln!("✗ Error setting mint URL: {e}");
        }
    }

    println!("\n=== Enabling quote locking ===");
    match client.set_quote_locking(true).await {
        Ok(response) => {
            println!("✓ Quote locking: {}", response.data.user().lock_quote);
        }
        Err(e) => {
            eprintln!("✗ Error enabling quote locking: {e}");
        }
    }

    println!("\n=== Reading account settings ===");
    match client.get_user_info().await {
        Ok(response) => {
            let user = response.data.user();
            println!("  mint: {}", user.mint_url.as_deref().unwrap_or("None"));
            println!("  lock quotes: {}", user.lock_quote);
        }
        Err(e) => {
            eprintln!("✗ Error reading settings: {e}");
        }
    }

    Ok(())
}
