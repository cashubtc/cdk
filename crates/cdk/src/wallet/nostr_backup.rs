//! Nostr Mint Backup
//!
//! This module provides functionality to backup and restore the mint list
//! to/from Nostr relays using NUT-27 specification.

use std::collections::BTreeSet;
use std::time::Duration;

use nostr_sdk::prelude::*;
use nostr_sdk::{Client as NostrClient, Filter, Keys};
use tracing::instrument;

use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut27::{
    self, backup_filter_params, create_backup_event, decrypt_backup_event, MintBackup,
};

/// Request to publish an encrypted NUT-27 mint backup.
#[derive(Debug, Clone)]
pub struct MintBackupRequest {
    /// Relay URLs that receive the backup event.
    pub relays: Vec<String>,
    /// Client name included in the event tags.
    pub client: Option<String>,
}

impl MintBackupRequest {
    /// Create a request for the provided relay URLs.
    pub fn new(relays: Vec<String>) -> Self {
        Self {
            relays,
            client: None,
        }
    }

    /// Include a client name in the published event.
    pub fn with_client(mut self, client: impl Into<String>) -> Self {
        self.client = Some(client.into());
        self
    }
}

/// Whether restoring a mint backup changes manager configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum MintRestorePolicy {
    /// Return the decrypted backup without registering its mints.
    #[default]
    Preview,
    /// Register every mint not already known to the manager.
    Register,
}

/// Request to fetch and decrypt a NUT-27 mint backup.
#[derive(Debug, Clone)]
pub struct MintRestoreRequest {
    /// Relay URLs queried for the backup event.
    pub relays: Vec<String>,
    /// Whether discovered mints should be registered.
    pub policy: MintRestorePolicy,
    /// Timeout for waiting for relay responses.
    pub timeout: Duration,
}

impl MintRestoreRequest {
    /// Create a preview request with a ten-second timeout.
    pub fn new(relays: Vec<String>) -> Self {
        Self {
            relays,
            policy: MintRestorePolicy::Preview,
            timeout: Duration::from_secs(10),
        }
    }

    /// Select whether the restored mints are registered.
    pub fn with_policy(mut self, policy: MintRestorePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Change the relay response timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Receipt for a published mint backup.
#[derive(Debug, Clone)]
pub struct MintBackupReceipt {
    /// The event ID of the published backup
    pub event_id: EventId,
    /// The public key used for the backup
    pub public_key: PublicKey,
    /// Number of mints backed up
    pub mint_count: usize,
}

/// Receipt for a fetched and decrypted mint backup.
#[derive(Debug, Clone)]
pub struct MintRestoreReceipt {
    /// The restored mint backup data
    pub backup: MintBackup,
    /// Number of mints found in the backup
    pub mint_count: usize,
    /// Number of mints that were newly added (not already in wallet)
    pub mints_added: usize,
}

impl super::advanced::AdvancedWalletManager<'_> {
    /// Derive the Nostr keys used for mint backup from the wallet seed
    ///
    /// These keys can be used to identify and decrypt backup events.
    pub fn backup_keys(&self) -> Result<Keys, Error> {
        nut27::derive_nostr_keys(self.core_manager().seed())
            .map_err(|e| Error::Custom(e.to_string()))
    }

    /// Publish the current mint list as an encrypted NIP-78 event.
    #[instrument(skip(self, request))]
    pub async fn backup_mints(
        &self,
        request: MintBackupRequest,
    ) -> Result<MintBackupReceipt, Error> {
        let keys = self.backup_keys()?;

        let wallets = self.core_manager().get_wallets().await;
        let mint_urls: Vec<MintUrl> = wallets
            .iter()
            .map(|w| w.mint_url.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let backup = MintBackup::new(mint_urls.clone());

        let event = create_backup_event(&keys, &backup, request.client.as_deref())
            .map_err(|e| Error::Custom(format!("Failed to create backup event: {e}")))?;

        let event_id = event.id;

        let client = NostrClient::new(keys.clone());

        for relay in &request.relays {
            client
                .add_write_relay(relay)
                .await
                .map_err(|e| Error::Custom(format!("Failed to add relay: {e}")))?;
        }

        client.connect().await;

        client
            .send_event(&event)
            .await
            .map_err(|e| Error::Custom(format!("Failed to publish backup event: {e}")))?;

        client.disconnect().await;

        Ok(MintBackupReceipt {
            event_id,
            public_key: keys.public_key(),
            mint_count: mint_urls.len(),
        })
    }

    /// Fetch and decrypt the newest mint backup, optionally registering its mints.
    #[instrument(skip(self, request))]
    pub async fn restore_mints(
        &self,
        request: MintRestoreRequest,
    ) -> Result<MintRestoreReceipt, Error> {
        let keys = self.backup_keys()?;

        let (kind, pubkey, d_tag) = backup_filter_params(&keys);

        let filter = Filter::new()
            .kind(kind)
            .author(pubkey)
            .identifier(d_tag)
            .limit(1);

        let client = NostrClient::new(keys.clone());

        for relay in &request.relays {
            client
                .add_read_relay(relay)
                .await
                .map_err(|e| Error::Custom(format!("Failed to add relay: {e}")))?;
        }

        client.connect().await;

        let events = client
            .fetch_events(filter, request.timeout)
            .await
            .map_err(|e| Error::Custom(format!("Failed to fetch backup events: {e}")))?;

        client.disconnect().await;

        // Addressable events ensure only one event per pubkey+d-tag combination
        let event = events
            .into_iter()
            .next()
            .ok_or_else(|| Error::Custom("No backup event found".to_string()))?;

        let backup = decrypt_backup_event(&keys, &event)
            .map_err(|e| Error::Custom(format!("Failed to decrypt backup event: {e}")))?;

        let mint_count = backup.mints.len();
        let mut mints_added = 0;

        if request.policy == MintRestorePolicy::Register {
            for mint_url in &backup.mints {
                if !self.core_manager().has_mint(mint_url).await {
                    // Ignore errors for individual mints to continue restoring others
                    // add_wallet fetches mint info and creates wallets for all supported units
                    if self
                        .core_manager()
                        .add_wallet(mint_url.clone())
                        .await
                        .is_ok()
                    {
                        mints_added += 1;
                    }
                }
            }
        }

        Ok(MintRestoreReceipt {
            backup,
            mint_count,
            mints_added,
        })
    }
}
