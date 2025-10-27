//! Create a lightning address and wait for payment
//!
//! This example demonstrates:
//! - Generating Nostr keys for authentication
//! - Displaying the npub.cash URL for the user's public key
//! - Polling for quote updates
//! - Waiting for payment notifications
//!
//! Note: The current npub.cash SDK only supports reading quotes.
//! To create a new quote/invoice, you would need to:
//! 1. Use the npub.cash web interface or
//! 2. Implement the POST /api/v2/wallet/quotes endpoint
//!
//! This example shows how to monitor for new quotes once they exist.

use std::sync::Arc;
use std::time::Duration;

use cdk_npubcash::{JwtAuthProvider, NpubCashClient};
use nostr_sdk::{Keys, ToBech32};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(not(target_arch = "wasm32"))]
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    tracing::debug!("Starting NpubCash Payment Monitor");
    println!("=== NpubCash Payment Monitor ===\n");

    let base_url =
        std::env::var("NPUBCASH_URL").unwrap_or_else(|_| "https://npubx.cash".to_string());
    tracing::debug!("Using base URL: {}", base_url);

    let keys = if let Ok(nsec) = std::env::var("NOSTR_NSEC") {
        tracing::debug!("Loading Nostr keys from NOSTR_NSEC environment variable");
        println!("Using provided Nostr keys from NOSTR_NSEC");
        Keys::parse(&nsec)?
    } else {
        tracing::debug!("No NOSTR_NSEC found, generating new keys");
        println!("No NOSTR_NSEC found, generating new keys");
        let new_keys = Keys::generate();
        println!("\n⚠️  Save this private key (nsec) to reuse the same identity:");
        println!("   {}", new_keys.secret_key().to_bech32()?);
        println!();
        new_keys
    };

    let npub = keys.public_key().to_bech32()?;
    tracing::debug!("Generated npub: {}", npub);
    println!("Public key (npub): {npub}");
    println!("\nYour npub.cash address:");
    println!("   {npub}/@{base_url}");
    println!("\nAnyone can send you ecash at this address!");
    println!("{}", "=".repeat(60));

    tracing::debug!("Creating JWT auth provider");
    let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));

    tracing::debug!("Initializing NpubCash client");
    let client = NpubCashClient::new(base_url, auth_provider);

    println!("\n=== Checking for existing quotes ===");
    tracing::debug!("Fetching all existing quotes");
    match client.get_quotes(None).await {
        Ok(quotes) => {
            tracing::debug!("Successfully fetched {} quotes", quotes.len());
            if quotes.is_empty() {
                println!("No quotes found yet.");
                println!("\nTo create a new quote:");
                println!("  1. Visit your npub.cash address in a browser");
                println!("  2. Or use the npub.cash web interface");
                println!("  3. Or implement the POST /api/v2/wallet/quotes endpoint");
            } else {
                println!("Found {} existing quote(s):", quotes.len());
                for (i, quote) in quotes.iter().enumerate() {
                    tracing::debug!(
                        "Quote {}: ID={}, amount={}, unit={}",
                        i + 1,
                        quote.id,
                        quote.amount,
                        quote.unit
                    );
                    println!("\n{}. Quote ID: {}", i + 1, quote.id);
                    println!("   Amount: {} {}", quote.amount, quote.unit);
                }
            }
        }
        Err(e) => {
            tracing::error!("Error fetching quotes: {}", e);
            eprintln!("Error fetching quotes: {e}");
        }
    }

    println!("\n{}", "=".repeat(60));
    println!("=== Polling for quote updates ===");
    println!("Checking for new payments every 5 seconds... Press Ctrl+C to stop.\n");

    tracing::debug!("Starting quote polling with 5 second interval");

    // Get initial timestamp for polling
    let mut last_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    // Poll for quotes and handle Ctrl+C
    tokio::select! {
        _ = async {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;

                match client.get_quotes(Some(last_timestamp)).await {
                    Ok(quotes) => {
                        if !quotes.is_empty() {
                            tracing::debug!("Found {} new quotes", quotes.len());

                            // Update timestamp to most recent quote
                            if let Some(max_ts) = quotes.iter().map(|q| q.created_at).max() {
                                last_timestamp = max_ts;
                            }

                            for quote in quotes {
                                tracing::info!(
                                    "New quote received: ID={}, amount={}, unit={}",
                                    quote.id,
                                    quote.amount,
                                    quote.unit
                                );
                                println!("🔔 New quote received!");
                                println!("   Quote ID: {}", quote.id);
                                println!("   Amount: {} {}", quote.amount, quote.unit);
                                println!(
                                    "   Timestamp: {}",
                                    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
                                );
                                println!();
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Error polling quotes: {}", e);
                    }
                }
            }
        } => {}
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received Ctrl+C signal, stopping payment monitor");
        }
    }
    println!("\n✓ Stopped monitoring for payments");

    Ok(())
}
