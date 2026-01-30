//! Wallet Repository
//!
//! Simple container that manages [`Wallet`] instances by mint URL.

use std::collections::BTreeMap;
#[cfg(feature = "npubcash")]
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::database;
use cdk_common::database::WalletDatabase;
use cdk_common::wallet::WalletKey;
use tokio::sync::RwLock;
use tracing::instrument;
use zeroize::Zeroize;

use super::builder::WalletBuilder;
use super::{Error, MintConnector};
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

/// Data extracted from a token
///
/// Contains the mint URL, proofs, and metadata from a parsed token.
#[derive(Debug, Clone)]
pub struct TokenData {
    /// The mint URL from the token
    pub mint_url: MintUrl,
    /// The proofs contained in the token
    pub proofs: cdk_common::Proofs,
    /// The memo from the token, if present
    pub memo: Option<String>,
    /// Value of token
    pub value: cdk_common::Amount,
    /// Unit of token
    pub unit: CurrencyUnit,
    /// Fee to redeem
    ///
    /// If the token is for a mint that we do not know, we cannot get the fee.
    /// To avoid just erroring and still allow decoding, this is an option.
    /// None does not mean there is no fee, it means we do not know the fee.
    pub redeem_fee: Option<cdk_common::Amount>,
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

/// Repository for managing Wallet instances by mint URL and currency unit
///
/// Simple container that bootstraps wallets from database and provides
/// access to individual Wallet instances. Each wallet is uniquely identified
/// by the combination of mint URL and currency unit.
#[derive(Clone)]
pub struct WalletRepository {
    /// Storage backend
    localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// Wallets indexed by (mint URL, currency unit)
    wallets: Arc<RwLock<BTreeMap<WalletKey, Wallet>>>,
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

    /// Get wallet for a mint URL and currency unit
    ///
    /// Returns an error if no wallet exists for the given mint URL and unit combination.
    #[instrument(skip(self))]
    pub async fn get_wallet(
        &self,
        mint_url: &MintUrl,
        unit: &CurrencyUnit,
    ) -> Result<Wallet, Error> {
        let key = WalletKey::new(mint_url.clone(), unit.clone());
        self.wallets
            .read()
            .await
            .get(&key)
            .cloned()
            .ok_or_else(|| Error::UnknownWallet(key))
    }

    /// Get all wallets for a specific mint URL (any currency unit)
    #[instrument(skip(self))]
    pub async fn get_wallets_for_mint(&self, mint_url: &MintUrl) -> Vec<Wallet> {
        self.wallets
            .read()
            .await
            .iter()
            .filter(|(key, _)| &key.mint_url == mint_url)
            .map(|(_, wallet)| wallet.clone())
            .collect()
    }

    /// Check if a specific wallet exists (mint URL + unit combination)
    #[instrument(skip(self))]
    pub async fn has_wallet(&self, mint_url: &MintUrl, unit: &CurrencyUnit) -> bool {
        let key = WalletKey::new(mint_url.clone(), unit.clone());
        self.wallets.read().await.contains_key(&key)
    }

    /// Add a mint to the repository
    ///
    /// Fetches the mint info to discover all supported currency units and creates
    /// a wallet for each unit. Returns all created wallets.
    #[instrument(skip(self))]
    pub async fn add_mint(&self, mint_url: MintUrl) -> Result<Vec<Wallet>, Error> {
        self.add_mint_with_config(mint_url, None).await
    }

    /// Add a mint to the repository with a custom configuration
    ///
    /// Fetches the mint info to discover all supported currency units and creates
    /// a wallet for each unit with the given configuration. Returns all created wallets.
    #[instrument(skip(self))]
    pub async fn add_mint_with_config(
        &self,
        mint_url: MintUrl,
        config: Option<WalletConfig>,
    ) -> Result<Vec<Wallet>, Error> {
        // Fetch mint info to get supported units
        let mint_info = self.fetch_mint_info(&mint_url).await?;
        let supported_units = mint_info.supported_units();

        if supported_units.is_empty() {
            return Err(Error::Custom(
                "Mint does not support any currency units".into(),
            ));
        }

        let mut wallets = Vec::new();
        for unit in supported_units {
            // Skip if wallet already exists for this unit
            if self.has_wallet(&mint_url, unit).await {
                if let Ok(existing) = self.get_wallet(&mint_url, unit).await {
                    wallets.push(existing);
                }
                continue;
            }

            let wallet = self
                .create_wallet(mint_url.clone(), unit.clone(), config.clone())
                .await?;
            wallets.push(wallet);
        }

        Ok(wallets)
    }

