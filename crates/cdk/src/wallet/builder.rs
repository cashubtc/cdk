use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database;
#[cfg(feature = "auth")]
use cdk_common::AuthToken;
#[cfg(feature = "auth")]
use tokio::sync::RwLock;

use crate::cdk_database::WalletDatabase;
use crate::error::Error;
use crate::mint_url::MintUrl;
use crate::nuts::CurrencyUnit;
#[cfg(feature = "auth")]
use crate::wallet::auth::AuthWallet;
use crate::wallet::key_manager::KeyManager;
use crate::wallet::{HttpClient, MintConnector, SubscriptionManager, Wallet};

/// Builder for creating a new [`Wallet`]
pub struct WalletBuilder {
    mint_url: Option<MintUrl>,
    unit: Option<CurrencyUnit>,
    localstore: Option<Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>>,
    target_proof_count: Option<usize>,
    #[cfg(feature = "auth")]
    auth_wallet: Option<AuthWallet>,
    seed: Option<[u8; 64]>,
    use_http_subscription: bool,
    client: Option<Arc<dyn MintConnector + Send + Sync>>,
    key_manager: Option<Arc<KeyManager>>,
    key_managers: HashMap<MintUrl, Arc<KeyManager>>,
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
            use_http_subscription: false,
            key_manager: None,
            key_managers: HashMap::new(),
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
        localstore: Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>,
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

    /// Set a shared KeyManager
    ///
    /// This allows multiple wallets to share the same KeyManager instance for
    /// optimal performance and memory usage. If not provided, a new KeyManager
    /// will be created for each wallet.
    pub fn key_manager(mut self, key_manager: Arc<KeyManager>) -> Self {
        self.key_manager = Some(key_manager);
        self
    }

    /// Set a HashMap of KeyManagers for reusing across multiple wallets
    ///
    /// This allows the builder to reuse existing KeyManager instances or create new ones.
    /// Useful when creating multiple wallets that share KeyManagers.
    pub fn key_managers(mut self, key_managers: HashMap<MintUrl, Arc<KeyManager>>) -> Self {
        self.key_managers = key_managers;
        self
    }

    /// Set auth CAT (Clear Auth Token)
    #[cfg(feature = "auth")]
    pub fn set_auth_cat(mut self, cat: String) -> Self {
        let mint_url = self.mint_url.clone().expect("Mint URL required");
        let localstore = self.localstore.clone().expect("Localstore required");

        let key_manager = self.key_manager.clone().unwrap_or_else(|| {
            // Check if we already have a KeyManager for this mint in the HashMap
            if let Some(km) = self.key_managers.get(&mint_url) {
                km.clone()
            } else {
                // Create a new one
                Arc::new(KeyManager::new(
                    mint_url.clone(),
                    localstore.clone(),
                    Arc::new(HttpClient::new(mint_url.clone(), None)),
                ))
            }
        });

        self.auth_wallet = Some(AuthWallet::new(
            mint_url,
            Some(AuthToken::ClearAuth(cat)),
            localstore,
            key_manager,
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

        let key_manager = self.key_manager.unwrap_or_else(|| {
            // Check if we already have a KeyManager for this mint in the HashMap
            if let Some(km) = self.key_managers.get(&mint_url) {
                km.clone()
            } else {
                // Create a new one
                Arc::new(KeyManager::new(
                    mint_url.clone(),
                    localstore.clone(),
                    client.clone(),
                ))
            }
        });

        Ok(Wallet {
            mint_url,
            unit,
            localstore,
            key_manager,
            target_proof_count: self.target_proof_count.unwrap_or(3),
            #[cfg(feature = "auth")]
            auth_wallet: Arc::new(RwLock::new(self.auth_wallet)),
            seed,
            client: client.clone(),
            subscription: SubscriptionManager::new(client, self.use_http_subscription),
        })
    }
}
