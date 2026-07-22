use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::{database, AuthToken};
use tokio::sync::RwLock as TokioRwLock;
use zeroize::Zeroize;

use crate::cdk_database::WalletDatabase;
use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
use crate::wallet::auth::{AuthMintConnector, AuthWallet};
use crate::wallet::mint_connector::transport::{Async, RateLimitedTransport};
use crate::wallet::mint_connector::{RateLimitedAuthHttpClient, RateLimitedHttpClient};
use crate::wallet::mint_metadata_cache::MintMetadataCache;
use crate::wallet::{
    AuthHttpClient, HttpClient, MintConnector, RateLimitConfig, SubscriptionManager, TokenBucket,
    Wallet,
};

/// Builder for creating a new [`Wallet`]
pub struct WalletBuilder {
    mint_url: Option<MintUrl>,
    unit: Option<CurrencyUnit>,
    localstore: Option<Arc<dyn WalletDatabase<database::Error> + Send + Sync>>,
    target_proof_count: Option<usize>,
    auth_wallet: Option<AuthWallet>,
    auth_connector: Option<Arc<dyn AuthMintConnector + Send + Sync>>,
    seed: Option<[u8; 64]>,
    use_http_subscription: bool,
    client: Option<Arc<dyn MintConnector + Send + Sync>>,
    metadata_cache_ttl: Option<Duration>,
    metadata_cache: Option<Arc<MintMetadataCache>>,
    metadata_caches: HashMap<MintUrl, Arc<MintMetadataCache>>,
    rate_limit: Option<RateLimitConfig>,
    auth_cat: Option<String>,
}

impl std::fmt::Debug for WalletBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletBuilder")
            .field("mint_url", &self.mint_url)
            .field("unit", &self.unit)
            .field("target_proof_count", &self.target_proof_count)
            .finish_non_exhaustive()
    }
}

impl Default for WalletBuilder {
    fn default() -> Self {
        Self {
            mint_url: None,
            unit: None,
            localstore: None,
            target_proof_count: Some(3),
            auth_wallet: None,
            auth_connector: None,
            seed: None,
            client: None,
            metadata_cache_ttl: Some(Duration::from_secs(3600)),
            use_http_subscription: false,
            metadata_cache: None,
            metadata_caches: HashMap::new(),
            rate_limit: Some(RateLimitConfig::default()),
            auth_cat: None,
        }
    }
}

impl Drop for WalletBuilder {
    fn drop(&mut self) {
        self.seed.zeroize();
    }
}

impl WalletBuilder {
    /// Create a new WalletBuilder
    pub fn new() -> Self {
        Self::default()
    }

    /// Use HTTP for wallet subscriptions to mint events
    pub fn use_http_subscription(mut self) -> Self {
        self.use_http_subscription = true;
        self
    }

    /// Set metadata_cache_ttl
    ///
    /// The TTL determines how often the wallet checks the mint for new keysets and information.
    ///
    /// If `None`, the cache will never expire and the wallet will use cached data indefinitely
    /// (unless manually refreshed).
    ///
    /// The default value is 1 hour (3600 seconds).
    pub fn set_metadata_cache_ttl(mut self, metadata_cache_ttl: Option<Duration>) -> Self {
        self.metadata_cache_ttl = metadata_cache_ttl;
        self
    }

    /// If WS is preferred (with fallback to HTTP is it is not supported by the mint) for the wallet
    /// subscriptions to mint events
    pub fn prefer_ws_subscription(mut self) -> Self {
        self.use_http_subscription = false;
        self
    }

    /// Set the mint URL
    pub fn mint_url(mut self, mint_url: MintUrl) -> Self {
        self.mint_url = Some(mint_url);
        self
    }

    /// Set the currency unit
    pub fn unit(mut self, unit: CurrencyUnit) -> Self {
        self.unit = Some(unit);
        self
    }

    /// Set the local storage backend
    pub fn localstore(
        mut self,
        localstore: Arc<dyn WalletDatabase<database::Error> + Send + Sync>,
    ) -> Self {
        self.localstore = Some(localstore);
        self
    }

    /// Set the target proof count
    pub fn target_proof_count(mut self, count: usize) -> Self {
        self.target_proof_count = Some(count);
        self
    }