    /// Update configuration for an existing mint and unit
    ///
    /// This re-creates the wallet with the new configuration.
    #[instrument(skip(self))]
    pub async fn set_mint_config(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: WalletConfig,
    ) -> Result<Wallet, Error> {
        // Re-create wallet with new config
        self.create_wallet(mint_url, unit, Some(config)).await
    }

    /// Create and add a new wallet for a mint URL and currency unit
    /// Returns the created wallet
    #[instrument(skip(self))]
    pub async fn create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: Option<WalletConfig>,
    ) -> Result<Wallet, Error> {
        let wallet = self
            .create_wallet_internal(mint_url.clone(), unit.clone(), config.as_ref())
            .await?;

        // Insert into wallets map using WalletKey
        let key = WalletKey::new(mint_url, unit);
        let mut wallets = self.wallets.write().await;
        wallets.insert(key, wallet.clone());

        Ok(wallet)
    }

    /// Remove a wallet from the repository
    #[instrument(skip(self))]
    pub async fn remove_wallet(
        &self,
        mint_url: MintUrl,
        currency_unit: CurrencyUnit,
    ) -> Result<(), Error> {
        let key = WalletKey::new(mint_url.clone(), currency_unit.clone());
        let mut wallets = self.wallets.write().await;

        if !wallets.contains_key(&key) {
            return Err(Error::UnknownWallet(key));
        }

        // Check if this is the last wallet for the mint
        let is_last_wallet = wallets.keys().filter(|k| k.mint_url == mint_url).count() == 1;

        if is_last_wallet {
            self.localstore.remove_mint(mint_url).await?;
        }

        wallets.remove(&key);
        Ok(())
    }

