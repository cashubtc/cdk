//! FFI bindings for the NpubCash client SDK
//!
//! This module provides FFI-compatible bindings for interacting with the NpubCash API.
//! The client can be used standalone without requiring a wallet.

use std::sync::Arc;

use cdk_nostr::npubcash::{JwtAuthProvider, NpubCashClient as CdkNpubCashClient};

use crate::error::FfiError;
use crate::nostr::NostrSigner;
use crate::types::MintQuote;

/// FFI-compatible NpubCash client
///
/// This client provides access to the NpubCash API for fetching quotes
/// and managing user settings.
#[derive(uniffi::Object)]
pub struct NpubCashClient {
    inner: Arc<CdkNpubCashClient>,
    identity_pubkey: String,
}

impl NpubCashClient {
    fn build(base_url: String, keys: cdk_nostr::nostr_sdk::Keys) -> Self {
        let identity_pubkey = keys.public_key().to_hex();
        let auth_provider = Arc::new(JwtAuthProvider::new(base_url.clone(), keys));
        let client = CdkNpubCashClient::new(base_url, auth_provider);

        Self {
            inner: Arc::new(client),
            identity_pubkey,
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
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
    #[uniffi::constructor]
    pub fn new(base_url: String, nostr_secret_key: String) -> Result<Self, FfiError> {
        let signer = NostrSigner::parse_secret_key(&nostr_secret_key)?;
        Ok(Self::build(base_url, signer.keys().clone()))
    }

    /// Create a client using an existing typed Nostr identity.
    ///
    /// This is the recommended mobile constructor. The exact supplied signer
    /// is used for npub.cash authentication, so a mnemonic-derived, imported,
    /// or generated active identity can be shared with event signing, NIP-44,
    /// the NIP-17 inbox, and the application's primary P2PK identity.
    #[uniffi::constructor]
    pub fn with_signer(base_url: String, signer: Arc<NostrSigner>) -> Self {
        Self::build(base_url, signer.keys().clone())
    }

    /// Hex-encoded x-only public key used for npub.cash authentication.
    pub fn identity_pubkey(&self) -> String {
        self.identity_pubkey.clone()
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

    /// Resolve full quote data for specific quote IDs
    ///
    /// Asks the NpubCash server for the quotes matching `quote_ids`. Used to
    /// reconcile local state with the server: fetch all quote IDs, determine
    /// which ones are unknown locally, and resolve only those.
    ///
    /// # Arguments
    ///
    /// * `quote_ids` - Quote IDs to resolve
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    pub async fn get_missing_quotes(
        &self,
        quote_ids: Vec<String>,
    ) -> Result<Vec<NpubCashQuote>, FfiError> {
        let quotes = self
            .inner
            .get_missing_quotes(&quote_ids)
            .await
            .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(quotes.into_iter().map(Into::into).collect())
    }

    /// Enable or disable NUT-20 quote locking for this NpubCash account
    ///
    /// When enabled, the NpubCash server creates new mint quotes locked to the
    /// account's Nostr public key, so claiming them requires a NUT-20 quote
    /// signature from the matching secret key. The server rejects enabling
    /// locking when the configured mint does not support NUT-20.
    ///
    /// Already-created quotes keep their original lock state.
    ///
    /// # Arguments
    ///
    /// * `lock_quotes` - Whether new quotes should be locked to the npub
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    pub async fn set_quote_locking(
        &self,
        lock_quotes: bool,
    ) -> Result<NpubCashUserResponse, FfiError> {
        let response = self
            .inner
            .set_quote_locking(lock_quotes)
            .await
            .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(response.into())
    }

    /// Fetch the NpubCash account settings
    ///
    /// Returns the configured mint URL and whether quote locking is enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the API request fails or authentication fails
    pub async fn get_user_info(&self) -> Result<NpubCashUserResponse, FfiError> {
        let response = self
            .inner
            .get_user_info()
            .await
            .map_err(|e| FfiError::internal(e.to_string()))?;

        Ok(response.into())
    }
}

/// A quote from the NpubCash service
#[derive(Debug, Clone, uniffi::Record)]
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

impl From<cdk_nostr::npubcash::Quote> for NpubCashQuote {
    fn from(quote: cdk_nostr::npubcash::Quote) -> Self {
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
#[uniffi::export]
pub fn npubcash_quote_to_mint_quote(quote: NpubCashQuote) -> MintQuote {
    let cdk_quote = cdk_nostr::npubcash::Quote {
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
#[derive(Debug, Clone, uniffi::Record)]
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

impl From<cdk_nostr::npubcash::UserResponse> for NpubCashUserResponse {
    fn from(response: cdk_nostr::npubcash::UserResponse) -> Self {
        let user = response.data.into_user();
        Self {
            error: response.error,
            pubkey: user.pubkey,
            mint_url: user.mint_url,
            lock_quote: user.lock_quote,
        }
    }
}

/// Derive Nostr keys from a wallet seed for Rust compatibility.
///
/// This is deliberately not exported through UniFFI: Swift and Kotlin should
/// use `NostrSigner.fromMnemonic` or `NostrSigner.fromWalletRepository` so a
/// BIP-39 seed never crosses the language boundary.
///
/// # Arguments
///
/// * `seed` - The wallet seed bytes (must be at least 64 bytes)
///
/// # Returns
///
/// The hex-encoded Nostr secret key that can be used with `NpubCashClient::new()`
///
/// # Errors
///
/// Returns an error if the seed is too short or key derivation fails
pub fn npubcash_derive_secret_key_from_seed(seed: Vec<u8>) -> Result<String, FfiError> {
    if seed.len() < 64 {
        return Err(FfiError::internal(
            "Seed must be at least 64 bytes".to_string(),
        ));
    }

    let seed: [u8; 64] = seed[..64]
        .try_into()
        .map_err(|_| FfiError::internal("Failed to read wallet seed bytes".to_string()))?;
    let secret_key = cdk::wallet::derive_npubcash_secret_key_from_seed(&seed)
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
#[uniffi::export]
pub fn npubcash_get_pubkey(nostr_secret_key: String) -> Result<String, FfiError> {
    Ok(NostrSigner::parse_secret_key(&nostr_secret_key)?.public_key_hex())
}

#[cfg(test)]
mod tests {
    use cdk_nostr::nostr_sdk::{Keys, ToBech32};
    use cdk_nostr::npubcash::types::UserDataContainer;
    use cdk_nostr::npubcash::{UserData, UserResponse};

    use super::*;
    use crate::types::Amount;

    const HEX_SECRET_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const HEX_PUBLIC_KEY: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";

    #[test]
    fn npubcash_seed_derivation_uses_wallet_nip06_key() {
        let seed = [0x42u8; 64];
        let secret_hex = npubcash_derive_secret_key_from_seed(seed.to_vec())
            .expect("npubcash key derives from wallet seed");
        let wallet_secret = cdk::wallet::derive_npubcash_secret_key_from_seed(&seed)
            .expect("wallet npubcash key derives");

        assert_eq!(secret_hex, wallet_secret.to_secret_hex());
        assert_ne!(&wallet_secret.to_secret_bytes()[..], &seed[..32]);
    }

    #[test]
    fn npubcash_seed_derivation_rejects_short_seed() {
        assert!(npubcash_derive_secret_key_from_seed(vec![0x42u8; 32]).is_err());
    }

    #[test]
    fn npubcash_get_pubkey_accepts_hex_and_nsec() {
        assert_eq!(
            npubcash_get_pubkey(HEX_SECRET_KEY.to_string()).expect("hex key parses"),
            HEX_PUBLIC_KEY
        );

        let keys = Keys::generate();
        let nsec = keys.secret_key().to_bech32().expect("nsec encodes");
        assert_eq!(
            npubcash_get_pubkey(nsec).expect("nsec key parses"),
            keys.public_key().to_hex()
        );
    }

    #[test]
    fn npubcash_get_pubkey_rejects_invalid_key() {
        assert!(npubcash_get_pubkey("not-a-key".to_string()).is_err());
    }

    #[test]
    fn client_rejects_invalid_secret_key() {
        assert!(
            NpubCashClient::new("https://npub.cash".to_string(), "invalid".to_string()).is_err()
        );
    }

    #[test]
    fn client_uses_exactly_the_supplied_typed_signer() {
        let signer = Arc::new(
            NostrSigner::from_secret_key_hex(HEX_SECRET_KEY.to_string())
                .expect("typed signer parses"),
        );
        let client = NpubCashClient::with_signer("https://npub.cash".to_string(), signer.clone());

        assert_eq!(client.identity_pubkey(), signer.public_key_hex());
        assert_eq!(client.identity_pubkey(), HEX_PUBLIC_KEY);
    }

    #[test]
    fn client_accepts_mnemonic_derived_and_generated_identities() {
        let mnemonic = Arc::new(
            NostrSigner::from_mnemonic(
                "leader monkey parrot ring guide accident before fence cannon height naive bean"
                    .to_string(),
                None,
            )
            .expect("mnemonic signer derives"),
        );
        let mnemonic_client =
            NpubCashClient::with_signer("https://npub.cash".to_string(), mnemonic.clone());
        assert_eq!(mnemonic_client.identity_pubkey(), mnemonic.public_key_hex());

        let generated = Arc::new(NostrSigner::generate());
        let generated_client =
            NpubCashClient::with_signer("https://npub.cash".to_string(), generated.clone());
        assert_eq!(
            generated_client.identity_pubkey(),
            generated.public_key_hex()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_quotes_surfaces_request_errors() {
        let client = NpubCashClient::new(
            // Unroutable loopback port: the request fails fast without network
            "http://127.0.0.1:1".to_string(),
            HEX_SECRET_KEY.to_string(),
        )
        .expect("client builds with a valid key");

        assert!(client.get_quotes(None).await.is_err());
    }

    #[test]
    fn npubcash_quote_to_mint_quote_maps_fields_without_secret_key() {
        let quote = NpubCashQuote {
            id: "quote-id".to_string(),
            amount: 42,
            unit: "sat".to_string(),
            created_at: 1,
            paid_at: Some(10),
            expires_at: Some(100),
            mint_url: Some("https://mint.example.com".to_string()),
            request: Some("lnbc42n1example".to_string()),
            state: Some("PAID".to_string()),
            locked: Some(true),
        };

        let mint_quote = npubcash_quote_to_mint_quote(quote);

        assert_eq!(mint_quote.id, "quote-id");
        assert_eq!(mint_quote.amount, Some(Amount::new(42)));
        assert_eq!(mint_quote.request, "lnbc42n1example");
        assert_eq!(mint_quote.expiry, 100);
        assert_eq!(mint_quote.updated_at, 10);
        assert!(
            mint_quote.secret_key.is_none(),
            "conversion must not invent a secret key"
        );
    }

    #[test]
    fn user_response_conversion_supports_wrapped_and_flat_layouts() {
        let user = UserData {
            pubkey: "npub1test".to_string(),
            mint_url: Some("https://mint.example.com".to_string()),
            lock_quote: true,
        };

        let wrapped: NpubCashUserResponse = UserResponse {
            error: false,
            data: UserDataContainer::Wrapped { user: user.clone() },
        }
        .into();
        assert!(!wrapped.error);
        assert_eq!(wrapped.pubkey, "npub1test");
        assert_eq!(
            wrapped.mint_url.as_deref(),
            Some("https://mint.example.com")
        );
        assert!(wrapped.lock_quote);

        let flat: NpubCashUserResponse = UserResponse {
            error: false,
            data: UserDataContainer::Flat(UserData {
                lock_quote: false,
                mint_url: None,
                ..user
            }),
        }
        .into();
        assert_eq!(flat.pubkey, "npub1test");
        assert_eq!(flat.mint_url, None);
        assert!(!flat.lock_quote);
    }
}
