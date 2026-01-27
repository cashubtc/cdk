//! Wallet Repository
//!
//! Simple container that manages [`Wallet`] instances by mint URL.

use std::collections::BTreeMap;
use std::sync::Arc;


use cdk_common::database::WalletDatabase;
use cdk_common::{database, KeySetInfo};
use tokio::sync::RwLock;
use tracing::instrument;
use zeroize::Zeroize;

use super::builder::WalletBuilder;
use super::Error;
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
use crate::wallet::mint_connector::transport::tor_transport::TorAsync;
use crate::Wallet;

/// Transfer mode
#[derive(Debug, Clone)]
pub enum TransferMode {
    /// Transfer entire balance
    FullBalance,
    /// Transfer specific amount (amount to be received at target)
    ExactReceive(cdk_common::Amount),
}

/// Result of a transfer operation
#[derive(Debug, Clone)]
pub struct TransferResult {
    /// Amount sent from source (including fees)
    pub amount_sent: cdk_common::Amount,
    /// Amount received at target
    pub amount_received: cdk_common::Amount,
    /// Fees paid
    pub fees_paid: cdk_common::Amount,
    /// Source balance after transfer
    pub source_balance_after: cdk_common::Amount,
    /// Target balance after transfer
    pub target_balance_after: cdk_common::Amount,
}

/// Configuration for individual wallets within WalletRepository
#[derive(Clone, Default, Debug)]
pub struct WalletConfig {
    /// Custom mint connector implementation
    pub mint_connector: Option<Arc<dyn super::MintConnector + Send + Sync>>,
    /// Custom auth connector implementation
    #[cfg(feature = "auth")]
    pub auth_connector: Option<Arc<dyn super::auth::AuthMintConnector + Send + Sync>>,
    /// Target number of proofs to maintain at each denomination
    pub target_proof_count: Option<usize>,
    /// Metadata cache TTL
    ///
    /// The TTL determines how often the wallet checks the mint for new keysets and information.
    ///
    /// If `None`, the cache will never expire and the wallet will use cached data indefinitely
    /// (unless manually refreshed).
    ///
    /// The default value is 1 hour (3600 seconds).
    pub metadata_cache_ttl: Option<std::time::Duration>,
}

impl WalletConfig {
    /// Create a new empty WalletConfig
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom mint connector
    pub fn with_mint_connector(
        mut self,
        connector: Arc<dyn super::MintConnector + Send + Sync>,
    ) -> Self {
        self.mint_connector = Some(connector);
        self
    }

    /// Set custom auth connector
    #[cfg(feature = "auth")]
    pub fn with_auth_connector(
        mut self,
        connector: Arc<dyn super::auth::AuthMintConnector + Send + Sync>,
    ) -> Self {
        self.auth_connector = Some(connector);
        self
    }

    /// Set target proof count
    pub fn with_target_proof_count(mut self, count: usize) -> Self {
        self.target_proof_count = Some(count);
        self
    }

    /// Set metadata cache TTL
    ///
    /// The TTL determines how often the wallet checks the mint for new keysets and information.
    ///
    /// If `None`, the cache will never expire and the wallet will use cached data indefinitely
    /// (unless manually refreshed).
    ///
    /// The default value is 1 hour (3600 seconds).
    pub fn with_metadata_cache_ttl(mut self, ttl: Option<std::time::Duration>) -> Self {
        self.metadata_cache_ttl = ttl;
        self
    }
}

/// Repository for managing Wallet instances by mint URL
///
/// Simple container that bootstraps wallets from database and provides
/// access to individual Wallet instances.
#[derive(Clone)]
pub struct WalletRepository {
    /// Storage backend
    localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// Wallets indexed by mint URL
    wallets: Arc<RwLock<BTreeMap<MintUrl, Wallet>>>,
    /// Proxy configuration for HTTP clients (optional)
    proxy_config: Option<url::Url>,
    /// Shared Tor transport to be cloned into each TorHttpClient (if enabled)
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    shared_tor_transport: Option<TorAsync>,
}

impl std::fmt::Debug for WalletRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletRepository").finish_non_exhaustive()
    }
}

impl WalletRepository {
    /// Create new repository and load existing wallets from database
    pub async fn new(
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        seed: [u8; 64],
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: None,
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            shared_tor_transport: None,
        };

        // Automatically load wallets from database
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Get the wallet seed
    pub fn seed(&self) -> &[u8; 64] {
        &self.seed
    }

