//! Settings management example for the `NpubCash` SDK
//!
//! This example demonstrates:
//! - Setting the mint URL
//! - Handling API responses
//!
//! Note: Quote locking is always enabled by default on the NPubCash server.
//! The ability to toggle quote locking has been removed from the SDK.

use std::sync::Arc;

use cdk_npubcash::{JwtAuthProvider, NpubCashClient};
use nostr_sdk::Keys;

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
            println!("✓ Successfully set mint URL");
            println!(
                "  Current mint URL: {}",
                response.data.mint_url.as_deref().unwrap_or("None")
            );
            println!("  Lock quotes: {}", response.data.lock_quotes);
            println!("\nNote: Quotes are always locked by default for security.");
        }
        Err(e) => {
            eprintln!("✗ Error setting mint URL: {e}");
        }
    }

    Ok(())
}
