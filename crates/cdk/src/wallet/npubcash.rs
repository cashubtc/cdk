//! NpubCash integration for CDK Wallet
//!
//! This module provides integration between the CDK wallet and the NpubCash service,
//! allowing wallets to sync quotes, subscribe to updates, and manage NpubCash settings.

use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::Network;
use cdk_common::SECP256K1;
use cdk_npubcash::{JwtAuthProvider, NpubCashClient, Quote};
use tracing::instrument;

use crate::error::Error;
use crate::nuts::SecretKey;
use crate::wallet::types::{MintQuote, TransactionDirection};
use crate::wallet::Wallet;

/// KV store namespace for npubcash-related data
pub const NPUBCASH_KV_NAMESPACE: &str = "npubcash";
/// KV store secondary namespace marking quotes that came from NpubCash
const QUOTES_KV_SECONDARY_NAMESPACE: &str = "quotes";
/// Quote marker for the current NIP-06 NpubCash signing key
const QUOTE_KEY_NIP06: &[u8] = b"nip06";
/// Quote marker for quotes imported from the legacy seed-prefix NpubCash identity
const QUOTE_KEY_LEGACY_SEED_PREFIX: &[u8] = b"legacy-seed-prefix";
/// KV store key for the last fetch timestamp (stored as u64 Unix timestamp)
const LAST_FETCH_TIMESTAMP_KEY: &str = "last_fetch_timestamp";
/// KV store key for whether legacy seed-prefix quotes have been imported
const LEGACY_QUOTES_IMPORTED_KEY: &str = "legacy_quotes_imported";
/// KV store key for the active mint URL
pub const ACTIVE_MINT_KEY: &str = "active_mint";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NpubCashQuoteKey {
    Nip06,
    LegacySeedPrefix,
}

impl NpubCashQuoteKey {
    fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Nip06 => QUOTE_KEY_NIP06,
            Self::LegacySeedPrefix => QUOTE_KEY_LEGACY_SEED_PREFIX,
        }
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        match bytes {
            QUOTE_KEY_LEGACY_SEED_PREFIX => Self::LegacySeedPrefix,
            // Empty values were written by the first non-persisting-key
            // implementation and used the current NIP-06 key.
            _ => Self::Nip06,
        }
    }
}

