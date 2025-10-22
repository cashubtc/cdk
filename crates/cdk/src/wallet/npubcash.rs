//! NpubCash integration for CDK Wallet
//!
//! This module provides integration between the CDK wallet and the NpubCash service,
//! allowing wallets to sync quotes, subscribe to updates, and manage NpubCash settings.

use std::sync::Arc;

use cdk_npubcash::{JwtAuthProvider, NpubCashClient, PollingHandle, Quote, UserResponse};
use nostr_sdk::Keys;
use tracing::instrument;

use crate::error::Error;
use crate::nuts::SecretKey;
use crate::wallet::types::{MintQuote, TransactionDirection};
use crate::wallet::Wallet;

/// NpubCash client wrapper for wallet integration
#[derive(Debug, Clone)]
pub struct NpubCashWallet {
    client: Arc<NpubCashClient>,
}

impl NpubCashWallet {
    /// Create a new NpubCash wallet instance
    ///
    /// # Arguments
    ///
    /// * `npubcash_url` - Base URL of the NpubCash service (e.g., "<https://npubx.cash>")
    /// * `keys` - Nostr keys for authentication
    pub fn new(npubcash_url: String, keys: Keys) -> Self {
        let auth_provider = Arc::new(JwtAuthProvider::new(npubcash_url.clone(), keys));
        let client = Arc::new(NpubCashClient::new(npubcash_url, auth_provider));

        Self { client }
    }

    /// Fetch all quotes from NpubCash
    ///
    /// This method fetches all available quotes with automatic pagination.
    #[instrument(skip(self))]
    pub async fn sync_quotes(&self) -> Result<Vec<Quote>, Error> {
        tracing::info!("Syncing all quotes from NpubCash");

        let quotes = self
            .client
            .get_all_quotes()
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync quotes: {}", e)))?;

        tracing::debug!("Successfully synced {} quotes", quotes.len());
        Ok(quotes)
    }

    /// Fetch quotes since a specific timestamp
    ///
    /// # Arguments
    ///
    /// * `since` - Unix timestamp to fetch quotes from
    #[instrument(skip(self))]
    pub async fn sync_quotes_since(&self, since: u64) -> Result<Vec<Quote>, Error> {
        tracing::info!("Syncing quotes since timestamp: {}", since);

        let quotes = self
            .client
            .get_quotes_since(since)
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync quotes: {}", e)))?;

        tracing::debug!("Successfully synced {} quotes", quotes.len());
        Ok(quotes)
    }

    /// Subscribe to real-time quote updates via polling
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call when new quotes are found
    ///
    /// # Returns
    ///
    /// A [`PollingHandle`] that will stop polling when dropped
    #[instrument(skip(self, callback))]
    pub async fn subscribe_updates<F>(&self, callback: F) -> Result<PollingHandle, Error>
    where
        F: FnMut(Vec<Quote>) + Send + 'static,
    {
        use std::time::Duration;

        tracing::info!("Starting NpubCash quote polling");

        let handle = self
            .client
            .poll_quotes_with_callback(Duration::from_secs(5), callback)
            .await
            .map_err(|e| Error::Custom(format!("Failed to start polling: {}", e)))?;

        tracing::debug!("Successfully started quote polling");
        Ok(handle)
    }

    /// Set the mint URL in NpubCash settings
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint URL to set
    #[instrument(skip(self, mint_url))]
    pub async fn set_mint_url(&self, mint_url: impl Into<String>) -> Result<UserResponse, Error> {
        let mint_url = mint_url.into();
        tracing::info!("Setting NpubCash mint URL to: {}", mint_url);

        let response = self
            .client
            .settings
            .set_mint_url(mint_url)
            .await
            .map_err(|e| Error::Custom(format!("Failed to set mint URL: {}", e)))?;

        tracing::debug!("Successfully updated mint URL");
        Ok(response)
    }

    /// Get the underlying NpubCash client for advanced operations
    pub fn client(&self) -> &Arc<NpubCashClient> {
        &self.client
    }
}

impl Wallet {
    /// Enable NpubCash integration for this wallet
    ///
    /// # Arguments
    ///
    /// * `npubcash_url` - Base URL of the NpubCash service (e.g., "<https://npubx.cash>")
    ///
    /// # Errors
    ///
    /// Returns an error if the NpubCash wallet cannot be initialized
    #[instrument(skip(self))]
    pub async fn enable_npubcash(&self, npubcash_url: String) -> Result<(), Error> {
        let keys = self.derive_npubcash_keys()?;
        let npubcash_wallet = NpubCashWallet::new(npubcash_url, keys);

        let mut wallet = self.npubcash_wallet.write().await;
        *wallet = Some(npubcash_wallet);

        tracing::info!("NpubCash integration enabled");
        Ok(())
    }

    /// Derive Nostr keys from wallet seed for NpubCash authentication
    ///
    /// This uses the first 32 bytes of the wallet seed to derive a Nostr keypair.
    ///
    /// # Errors
    ///
    /// Returns an error if the key derivation fails
    fn derive_npubcash_keys(&self) -> Result<nostr_sdk::Keys, Error> {
        use nostr_sdk::SecretKey;

        let secret_key = SecretKey::from_slice(&self.seed[..32])
            .map_err(|e| Error::Custom(format!("Failed to derive Nostr keys: {}", e)))?;

        Ok(nostr_sdk::Keys::new(secret_key))
    }

    /// Get the Nostr keys used for NpubCash authentication
    ///
    /// Returns the derived Nostr keys from the wallet seed.
    /// These keys are used for authenticating with the NpubCash service.
    ///
    /// # Errors
    ///
    /// Returns an error if the key derivation fails
    pub fn get_npubcash_keys(&self) -> Result<nostr_sdk::Keys, Error> {
        self.derive_npubcash_keys()
    }

