//! Wallet Manager
//!
//! Simple container that manages [`Wallet`] instances by mint URL.

use std::collections::BTreeMap;
use std::fmt;
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
use super::{
    AuthMintConnector, Error, MintConnector, RateLimitConfig, RateLimiterManager, WalletIdentity,
};
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
#[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
use crate::wallet::mint_connector::transport::TorAsync;
use crate::{Amount, OidcClient, Wallet};

/// Protocol details extracted from an encoded token.
///
/// Contains the mint URL, proofs, and metadata from a parsed token.
#[derive(Debug, Clone)]
pub struct DecodedToken {
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

/// Expert per-mint configuration for wallets managed by [`WalletManager`].
#[derive(Clone, Default)]
pub struct MintAdvancedOptions {
    /// Custom mint connector implementation
    mint_connector: Option<Arc<dyn super::MintConnector + Send + Sync>>,
    /// Custom auth connector implementation
    auth_connector: Option<Arc<dyn super::auth::AuthMintConnector + Send + Sync>>,
    /// Target number of proofs to maintain at each denomination
    target_proof_count: Option<usize>,
    /// Metadata cache TTL
    ///
    /// The TTL determines how often the wallet checks the mint for new keysets and information.
    ///
    /// If `None`, the cache will never expire and the wallet will use cached data indefinitely
    /// (unless manually refreshed).
    ///
    /// The default value is 1 hour (3600 seconds).
    metadata_cache_ttl: Option<Option<std::time::Duration>>,
}

impl fmt::Debug for MintAdvancedOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MintAdvancedOptions")
            .field(
                "mint_connector",
                &self.mint_connector.as_ref().map(|_| "[CONFIGURED]"),
            )
            .field(
                "auth_connector",
                &self.auth_connector.as_ref().map(|_| "[CONFIGURED]"),
            )
            .field("target_proof_count", &self.target_proof_count)
            .field("metadata_cache_ttl", &self.metadata_cache_ttl)
            .finish()
    }
}

impl MintAdvancedOptions {
    /// Create empty advanced options.
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
    pub fn with_metadata_cache_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.metadata_cache_ttl = Some(Some(ttl));
        self
    }

    /// Keep cached mint metadata until an explicit refresh is requested.
    pub fn without_metadata_cache_expiry(mut self) -> Self {
        self.metadata_cache_ttl = Some(None);
        self
    }
}

/// Request to discover a mint's units and register wallets for them.
#[derive(Debug, Clone)]
pub struct MintRegistrationRequest {
    /// Mint to register.
    pub mint_url: MintUrl,
    /// Explicitly advanced connector, cache, and proof-management options.
    pub(crate) advanced: MintAdvancedOptions,
}

impl MintRegistrationRequest {
    /// Register a mint using default transport and proof-management settings.
    pub fn new(mint_url: MintUrl) -> Self {
        Self {
            mint_url,
            advanced: MintAdvancedOptions::default(),
        }
    }

    /// Apply expert per-mint options.
    pub fn with_advanced(mut self, advanced: MintAdvancedOptions) -> Self {
        self.advanced = advanced;
        self
    }
}

impl From<MintUrl> for MintRegistrationRequest {
    fn from(mint_url: MintUrl) -> Self {
        Self::new(mint_url)
    }
}

/// Request to create or replace one mint-and-unit wallet configuration.
#[derive(Debug, Clone)]
pub struct WalletConfigurationRequest {
    /// Wallet being configured.
    pub identity: super::WalletIdentity,
    /// Explicitly advanced connector, cache, and proof-management options.
    pub(crate) advanced: MintAdvancedOptions,
}

impl WalletConfigurationRequest {
    /// Configure a wallet using default transport and proof-management settings.
    pub fn new(identity: super::WalletIdentity) -> Self {
        Self {
            identity,
            advanced: MintAdvancedOptions::default(),
        }
    }

    /// Apply expert per-mint options.
    pub fn with_advanced(mut self, advanced: MintAdvancedOptions) -> Self {
        self.advanced = advanced;
        self
    }
}

/// Builder for creating [`WalletManager`] instances
///
/// # Example
/// ```no_run
/// # use std::sync::Arc;
/// # use cdk::wallet::WalletManagerBuilder;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
/// let seed = [0u8; 64];
/// let wallet_manager = WalletManagerBuilder::new()
///     .with_store(localstore)
///     .with_seed(seed)
///     .build()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct WalletManagerBuilder {
    localstore: Option<Arc<dyn WalletDatabase<database::Error> + Send + Sync>>,
    seed: Option<[u8; 64]>,
    proxy_config: Option<url::Url>,
    danger_accept_invalid_certs: bool,
    rate_limit: Option<RateLimitConfig>,
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    use_tor: bool,
}

impl std::fmt::Debug for WalletManagerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletManagerBuilder")
            .field("localstore", &self.localstore.as_ref().map(|_| "..."))
            .field("seed", &"[REDACTED]")
            .field(
                "proxy_config",
                &self.proxy_config.as_ref().map(|_| "[CONFIGURED]"),
            )
            .field(
                "danger_accept_invalid_certs",
                &self.danger_accept_invalid_certs,
            )
            .field("rate_limit", &self.rate_limit)
            .finish()
    }
}

