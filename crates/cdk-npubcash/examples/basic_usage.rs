//! Basic usage example for the `NpubCash` SDK
//!
//! This example demonstrates:
//! - Creating a client with authentication
//! - Fetching all quotes
//! - Fetching quotes since a timestamp
//! - Error handling

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

    println!("\n=== Fetching all quotes ===");
    match client.get_quotes(None).await {
        Ok(quotes) => {
            println!("Successfully fetched {} quotes", quotes.len());
            if let Some(first) = quotes.first() {
                println!("\nFirst quote:");
                println!("  ID: {}", first.id);
                println!("  Amount: {}", first.amount);
                println!("  Unit: {}", first.unit);
            }
        }
        Err(e) => {
            eprintln!("Error fetching quotes: {e}");
        }
    }

    let one_hour_ago = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)?
        .as_secs()
        - 3600;

    println!("\n=== Fetching quotes from last hour ===");
    match client.get_quotes(Some(one_hour_ago)).await {
        Ok(quotes) => {
            println!("Found {} quotes in the last hour", quotes.len());
        }
        Err(e) => {
            eprintln!("Error fetching recent quotes: {e}");
        }
    }

    Ok(())
}
