//! FFI bindings for the NpubCash client SDK
//!
//! This module provides FFI-compatible bindings for interacting with the NpubCash API.
//! The client can be used standalone without requiring a wallet.

use std::sync::Arc;

use cdk_npubcash::{JwtAuthProvider, NpubCashClient as CdkNpubCashClient};

use crate::error::FfiError;
use crate::types::MintQuote;

/// FFI-compatible NpubCash client
///
/// This client provides access to the NpubCash API for fetching quotes
/// and managing user settings.
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Object))]
pub struct NpubCashClient {
    inner: Arc<CdkNpubCashClient>,
}

#[cfg_attr(feature = "uniffi-bindings", uniffi::export(async_runtime = "tokio"))]
impl NpubCashClient {
    /// Create a new NpubCash client
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL of the NpubCash service (e.g., <https://npub.cash>)
    /// * `nostr_secret_key` - Nostr secret key for authentication. Accepts either:
    ///   - Hex-encoded secret key (64 characters)
    ///   - Bech32 `nsec` format (e.g., "nsec1...")
    ///
    /// # Errors
    ///
    /// Returns an error if the secret key is invalid or cannot be parsed
    #[cfg_attr(feature = "uniffi-bindings", uniffi::constructor)]
    pub fn new(base_url: String, nostr_secret_key: String) -> Result<Self, FfiError> {
        let keys = parse_nostr_secret_key(&nostr_secret_key)?;
        let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
        let client = CdkNpubCashClient::new(base_url, auth_provider);

        Ok(Self {
            inner: Arc::new(client),
        })
    }

    /// Fetch quotes from NpubCash
    ///
    /// # Arguments
    ///
    /// * `since` - Optional Unix timestamp to fetch quotes from. If `None`, fetches all quotes.
    ///
    /// # Returns
    ///
    /// A list of quotes from the NpubCash service. The client automatically handles
    /// pagination to fetch all available quotes.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    pub async fn get_quotes(&self, since: Option<u64>) -> Result<Vec<NpubCashQuote>, FfiError> {
        let quotes = self
            .inner
            .get_quotes(since)
            .await
            .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(quotes.into_iter().map(Into::into).collect())
    }

    /// Set the mint URL for the user on the NpubCash server
    ///
    /// Updates the default mint URL used by the NpubCash server when creating quotes.
    ///
    /// # Arguments
    ///
    /// * `mint_url` - URL of the Cashu mint to use (e.g., <https://mint.example.com>)
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    pub async fn set_mint_url(&self, mint_url: String) -> Result<NpubCashUserResponse, FfiError> {
        let response = self
            .inner
            .set_mint_url(mint_url)
            .await
            .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(response.into())
    }
}

/// A quote from the NpubCash service
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct NpubCashQuote {
    /// Unique identifier for the quote
    pub id: String,
    /// Amount in the specified unit
    pub amount: u64,
    /// Currency or unit for the amount (e.g., "sat")
    pub unit: String,
    /// Unix timestamp when the quote was created
    pub created_at: u64,
    /// Unix timestamp when the quote was paid (if paid)
    pub paid_at: Option<u64>,
    /// Unix timestamp when the quote expires
    pub expires_at: Option<u64>,
    /// Mint URL associated with the quote
    pub mint_url: Option<String>,
    /// Lightning invoice request
    pub request: Option<String>,
    /// Quote state (e.g., "PAID", "PENDING")
    pub state: Option<String>,
    /// Whether the quote is locked
    pub locked: Option<bool>,
}

impl From<cdk_npubcash::Quote> for NpubCashQuote {
    fn from(quote: cdk_npubcash::Quote) -> Self {
        Self {
            id: quote.id,
            amount: quote.amount,
            unit: quote.unit,
            created_at: quote.created_at,
            paid_at: quote.paid_at,
            expires_at: quote.expires_at,
            mint_url: quote.mint_url,
            request: quote.request,
            state: quote.state,
            locked: quote.locked,
        }
    }
}