impl Default for WalletManagerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl WalletManagerBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            localstore: None,
            seed: None,
            proxy_config: None,
            danger_accept_invalid_certs: false,
            rate_limit: Some(RateLimitConfig::default()),
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            use_tor: false,
        }
    }

    /// Set the storage backend
    pub fn with_store(
        mut self,
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    ) -> Self {
        self.localstore = Some(localstore);
        self
    }

    /// Set the wallet seed
    pub fn with_seed(mut self, seed: [u8; 64]) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Set the proxy URL for HTTP clients
    pub fn with_proxy(mut self, proxy_url: url::Url) -> Self {
        self.proxy_config = Some(proxy_url);
        self
    }

    /// Disable TLS certificate verification for proxied HTTPS clients.
    ///
    /// This permits man-in-the-middle attacks and should only be used for
    /// local debugging or trusted test environments.
    pub fn with_danger_accept_invalid_certs(mut self, accept_invalid_certs: bool) -> Self {
        self.danger_accept_invalid_certs = accept_invalid_certs;
        self
    }

    /// Enable Tor transport
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    pub fn with_tor(mut self) -> Self {
        self.use_tor = true;
        self
    }

    /// Set the rate-limiting configuration shared by every wallet this
    /// manager builds.
    ///
    /// Rate limiting is on by default with [`RateLimitConfig::default`].
    pub fn with_rate_limiting_config(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit = Some(config);
        self
    }

    /// Start with pacing turned off.
    ///
    /// The limiter is still built, so
    /// [`WalletManager::set_rate_limiting_config`] can turn pacing back on
    /// later. Unlike [`WalletBuilder::with_rate_limiting_disabled`], this keeps
    /// the manager's shared limiter so every managed wallet can be enabled
    /// together later.
    pub fn with_rate_limiting_disabled(mut self) -> Self {
        self.rate_limit = None;
        self
    }

    /// Build the WalletManager and load existing wallets from the database.
    ///
    /// This only uses persisted mint metadata and does not make network requests.
    pub async fn build(self) -> Result<WalletManager, Error> {
        let localstore = self
            .localstore
            .ok_or(Error::Custom("localstore is required".to_string()))?;
        let seed = self
            .seed
            .ok_or(Error::Custom("seed is required".to_string()))?;

        let rate_limiter = RateLimiterManager::new(
            self.rate_limit.unwrap_or_default(),
            Some(localstore.clone()),
        );
        rate_limiter.set_enabled(self.rate_limit.is_some());

        let wallet = WalletManager {
            rate_limiter,
            localstore,
            seed,
            wallets: Arc::new(RwLock::new(BTreeMap::new())),
            proxy_config: self.proxy_config,
            danger_accept_invalid_certs: self.danger_accept_invalid_certs,
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            shared_tor_transport: if self.use_tor {
                Some(TorAsync::new())
            } else {
                None
            },
        };

        wallet.load_wallets().await?;
        Ok(wallet)
    }
}

fn proxy_http_client(
    mint_url: MintUrl,
    proxy_url: &url::Url,
    accept_invalid_certs: bool,
) -> Result<crate::wallet::HttpClient, Error> {
    validate_proxy_url(proxy_url)?;

    crate::wallet::HttpClient::with_proxy(mint_url, proxy_url.clone(), None, accept_invalid_certs)
}

fn proxy_auth_http_client(
    mint_url: MintUrl,
    proxy_url: &url::Url,
    accept_invalid_certs: bool,
) -> Result<crate::wallet::AuthHttpClient, Error> {
    validate_proxy_url(proxy_url)?;

    crate::wallet::AuthHttpClient::with_proxy(
        mint_url,
        proxy_url.clone(),
        None,
        accept_invalid_certs,
        None,
    )
}

fn validate_proxy_url(proxy_url: &url::Url) -> Result<(), Error> {
    match proxy_url.scheme() {
        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h" => {}
        scheme => {
            return Err(Error::HttpError(
                None,
                format!("Unsupported proxy URL scheme: {scheme}"),
            ));
        }
    }

    Ok(())
}

/// Manager for managing Wallet instances by mint URL and currency unit
///
/// Simple container that bootstraps wallets from database and provides
/// access to individual Wallet instances. Each wallet is uniquely identified
/// by the combination of mint URL and currency unit.
///
/// Every wallet shares the manager's [`RateLimiterManager`], which keys
/// budgets by the host each request is addressed to. Wallets at one mint pace
/// one combined burst regardless of currency unit, and traffic to a third-party
/// host (an LNURL service, an OIDC provider) paces against that host's own
/// budget rather than any mint's.
///
/// Because that limiter is shared, pacing is configured for the manager as a
/// whole, at build time through [`WalletManagerBuilder::with_rate_limiting_config`]
/// or later through [`WalletManager::set_rate_limiting_config`], never per
/// wallet. Proxied and Tor wallets are built with a custom client, so their
/// limiter is wired to nothing and they report
/// [`Wallet::is_rate_limited`] as false whatever the manager is set to.
#[derive(Clone)]
pub struct WalletManager {
    /// Storage backend
    localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    seed: [u8; 64],
    /// Wallets indexed by (mint URL, currency unit)
    wallets: Arc<RwLock<BTreeMap<WalletKey, Wallet>>>,
    /// Hands out one shared rate-limit budget per destination host, injected
    /// into every wallet this manager builds.
    rate_limiter: RateLimiterManager,
    /// Proxy configuration for HTTP clients (optional)
    proxy_config: Option<url::Url>,
    /// Whether proxied HTTPS clients should accept invalid TLS certificates
    danger_accept_invalid_certs: bool,
    /// Shared Tor transport to be cloned into each TorHttpClient (if enabled)
    #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
    shared_tor_transport: Option<TorAsync>,
}

impl std::fmt::Debug for WalletManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletManager").finish_non_exhaustive()
    }
}

impl WalletManager {
    /// Get the wallet seed
    #[cfg(feature = "nostr")]
    pub(crate) fn seed(&self) -> &[u8; 64] {
        &self.seed
    }

