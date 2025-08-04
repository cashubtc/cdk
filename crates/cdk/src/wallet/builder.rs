#[cfg(feature = "auth")]
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
use crate::wallet::{HttpClient, MintConnector, SubscriptionManager, Wallet};

/// Builder for creating a new [`Wallet`]
#[derive(Debug)]
pub struct WalletBuilder {
    mint_url: Option<MintUrl>,
    unit: Option<CurrencyUnit>,
    localstore: Option<Arc<dyn WalletDatabase<Err = database::Error> + Send + Sync>>,
    target_proof_count: Option<usize>,
    #[cfg(feature = "auth")]
    auth_wallet: Option<AuthWallet>,
    seed: Option<[u8; 64]>,
    client: Option<Arc<dyn MintConnector + Send + Sync>>,
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
        }
    }
}

impl WalletBuilder {
    /// Create a new WalletBuilder
    pub fn new() -> Self {
        Self::default()
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

    /// Set auth CAT (Clear Auth Token)
    #[cfg(feature = "auth")]
    pub fn set_auth_cat(mut self, cat: String) -> Self {
        self.auth_wallet = Some(AuthWallet::new(
            self.mint_url.clone().expect("Mint URL required"),
            Some(AuthToken::ClearAuth(cat)),
            self.localstore.clone().expect("Localstore required"),
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

        Ok(Wallet {
            mint_url,
            unit,
            localstore,
            target_proof_count: self.target_proof_count.unwrap_or(3),
            #[cfg(feature = "auth")]
            auth_wallet: Arc::new(RwLock::new(self.auth_wallet)),
            seed,
            client: client.clone(),
            subscription: SubscriptionManager::new(client),
        })
    }
}
