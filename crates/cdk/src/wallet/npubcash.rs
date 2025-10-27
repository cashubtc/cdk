//! NpubCash integration for CDK Wallet
//!
//! This module provides integration between the CDK wallet and the NpubCash service,
//! allowing wallets to sync quotes, subscribe to updates, and manage NpubCash settings.

use std::sync::Arc;

use cdk_npubcash::{JwtAuthProvider, NpubCashClient, Quote};
use tracing::instrument;

use crate::error::Error;
use crate::nuts::SecretKey;
use crate::wallet::types::{MintQuote, TransactionDirection};
use crate::wallet::Wallet;

impl Wallet {
    /// Enable NpubCash integration for this wallet
    ///
    /// # Arguments
    ///
    /// * `npubcash_url` - Base URL of the NpubCash service (e.g., "<https://npubx.cash>")
    ///
    /// # Errors
    ///
    /// Returns an error if the NpubCash client cannot be initialized
    #[instrument(skip(self))]
    pub async fn enable_npubcash(&self, npubcash_url: String) -> Result<(), Error> {
        let keys = self.derive_npubcash_keys()?;
        let auth_provider = Arc::new(JwtAuthProvider::new(npubcash_url.clone(), keys));
        let client = Arc::new(NpubCashClient::new(npubcash_url.clone(), auth_provider));

        let mut npubcash = self.npubcash_client.write().await;
        *npubcash = Some(client.clone());
        drop(npubcash);

        tracing::info!("NpubCash integration enabled");

        // Automatically set the mint URL on the NpubCash server
        let mint_url = self.mint_url.to_string();
        match client.set_mint_url(&mint_url).await {
            Ok(_) => {
                tracing::info!(
                    "Mint URL '{}' set on NpubCash server at '{}'",
                    mint_url,
                    npubcash_url
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to set mint URL on NpubCash server: {}. Quotes may use server default.",
                    e
                );
            }
        }

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

    /// Helper to get NpubCash client reference
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled
    async fn get_npubcash_client(&self) -> Result<Arc<NpubCashClient>, Error> {
        self.npubcash_client
            .read()
            .await
            .clone()
            .ok_or_else(|| Error::Custom("NpubCash not enabled".to_string()))
    }

    /// Helper to process npubcash quotes and add them to the wallet
    ///
    /// # Errors
    ///
    /// Returns an error if adding quotes fails
    async fn process_npubcash_quotes(&self, quotes: Vec<Quote>) -> Result<Vec<MintQuote>, Error> {
        let mut mint_quotes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            if let Some(mint_quote) = self.add_npubcash_mint_quote(quote).await? {
                mint_quotes.push(mint_quote);
            }
        }
        Ok(mint_quotes)
    }

    /// Sync quotes from NpubCash and add them to the wallet
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or the sync fails
    #[instrument(skip(self))]
    pub async fn sync_npubcash_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let client = self.get_npubcash_client().await?;
        let quotes = client
            .get_quotes(None)
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync quotes: {}", e)))?;
        self.process_npubcash_quotes(quotes).await
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
        let client = self.get_npubcash_client().await?;
        let quotes = client
            .get_quotes(Some(since))
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync quotes: {}", e)))?;
        self.process_npubcash_quotes(quotes).await
    }

    /// Subscribe to NpubCash quote updates via polling and add them to the wallet
    ///
    /// This method polls for new quotes every 5 seconds and calls the callback
    /// with newly added quotes. This function runs indefinitely and only returns
    /// on error.
    ///
    /// # Arguments
    ///
    /// * `callback` - Function to call when new quotes are found and added to wallet
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or if fetching/processing quotes fails
    #[instrument(skip(self, callback))]
    pub async fn subscribe_npubcash_updates<F>(&self, callback: F) -> Result<(), Error>
    where
        F: Fn(Vec<MintQuote>) + Send + Sync + 'static,
    {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        tracing::info!("Starting NpubCash quote polling");

        // Verify NpubCash is enabled
        let client = self.get_npubcash_client().await?;

        // Get initial timestamp
        let mut last_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Custom(e.to_string()))?
            .as_secs();

        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;

            // Fetch raw npubcash quotes
            match client.get_quotes(Some(last_timestamp)).await {
                Ok(npubcash_quotes) => {
                    if !npubcash_quotes.is_empty() {
                        tracing::debug!("Found {} new quotes", npubcash_quotes.len());

                        // Update timestamp to most recent quote
                        if let Some(max_ts) = npubcash_quotes.iter().map(|q| q.created_at).max() {
                            last_timestamp = max_ts;
                        }

                        // Convert quotes and add to wallet
                        let mut mint_quotes = Vec::with_capacity(npubcash_quotes.len());
                        for quote in npubcash_quotes {
                            match self.add_npubcash_mint_quote(quote).await {
                                Ok(Some(mint_quote)) => mint_quotes.push(mint_quote),
                                Ok(None) => (),
                                Err(e) => tracing::error!("Failed to add NpubCash quote: {}", e),
                            }
                        }

                        if !mint_quotes.is_empty() {
                            callback(mint_quotes);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("Error polling quotes: {}", e);
                    // Continue polling despite errors
                }
            }
        }
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
        let client = self.get_npubcash_client().await?;
        client
            .set_mint_url(mint_url)
            .await
            .map_err(|e| Error::Custom(e.to_string()))
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

        let mut tx = self.localstore.begin_db_transaction().await?;
        tx.add_mint_quote(mint_quote.clone()).await?;
        tx.commit().await?;

        tracing::info!("Added NpubCash quote {} to wallet database", mint_quote.id);
        Ok(Some(mint_quote))
    }

    /// Get reference to the NpubCash client if enabled
    pub async fn npubcash_client(&self) -> Option<Arc<NpubCashClient>> {
        self.npubcash_client.read().await.clone()
    }

    /// Check if NpubCash is enabled for this wallet
    pub async fn is_npubcash_enabled(&self) -> bool {
        self.npubcash_client.read().await.is_some()
    }
}
