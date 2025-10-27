//! # NpubCash SDK
//!
//! Rust client SDK for the NpubCash v2 API.
//!
//! ## Features
//!
//! - HTTP client for fetching quotes with auto-pagination
//! - NIP-98 and JWT authentication
//! - User settings management
//!
//! ## Quick Start
//!
//! ```no_run
//! use std::sync::Arc;
//!
//! use cdk_npubcash::{JwtAuthProvider, NpubCashClient};
//! use nostr_sdk::Keys;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create authentication provider with Nostr keys
//!     let base_url = "https://npubx.cash".to_string();
//!     let keys = Keys::generate();
//!     let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
//!
//!     // Create the NpubCash client
//!     let client = NpubCashClient::new(base_url, auth_provider);
//!
//!     // Fetch all quotes
//!     let quotes = client.get_quotes(None).await?;
//!     println!("Found {} quotes", quotes.len());
//!
//!     // Fetch quotes since a specific timestamp
//!     let recent_quotes = client.get_quotes(Some(1234567890)).await?;
//!     println!("Found {} recent quotes", recent_quotes.len());
//!
//!     // Update mint URL setting
//!     client.set_mint_url("https://example-mint.tld").await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Authentication
//!
//! The SDK uses NIP-98 HTTP authentication for initial requests and JWT tokens
//! for subsequent requests. The [`JwtAuthProvider`] handles this automatically,
//! including token caching and refresh.
//!
//! ## Fetching Quotes
//!
//! ```no_run
//! # use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
//! # use nostr_sdk::Keys;
//! # use std::sync::Arc;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let base_url = "https://npubx.cash".to_string();
//! # let keys = Keys::generate();
//! # let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
//! # let client = NpubCashClient::new(base_url, auth_provider);
//! // Fetch all quotes
//! let all_quotes = client.get_quotes(None).await?;
//!
//! // Fetch quotes since a specific timestamp
//! let recent_quotes = client.get_quotes(Some(1234567890)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Managing Settings
//!
//! ```no_run
//! # use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
//! # use nostr_sdk::Keys;
//! # use std::sync::Arc;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let base_url = "https://npubx.cash".to_string();
//! # let keys = Keys::generate();
//! # let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
//! # let client = NpubCashClient::new(base_url, auth_provider);
//! // Set mint URL
//! let response = client.set_mint_url("https://my-mint.com").await?;
//! println!("Mint URL: {:?}", response.data.mint_url);
//! println!("Lock quotes: {}", response.data.lock_quotes);
//! # Ok(())
//! # }
//! ```
//!
//! **Note:** Quotes are always locked by default on the NPubCash server for security.

#![warn(missing_docs)]
#![allow(clippy::doc_markdown)]

pub mod auth;
pub mod client;
pub mod error;
pub mod types;

// Re-export main types for convenient access
pub use auth::JwtAuthProvider;
pub use client::NpubCashClient;
pub use error::{Error, Result};
pub use types::{
    Metadata, Nip98Data, Nip98Response, Quote, QuotesData, QuotesResponse, UserData, UserResponse,
};

/// Extract authentication URL (scheme + host + path, no query params)
///
/// # Arguments
///
/// * `url` - The full URL to parse
///
/// # Errors
///
/// Returns an error if the URL is invalid or missing required components
pub(crate) fn extract_auth_url(url: &str) -> Result<String> {
    let parsed_url = url::Url::parse(url)?;
    let host = parsed_url
        .host_str()
        .ok_or_else(|| Error::Custom("Invalid URL: missing host".to_string()))?;

    Ok(format!(
        "{}://{}{}",
        parsed_url.scheme(),
        host,
        parsed_url.path()
    ))
}