    /// Get wallet for a mint URL and currency unit
    ///
    /// Returns an error if no wallet exists for the given mint URL and unit combination.
    #[instrument(skip(self))]
    pub(crate) async fn get_wallet(
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
    pub(crate) async fn get_wallets_for_mint(&self, mint_url: &MintUrl) -> Vec<Wallet> {
        self.wallets
            .read()
            .await
            .iter()
            .filter(|(key, _)| &key.mint_url == mint_url)
            .map(|(_, wallet)| wallet.clone())
            .collect()
    }

    /// Create an OIDC client using a wallet connector for this mint when available.
    #[instrument(skip(self))]
    pub(crate) async fn oidc_client_for_mint(
        &self,
        mint_url: &MintUrl,
        openid_discovery: String,
        client_id: Option<String>,
    ) -> OidcClient {
        match self.get_wallets_for_mint(mint_url).await.into_iter().next() {
            Some(wallet) => wallet.oidc_client(openid_discovery, client_id),
            None => OidcClient::new(openid_discovery, client_id),
        }
    }

    /// Check if a specific wallet exists (mint URL + unit combination)
    #[instrument(skip(self))]
    pub(crate) async fn has_wallet(&self, mint_url: &MintUrl, unit: &CurrencyUnit) -> bool {
        let key = WalletKey::new(mint_url.clone(), unit.clone());
        self.wallets.read().await.contains_key(&key)
    }

    /// Add wallets for a mint to the manager
    ///
    /// Fetches the mint info to discover all supported currency units and creates
    /// a wallet for each unit. Returns all created wallets.
    #[cfg(feature = "nostr")]
    #[instrument(skip(self))]
    pub(crate) async fn add_wallet(&self, mint_url: MintUrl) -> Result<Vec<Wallet>, Error> {
        self.add_wallet_with_config(mint_url, None).await
    }

    /// Add wallets for a mint to the manager with a custom configuration
    ///
    /// Fetches the mint info to discover all supported currency units and creates
    /// a wallet for each unit with the given configuration. Returns all created wallets.
    #[instrument(skip(self, config))]
    pub(crate) async fn add_wallet_with_config(
        &self,
        mint_url: MintUrl,
        config: Option<MintAdvancedOptions>,
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
            let wallet = self
                .get_or_create_wallet(mint_url.clone(), unit.clone(), config.clone())
                .await?;
            wallets.push(wallet);
        }

        Ok(wallets)
    }

