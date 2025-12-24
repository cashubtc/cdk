//! Nostr Mint Backup
//!
//! This module provides functionality to backup and restore the mint list
//! to/from Nostr relays using NUT-XX specification.

use std::time::Duration;

use nostr_sdk::prelude::*;
use nostr_sdk::{Client as NostrClient, Filter, Keys};
use tracing::instrument;

use super::multi_mint_wallet::MultiMintWallet;
use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nutxx::{
    self, backup_filter_params, create_backup_event, decrypt_backup_event, MintBackup,
};

/// Options for backup operations
#[derive(Debug, Clone, Default)]
pub struct BackupOptions {
    /// Client name to include in the event tags
    pub client: Option<String>,
}

impl BackupOptions {
    /// Create new backup options
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the client name
    pub fn client(mut self, client: impl Into<String>) -> Self {
        self.client = Some(client.into());
        self
    }
}

/// Options for restore operations
#[derive(Debug, Clone)]
pub struct RestoreOptions {
    /// Timeout for waiting for relay responses
    pub timeout: Duration,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }
}

impl RestoreOptions {
    /// Create new restore options
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timeout for relay responses
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Result of a backup operation
#[derive(Debug, Clone)]
pub struct BackupResult {
    /// The event ID of the published backup
    pub event_id: EventId,
    /// The public key used for the backup
    pub public_key: PublicKey,
    /// Number of mints backed up
    pub mint_count: usize,
}

/// Result of a restore operation
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// The restored mint backup data
    pub backup: MintBackup,
    /// Number of mints found in the backup
    pub mint_count: usize,
    /// Number of mints that were newly added (not already in wallet)
    pub mints_added: usize,
}

impl MultiMintWallet {
    /// Derive the Nostr keys used for mint backup from the wallet seed
    ///
    /// These keys can be used to identify and decrypt backup events.
    pub fn backup_keys(&self) -> Result<Keys, Error> {
        nutxx::derive_nostr_keys(self.seed()).map_err(|e| Error::Custom(e.to_string()))
    }

    /// Backup the current mint list to Nostr relays
    ///
    /// This creates an encrypted NIP-78 addressable event containing all mint URLs
    /// and publishes it to the specified relays.
    ///
    /// # Arguments
    ///
    /// * `relays` - List of relay URLs to publish the backup to
    /// * `options` - Optional backup configuration
    ///
    /// # Example
    ///
    /// ```ignore
    /// let relays = vec!["wss://relay.damus.io", "wss://nos.lol"];
    /// let result = wallet.backup_mints(
    ///     relays,
    ///     BackupOptions::new().client("my-wallet"),
    /// ).await?;
    /// println!("Backup published with event ID: {}", result.event_id);
    /// ```
    #[instrument(skip(self, relays))]
    pub async fn backup_mints<S>(
        &self,
        relays: Vec<S>,
        options: BackupOptions,
    ) -> Result<BackupResult, Error>
    where
        S: AsRef<str>,
    {
        // Get the backup keys
        let keys = self.backup_keys()?;

        // Get all mint URLs from the wallet
        let wallets = self.get_wallets().await;
        let mint_urls: Vec<MintUrl> = wallets.iter().map(|w| w.mint_url.clone()).collect();

        // Create the backup data
        let backup = MintBackup::new(mint_urls.clone());

        // Create the encrypted event
        let event = create_backup_event(&keys, &backup, options.client.as_deref())
            .map_err(|e| Error::Custom(format!("Failed to create backup event: {e}")))?;

        let event_id = event.id;

        // Create Nostr client and connect to relays
        let client = NostrClient::new(keys.clone());

        for relay in relays.iter() {
            client
                .add_write_relay(relay.as_ref())
                .await
                .map_err(|e| Error::Custom(format!("Failed to add relay: {e}")))?;
        }

        client.connect().await;

        // Publish the event
        client
            .send_event(&event)
            .await
            .map_err(|e| Error::Custom(format!("Failed to publish backup event: {e}")))?;

        // Disconnect from relays
        client.disconnect().await;

        Ok(BackupResult {
            event_id,
            public_key: keys.public_key(),
            mint_count: mint_urls.len(),
        })
    }