/// Derive the current NpubCash secret key from a wallet seed
///
/// Uses NIP-06 BIP-32 derivation (`m/44'/1237'/0'/0/0`) so the key never
/// equals raw seed material and cannot be used to recover the seed.
///
/// # Errors
///
/// Returns an error if the key derivation fails
pub fn derive_npubcash_secret_key_from_seed(seed: &[u8; 64]) -> Result<SecretKey, Error> {
    let path = DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(44)?,
        ChildNumber::from_hardened_idx(1237)?,
        ChildNumber::from_hardened_idx(0)?,
        ChildNumber::from_normal_idx(0)?,
        ChildNumber::from_normal_idx(0)?,
    ]);

    let xpriv = Xpriv::new_master(Network::Bitcoin, seed)?;

    Ok(SecretKey::from(
        xpriv.derive_priv(&SECP256K1, &path)?.private_key,
    ))
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

        if let Err(e) = self.import_legacy_npubcash_quotes_once(&npubcash_url).await {
            tracing::warn!("Failed to import legacy NpubCash quotes: {}", e);
        }

        Ok(())
    }

    /// Derive the NpubCash secret key from the wallet seed
    ///
    /// Uses NIP-06 BIP-32 derivation (`m/44'/1237'/0'/0/0`) so the key never
    /// equals raw seed material and cannot be used to recover the seed.
    ///
    /// # Errors
    ///
    /// Returns an error if the key derivation fails
    pub(crate) fn derive_npubcash_secret_key(&self) -> Result<SecretKey, Error> {
        derive_npubcash_secret_key_from_seed(&self.seed)
    }

    fn derive_legacy_npubcash_secret_key(&self) -> Result<SecretKey, Error> {
        Ok(SecretKey::from_slice(&self.seed[..32])?)
    }

    /// Derive Nostr keys from wallet seed for NpubCash authentication
    ///
    /// Uses NIP-06 derivation (`m/44'/1237'/0'/0/0`) from the wallet seed.
    ///
    /// # Errors
    ///
    /// Returns an error if the key derivation fails
    fn derive_npubcash_keys(&self) -> Result<nostr_sdk::Keys, Error> {
        let secret_key = self.derive_npubcash_secret_key()?;

        let nostr_secret = nostr_sdk::SecretKey::from_slice(&secret_key.to_secret_bytes())
            .map_err(|e| Error::Custom(format!("Failed to derive Nostr keys: {}", e)))?;

        Ok(nostr_sdk::Keys::new(nostr_secret))
    }

    fn derive_legacy_npubcash_keys(&self) -> Result<nostr_sdk::Keys, Error> {
        let secret_key = self.derive_legacy_npubcash_secret_key()?;
        let nostr_secret = nostr_sdk::SecretKey::from_slice(&secret_key.to_secret_bytes())
            .map_err(|e| Error::Custom(format!("Failed to derive legacy Nostr keys: {}", e)))?;

        Ok(nostr_sdk::Keys::new(nostr_secret))
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
        self.process_npubcash_quotes_with_key(quotes, NpubCashQuoteKey::Nip06)
            .await
    }

    async fn process_npubcash_quotes_with_key(
        &self,
        quotes: Vec<Quote>,
        key: NpubCashQuoteKey,
    ) -> Result<Vec<MintQuote>, Error> {
        let mut mint_quotes = Vec::with_capacity(quotes.len());
        for quote in quotes {
            if let Some(mint_quote) = self.add_npubcash_mint_quote_with_key(quote, key).await? {
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
        self: Arc<Self>,
        split_target: cdk_common::amount::SplitTarget,
        spending_conditions: Option<crate::nuts::SpendingConditions>,
        poll_interval: std::time::Duration,
    ) -> crate::wallet::streams::npubcash::WalletNpubCashProofStream {
        crate::wallet::streams::npubcash::WalletNpubCashProofStream::new(
            self,
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
    /// Converts an NpubCash quote to a wallet MintQuote and stores it. The
    /// NUT-20 signing key is not persisted; the quote is marked in the KV
    /// store so the NpubCash key can be re-derived from the seed at claim
    /// time.
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
        self.add_npubcash_mint_quote_with_key(npubcash_quote, NpubCashQuoteKey::Nip06)
            .await
    }

    async fn add_npubcash_mint_quote_with_key(
        &self,
        npubcash_quote: cdk_npubcash::Quote,
        key: NpubCashQuoteKey,
    ) -> Result<Option<MintQuote>, Error> {
        let mint_quote: MintQuote = npubcash_quote.into();

        let exists = self
            .list_transactions(Some(TransactionDirection::Incoming))
            .await?
            .iter()
            .any(|tx| tx.quote_id.as_ref() == Some(&mint_quote.id));

        if exists {
            return Ok(None);
        }

        self.localstore
            .kv_write(
                NPUBCASH_KV_NAMESPACE,
                QUOTES_KV_SECONDARY_NAMESPACE,
                &mint_quote.id,
                key.as_bytes(),
            )
            .await?;

        self.localstore.add_mint_quote(mint_quote.clone()).await?;

        tracing::info!("Added NpubCash quote {} to wallet database", mint_quote.id);
        Ok(Some(mint_quote))
    }

    pub(crate) async fn npubcash_quote_key(
        &self,
        quote_id: &str,
    ) -> Result<Option<NpubCashQuoteKey>, Error> {
        Ok(self
            .localstore
            .kv_read(
                NPUBCASH_KV_NAMESPACE,
                QUOTES_KV_SECONDARY_NAMESPACE,
                quote_id,
            )
            .await?
            .map(|value| NpubCashQuoteKey::from_bytes(&value)))
    }

    pub(crate) fn is_legacy_npubcash_secret_key(&self, secret_key: &SecretKey) -> bool {
        secret_key.as_secret_bytes() == &self.seed[..32]
    }

    pub(crate) fn npubcash_quote_secret_key(
        &self,
        key: NpubCashQuoteKey,
    ) -> Result<SecretKey, Error> {
        match key {
            NpubCashQuoteKey::Nip06 => self.derive_npubcash_secret_key(),
            NpubCashQuoteKey::LegacySeedPrefix => self.derive_legacy_npubcash_secret_key(),
        }
    }

    pub(crate) async fn scrub_legacy_npubcash_quote(&self, quote: &MintQuote) -> Result<(), Error> {
        let mut scrubbed = quote.clone();
        scrubbed.secret_key = None;

        self.localstore
            .kv_write(
                NPUBCASH_KV_NAMESPACE,
                QUOTES_KV_SECONDARY_NAMESPACE,
                &scrubbed.id,
                NpubCashQuoteKey::LegacySeedPrefix.as_bytes(),
            )
            .await?;

        self.localstore.add_mint_quote(scrubbed).await?;

        Ok(())
    }

    async fn import_legacy_npubcash_quotes_once(&self, npubcash_url: &str) -> Result<(), Error> {
        if self
            .localstore
            .kv_read(NPUBCASH_KV_NAMESPACE, "", LEGACY_QUOTES_IMPORTED_KEY)
            .await?
            .is_some()
        {
            return Ok(());
        }

        let keys = self.derive_legacy_npubcash_keys()?;
        let auth_provider = Arc::new(JwtAuthProvider::new(npubcash_url.to_string(), keys));
        let client = NpubCashClient::new(npubcash_url.to_string(), auth_provider);
        let quotes = client
            .get_quotes(None)
            .await
            .map_err(|e| Error::Custom(format!("Failed to sync legacy NpubCash quotes: {}", e)))?;

        self.process_npubcash_quotes_with_key(quotes, NpubCashQuoteKey::LegacySeedPrefix)
            .await?;

        self.localstore
            .kv_write(NPUBCASH_KV_NAMESPACE, "", LEGACY_QUOTES_IMPORTED_KEY, &[1])
            .await?;

        Ok(())
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::database::{self, WalletDatabase};

    use super::*;
    use crate::mint_url::MintUrl;
    use crate::nuts::CurrencyUnit;
    use crate::wallet::WalletBuilder;

    async fn build_test_wallet(seed: [u8; 64]) -> Wallet {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("memory db"),
        );

        WalletBuilder::new()
            .mint_url(MintUrl::from_str("https://mint.example.com").expect("valid mint url"))
            .unit(CurrencyUnit::Sat)
            .localstore(localstore)
            .seed(seed)
            .build()
            .expect("wallet builds")
    }

    fn test_quote() -> Quote {
        Quote {
            id: "npubcash-quote-1".to_string(),
            amount: 1000,
            unit: "sat".to_string(),
            created_at: 0,
            paid_at: None,
            expires_at: None,
            mint_url: Some("https://mint.example.com".to_string()),
            request: Some("lnbc100n1pjz".to_string()),
            state: Some("PAID".to_string()),
            locked: None,
        }
    }

    #[tokio::test]
    async fn npubcash_key_is_nip06_derived_not_raw_seed() {
        let seed = [0x42u8; 64];
        let wallet = build_test_wallet(seed).await;

        let secret_key = wallet.derive_npubcash_secret_key().expect("key derives");

        assert_ne!(
            &secret_key.to_secret_bytes()[..],
            &seed[..32],
            "npubcash key must not equal raw wallet seed bytes"
        );

        let xpriv = Xpriv::new_master(Network::Bitcoin, &seed).expect("master key");
        let path = DerivationPath::from_str("m/44'/1237'/0'/0/0").expect("valid path");
        let expected = xpriv
            .derive_priv(&SECP256K1, &path)
            .expect("derivation")
            .private_key;

        assert_eq!(secret_key.to_secret_bytes(), expected.secret_bytes());
    }

    #[tokio::test]
    async fn add_npubcash_mint_quote_does_not_persist_secret_key() {
        let seed = [0x42u8; 64];
        let wallet = build_test_wallet(seed).await;

        let stored = wallet
            .add_npubcash_mint_quote(test_quote())
            .await
            .expect("add_npubcash_mint_quote succeeds")
            .expect("quote was inserted");

        assert!(
            stored.secret_key.is_none(),
            "npubcash quotes must not carry a persisted secret key"
        );

        let persisted = wallet
            .localstore
            .get_mint_quote(&stored.id)
            .await
            .expect("quote lookup")
            .expect("quote in store");
        assert!(
            persisted.secret_key.is_none(),
            "no secret key may be written to the localstore"
        );

        assert_eq!(
            wallet
                .npubcash_quote_key(&stored.id)
                .await
                .expect("kv lookup"),
            Some(NpubCashQuoteKey::Nip06)
        );

        let signing_key = wallet
            .mint_quote_signing_key(&persisted)
            .await
            .expect("signing key lookup")
            .expect("npubcash quote signing key is re-derivable");

        assert_eq!(
            signing_key.to_secret_bytes(),
            wallet
                .derive_npubcash_secret_key()
                .expect("key derives")
                .to_secret_bytes()
        );
        assert_ne!(&signing_key.to_secret_bytes()[..], &seed[..32]);
    }

    #[tokio::test]
    async fn legacy_npubcash_quote_uses_legacy_key_without_persisting_it() {
        let seed = [0x42u8; 64];
        let wallet = build_test_wallet(seed).await;

        let stored = wallet
            .add_npubcash_mint_quote_with_key(test_quote(), NpubCashQuoteKey::LegacySeedPrefix)
            .await
            .expect("legacy quote is added")
            .expect("quote was inserted");

        assert!(stored.secret_key.is_none());
        assert_eq!(
            wallet
                .npubcash_quote_key(&stored.id)
                .await
                .expect("kv lookup"),
            Some(NpubCashQuoteKey::LegacySeedPrefix)
        );

        let signing_key = wallet
            .mint_quote_signing_key(&stored)
            .await
            .expect("signing key lookup")
            .expect("legacy npubcash signing key is re-derivable");

        assert_eq!(&signing_key.to_secret_bytes()[..], &seed[..32]);
    }

    #[tokio::test]
    async fn legacy_persisted_npubcash_key_is_scrubbed() {
        let seed = [0x42u8; 64];
        let wallet = build_test_wallet(seed).await;
        let mut legacy_quote: MintQuote = test_quote().into();
        legacy_quote.secret_key = Some(
            wallet
                .derive_legacy_npubcash_secret_key()
                .expect("legacy key derives"),
        );

        wallet
            .localstore
            .add_mint_quote(legacy_quote.clone())
            .await
            .expect("legacy quote is stored");

        let signing_key = wallet
            .mint_quote_signing_key(&legacy_quote)
            .await
            .expect("signing key lookup")
            .expect("legacy signing key is returned");

        assert_eq!(&signing_key.to_secret_bytes()[..], &seed[..32]);

        let scrubbed = wallet
            .localstore
            .get_mint_quote(&legacy_quote.id)
            .await
            .expect("quote lookup")
            .expect("quote remains stored");
        assert!(
            scrubbed.secret_key.is_none(),
            "legacy raw seed key should be removed from storage"
        );
        assert_eq!(
            wallet
                .npubcash_quote_key(&legacy_quote.id)
                .await
                .expect("kv lookup"),
            Some(NpubCashQuoteKey::LegacySeedPrefix)
        );
    }
}