    /// Create with proxy configuration
    pub async fn new_with_proxy(
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        seed: [u8; 64],
        proxy_url: url::Url,
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: Some(proxy_url),
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            shared_tor_transport: None,
        };

        // Automatically load wallets from database
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Create with Tor transport (feature-gated)
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    pub async fn new_with_tor(
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
        seed: [u8; 64],
    ) -> Result<Self, Error> {
        let wallet = Self {
            localstore,
            seed,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: None,
            shared_tor_transport: Some(TorAsync::new()),
        };

        // Automatically load wallets from database
        wallet.load_wallets().await?;

        Ok(wallet)
    }

    /// Get wallet for a mint URL (returns None if not found)
    #[instrument(skip(self))]
    pub async fn get_wallet(&self, mint_url: &MintUrl) -> Option<Wallet> {
        self.wallets.read().await.get(mint_url).cloned()
    }

    /// Add a mint to the repository with default unit (Sat)
    #[instrument(skip(self))]
    pub async fn add_mint(&self, mint_url: MintUrl) -> Result<Wallet, Error> {
        self.create_wallet(mint_url, CurrencyUnit::Sat, None).await
    }

    /// Add a mint to the repository with a custom configuration and default unit (Sat)
    #[instrument(skip(self))]
    pub async fn add_mint_with_config(
        &self,
        mint_url: MintUrl,
        config: WalletConfig,
    ) -> Result<Wallet, Error> {
        self.create_wallet(mint_url, CurrencyUnit::Sat, Some(config))
            .await
    }

    /// Update configuration for an existing mint
    ///
    /// This re-creates the wallet with the new configuration.
    #[instrument(skip(self))]
    pub async fn set_mint_config(
        &self,
        mint_url: MintUrl,
        config: WalletConfig,
    ) -> Result<Wallet, Error> {
        // Get existing unit from wallet if it exists, otherwise default to Sat
        let unit = if let Some(wallet) = self.get_wallet(&mint_url).await {
            wallet.unit.clone()
        } else {
            CurrencyUnit::Sat
        };

        // Re-create wallet with new config
        self.create_wallet(mint_url, unit, Some(config)).await
    }