    /// Restore mint list from Nostr relays
    ///
    /// This fetches the most recent backup event from the specified relays,
    /// decrypts it, and optionally adds the discovered mints to the wallet.
    ///
    /// # Arguments
    ///
    /// * `relays` - List of relay URLs to fetch the backup from
    /// * `add_mints` - If true, automatically add discovered mints to the wallet
    /// * `options` - Optional restore configuration
    ///
    /// # Example
    ///
    /// ```ignore
    /// let relays = vec!["wss://relay.damus.io", "wss://nos.lol"];
    /// let result = wallet.restore_mints(
    ///     relays,
    ///     true, // automatically add mints
    ///     RestoreOptions::default(),
    /// ).await?;
    /// println!("Restored {} mints, {} newly added", result.mint_count, result.mints_added);
    /// ```
    #[instrument(skip(self, relays))]
    pub async fn restore_mints<S>(
        &self,
        relays: Vec<S>,
        add_mints: bool,
        options: RestoreOptions,
    ) -> Result<RestoreResult, Error>
    where
        S: AsRef<str>,
    {
        // Get the backup keys
        let keys = self.backup_keys()?;

        // Get filter parameters for the backup event
        let (kind, pubkey, d_tag) = backup_filter_params(&keys);

        // Create filter for addressable event
        let filter = Filter::new()
            .kind(kind)
            .author(pubkey)
            .identifier(d_tag)
            .limit(1);

        // Create Nostr client and connect to relays
        let client = NostrClient::new(keys.clone());

        for relay in relays.iter() {
            client
                .add_read_relay(relay.as_ref())
                .await
                .map_err(|e| Error::Custom(format!("Failed to add relay: {e}")))?;
        }

        client.connect().await;

        // Fetch events matching the filter
        let events = client
            .fetch_events(filter, options.timeout)
            .await
            .map_err(|e| Error::Custom(format!("Failed to fetch backup events: {e}")))?;

        // Disconnect from relays
        client.disconnect().await;

        // Get the most recent event (should be only one due to addressable event semantics)
        let event = events
            .into_iter()
            .next()
            .ok_or_else(|| Error::Custom("No backup event found".to_string()))?;

        // Decrypt and parse the backup
        let backup = decrypt_backup_event(&keys, &event)
            .map_err(|e| Error::Custom(format!("Failed to decrypt backup event: {e}")))?;

        let mint_count = backup.mints.len();
        let mut mints_added = 0;

        // Optionally add mints to the wallet
        if add_mints {
            for mint_url in &backup.mints {
                // Check if mint already exists
                if !self.has_mint(mint_url).await {
                    // Try to add the mint (ignore errors for individual mints)
                    if self.add_mint(mint_url.clone()).await.is_ok() {
                        mints_added += 1;
                    }
                }
            }
        }

        Ok(RestoreResult {
            backup,
            mint_count,
            mints_added,
        })
    }

    /// Fetch the backup without adding mints to the wallet
    ///
    /// This is useful for previewing what mints are in the backup before
    /// deciding to add them.
    ///
    /// # Arguments
    ///
    /// * `relays` - List of relay URLs to fetch the backup from
    /// * `options` - Optional restore configuration
    #[instrument(skip(self, relays))]
    pub async fn fetch_backup<S>(
        &self,
        relays: Vec<S>,
        options: RestoreOptions,
    ) -> Result<MintBackup, Error>
    where
        S: AsRef<str>,
    {
        let result = self.restore_mints(relays, false, options).await?;
        Ok(result.backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_options() {
        let options = BackupOptions::new().client("test-client");
        assert_eq!(options.client, Some("test-client".to_string()));
    }

    #[test]
    fn test_restore_options() {
        let options = RestoreOptions::new().timeout(Duration::from_secs(30));
        assert_eq!(options.timeout, Duration::from_secs(30));
    }
}
