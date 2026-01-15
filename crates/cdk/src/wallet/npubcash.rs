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

/// KV store namespace for npubcash-related data
pub const NPUBCASH_KV_NAMESPACE: &str = "npubcash";
/// KV store key for the last fetch timestamp (stored as u64 Unix timestamp)
const LAST_FETCH_TIMESTAMP_KEY: &str = "last_fetch_timestamp";
/// KV store key for the active mint URL
pub const ACTIVE_MINT_KEY: &str = "active_mint";

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
    /// This method fetches quotes from the last stored fetch timestamp and updates
    /// the timestamp after successful fetch. If no timestamp is stored, it fetches
    /// all quotes.
    ///
    /// # Errors
    ///
    /// Returns an error if NpubCash is not enabled or the sync fails
    #[instrument(skip(self))]
    pub async fn sync_npubcash_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        let client = self.get_npubcash_client().await?;

        // Get the last fetch timestamp from KV store
        let since = self.get_last_npubcash_fetch_timestamp().await?;

        let quotes = client
            .get_quotes(since)
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync quotes: {}", e)))?;

        // Update the last fetch timestamp to the max created_at from fetched quotes
        if let Some(max_ts) = quotes.iter().map(|q| q.created_at).max() {
            self.set_last_npubcash_fetch_timestamp(max_ts).await?;
        }

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

    /// Create a stream that continuously polls NpubCash and yields proofs as payments arrive
    ///
    /// # Arguments
    ///
    /// * `split_target` - How to split the minted proofs
    /// * `spending_conditions` - Optional spending conditions for the minted proofs
    /// * `poll_interval` - How often to check for new quotes
    pub fn npubcash_proof_stream(
        &self,
        split_target: cdk_common::amount::SplitTarget,
        spending_conditions: Option<crate::nuts::SpendingConditions>,
        poll_interval: std::time::Duration,
    ) -> crate::wallet::streams::npubcash::WalletNpubCashProofStream {
        crate::wallet::streams::npubcash::WalletNpubCashProofStream::new(
            self.clone(),
            poll_interval,
            split_target,
            spending_conditions,
        )
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

        self.localstore.add_mint_quote(mint_quote.clone()).await?;

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

    /// Get the last fetch timestamp from KV store
    ///
    /// Returns the Unix timestamp of the last successful npubcash fetch,
    /// or `None` if no fetch has been recorded yet.
    async fn get_last_npubcash_fetch_timestamp(&self) -> Result<Option<u64>, Error> {
        let value = self
            .localstore
            .kv_read(NPUBCASH_KV_NAMESPACE, "", LAST_FETCH_TIMESTAMP_KEY)
            .await?;

        match value {
            Some(bytes) => {
                let timestamp =
                    u64::from_be_bytes(bytes.try_into().map_err(|_| {
                        Error::Custom("Invalid timestamp format in KV store".into())
                    })?);
                Ok(Some(timestamp))
            }
            None => Ok(None),
        }
    }

    /// Store the last fetch timestamp in KV store
    ///
    /// # Arguments
    ///
    /// * `timestamp` - Unix timestamp of the fetch
    async fn set_last_npubcash_fetch_timestamp(&self, timestamp: u64) -> Result<(), Error> {
        self.localstore
            .kv_write(
                NPUBCASH_KV_NAMESPACE,
                "",
                LAST_FETCH_TIMESTAMP_KEY,
                &timestamp.to_be_bytes(),
            )
            .await?;
        Ok(())
    }
}