    /// Create and add a new wallet for a mint URL
    /// Returns the created wallet
    #[instrument(skip(self))]
    pub async fn create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: Option<WalletConfig>,
    ) -> Result<Wallet, Error> {
        let wallet = self
            .create_wallet_internal(mint_url.clone(), unit, config.as_ref())
            .await?;

        // Insert into wallets map
        let mut wallets = self.wallets.write().await;
        wallets.insert(mint_url, wallet.clone());

        Ok(wallet)
    }

    /// Remove a wallet from the repository
    #[instrument(skip(self))]
    pub async fn remove_wallet(&self, mint_url: &MintUrl) {
        let mut wallets = self.wallets.write().await;
        wallets.remove(mint_url);
    }

    /// Get all wallets
    #[instrument(skip(self))]
    pub async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.read().await.values().cloned().collect()
    }

    /// Check if wallet exists for mint
    #[instrument(skip(self))]
    pub async fn has_mint(&self, mint_url: &MintUrl) -> bool {
        self.wallets.read().await.contains_key(mint_url)
    }

    /// Get keysets for a mint url
    pub async fn get_mint_keysets(&self, mint_url: &MintUrl) -> Result<Vec<KeySetInfo>, Error> {
        let wallets = self.wallets.read().await;
        let target_wallet = wallets.get(mint_url).ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;

        target_wallet.load_mint_keysets().await
    }

    /// Get balances for all wallets
    ///
    /// Returns a map of mint URL to balance for each wallet in the repository.
    #[instrument(skip(self))]
    pub async fn get_balances(&self) -> Result<BTreeMap<MintUrl, cdk_common::Amount>, Error> {
        let wallets = self.wallets.read().await;
        let mut balances = BTreeMap::new();

        for (mint_url, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            balances.insert(mint_url.clone(), balance);
        }

        Ok(balances)
    }

    /// Transfer funds from one mint to another via Lightning swap
    #[instrument(skip(self))]
    pub async fn transfer(
        &self,
        source_mint: &MintUrl,
        target_mint: &MintUrl,
        mode: TransferMode,
    ) -> Result<TransferResult, Error> {
        if source_mint == target_mint {
            return Err(Error::Custom(
                "Source and target mints must be different".into(),
            ));
        }

        let source_wallet = self.get_wallet(source_mint).await.ok_or(Error::UnknownMint {
            mint_url: source_mint.to_string(),
        })?;
        let target_wallet = self.get_wallet(target_mint).await.ok_or(Error::UnknownMint {
            mint_url: target_mint.to_string(),
        })?;

        let mut amount_to_receive = match mode {
            TransferMode::ExactReceive(amt) => amt,
            TransferMode::FullBalance => source_wallet.total_balance().await?,
        };

        // 1. Get Mint Quote from Target
        let mint_quote = target_wallet.mint_quote(amount_to_receive, None).await?;

        // 2. Get Melt Quote from Source
        let melt_quote = source_wallet
            .melt_quote(mint_quote.request.clone(), None)
            .await?;

        let fee = melt_quote.fee_reserve;
        let total_cost = amount_to_receive + fee;

        if let TransferMode::FullBalance = mode {
            let balance = source_wallet.total_balance().await?;
            if total_cost > balance {
                let deficit = total_cost - balance;
                amount_to_receive = amount_to_receive
                    .checked_sub(deficit)
                    .ok_or(Error::InsufficientFunds)?;

                // Retry quote with new amount
                let mint_quote_2 = target_wallet.mint_quote(amount_to_receive, None).await?;
                let melt_quote_2 = source_wallet
                    .melt_quote(mint_quote_2.request.clone(), None)
                    .await?;

                let total_cost_2 = amount_to_receive + melt_quote_2.fee_reserve;
                if total_cost_2 > balance {
                    return Err(Error::InsufficientFunds);
                }

                // Use second attempt
                let melted = source_wallet.melt(&melt_quote_2.id).await?;
                let _minted = target_wallet
                    .mint(
                        &mint_quote_2.id,
                        crate::amount::SplitTarget::default(),
                        None,
                    )
                    .await?;

                return Ok(TransferResult {
                    amount_sent: melted.amount + melted.fee_paid,
                    amount_received: amount_to_receive,
                    fees_paid: melted.fee_paid,
                    source_balance_after: source_wallet.total_balance().await?,
                    target_balance_after: target_wallet.total_balance().await?,
                });
            }
        }

        let melted = source_wallet.melt(&melt_quote.id).await?;
        let _minted = target_wallet
            .mint(&mint_quote.id, crate::amount::SplitTarget::default(), None)
            .await?;

        Ok(TransferResult {
            amount_sent: melted.amount + melted.fee_paid,
            amount_received: amount_to_receive,
            fees_paid: melted.fee_paid,
            source_balance_after: source_wallet.total_balance().await?,
            target_balance_after: target_wallet.total_balance().await?,
        })
    }

    /// Fetch mint info for the given mint URL
    #[instrument(skip(self))]
    pub async fn fetch_mint_info(&self, mint_url: &MintUrl) -> Result<Option<crate::nuts::MintInfo>, Error> {
         let wallet = self.get_wallet(mint_url).await.ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;
        wallet.fetch_mint_info().await
    }

    /// Mint blind auth tokens
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn mint_blind_auth(&self, mint_url: &MintUrl, amount: cdk_common::Amount) -> Result<crate::nuts::Proofs, Error> {
        let wallet = self.get_wallet(mint_url).await.ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;
        wallet.mint_blind_auth(amount).await
    }

    /// Set CAT
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn set_cat(&self, mint_url: &MintUrl, cat: String) -> Result<(), Error> {
        let wallet = self.get_wallet(mint_url).await.ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;
        wallet.set_cat(cat).await
    }

    /// Set refresh token
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn set_refresh_token(&self, mint_url: &MintUrl, refresh_token: String) -> Result<(), Error> {
        let wallet = self.get_wallet(mint_url).await.ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;
        wallet.set_refresh_token(refresh_token).await
    }

    /// Refresh access token
    #[cfg(feature = "auth")]
    #[instrument(skip(self))]
    pub async fn refresh_access_token(&self, mint_url: &MintUrl) -> Result<(), Error> {
        let wallet = self.get_wallet(mint_url).await.ok_or(Error::UnknownMint {
            mint_url: mint_url.to_string(),
        })?;
        wallet.refresh_access_token().await
    }

    /// Get total balance across all wallets
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<cdk_common::Amount, Error> {
        let balances = self.get_balances().await?;
        Ok(balances.values().fold(cdk_common::Amount::ZERO, |acc, &x| acc + x))
    }

    /// Get or create a wallet for a mint URL
    ///
    /// If a wallet for the mint URL already exists, returns it.
    /// Otherwise creates a new wallet with the specified unit and adds it to the repository.
    #[instrument(skip(self))]
    pub async fn get_or_create_wallet(
        &self,
        mint_url: &MintUrl,
        unit: CurrencyUnit,
    ) -> Result<Wallet, Error> {
        if let Some(wallet) = self.get_wallet(mint_url).await {
            return Ok(wallet);
        }

        self.create_wallet(mint_url.clone(), unit, None).await
    }

    /// Internal: Create wallet with optional custom configuration
    ///
    /// Priority order for configuration:
    /// 1. Custom connector from config (if provided)
    /// 2. Global settings (proxy/Tor)
    /// 3. Default HttpClient
    async fn create_wallet_internal(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: Option<&WalletConfig>,
    ) -> Result<Wallet, Error> {
        // Check if custom connector is provided in config
        if let Some(cfg) = config {
            if let Some(custom_connector) = &cfg.mint_connector {
                // Use custom connector with WalletBuilder
                let mut builder = WalletBuilder::new()
                    .mint_url(mint_url.clone())
                    .unit(unit.clone())
                    .localstore(self.localstore.clone())
                    .seed(self.seed)
                    .target_proof_count(cfg.target_proof_count.unwrap_or(3))
                    .shared_client(custom_connector.clone());

                if let Some(ttl) = cfg.metadata_cache_ttl {
                    builder = builder.set_metadata_cache_ttl(Some(ttl));
                }

                return builder.build();
            }
        }

        // Fall back to existing logic: proxy/Tor/default
        let target_proof_count = config.and_then(|c| c.target_proof_count).unwrap_or(3);
        let metadata_cache_ttl = config.and_then(|c| c.metadata_cache_ttl);

        let wallet = if let Some(proxy_url) = &self.proxy_config {
            // Create wallet with proxy-configured client
            let client = crate::wallet::HttpClient::with_proxy(
                mint_url.clone(),
                proxy_url.clone(),
                None,
                true,
            )
            .unwrap_or_else(|_| {
                #[cfg(feature = "auth")]
                {
                    crate::wallet::HttpClient::new(mint_url.clone(), None)
                }
                #[cfg(not(feature = "auth"))]
                {
                    crate::wallet::HttpClient::new(mint_url.clone())
                }
            });
            let mut builder = WalletBuilder::new()
                .mint_url(mint_url.clone())
                .unit(unit.clone())
                .localstore(self.localstore.clone())
                .seed(self.seed)
                .target_proof_count(target_proof_count)
                .client(client);

            if let Some(ttl) = metadata_cache_ttl {
                builder = builder.set_metadata_cache_ttl(Some(ttl));
            }

            builder.build()?
        } else {
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            if let Some(tor) = &self.shared_tor_transport {
                // Create wallet with Tor transport client, cloning the shared transport
                let client = {
                    let transport = tor.clone();
                    #[cfg(feature = "auth")]
                    {
                        crate::wallet::TorHttpClient::with_transport(
                            mint_url.clone(),
                            transport,
                            None,
                        )
                    }
                    #[cfg(not(feature = "auth"))]
                    {
                        crate::wallet::TorHttpClient::with_transport(mint_url.clone(), transport)
                    }
                };

                let mut builder = WalletBuilder::new()
                    .mint_url(mint_url.clone())
                    .unit(unit.clone())
                    .localstore(self.localstore.clone())
                    .seed(self.seed)
                    .target_proof_count(target_proof_count)
                    .client(client);

                if let Some(ttl) = metadata_cache_ttl {
                    builder = builder.set_metadata_cache_ttl(Some(ttl));
                }

                builder.build()?
            } else {
                // Create wallet with default client
                let wallet = Wallet::new(
                    &mint_url.to_string(),
                    unit.clone(),
                    self.localstore.clone(),
                    self.seed,
                    Some(target_proof_count),
                )?;
                if let Some(ttl) = metadata_cache_ttl {
                    wallet.set_metadata_cache_ttl(Some(ttl));
                }
                wallet
            }

            #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
            {
                // Create wallet with default client
                let wallet = Wallet::new(
                    &mint_url.to_string(),
                    unit.clone(),
                    self.localstore.clone(),
                    self.seed,
                    Some(target_proof_count),
                )?;
                if let Some(ttl) = metadata_cache_ttl {
                    wallet.set_metadata_cache_ttl(Some(ttl));
                }
                wallet
            }
        };

        Ok(wallet)
    }

    /// Load all wallets from database
    ///
    /// This loads wallets for all mints stored in the database.
    /// Since wallets now have per-wallet units, this creates wallets with
    /// a default unit (Sat). Callers should use `create_wallet` with the
    /// appropriate unit for new wallets.
    #[instrument(skip(self))]
    async fn load_wallets(&self) -> Result<(), Error> {
        let mints = self.localstore.get_mints().await.map_err(Error::Database)?;

        for (mint_url, _mint_info) in mints {
            // Add mint to the repository if not already present
            // Use default unit (Sat) for backward compatibility
            if !self.has_mint(&mint_url).await {
                let wallet = self
                    .create_wallet_internal(mint_url.clone(), CurrencyUnit::Sat, None)
                    .await?;

                let mut wallets = self.wallets.write().await;
                wallets.insert(mint_url, wallet);
            }
        }

        Ok(())
    }

    #[cfg(feature = "npubcash")]
    pub async fn get_active_npubcash_mint(&self) -> Result<Option<MintUrl>, Error> {
        use super::npubcash::{ACTIVE_MINT_KEY, NPUBCASH_KV_NAMESPACE};
        let value = self
            .localstore
            .kv_read(NPUBCASH_KV_NAMESPACE, "", ACTIVE_MINT_KEY)
            .await?;
        match value {
            Some(bytes) => {
                let s = String::from_utf8(bytes)
                    .map_err(|_| Error::Custom("Invalid active mint URL".into()))?;
                Ok(Some(MintUrl::from_str(&s)?))
            }
            None => Ok(None),
        }
    }

    #[cfg(feature = "npubcash")]
    pub async fn set_active_npubcash_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
        use super::npubcash::{ACTIVE_MINT_KEY, NPUBCASH_KV_NAMESPACE};
        self.localstore
            .kv_write(
                NPUBCASH_KV_NAMESPACE,
                "",
                ACTIVE_MINT_KEY,
                mint_url.to_string().as_bytes(),
            )
            .await?;
        Ok(())
    }

    #[cfg(feature = "npubcash")]
    pub async fn sync_npubcash_quotes(&self) -> Result<Vec<crate::wallet::types::MintQuote>, Error> {
        let active_mint = self.get_active_npubcash_mint().await?;
        if let Some(mint_url) = active_mint {
            let wallet = self.get_wallet(&mint_url).await.ok_or(Error::UnknownMint {
                mint_url: mint_url.to_string(),
            })?;
            wallet.sync_npubcash_quotes().await
        } else {
            Err(Error::Custom("No active NpubCash mint set".into()))
        }
    }
}