    /// Sync quotes from NpubCash and add them to the wallet
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or the sync fails
    #[instrument(skip(self))]
    pub async fn sync_npubcash_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let wallet = self.npubcash_wallet.read().await;
        let npubcash = wallet
            .as_ref()
            .ok_or_else(|| Error::Custom("NpubCash not enabled".to_string()))?;

        let quotes = npubcash.sync_quotes().await?;
        drop(wallet);

        let mut mint_quotes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let mint_quote = self.add_npubcash_mint_quote(quote).await?;

            if let Some(mint_quote) = mint_quote {
                mint_quotes.push(mint_quote);
            }
        }

        Ok(mint_quotes)
    }

    /// Sync quotes from NpubCash since a specific timestamp and add them to the wallet
    ///
    /// # Arguments
    ///
    /// * `since` - Unix timestamp to fetch quotes from
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or the sync fails
    #[instrument(skip(self))]
    pub async fn sync_npubcash_quotes_since(&self, since: u64) -> Result<Vec<MintQuote>, Error> {
        let wallet = self.npubcash_wallet.read().await;
        let npubcash = wallet
            .as_ref()
            .ok_or_else(|| Error::Custom("NpubCash not enabled".to_string()))?;

        let quotes = npubcash.sync_quotes_since(since).await?;
        drop(wallet);

        let mut mint_quotes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let mint_quote = self.add_npubcash_mint_quote(quote).await?;
            if let Some(mint_quote) = mint_quote {
                mint_quotes.push(mint_quote);
            }
        }

        Ok(mint_quotes)
    }

    /// Subscribe to NpubCash quote updates via polling and add them to the wallet
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call when new quotes are found and added to wallet
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or polling fails to start
    #[instrument(skip(self, callback))]
    pub async fn subscribe_npubcash_updates<F>(
        &self,
        callback: F,
    ) -> Result<cdk_npubcash::PollingHandle, Error>
    where
        F: Fn(Vec<MintQuote>) + Send + Sync + 'static,
    {
        use std::sync::Arc;

        let wallet_clone = self.clone();
        let callback = Arc::new(callback);

        let internal_callback = move |npubcash_quotes: Vec<cdk_npubcash::Quote>| {
            let wallet = wallet_clone.clone();
            let cb = callback.clone();

            tokio::spawn(async move {
                let mut mint_quotes = Vec::with_capacity(npubcash_quotes.len());
                for quote in npubcash_quotes {
                    match wallet.add_npubcash_mint_quote(quote).await {
                        Ok(Some(mint_quote)) => mint_quotes.push(mint_quote),
                        Ok(None) => (),
                        Err(e) => tracing::error!("Failed to add NpubCash quote: {}", e),
                    }
                }

                if !mint_quotes.is_empty() {
                    cb(mint_quotes);
                }
            });
        };

        let npubcash_wallet = self.npubcash_wallet.read().await;
        let npubcash = npubcash_wallet
            .as_ref()
            .ok_or_else(|| Error::Custom("NpubCash not enabled".to_string()))?;

        npubcash.subscribe_updates(internal_callback).await
    }

    /// Set the mint URL in NpubCash settings
    ///
    /// # Arguments
    ///
    /// * `mint_url` - The mint URL to set
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or the update fails
    #[instrument(skip(self, mint_url))]
    pub async fn set_npubcash_mint_url(
        &self,
        mint_url: impl Into<String>,
    ) -> Result<cdk_npubcash::UserResponse, Error> {
        let wallet = self.npubcash_wallet.read().await;
        let npubcash = wallet
            .as_ref()
            .ok_or_else(|| Error::Custom("NpubCash not enabled".to_string()))?;

        npubcash.set_mint_url(mint_url).await
    }

    /// Add an NpubCash quote to the wallet's mint quote database
    ///
    /// Converts an NpubCash quote to a wallet MintQuote and stores it using the
    /// NpubCash-derived secret key for signing.
    ///
    /// # Arguments
    ///
    /// * `npubcash_quote` - The NpubCash quote to add
    ///
    /// # Errors
    ///
    /// Returns an error if the conversion fails or the database operation fails
    #[instrument(skip(self))]
    pub async fn add_npubcash_mint_quote(
        &self,
        npubcash_quote: cdk_npubcash::Quote,
    ) -> Result<Option<MintQuote>, Error> {
        let npubcash_keys = self.derive_npubcash_keys()?;
        let secret_key = SecretKey::from_slice(&npubcash_keys.secret_key().to_secret_bytes())
            .map_err(|e| Error::Custom(format!("Failed to convert secret key: {}", e)))?;

        let mut mint_quote: MintQuote = npubcash_quote.into();
        mint_quote.secret_key = Some(secret_key);

        let exists = self
            .list_transactions(Some(TransactionDirection::Incoming))
            .await?
            .iter()
            .any(|tx| tx.quote_id.as_ref() == Some(&mint_quote.id));

        if exists {
            return Ok(None);
        }

        self.localstore.add_mint_quote(mint_quote.clone()).await?;

        tracing::info!("Added NpubCash quote {} to wallet database", mint_quote.id);
        Ok(Some(mint_quote))
    }

    /// Get reference to the NpubCash wallet if enabled
    pub async fn npubcash_wallet(&self) -> Option<NpubCashWallet> {
        self.npubcash_wallet.read().await.clone()
    }

    /// Check if NpubCash is enabled for this wallet
    pub async fn is_npubcash_enabled(&self) -> bool {
        self.npubcash_wallet.read().await.is_some()
    }
}