    /// Return the wallet for a mint and unit, creating it if it does not exist yet.
    ///
    /// An existing wallet is returned untouched. Use [`Self::create_wallet`] to
    /// replace it with a new configuration.
    ///
    /// The write lock is held across the lookup and the insert so that concurrent
    /// callers for the same mint and unit all observe the same wallet instead of
    /// each building one and the last writer winning.
    #[instrument(skip(self, config))]
    pub(crate) async fn get_or_create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: Option<MintAdvancedOptions>,
    ) -> Result<Wallet, Error> {
        let key = WalletKey::new(mint_url.clone(), unit.clone());
        let mut wallets = self.wallets.write().await;

        if let Some(existing) = wallets.get(&key) {
            return Ok(existing.clone());
        }

        let wallet = self
            .create_wallet_internal(mint_url, unit, config.as_ref())
            .await?;
        wallets.insert(key, wallet.clone());

        Ok(wallet)
    }

    /// Create and add a new wallet for a mint URL and currency unit
    /// Returns the created wallet
    #[instrument(skip(self, config))]
    pub(crate) async fn create_wallet(
        &self,
        mint_url: MintUrl,
        unit: CurrencyUnit,
        config: Option<MintAdvancedOptions>,
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

    /// Wait until the rate-limit budgets drawn down by every wallet in this
    /// manager have been handed to storage.
    ///
    /// The manager owns the limiter its wallets share, so this is the
    /// shutdown barrier to await before dropping it. Equivalent to
    /// [`Wallet::flush_rate_limits`] on any one of its wallets, and safe to call
    /// when the manager holds no wallets at all. The same caveat applies:
    /// without it, persistence is best effort and a rebuild can outrun the
    /// detached writer.
    pub async fn flush_rate_limits(&self) {
        self.rate_limiter.flush().await;
    }

    /// Reconfigure pacing for every wallet in this manager, or turn it off
    /// with `None`.
    ///
    /// Pacing is a manager-wide property because one limiter is shared, so
    /// there is deliberately no per-wallet equivalent at creation time: it would
    /// silently reconfigure sibling wallets.
    pub fn set_rate_limiting_config(&self, config: Option<RateLimitConfig>) {
        match config {
            Some(config) => self.rate_limiter.set_config(config),
            None => self.rate_limiter.set_enabled(false),
        }
    }

    /// Whether this manager is pacing requests right now.
    ///
    /// Individual wallets can still report false while this is true: a proxied
    /// or Tor wallet is built with a custom client, which leaves its limiter
    /// wired to nothing.
    pub fn is_rate_limited(&self) -> bool {
        self.rate_limiter.is_enabled()
    }

    /// Remove a wallet from the in-memory manager
    ///
    /// This only removes the wallet from the in-memory map. It does not remove
    /// the mint from the database. Use the database directly if you need to
    /// explicitly remove persisted mint data.
    ///
    /// The origin's shared rate-limit bucket stays in the
    /// [`RateLimiterManager`] while any handle is live or its budget is still
    /// recovering, so re-adding a wallet for the same origin inherits the same
    /// live budget rather than starting full and bursting again. Once no wallet
    /// holds it and its budget has fully recovered, a later wallet creation
    /// evicts it; the persisted budget still survives a re-add.
    #[instrument(skip(self))]
    pub(crate) async fn remove_wallet(
        &self,
        mint_url: MintUrl,
        currency_unit: CurrencyUnit,
    ) -> Result<(), Error> {
        let key = WalletKey::new(mint_url, currency_unit);
        let mut wallets = self.wallets.write().await;

        if !wallets.contains_key(&key) {
            return Err(Error::UnknownWallet(key));
        }

        wallets.remove(&key);
        Ok(())
    }

    /// Get all wallets
    #[instrument(skip(self))]
    pub(crate) async fn get_wallets(&self) -> Vec<Wallet> {
        self.wallets.read().await.values().cloned().collect()
    }

    /// Check if any wallet exists for a mint (regardless of currency unit)
    #[instrument(skip(self))]
    pub(crate) async fn has_mint(&self, mint_url: &MintUrl) -> bool {
        self.wallets
            .read()
            .await
            .keys()
            .any(|key| &key.mint_url == mint_url)
    }
    /// Fetch mint info from a mint URL
    ///
    /// Creates a temporary HTTP client to fetch the mint info.
    /// This is useful to discover supported currency units before adding a mint.
    pub(crate) async fn fetch_mint_info(
        &self,
        mint_url: &MintUrl,
    ) -> Result<crate::nuts::MintInfo, Error> {
        // Create an HTTP client based on the manager configuration
        let client: Arc<dyn MintConnector + Send + Sync> =
            if let Some(proxy_url) = &self.proxy_config {
                Arc::new(proxy_http_client(
                    mint_url.clone(),
                    proxy_url,
                    self.danger_accept_invalid_certs,
                )?)
            } else {
                #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
                if let Some(tor) = &self.shared_tor_transport {
                    let transport = tor.clone();
                    Arc::new(crate::wallet::TorHttpClient::with_transport(
                        mint_url.clone(),
                        transport,
                        None,
                    ))
                } else {
                    Arc::new(crate::wallet::HttpClient::new(mint_url.clone(), None))
                }

                #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
                {
                    Arc::new(crate::wallet::HttpClient::new(mint_url.clone(), None))
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
        config: Option<&MintAdvancedOptions>,
    ) -> Result<Wallet, Error> {
        let target_proof_count = config.and_then(|c| c.target_proof_count).unwrap_or(3);
        let metadata_cache_ttl = config.and_then(|c| c.metadata_cache_ttl);
        let configured_auth_connector = config.and_then(|c| c.auth_connector.clone());

        // Check if custom connector is provided in config
        if let Some(cfg) = config {
            if let Some(custom_connector) = &cfg.mint_connector {
                // Use custom connector with WalletBuilder
                let mut builder = WalletBuilder::new()
                    .with_mint_url(mint_url.clone())
                    .with_unit(unit.clone())
                    .with_store(self.localstore.clone())
                    .with_seed(self.seed)
                    .with_target_proof_count(target_proof_count)
                    .with_rate_limiter(self.rate_limiter.clone())
                    .with_shared_connector(custom_connector.clone());

                if let Some(auth_connector) = configured_auth_connector.clone() {
                    builder = builder.with_authentication_connector(auth_connector);
                }

                if let Some(ttl) = metadata_cache_ttl {
                    builder = builder.with_metadata_cache_ttl(ttl);
                }

                return builder.build();
            }
        }

        // Fall back to existing logic: proxy/Tor/default
        let wallet = if let Some(proxy_url) = &self.proxy_config {
            // Create wallet with proxy-configured client
            let client = proxy_http_client(
                mint_url.clone(),
                proxy_url,
                self.danger_accept_invalid_certs,
            )?;
            let auth_connector = match configured_auth_connector.clone() {
                Some(auth_connector) => auth_connector,
                None => Arc::new(proxy_auth_http_client(
                    mint_url.clone(),
                    proxy_url,
                    self.danger_accept_invalid_certs,
                )?) as Arc<dyn AuthMintConnector + Send + Sync>,
            };
            let mut builder = WalletBuilder::new()
                .with_mint_url(mint_url.clone())
                .with_unit(unit.clone())
                .with_store(self.localstore.clone())
                .with_seed(self.seed)
                .with_target_proof_count(target_proof_count)
                .with_rate_limiter(self.rate_limiter.clone())
                .with_connector(client)
                .with_authentication_connector(auth_connector);

            if let Some(ttl) = metadata_cache_ttl {
                builder = builder.with_metadata_cache_ttl(ttl);
            }

            builder.build()?
        } else {
            #[cfg(all(feature = "tor", not(target_arch = "wasm32")))]
            if let Some(tor) = &self.shared_tor_transport {
                // Create wallet with Tor transport client, cloning the shared transport
                let client = crate::wallet::TorHttpClient::with_transport(
                    mint_url.clone(),
                    tor.clone(),
                    None,
                );
                let auth_connector = configured_auth_connector.clone().unwrap_or_else(|| {
                    Arc::new(crate::wallet::TorAuthHttpClient::with_transport(
                        mint_url.clone(),
                        tor.clone(),
                        None,
                    )) as Arc<dyn AuthMintConnector + Send + Sync>
                });

                let mut builder = WalletBuilder::new()
                    .with_mint_url(mint_url.clone())
                    .with_unit(unit.clone())
                    .with_store(self.localstore.clone())
                    .with_seed(self.seed)
                    .with_target_proof_count(target_proof_count)
                    .with_rate_limiter(self.rate_limiter.clone())
                    .with_connector(client)
                    .with_authentication_connector(auth_connector);

                if let Some(ttl) = metadata_cache_ttl {
                    builder = builder.with_metadata_cache_ttl(ttl);
                }

                builder.build()?
            } else {
                // Create wallet with default client
                let mut builder = WalletBuilder::new()
                    .with_mint_url(mint_url.clone())
                    .with_unit(unit.clone())
                    .with_store(self.localstore.clone())
                    .with_seed(self.seed)
                    .with_target_proof_count(target_proof_count)
                    .with_rate_limiter(self.rate_limiter.clone());

                if let Some(auth_connector) = configured_auth_connector.clone() {
                    builder = builder.with_authentication_connector(auth_connector);
                }

                if let Some(ttl) = metadata_cache_ttl {
                    builder = builder.with_metadata_cache_ttl(ttl);
                }

                builder.build()?
            }

            #[cfg(not(all(feature = "tor", not(target_arch = "wasm32"))))]
            {
                // Create wallet with default client
                let mut builder = WalletBuilder::new()
                    .with_mint_url(mint_url.clone())
                    .with_unit(unit.clone())
                    .with_store(self.localstore.clone())
                    .with_seed(self.seed)
                    .with_target_proof_count(target_proof_count)
                    .with_rate_limiter(self.rate_limiter.clone());

                if let Some(auth_connector) = configured_auth_connector.clone() {
                    builder = builder.with_authentication_connector(auth_connector);
                }

                if let Some(ttl) = metadata_cache_ttl {
                    builder = builder.with_metadata_cache_ttl(ttl);
                }

                builder.build()?
            }
        };

        Ok(wallet)
    }

    /// Load all wallets from database
    ///
    /// This loads wallets for all mints stored in the database. For each mint,
    /// it uses the persisted mint info to discover supported units and creates
    /// a wallet for each supported unit. This does not make network requests.
    #[instrument(skip(self))]
    async fn load_wallets(&self) -> Result<(), Error> {
        let mints = self.localstore.get_mints().await.map_err(Error::Database)?;

        for (mint_url, mint_info) in mints {
            let units = mint_info
                .map(|info| {
                    let supported_units = info.supported_units();
                    if supported_units.is_empty() {
                        vec![CurrencyUnit::Sat]
                    } else {
                        supported_units.into_iter().cloned().collect()
                    }
                })
                // Older databases may not have mint metadata. Keep the
                // established single-sat wallet behavior for those records.
                .unwrap_or_else(|| vec![CurrencyUnit::Sat]);

            for unit in units {
                self.get_or_create_wallet(mint_url.clone(), unit, None)
                    .await?;
            }
        }

        Ok(())
    }

    /// Get the currently active NpubCash mint URL
    ///
    /// Returns the mint URL that has been set as active for NpubCash operations,
    /// or None if no active mint has been configured.
    #[cfg(feature = "npubcash")]
    pub(crate) async fn get_active_npubcash_mint(&self) -> Result<Option<MintUrl>, Error> {
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
    pub(crate) async fn set_active_npubcash_mint(&self, mint_url: MintUrl) -> Result<(), Error> {
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
    pub(crate) async fn synchronize_npubcash_quotes(
        &self,
    ) -> Result<Vec<cdk_common::wallet::MintQuote>, Error> {
        let active_mint = self.get_active_npubcash_mint().await?;
        if let Some(mint_url) = active_mint {
            // NpubCash typically uses Sat, try to find a Sat wallet first
            let wallet = self.get_wallet(&mint_url, &CurrencyUnit::Sat).await?;
            wallet.advanced().synchronize_npubcash_quotes().await
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
    /// A [`DecodedToken`] containing the mint URL and proofs.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use cdk::wallet::WalletManager;
    /// # use cdk::nuts::Token;
    /// # use std::str::FromStr;
    /// # async fn example(wallet: &WalletManager) -> Result<(), Box<dyn std::error::Error>> {
    /// let token = Token::from_str("cashuA...")?;
    /// let token_data = wallet.advanced().inspect_token(&token).await?;
    /// println!("Mint: {}", token_data.mint_url);
    /// println!("Proofs: {} total", token_data.proofs.len());
    /// # Ok(())
    /// # }
    /// ```
    #[instrument(skip(self, token))]
    pub(crate) async fn get_token_data(
        &self,
        token: &crate::nuts::nut00::Token,
    ) -> Result<DecodedToken, Error> {
        let mint_url = token.mint_url()?;
        let unit = token.unit().unwrap_or_default();

        // Get the keysets for this mint using the token's unit
        let wallet = self.get_wallet(&mint_url, &unit).await?;
        let proofs = wallet.token_proofs(token).await?;

        // Get the memo
        let memo = token.memo().clone();
        let redeem_fee = wallet.get_proofs_fee(&proofs).await?;

        Ok(DecodedToken {
            value: cdk_common::nuts::nut00::ProofsMethods::total_amount(&proofs)?,
            mint_url,
            proofs,
            memo,
            unit,
            redeem_fee: Some(redeem_fee.total),
        })
    }

    /// List transactions across all wallets
    #[instrument(skip(self))]
    pub(crate) async fn list_transactions(
        &self,
        direction: Option<cdk_common::wallet::TransactionDirection>,
    ) -> Result<Vec<cdk_common::wallet::Transaction>, Error> {
        let mut transactions = Vec::new();

        for wallet in self.wallets.read().await.values() {
            let wallet_transactions = wallet.list_transactions(direction).await?;
            transactions.extend(wallet_transactions);
        }

        transactions.sort();

        Ok(transactions)
    }

    /// Check all pending mint quotes and mint any that are paid
    #[instrument(skip(self))]
    pub(crate) async fn check_all_mint_quotes(
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
            let minted = wallet.mint_unissued_quotes().await?;
            total_minted += minted;
        }

        Ok(total_minted)
    }
}

impl Drop for WalletManager {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

impl WalletManager {
    /// Discover a mint's supported units and register wallets for them.
    pub async fn register_mint<R>(&self, request: R) -> Result<Vec<Wallet>, Error>
    where
        R: Into<crate::wallet::MintRegistrationRequest>,
    {
        let request = request.into();
        self.add_wallet_with_config(request.mint_url, Some(request.advanced))
            .await
    }

    /// Create or replace one mint-and-unit wallet configuration.
    pub async fn configure_wallet(
        &self,
        request: crate::wallet::WalletConfigurationRequest,
    ) -> Result<Wallet, Error> {
        self.create_wallet(
            request.identity.mint_url,
            request.identity.unit,
            Some(request.advanced),
        )
        .await
    }

    /// Return an already configured wallet.
    pub async fn wallet(&self, identity: WalletIdentity) -> Result<Wallet, Error> {
        self.get_wallet(&identity.mint_url, &identity.unit).await
    }

    /// Return a wallet, creating its local configuration when absent.
    pub async fn open_wallet(&self, identity: WalletIdentity) -> Result<Wallet, Error> {
        self.get_or_create_wallet(identity.mint_url, identity.unit, None)
            .await
    }

    /// List all configured mint wallets.
    pub async fn wallets(&self) -> Vec<Wallet> {
        self.get_wallets().await
    }

    /// List configured units for one mint.
    pub async fn wallets_for_mint(&self, mint_url: &MintUrl) -> Vec<Wallet> {
        self.get_wallets_for_mint(mint_url).await
    }

    /// Whether a wallet is configured for this mint and unit.
    pub async fn contains_wallet(&self, identity: &WalletIdentity) -> bool {
        self.has_wallet(&identity.mint_url, &identity.unit).await
    }

    /// Whether any wallet is configured for a mint.
    pub async fn contains_mint(&self, mint_url: &MintUrl) -> bool {
        self.has_mint(mint_url).await
    }

    /// Remove one wallet from this manager without deleting persisted mint data.
    pub async fn forget_wallet(&self, identity: WalletIdentity) -> Result<(), Error> {
        self.remove_wallet(identity.mint_url, identity.unit).await
    }

    /// Fetch a mint's current public capabilities.
    pub async fn mint_info(&self, mint_url: &MintUrl) -> Result<crate::nuts::MintInfo, Error> {
        self.fetch_mint_info(mint_url).await
    }

    /// Claim paid but unissued mint quotes, optionally for one mint only.
    pub async fn claim_pending_mints(&self, mint_url: Option<MintUrl>) -> Result<Amount, Error> {
        self.check_all_mint_quotes(mint_url).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use cdk_common::database::WalletDatabase;
    use cdk_common::nut00::KnownMethod;
    use cdk_common::nuts::{MintInfo, MintMethodSettings};
    use tokio::net::TcpListener;

    use super::*;
    use crate::nuts::{NUT04Settings, Nuts, PaymentMethod};

    async fn create_test_manager() -> WalletManager {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed(seed)
            .build()
            .await
            .expect("Failed to create WalletManager")
    }

    async fn create_test_manager_with_proxy(proxy_url: url::Url) -> WalletManager {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed(seed)
            .with_proxy(proxy_url)
            .build()
            .await
            .expect("Failed to create WalletManager")
    }

    async fn local_mint_url_with_connection_counter(
    ) -> (MintUrl, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind test mint listener");
        let address = listener
            .local_addr()
            .expect("Failed to get test mint listener address");
        let direct_connections = Arc::new(AtomicUsize::new(0));
        let connection_count = Arc::clone(&direct_connections);
        let handle = tokio::spawn(async move {
            while let Ok((_stream, _address)) = listener.accept().await {
                connection_count.fetch_add(1, Ordering::SeqCst);
            }
        });

        (
            format!("http://{address}")
                .parse()
                .expect("Failed to parse test mint URL"),
            direct_connections,
            handle,
        )
    }

    fn unsupported_proxy_url() -> url::Url {
        "gopher://127.0.0.1:1080"
            .parse()
            .expect("Failed to parse proxy URL")
    }

    fn mint_info_with_units(units: Vec<CurrencyUnit>) -> MintInfo {
        MintInfo::new().nuts(
            Nuts::new().nut04(NUT04Settings::new(
                units
                    .into_iter()
                    .map(|unit| MintMethodSettings {
                        method: PaymentMethod::Known(KnownMethod::Bolt11),
                        unit,
                        method_name: None,
                        min_amount: None,
                        max_amount: None,
                        options: None,
                    })
                    .collect(),
                false,
            )),
        )
    }

    #[test]
    fn builder_verifies_proxy_tls_certificates_by_default() {
        let builder = WalletManagerBuilder::new();

        assert!(!builder.danger_accept_invalid_certs);
    }

    #[test]
    fn builder_can_explicitly_accept_invalid_proxy_tls_certificates() {
        let builder = WalletManagerBuilder::new().with_danger_accept_invalid_certs(true);

        assert!(builder.danger_accept_invalid_certs);
    }

    #[tokio::test]
    async fn test_wallet_manager_creation() {
        let repo = create_test_manager().await;
        assert!(repo.wallets.try_read().is_ok());
    }

    #[tokio::test]
    async fn test_load_wallets_uses_persisted_metadata_without_network() {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let (mint_url, direct_connections, listener_handle) =
            local_mint_url_with_connection_counter().await;
        localstore
            .add_mint(
                mint_url.clone(),
                Some(mint_info_with_units(vec![
                    CurrencyUnit::Sat,
                    CurrencyUnit::Usd,
                ])),
            )
            .await
            .expect("Failed to add mint metadata");

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            WalletManagerBuilder::new()
                .with_store(localstore)
                .with_seed([0u8; 64])
                .build(),
        )
        .await;
        listener_handle.abort();

        let repo = result
            .expect("Manager startup should not wait for a mint request")
            .expect("Manager startup should succeed");

        assert_eq!(direct_connections.load(Ordering::SeqCst), 0);
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Usd).await);
    }

    #[tokio::test]
    async fn test_load_wallets_falls_back_to_sat_without_persisted_metadata() {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let (mint_url, direct_connections, listener_handle) =
            local_mint_url_with_connection_counter().await;
        localstore
            .add_mint(mint_url.clone(), None)
            .await
            .expect("Failed to add legacy mint");

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            WalletManagerBuilder::new()
                .with_store(localstore)
                .with_seed([0u8; 64])
                .build(),
        )
        .await;
        listener_handle.abort();

        let repo = result
            .expect("Manager startup should not wait for a mint request")
            .expect("Manager startup should succeed");

        assert_eq!(direct_connections.load(Ordering::SeqCst), 0);
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
    }

    #[tokio::test]
    async fn test_has_mint_empty() {
        let repo = create_test_manager().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();
        assert!(!repo.has_mint(&mint_url).await);
    }

    #[tokio::test]
    async fn test_create_and_get_wallet() {
        let repo = create_test_manager().await;
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
    async fn test_get_or_create_wallet_keeps_the_existing_wallet() {
        let repo = create_test_manager().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        repo.create_wallet(
            mint_url.clone(),
            CurrencyUnit::Sat,
            Some(MintAdvancedOptions::new().with_target_proof_count(5)),
        )
        .await
        .expect("Failed to create wallet");

        let wallet = repo
            .get_or_create_wallet(
                mint_url.clone(),
                CurrencyUnit::Sat,
                Some(MintAdvancedOptions::new().with_target_proof_count(99)),
            )
            .await
            .expect("Failed to get wallet");

        assert_eq!(wallet.target_proof_count, 5);
    }

    #[tokio::test]
    async fn test_get_or_create_wallet_creates_a_missing_wallet() {
        let repo = create_test_manager().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        let wallet = repo
            .get_or_create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet");

        assert_eq!(wallet.mint_url, mint_url);
        assert_eq!(wallet.unit, CurrencyUnit::Sat);
        assert!(repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
    }

    #[tokio::test]
    async fn test_fetch_mint_info_returns_error_when_proxy_setup_fails() {
        let repo = create_test_manager_with_proxy(unsupported_proxy_url()).await;
        let (mint_url, direct_connections, listener_handle) =
            local_mint_url_with_connection_counter().await;

        let result = repo.fetch_mint_info(&mint_url).await;

        listener_handle.abort();
        assert!(result.is_err());
        assert_eq!(direct_connections.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_create_wallet_returns_error_when_proxy_setup_fails() {
        let repo = create_test_manager_with_proxy(unsupported_proxy_url()).await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        let result = repo
            .create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await;

        assert!(result.is_err());
        assert!(!repo.has_mint(&mint_url).await);
        assert!(!repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);
    }

    #[tokio::test]
    async fn test_remove_wallet() {
        let repo = create_test_manager().await;
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
        let repo = create_test_manager().await;

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
    async fn test_remove_wallet_does_not_touch_db() {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let seed = [0u8; 64];
        let repo = WalletManagerBuilder::new()
            .with_store(localstore.clone())
            .with_seed(seed)
            .build()
            .await
            .expect("Failed to create WalletManager");

        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        // Add mint to DB manually to simulate existing state
        localstore.add_mint(mint_url.clone(), None).await.unwrap();

        // Create wallet in repo
        repo.create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("Failed to create wallet");

        // Remove wallet from in-memory repo
        repo.remove_wallet(mint_url.clone(), CurrencyUnit::Sat)
            .await
            .expect("Failed to remove wallet");

        // Verify wallet is gone from in-memory repo
        assert!(!repo.has_wallet(&mint_url, &CurrencyUnit::Sat).await);

        // Verify mint is still in DB (remove_wallet does not touch DB)
        assert!(localstore
            .get_mint(mint_url.clone())
            .await
            .unwrap()
            .is_some());
    }

    // The default rate-limit config admits exactly `capacity` (20) immediate
    // `try_acquire`s before pacing kicks in, and its burst tolerance (~57s) far
    // exceeds test wall-clock, so no slot is earned back mid-test.
    const DEFAULT_BURST: usize = 20;

    /// Resolve the bucket a wallet's requests to `url` would draw from.
    fn bucket_for(wallet: &Wallet, url: &str) -> crate::wallet::TokenBucket {
        wallet
            .rate_limiter
            .clone()
            .expect("default path retains its limiter")
            .bucket_for(&url::Url::parse(url).expect("valid url"))
    }

    #[tokio::test]
    async fn wallets_for_same_mint_share_one_rate_limit_budget() {
        let repo = create_test_manager().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        let sat = repo
            .create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("failed to create sat wallet");
        let usd = repo
            .create_wallet(mint_url.clone(), CurrencyUnit::Usd, None)
            .await
            .expect("failed to create usd wallet");

        let sat_bucket = bucket_for(&sat, "https://mint.example.com/v1/mint");
        let usd_bucket = bucket_for(&usd, "https://mint.example.com/v1/melt");

        // Interleave across the two handles: sharing one budget means the two
        // together drain a single burst, not one each.
        let mut admitted = 0;
        for _ in 0..DEFAULT_BURST {
            if sat_bucket.try_acquire().await {
                admitted += 1;
            }
            if usd_bucket.try_acquire().await {
                admitted += 1;
            }
        }
        assert_eq!(
            admitted, DEFAULT_BURST,
            "combined burst is one capacity, not two"
        );
        assert!(
            !sat_bucket.try_acquire().await,
            "shared budget already spent"
        );
        assert!(
            !usd_bucket.try_acquire().await,
            "shared budget already spent"
        );
    }

    #[tokio::test]
    async fn wallets_for_different_mints_have_independent_budgets() {
        let repo = create_test_manager().await;
        let mint_a: MintUrl = "https://mint-a.example.com".parse().unwrap();
        let mint_b: MintUrl = "https://mint-b.example.com".parse().unwrap();

        let wallet_a = repo
            .create_wallet(mint_a, CurrencyUnit::Sat, None)
            .await
            .expect("failed to create wallet a");
        let wallet_b = repo
            .create_wallet(mint_b, CurrencyUnit::Sat, None)
            .await
            .expect("failed to create wallet b");
        let bucket_a = bucket_for(&wallet_a, "https://mint-a.example.com/v1/info");
        let bucket_b = bucket_for(&wallet_b, "https://mint-b.example.com/v1/info");

        for _ in 0..DEFAULT_BURST {
            assert!(bucket_a.try_acquire().await);
        }
        assert!(
            !bucket_a.try_acquire().await,
            "mint A's own burst is drained"
        );
        assert!(
            bucket_b.try_acquire().await,
            "mint B has an untouched budget"
        );
    }

    #[tokio::test]
    async fn third_party_hosts_pace_against_their_own_budget() {
        // A wallet's transport also carries LNURL and OIDC traffic. That must
        // neither spend the mint's budget nor be paced by it, and two wallets at
        // different mints hitting one service must share that service's budget.
        let repo = create_test_manager().await;
        let wallet_a = repo
            .create_wallet(
                "https://mint-a.example.com".parse().unwrap(),
                CurrencyUnit::Sat,
                None,
            )
            .await
            .expect("failed to create wallet a");
        let wallet_b = repo
            .create_wallet(
                "https://mint-b.example.com".parse().unwrap(),
                CurrencyUnit::Sat,
                None,
            )
            .await
            .expect("failed to create wallet b");

        let mint_bucket = bucket_for(&wallet_a, "https://mint-a.example.com/v1/info");
        let lnurl = "https://pay.example.org/.well-known/lnurlp/alice";
        let lnurl_bucket = bucket_for(&wallet_a, lnurl);

        // Spending the mint's whole burst leaves the LNURL host untouched.
        for _ in 0..DEFAULT_BURST {
            assert!(mint_bucket.try_acquire().await);
        }
        assert!(!mint_bucket.try_acquire().await);
        assert!(
            lnurl_bucket.try_acquire().await,
            "the LNURL host keeps its own budget"
        );

        // Wallet B, a different mint entirely, draws from the same LNURL budget.
        for _ in 0..(DEFAULT_BURST - 1) {
            assert!(bucket_for(&wallet_b, lnurl).try_acquire().await);
        }
        assert!(
            !bucket_for(&wallet_b, lnurl).try_acquire().await,
            "the LNURL host's budget is shared across mints"
        );
    }

    /// Awaiting the barrier is what makes the handover deterministic: without
    /// it the rebuild races the detached writer. Capacity 2 and emission ~200ms
    /// keep the pace signal clear of scheduler noise.
    #[tokio::test]
    async fn flushing_a_wallet_hands_its_budget_to_the_rebuilt_one() {
        let cfg = RateLimitConfig::try_new(2, 300).expect("non-zero");
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();
        let mint_endpoint = "https://mint.example.com/v1/mint";

        let repo = WalletManagerBuilder::new()
            .with_store(localstore.clone())
            .with_seed([0u8; 64])
            .build()
            .await
            .expect("Failed to create WalletManager");
        let wallet = repo
            .create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("failed to create wallet");
        wallet.set_rate_limiting_config(cfg);

        let bucket = bucket_for(&wallet, mint_endpoint);
        bucket.acquire(async {}).await;
        bucket.acquire(async {}).await;
        wallet.flush_rate_limits().await;
        drop((bucket, wallet, repo));

        let rebuilt_repo = WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed([0u8; 64])
            .build()
            .await
            .expect("Failed to rebuild WalletManager");
        let rebuilt = rebuilt_repo
            .get_or_create_wallet(mint_url, CurrencyUnit::Sat, None)
            .await
            .expect("failed to rebuild wallet");
        rebuilt.set_rate_limiting_config(cfg);

        let rebuilt_bucket = bucket_for(&rebuilt, mint_endpoint);
        let start = Instant::now();
        rebuilt_bucket.acquire(async {}).await;
        rebuilt_bucket.acquire(async {}).await;
        assert!(
            start.elapsed() >= Duration::from_millis(150),
            "rebuilt wallet should inherit the flushed budget, took {:?}",
            start.elapsed()
        );

        let untouched = bucket_for(&rebuilt, "https://other.example.com/v1/info");
        let start = Instant::now();
        untouched.acquire(async {}).await;
        untouched.acquire(async {}).await;
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "an origin the flushed wallet never touched still bursts"
        );
    }

    /// A wallet built with a custom client keeps no limiter, and a fresh
    /// manager has no origins, so the barrier has nothing to wait for and
    /// must still return.
    #[tokio::test]
    async fn flush_rate_limits_is_a_no_op_without_a_limiter() {
        use crate::wallet::test_utils::MockMintConnector;

        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let unlimited = crate::wallet::WalletBuilder::default()
            .with_mint_url("https://mint.example.com".parse().unwrap())
            .with_unit(CurrencyUnit::Sat)
            .with_store(localstore)
            .with_seed([0u8; 64])
            .with_shared_connector(Arc::new(MockMintConnector::new()))
            .build()
            .expect("failed to build wallet");

        assert!(unlimited.rate_limiter.is_none());
        unlimited.flush_rate_limits().await;
        create_test_manager().await.flush_rate_limits().await;
    }

    async fn manager_with_rate_limit(rate_limit: Option<RateLimitConfig>) -> WalletManager {
        let localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("Failed to create in-memory database"),
        );
        let builder = WalletManagerBuilder::new()
            .with_store(localstore)
            .with_seed([0u8; 64]);
        let builder = match rate_limit {
            Some(config) => builder.with_rate_limiting_config(config),
            None => builder.with_rate_limiting_disabled(),
        };
        builder
            .build()
            .await
            .expect("Failed to create WalletManager")
    }

    #[tokio::test]
    async fn manager_starts_with_the_configured_rate_limit() {
        assert!(create_test_manager().await.is_rate_limited());
        assert!(manager_with_rate_limit(RateLimitConfig::try_new(5, 30))
            .await
            .is_rate_limited());
        assert!(!manager_with_rate_limit(None).await.is_rate_limited());
    }

    #[tokio::test]
    async fn manager_rate_limit_reaches_wallets_it_already_handed_out() {
        let repo = manager_with_rate_limit(None).await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();
        let wallet = repo
            .get_or_create_wallet(mint_url, CurrencyUnit::Sat, None)
            .await
            .expect("wallet should be created");
        assert!(!wallet.is_rate_limited());

        repo.set_rate_limiting_config(Some(RateLimitConfig::default()));
        assert!(repo.is_rate_limited());
        assert!(
            wallet.is_rate_limited(),
            "the wallet shares the manager's limiter"
        );

        repo.set_rate_limiting_config(None);
        assert!(!wallet.is_rate_limited());
    }

    #[tokio::test]
    async fn a_disabled_manager_admits_more_than_one_burst() {
        let repo = manager_with_rate_limit(None).await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();
        let wallet = repo
            .get_or_create_wallet(mint_url, CurrencyUnit::Sat, None)
            .await
            .expect("wallet should be created");

        let bucket = bucket_for(&wallet, "https://mint.example.com/v1/info");
        for _ in 0..(DEFAULT_BURST + 5) {
            assert!(bucket.try_acquire().await, "pacing is off");
        }
    }

    #[tokio::test]
    async fn get_or_create_wallet_second_unit_shares_the_budget() {
        let repo = create_test_manager().await;
        let mint_url: MintUrl = "https://mint.example.com".parse().unwrap();

        let sat = repo
            .get_or_create_wallet(mint_url.clone(), CurrencyUnit::Sat, None)
            .await
            .expect("failed to create sat wallet");
        let usd = repo
            .get_or_create_wallet(mint_url.clone(), CurrencyUnit::Usd, None)
            .await
            .expect("failed to create usd wallet");

        let sat_bucket = bucket_for(&sat, "https://mint.example.com/v1/mint");
        let usd_bucket = bucket_for(&usd, "https://mint.example.com/v1/melt");

        // Drain the whole burst through the Sat handle; the Usd handle sees an
        // already-spent budget because they share one bucket.
        for _ in 0..DEFAULT_BURST {
            assert!(sat_bucket.try_acquire().await);
        }
        assert!(
            !usd_bucket.try_acquire().await,
            "shared budget already spent"
        );
    }
}