impl Drop for WalletRepository {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}



#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cdk_common::database::WalletDatabase;

    use super::*;

    async fn create_test_repository() -> WalletRepository {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        WalletRepository::new(localstore, seed)
            .await
            .expect("Failed to create WalletRepository")
    }

    #[tokio::test]
    async fn test_wallet_repository_creation() {
        let repo = create_test_repository().await;
        assert!(repo.wallets.try_read().is_ok());
    }

    #[tokio::test]
    async fn test_has_mint_empty() {
        let repo = create_test_repository().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();
        assert!(!repo.has_mint(&mint_url).await);
    }

    #[tokio::test]
    async fn test_create_and_get_wallet() {
        let repo = create_test_repository().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        // Create a wallet
        let wallet = repo
            .create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet");

        assert_eq!(wallet.mint_url, mint_url);
        assert_eq!(wallet.unit, CurrencyUnit::Sat);

        // Verify we can get it back
        assert!(repo.has_mint(&mint_url).await);
        let retrieved = repo.get_wallet(&mint_url).await;
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_remove_wallet() {
        let repo = create_test_repository().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        // Create and then remove
        repo.create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet");

        assert!(repo.has_mint(&mint_url).await);
        repo.remove_wallet(&mint_url).await;
        assert!(!repo.has_mint(&mint_url).await);
    }

    #[tokio::test]
    async fn test_get_wallets() {
        let repo = create_test_repository().await;

        let mint1: MintUrl = "https://mint1.example.com".parse().unwrap();
        let mint2: MintUrl = "https://mint2.example.com".parse().unwrap();

        repo.create_wallet(mint1, CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet 1");
        repo.create_wallet(mint2, CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet 2");

        let wallets = repo.get_wallets().await;
        assert_eq!(wallets.len(), 2);
    }
}