    /// Get all wallets
    #[instrument(skip(self))]
    pub async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.read().await.values().cloned().collect()
    }

    /// Check if any wallet exists for a mint (regardless of currency unit)
    #[instrument(skip(self))]
    pub async fn has_mint(&self, mint_url: &MintUrl) -> bool {
        self.wallets
            .read()
            .await
            .keys()
            .any(|key| &key.mint_url == mint_url)
    }
    /// Get balances for all wallets
    ///
    /// Returns a map of (mint URL, currency unit) to balance for each wallet in the repository.
    #[instrument(skip(self))]
    pub async fn get_balances(&self) -> Result<BTreeMap<WalletKey, cdk_common::Amount>, Error> {
        let wallets = self.wallets.read().await;
        let mut balances = BTreeMap::new();

        for (key, wallet) in wallets.iter() {
            let balance = wallet.total_balance().await?;
            balances.insert(key.clone(), balance);
        }

        Ok(balances)
    }
    /// Get total balance across all wallets
    #[instrument(skip(self))]
    pub async fn total_balance(&self) -> Result<cdk_common::Amount, Error> {
        let balances = self.get_balances().await?;
        Ok(balances
            .values()
            .fold(cdk_common::Amount::ZERO, |acc, &x| acc + x))
    }
    /// Get or create a wallet for a mint URL and currency unit
    ///
    /// If a wallet for the mint URL and unit already exists, returns it.
    /// Otherwise creates a new wallet with the specified unit and adds it to the repository.
    #[instrument(skip(self))]
    pub async fn get_or_create_wallet(
        &self,
        mint_url: &MintUrl,
        unit: CurrencyUnit,
    ) -> Result<Wallet, Error> {
        if let Ok(wallet) = self.get_wallet(mint_url, &unit).await {
            return Ok(wallet);
        }

        self.create_wallet(mint_url.clone(), unit, None).await
    }

    /// Fetch mint info from a mint URL
    ///
    /// Creates a temporary HTTP client to fetch the mint info.
    /// This is useful to discover supported currency units before adding a mint.
    pub async fn fetch_mint_info(
        &self,
        mint_url: &MintUrl,
    ) -> Result<crate::nuts::MintInfo, Error> {
        // Create an HTTP client based on the repository configuration
        let client: Arc<dyn MintConnector + Send + Sync> =
            if let Some(proxy_url) = &self.proxy_config {
                Arc::new(
                    crate::wallet::HttpClient::with_proxy(
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
                    }),
                )
            } else {
                #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
                if let Some(tor) = &self.shared_tor_transport {
                    let transport = tor.clone();
                    #[cfg(feature = "auth")]
                    {
                        Arc::new(crate::wallet::TorHttpClient::with_transport(
                            mint_url.clone(),
                            transport,
                            None,
                        ))
                    }
                    #[cfg(not(feature = "auth"))]
                    {
                        Arc::new(crate::wallet::TorHttpClient::with_transport(
                            mint_url.clone(),
                            transport,
                        ))
                    }
                } else {
                    #[cfg(feature = "auth")]
                    {
                        Arc::new(crate::wallet::HttpClient::new(mint_url.clone(), None))
                    }
                    #[cfg(not(feature = "auth"))]
                    {
                        Arc::new(crate::wallet::HttpClient::new(mint_url.clone()))
                    }
                }

                #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
                {
                    #[cfg(feature = "auth")]
                    {
                        Arc::new(crate::wallet::HttpClient::new(mint_url.clone(), None))
                    }
                    #[cfg(not(feature = "auth"))]
                    {
                        Arc::new(crate::wallet::HttpClient::new(mint_url.clone()))
                    }
                }
            };

        client.get_mint_info().await
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
    /// For each mint, it fetches the mint info to discover supported units
    /// and creates a wallet for each supported unit.
    #[instrument(skip(self))]
    async fn load_wallets(&self) -> Result<(), Error> {
        let mints = self.localstore.get_mints().await.map_err(Error::Database)?;

        for (mint_url, _mint_info) in mints {
            // Try to fetch mint info and create wallets for all supported units
            // If fetch fails, fall back to creating just a Sat wallet
            let units = match self.fetch_mint_info(&mint_url).await {
                Ok(info) => {
                    let supported = info.supported_units();
                    if supported.is_empty() {
                        vec![CurrencyUnit::Sat]
                    } else {
                        supported.into_iter().cloned().collect()
                    }
                }
                Err(_) => {
                    // If we can't fetch mint info, use default Sat unit for backward compatibility
                    vec![CurrencyUnit::Sat]
                }
            };

            for unit in units {
                let key = WalletKey::new(mint_url.clone(), unit.clone());
                // Skip if wallet already exists
                if self.wallets.read().await.contains_key(&key) {
                    continue;
                }

                let wallet = self
                    .create_wallet_internal(mint_url.clone(), unit, None)
                    .await?;

                let mut wallets = self.wallets.write().await;
                wallets.insert(key, wallet);
            }
        }

        Ok(())
    }

    /// Get the currently active NpubCash mint URL
    ///
    /// Returns the mint URL that has been set as active for NpubCash operations,
    /// or None if no active mint has been configured.
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

    /// Set the active NpubCash mint URL
    ///
    /// This sets the mint that will be used for NpubCash operations.
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

    /// Sync NpubCash quotes from the active mint
    ///
    /// Retrieves pending mint quotes from the currently active NpubCash mint.
    /// Returns an error if no active mint has been configured.
    /// Uses Sat as the default unit for NpubCash operations.
    #[cfg(feature = "npubcash")]
    pub async fn sync_npubcash_quotes(
        &self,
    ) -> Result<Vec<crate::wallet::types::MintQuote>, Error> {
        let active_mint = self.get_active_npubcash_mint().await?;
        if let Some(mint_url) = active_mint {
            // NpubCash typically uses Sat, try to find a Sat wallet first
            let wallet = self.get_wallet(&mint_url, &CurrencyUnit::Sat).await?;
            wallet.sync_npubcash_quotes().await
        } else {
            Err(Error::Custom("No active NpubCash mint set".into()))
        }
    }

    // =========================================================================
    // Helper functions for token and proof operations
    // =========================================================================

    /// Get token data (mint URL and proofs) from a token
    ///
    /// This method extracts the mint URL and proofs from a token. It will automatically
    /// fetch the keysets from the mint if needed to properly decode the proofs.
    ///
    /// The mint must already be added to the wallet. If the mint is not in the wallet,
    /// use `add_mint` first or set `allow_untrusted` in receive options.
    ///
    /// # Arguments
    ///
    /// * `token` - The token to extract data from
    ///
    /// # Returns
    ///
    /// A `TokenData` struct containing the mint URL and proofs
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cdk::wallet::WalletRepository;
    /// # use cdk::nuts::Token;
    /// # use std::str::FromStr;
    /// # async fn example(wallet: &WalletRepository) -> Result<(), Box<dyn std::error::Error>> {
    /// let token = Token::from_str("cashuA...")?;
    /// let token_data = wallet.get_token_data(&token).await?;
    /// println!("Mint: {}", token_data.mint_url);
    /// println!("Proofs: {} total", token_data.proofs.len());
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, token))]
    pub async fn get_token_data(
        &self,
        token: &crate::nuts::nut00::Token,
    ) -> Result<TokenData, Error> {
        let mint_url = token.mint_url()?;
        let unit = token.unit().unwrap_or_default();

        // Get the keysets for this mint using the token's unit
        let wallet = self.get_wallet(&mint_url, &unit).await?;
        let keysets = wallet.get_mint_keysets().await?;
        // Extract proofs using the keysets
        let proofs = token.proofs(&keysets)?;

        // Get the memo
        let memo = token.memo().clone();
        let redeem_fee = wallet.get_proofs_fee(&proofs).await?;

        Ok(TokenData {
            value: cdk_common::nuts::nut00::ProofsMethods::total_amount(&proofs)?,
            mint_url,
            proofs,
            memo,
            unit,
            redeem_fee: Some(redeem_fee.total),
        })
    }

    /// List proofs for all wallets
    ///
    /// Returns a map of (mint URL, currency unit) to proofs for each wallet in the repository.
    #[instrument(skip(self))]
    pub async fn list_proofs(
        &self,
    ) -> Result<std::collections::BTreeMap<WalletKey, Vec<cdk_common::Proof>>, Error> {
        let mut mint_proofs = std::collections::BTreeMap::new();

        for (key, wallet) in self.wallets.read().await.iter() {
            let wallet_proofs = wallet.get_unspent_proofs().await?;
            mint_proofs.insert(key.clone(), wallet_proofs);
        }
        Ok(mint_proofs)
    }

    /// List transactions across all wallets
    #[instrument(skip(self))]
    pub async fn list_transactions(
        &self,
        direction: Option<cdk_common::wallet::TransactionDirection>,
    ) -> Result<Vec<cdk_common::wallet::Transaction>, Error> {
        let mut transactions = Vec::new();

        for (_, wallet) in self.wallets.read().await.iter() {
            let wallet_transactions = wallet.list_transactions(direction).await?;
            transactions.extend(wallet_transactions);
        }

        transactions.sort();

        Ok(transactions)
    }

    /// Check all pending mint quotes and mint any that are paid
    #[instrument(skip(self))]
    pub async fn check_all_mint_quotes(
        &self,
        mint_url: Option<MintUrl>,
    ) -> Result<cdk_common::Amount, Error> {
        let mut total_minted = cdk_common::Amount::ZERO;

        let wallets = self.wallets.read().await;
        let wallets_to_check: Vec<_> = match &mint_url {
            Some(url) => {
                // Get all wallets for this mint (any currency unit)
                let filtered: Vec<_> = wallets
                    .iter()
                    .filter(|(key, _)| &key.mint_url == url)
                    .map(|(_, wallet)| wallet.clone())
                    .collect();

                if filtered.is_empty() {
                    return Err(Error::UnknownMint {
                        mint_url: url.to_string(),
                    });
                }
                filtered
            }
            None => wallets.values().cloned().collect(),
        };
        drop(wallets);

        for wallet in wallets_to_check {
            let amount = wallet.check_all_mint_quotes().await?;
            total_minted += amount;
        }

        Ok(total_minted)
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
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
        let retrieved = repo.get_wallet(&mint_url, &CurrencyUnit::Sat).await;
        assert!(retrieved.is_ok());
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
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
        let _ = repo
            .remove_wallet(mint_url.clone(), CurrencyUnit::Sat)
            .await;
        assert!(!repo.has_mint(&mint_url).await);
        assert!(!repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
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

    #[tokio::test]
    async fn test_remove_wallet_persists_to_db() {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        let repo = WalletRepository::new(localstore.clone(), seed)
            .await
            .expect("Failed to create WalletRepository");

        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        // Add mint to DB manually to simulate existing state
        localstore.add_mint(mint_url.clone(), None).await.unwrap();

        // Verify mint is in DB
        assert!(localstore
            .get_mint(mint_url.clone())
            .await
            .unwrap()
            .is_some());

        // Create wallet in repo
        repo.create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet");

        // Remove wallet
        repo.remove_wallet(mint_url.clone(), CurrencyUnit::Sat)
            .await
            .expect("Failed to remove wallet");

        // Verify mint is REMOVED from DB
        assert!(localstore
            .get_mint(mint_url.clone())
            .await
            .unwrap()
            .is_none());
    }
}
