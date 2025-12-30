use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cdk_common::database;
use cdk_common::parking_lot::RwLock;
#[cfg(feature = "auth")]
use cdk_common::AuthToken;
#[cfg(feature = "auth")]
use tokio::sync::RwLock as TokioRwLock;

use crate::cdk_database::WalletDatabase;
use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
#[cfg(feature = "auth")]
use crate::wallet::auth::AuthWallet;
use crate::wallet::mint_metadata_cache::MintMetadataCache;
use crate::wallet::{HttpClient, MintConnector, SubscriptionManager, Wallet};

/// Builder for creating a new [`Wallet`]
pub struct WalletBuilder {
    mint_url: Option<MintUrl>,
    unit: Option<CurrencyUnit>,
    localstore: Option<Arc<dyn WalletDatabase<database::Error> + Send + Sync>>,
    target_proof_count: Option<usize>,
    #[cfg(feature = "auth")]
    auth_wallet: Option<AuthWallet>,
    seed: Option<[u8; 64]>,
    use_http_subscription: bool,
    client: Option<Arc<dyn MintConnector + Send + Sync>>,
    metadata_cache_ttl: Option<Duration>,
    metadata_cache: Option<Arc<MintMetadataCache>>,
    metadata_caches: HashMap<MintUrl, Arc<MintMetadataCache>>,
}

impl Default for WalletBuilder {
    fn default() -> Self {
        Self {
            mint_url: None,
            unit: None,
            localstore: None,
            target_proof_count: Some(3),
            #[cfg(feature = "auth")]
            auth_wallet: None,
            seed: None,
            client: None,
            metadata_cache_ttl: None,
            use_http_subscription: false,
            metadata_cache: None,
            metadata_caches: HashMap::new(),
        }
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
    #[cfg(feature = "auth")]
    pub fn auth_wallet(mut self, auth_wallet: AuthWallet) -> Self {
        self.auth_wallet = Some(auth_wallet);
        self
    }

    /// Set the seed bytes
    pub fn seed(mut self, seed: [u8; 64]) -> Self {
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

    /// Set auth CAT (Clear Auth Token)
    #[cfg(feature = "auth")]
    pub fn set_auth_cat(mut self, cat: String) -> Self {
        let mint_url = self.mint_url.clone().expect("Mint URL required");
        let localstore = self.localstore.clone().expect("Localstore required");

        let metadata_cache = self.metadata_cache.clone().unwrap_or_else(|| {
            // Check if we already have a cache for this mint in the HashMap
            if let Some(cache) = self.metadata_caches.get(&mint_url) {
                cache.clone()
            } else {
                // Create a new one
                Arc::new(MintMetadataCache::new(mint_url.clone()))
            }
        });

        self.auth_wallet = Some(AuthWallet::new(
            mint_url,
            Some(AuthToken::ClearAuth(cat)),
            localstore,
            metadata_cache,
            HashMap::new(),
            None,
        ));
        self
    }

    /// Build the wallet
    pub fn build(self) -> Result<Wallet, Error> {
        let mint_url = self
            .mint_url
            .ok_or(Error::Custom("Mint url required".to_string()))?;
        let unit = self
            .unit
            .ok_or(Error::Custom("Unit required".to_string()))?;
        let localstore = self
            .localstore
            .ok_or(Error::Custom("Localstore required".to_string()))?;
        let seed: [u8; 64] = self
            .seed
            .ok_or(Error::Custom("Seed required".to_string()))?;

        let client = match self.client {
            Some(client) => client,
            None => {
                #[cfg(feature = "auth")]
                {
                    Arc::new(HttpClient::new(mint_url.clone(), self.auth_wallet.clone()))
                        as Arc<dyn MintConnector + Send + Sync>
                }

                #[cfg(not(feature = "auth"))]
                {
                    Arc::new(HttpClient::new(mint_url.clone()))
                        as Arc<dyn MintConnector + Send + Sync>
                }
            }
        };

        let metadata_cache_ttl = self.metadata_cache_ttl;

        let metadata_cache = self.metadata_cache.unwrap_or_else(|| {
            // Check if we already have a cache for this mint in the HashMap
            if let Some(cache) = self.metadata_caches.get(&mint_url) {
                cache.clone()
            } else {
                // Create a new one
                Arc::new(MintMetadataCache::new(mint_url.clone()))
            }
        });

        Ok(Wallet {
            mint_url,
            unit,
            localstore,
            metadata_cache,
            metadata_cache_ttl: Arc::new(RwLock::new(metadata_cache_ttl)),
            target_proof_count: self.target_proof_count.unwrap_or(3),
            #[cfg(feature = "auth")]
            auth_wallet: Arc::new(TokioRwLock::new(self.auth_wallet)),
            seed,
            client: client.clone(),
            subscription: SubscriptionManager::new(client, self.use_http_subscription),
            in_error_swap_reverted_proofs: Arc::new(false.into()),
        })
    }
}