/// Convert a NpubCash quote to a wallet MintQuote
///
/// This allows the quote to be used with the wallet's minting functions.
/// Note that the resulting MintQuote will not have a secret key set,
/// which may be required for locked quotes.
///
/// # Arguments
///
/// * `quote` - The NpubCash quote to convert
///
/// # Returns
///
/// A MintQuote that can be used with wallet minting functions
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn npubcash_quote_to_mint_quote(quote: NpubCashQuote) -> MintQuote {
    let cdk_quote = cdk_npubcash::Quote {
        id: quote.id,
        amount: quote.amount,
        unit: quote.unit,
        created_at: quote.created_at,
        paid_at: quote.paid_at,
        expires_at: quote.expires_at,
        mint_url: quote.mint_url,
        request: quote.request,
        state: quote.state,
        locked: quote.locked,
    };

    let mint_quote: cdk::wallet::MintQuote = cdk_quote.into();
    mint_quote.into()
}

/// Response from updating user settings on NpubCash
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[derive(Debug, Clone)]
pub struct NpubCashUserResponse {
    /// Whether the request resulted in an error
    pub error: bool,
    /// User's public key
    pub pubkey: String,
    /// Configured mint URL
    pub mint_url: Option<String>,
    /// Whether quotes are locked
    pub lock_quote: bool,
}

impl From<cdk_npubcash::UserResponse> for NpubCashUserResponse {
    fn from(response: cdk_npubcash::UserResponse) -> Self {
        Self {
            error: response.error,
            pubkey: response.data.user.pubkey,
            mint_url: response.data.user.mint_url,
            lock_quote: response.data.user.lock_quote,
        }
    }
}

/// Derive Nostr keys from a wallet seed
///
/// This function derives the same Nostr keys that a wallet would use for NpubCash
/// authentication. It takes the first 32 bytes of the seed as the secret key.
///
/// # Arguments
///
/// * `seed` - The wallet seed bytes (must be at least 32 bytes)
///
/// # Returns
///
/// The hex-encoded Nostr secret key that can be used with `NpubCashClient::new()`
///
/// # Errors
///
/// Returns an error if the seed is too short or key derivation fails
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn npubcash_derive_secret_key_from_seed(seed: Vec<u8>) -> Result<String, FfiError> {
    if seed.len() < 32 {
        return Err(FfiError::internal(
            "Seed must be at least 32 bytes".to_string(),
        ));
    }

    // Use the first 32 bytes of the seed as the secret key
    let secret_key = nostr_sdk::SecretKey::from_slice(&seed[..32])
        .map_err(|e| FfiError::internal(format!("Failed to derive secret key: {}", e)))?;

    Ok(secret_key.to_secret_hex())
}

/// Get the public key for a given Nostr secret key
///
/// # Arguments
///
/// * `nostr_secret_key` - Nostr secret key. Accepts either:
///   - Hex-encoded secret key (64 characters)
///   - Bech32 `nsec` format (e.g., "nsec1...")
///
/// # Returns
///
/// The hex-encoded public key
///
/// # Errors
///
/// Returns an error if the secret key is invalid
#[cfg_attr(feature = "uniffi-bindings", uniffi::export)]
pub fn npubcash_get_pubkey(nostr_secret_key: String) -> Result<String, FfiError> {
    let keys = parse_nostr_secret_key(&nostr_secret_key)?;
    Ok(keys.public_key().to_hex())
}

/// Parse a Nostr secret key from either hex or nsec format
fn parse_nostr_secret_key(key: &str) -> Result<nostr_sdk::Keys, FfiError> {
    // Try parsing as nsec (bech32) first
    if key.starts_with("nsec") {
        nostr_sdk::Keys::parse(key)
            .map_err(|e| FfiError::internal(format!("Invalid nsec key: {}", e)))
    } else {
        // Try parsing as hex
        let secret_key = nostr_sdk::SecretKey::parse(key)
            .map_err(|e| FfiError::internal(format!("Invalid hex secret key: {}", e)))?;
        Ok(nostr_sdk::Keys::new(secret_key))
    }
}