    /// Set the auth wallet
    pub fn auth_wallet(mut self, auth_wallet: AuthWallet) -> Self {
        self.auth_wallet = Some(auth_wallet);
        self
    }

    /// Set the auth connector used when an auth wallet is created from mint info
    pub fn auth_connector(
        mut self,
        auth_connector: Arc<dyn AuthMintConnector + Send + Sync>,
    ) -> Self {
        self.auth_connector = Some(auth_connector);
        self
    }

    /// Set the seed bytes
    pub fn seed(mut self, seed: [u8; 64]) -> Self {
        self.seed.zeroize();
        self.seed = Some(seed);
        self
    }

    /// Set a custom client connector
    pub fn client<C: MintConnector + 'static + Send + Sync>(mut self, client: C) -> Self {
        self.client = Some(Arc::new(client));
        self
    }

    /// Set a custom client connector from Arc
    pub fn shared_client(mut self, client: Arc<dyn MintConnector + Send + Sync>) -> Self {
        self.client = Some(client);
        self
    }

    /// Set a shared MintMetadataCache
    ///
    /// This allows multiple wallets to share the same metadata cache instance for
    /// optimal performance and memory usage. If not provided, a new cache
    /// will be created for each wallet.
    pub fn metadata_cache(mut self, metadata_cache: Arc<MintMetadataCache>) -> Self {
        self.metadata_cache = Some(metadata_cache);
        self
    }

    /// Set a HashMap of MintMetadataCaches for reusing across multiple wallets
    ///
    /// This allows the builder to reuse existing cache instances or create new ones.
    /// Useful when creating multiple wallets that share metadata caches.
    pub fn metadata_caches(
        mut self,
        metadata_caches: HashMap<MintUrl, Arc<MintMetadataCache>>,
    ) -> Self {
        self.metadata_caches = metadata_caches;
        self
    }

    /// Set the rate-limiting configuration.
    ///
    /// Rate limiting is enabled by default with [`RateLimitConfig::default`].
    pub fn with_rate_limiting_config(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit = Some(config);
        self
    }

    /// Disable client-side rate limiting.
    pub fn without_rate_limiting(mut self) -> Self {
        self.rate_limit = None;
        self
    }

    /// Set auth CAT (Clear Auth Token)
    ///
    /// The auth wallet is constructed in [`WalletBuilder::build`] so its HTTP
    /// client can share the same rate-limit budget as the main client.
    ///
    /// # Errors
    ///
    /// Returns an error if `mint_url` or `localstore` have not been set on the builder.
    pub fn set_auth_cat(mut self, cat: String) -> Result<Self, Error> {
        if self.mint_url.is_none() {
            return Err(Error::Custom("Mint URL required".to_string()));
        }
        if self.localstore.is_none() {
            return Err(Error::Custom("Localstore required".to_string()));
        }

        self.auth_cat = Some(cat);
        self.auth_wallet = None;
        Ok(self)
    }

    /// Build the wallet
    pub fn build(mut self) -> Result<Wallet, Error> {
        let mint_url = self
            .mint_url
            .take()
            .ok_or(Error::Custom("Mint url required".to_string()))?;
        let unit = self
            .unit
            .take()
            .ok_or(Error::Custom("Unit required".to_string()))?;
        let localstore = self
            .localstore
            .take()
            .ok_or(Error::Custom("Localstore required".to_string()))?;
        let seed: [u8; 64] = self
            .seed
            .ok_or(Error::Custom("Seed required".to_string()))?;

        let metadata_cache = self.metadata_cache.take().unwrap_or_else(|| {
            // Check if we already have a cache for this mint in the HashMap
            if let Some(cache) = self.metadata_caches.get(&mint_url) {
                cache.clone()
            } else {
                // Create a new one
                Arc::new(MintMetadataCache::new(mint_url.clone()))
            }
        });

        metadata_cache.set_ttl(self.metadata_cache_ttl);

        // A single rate-limited transport, shared by the main client and the
        // blind-auth client so both draw down one persisted budget and reuse one
        // connection pool.
        let shared_transport = self.rate_limit.take().map(|config| {
            let bucket = TokenBucket::for_mint(config, &mint_url, localstore.clone());
            Arc::new(RateLimitedTransport::with_bucket(Async::default(), bucket))
        });

        // The auth wallet comes either from a CAT set on the builder (built here
        // so it can share the transport) or from a pre-built wallet supplied
        // directly, which is used verbatim.
        let auth_wallet = match self.auth_cat.take() {
            Some(cat) => {
                let cat = AuthToken::ClearAuth(cat);
                let auth_client: Arc<dyn AuthMintConnector + Send + Sync> = match &shared_transport
                {
                    Some(transport) => Arc::new(RateLimitedAuthHttpClient::with_shared_transport(
                        mint_url.clone(),
                        transport.clone(),
                        Some(cat),
                    )),
                    None => Arc::new(AuthHttpClient::new(mint_url.clone(), Some(cat))),
                };
                Some(AuthWallet::with_auth_client(
                    mint_url.clone(),
                    localstore.clone(),
                    metadata_cache.clone(),
                    HashMap::new(),
                    None,
                    auth_client,
                ))
            }
            None => self.auth_wallet.take(),
        };

        let client = match self.client.take() {
            Some(client) => client,
            None => match shared_transport {
                Some(transport) => Arc::new(RateLimitedHttpClient::with_shared_transport(
                    mint_url.clone(),
                    transport,
                    auth_wallet.clone(),
                )) as Arc<dyn MintConnector + Send + Sync>,
                None => Arc::new(HttpClient::new(mint_url.clone(), auth_wallet.clone()))
                    as Arc<dyn MintConnector + Send + Sync>,
            },
        };

        Ok(Wallet {
            mint_url,
            unit,
            localstore,
            metadata_cache,
            target_proof_count: self.target_proof_count.unwrap_or(3),
            auth_wallet: Arc::new(TokioRwLock::new(auth_wallet)),
            auth_connector: self.auth_connector.take(),
            #[cfg(feature = "npubcash")]
            npubcash_client: Arc::new(TokioRwLock::new(None)),
            seed,
            client: client.clone(),
            subscription: SubscriptionManager::new(client, self.use_http_subscription),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn test_default_ttl() {
        let builder = WalletBuilder::default();
        assert_eq!(builder.metadata_cache_ttl, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn rate_limiting_on_by_default() {
        let builder = WalletBuilder::default();
        assert!(builder.rate_limit.is_some());
    }

    #[test]
    fn without_rate_limiting_clears_it() {
        let builder = WalletBuilder::default().without_rate_limiting();
        assert!(builder.rate_limit.is_none());
    }

    #[tokio::test]
    async fn set_auth_cat_defers_construction() {
        let mint_url = MintUrl::from_str("https://mint.example.com").unwrap();
        let store = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        let builder = WalletBuilder::default()
            .mint_url(mint_url)
            .localstore(store)
            .set_auth_cat("cat".to_string())
            .unwrap();
        // Construction is deferred to build(): only the raw CAT is stored.
        assert_eq!(builder.auth_cat.as_deref(), Some("cat"));
        assert!(builder.auth_wallet.is_none());
    }

    #[test]
    fn set_auth_cat_requires_mint_and_store() {
        let err = WalletBuilder::default().set_auth_cat("cat".to_string());
        assert!(err.is_err());
    }

    async fn base_builder() -> WalletBuilder {
        let store = Arc::new(cdk_sqlite::wallet::memory::empty().await.unwrap());
        WalletBuilder::default()
            .mint_url(MintUrl::from_str("https://mint.example.com").unwrap())
            .unit(crate::nuts::CurrencyUnit::Sat)
            .localstore(store)
            .seed([0u8; 64])
    }

    #[tokio::test]
    async fn build_with_rate_limiting_and_auth_cat() {
        // Exercises the shared-bucket path: a rate-limited auth client plus a
        // rate-limited main client, both built in build().
        let wallet = base_builder()
            .await
            .set_auth_cat("cat".to_string())
            .unwrap()
            .build()
            .unwrap();
        assert!(wallet.auth_wallet.read().await.is_some());
    }

    #[tokio::test]
    async fn build_without_rate_limiting_and_auth_cat() {
        // Exercises the plain path: a plain auth client plus a plain main client.
        let wallet = base_builder()
            .await
            .without_rate_limiting()
            .set_auth_cat("cat".to_string())
            .unwrap()
            .build()
            .unwrap();
        assert!(wallet.auth_wallet.read().await.is_some());
    }
}
