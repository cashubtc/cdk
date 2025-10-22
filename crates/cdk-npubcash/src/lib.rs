//! # NpubCash SDK
//!
//! Rust client SDK for the NpubCash v2 API.
//!
//! ## Features
//!
//! - HTTP client for fetching quotes with auto-pagination
//! - NIP-98 and JWT authentication
//! - Polling for quote updates
//! - User settings management
//!
//! ## Quick Start
//!
//! ```no_run
//! use std::sync::Arc;
//! use std::time::Duration;
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
//!     let quotes = client.get_all_quotes().await?;
//!     println!("Found {} quotes", quotes.len());
//!
//!     // Poll for new quotes every 5 seconds
//!     client
//!         .poll_quotes_with_callback(Duration::from_secs(5), |quotes| {
//!             println!("Found {} new quotes", quotes.len());
//!             for quote in quotes {
//!                 println!("  - Quote {}: {} {}", quote.id, quote.amount, quote.unit);
//!             }
//!         })
//!         .await?;
//!
//!     // Update mint URL setting
//!     client
//!         .settings
//!         .set_mint_url("https://example-mint.tld")
//!         .await?;
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
//! let all_quotes = client.get_all_quotes().await?;
//!
//! // Fetch quotes since a specific timestamp
//! let recent_quotes = client.get_quotes_since(1234567890).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Polling for Updates
//!
//! ```no_run
//! # use cdk_npubcash::{NpubCashClient, JwtAuthProvider};
//! # use nostr_sdk::Keys;
//! # use std::sync::Arc;
//! # use std::time::Duration;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let base_url = "https://npubx.cash".to_string();
//! # let keys = Keys::generate();
//! # let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
//! # let client = NpubCashClient::new(base_url, auth_provider);
//! // Poll for new quotes every 10 seconds
//! let handle = client
//!     .poll_quotes_with_callback(Duration::from_secs(10), |quotes| {
//!         for quote in quotes {
//!             println!("New quote: {}", quote.id);
//!         }
//!     })
//!     .await?;
//!
//! // The polling continues until the handle is dropped
//! // drop(handle); to stop polling
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
//! let response = client.settings.set_mint_url("https://my-mint.com").await?;
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
pub mod settings;
pub mod types;

// Re-export main types for convenient access
pub use auth::{AuthProvider, JwtAuthProvider};
pub use client::{NpubCashClient, PollingHandle};
pub use error::{Error, Result};
pub use settings::SettingsManager;
pub use types::{
    Metadata, Nip98Data, Nip98Response, Quote, QuotesData, QuotesResponse, UserData, UserResponse,
};
